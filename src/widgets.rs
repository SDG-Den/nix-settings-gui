use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::dsl::{OptionSpec, WidgetKind};
use crate::edit::PendingEdit;
use crate::model::Record;
use crate::scan;
use crate::value;

/// Quote a value as a double-quoted Nix string. Backslashes, quotes, and
/// interpolation openers are escaped so the result never gains meaning.
pub fn quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

/// A string that contains a newline is written as an indented `''` string.
fn string_literal(s: &str) -> String {
    if s.contains('\n') {
        format!("''\n{s}\n''")
    } else {
        quote(s)
    }
}

/// Strip value modifiers (`lib.mkDefault`, `lib.mkForce`, `lib.mkOverride <n>`)
/// so the control compares and edits the value that is actually assigned.
fn unwrap_modifiers(text: &str) -> String {
    let mut t = text.trim();
    loop {
        let rest = t
            .strip_prefix("lib.mkDefault")
            .or_else(|| t.strip_prefix("lib.mkForce"))
            .or_else(|| {
                t.strip_prefix("lib.mkOverride").map(|r| {
                    r.trim_start()
                        .trim_start_matches(|c: char| c.is_ascii_digit())
                })
            });
        match rest.map(str::trim) {
            Some(r) if !r.is_empty() => t = r,
            _ => break,
        }
    }
    t.to_string()
}

/// Nix truthiness: only `false`, `null`, and the numeric zero forms are falsy.
fn truthy(value: &str) -> bool {
    !matches!(
        unwrap_modifiers(value).trim(),
        "false" | "null" | "0" | "0.0"
    )
}

/// A dropdown or radio choice: numbers and booleans stay verbsatim, anything
/// else is quoted, because `choices` in the DSL are lists of strings.
fn choice_literal(raw: &str) -> String {
    let t = raw.trim();
    if matches!(t, "true" | "false" | "null") || t.parse::<f64>().is_ok() {
        t.to_string()
    } else {
        quote(t)
    }
}

/// A list element or package reference: keep the text verbatim when it is a
/// valid Nix expression (`pkgs.git`, `"nix-command"`), otherwise quote it.
fn expr_literal(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "\"\"".to_string();
    }
    let parsed = rnix::Root::parse(t).ok().ok().and_then(|r| r.expr());
    if parsed.is_some() {
        t.to_string()
    } else {
        quote(t)
    }
}

/// A package entry that is a bare attribute name (`git`, `python3`, `gcc-13`)
/// rather than a qualified expression (`pkgs.git`).
fn is_bare_package(text: &str) -> bool {
    let t = text.trim();
    match t.chars().next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Serialize a package list. Bare names need `pkgs` in scope, so they go under
/// `with pkgs;`; already-qualified expressions stay verbatim in a plain list.
fn serialize_packages(entries: &[String]) -> String {
    let cleaned: Vec<String> = entries
        .iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    if cleaned.is_empty() {
        return "[]".to_string();
    }
    if cleaned.iter().all(|e| is_bare_package(e)) {
        format!("with pkgs; [ {} ]", cleaned.join(" "))
    } else {
        let items: Vec<String> = cleaned.iter().map(|e| expr_literal(e)).collect();
        value::serialize_list(&items)
    }
}

/// Serialize a module list, prefixing each name with the scope qualifier the
/// value already used (e.g. `self.modules.nixos.`), or none for bare names.
fn serialize_modules(names: &[String], prefix: &str) -> String {
    let items: Vec<String> = names
        .iter()
        .map(|n| format!("{prefix}{}", n.trim()))
        .collect();
    value::serialize_list(&items)
}

/// Canonical form of a list value as the editor would write it. Comparing this
/// against `to_nix` ignores source formatting (multi-line, extra spacing), so
/// an untouched list is never mistaken for a change.
fn canonical_list(original: &str, packages: bool) -> Option<String> {
    let items = value::list_items(original)?;
    if packages {
        Some(serialize_packages(&items))
    } else {
        let mapped: Vec<String> = items
            .iter()
            .map(|item| expr_literal(item))
            .filter(|item| !item.is_empty() && item != "\"\"")
            .collect();
        Some(value::serialize_list(&mapped))
    }
}

/// The module names a module control holds. A list literal yields its elements;
/// a bare single reference (e.g. `self.modules.homeManager.foo`) yields that one
/// expression.
fn module_items(text: &str) -> Option<Vec<String>> {
    value::list_items(text).or_else(|| value::single_expr(text).map(|e| vec![e]))
}

/// Canonical form of a module list. The editor stores each item with its scope
/// prefix stripped, so strip the same prefix before re-serializing.
fn canonical_module_list(original: &str, prefix: &str) -> Option<String> {
    let items = module_items(original)?;
    let names: Vec<String> = items
        .iter()
        .map(|item| {
            let raw = item.trim();
            if prefix.is_empty() {
                raw.to_string()
            } else {
                raw.strip_prefix(prefix).unwrap_or(raw).to_string()
            }
        })
        .filter(|name| !name.is_empty())
        .collect();
    Some(serialize_modules(&names, prefix))
}

/// A single package reference: a bare name is qualified with `pkgs.`.
fn package_literal(raw: &str) -> String {
    let t = raw.trim();
    if is_bare_package(t) {
        format!("pkgs.{t}")
    } else {
        expr_literal(t)
    }
}

fn number_literal(value: f64, digits: u32) -> String {
    if digits == 0 && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let precision = digits as usize;
        format!("{value:.precision$}")
    }
}

/// A path stays a bare path literal when it looks like one, else a string.
fn path_literal(raw: &str) -> String {
    let t = raw.trim();
    if t.starts_with("./") || t.starts_with("../") || t.starts_with('/') {
        t.to_string()
    } else {
        quote(t)
    }
}

/// Which flake module scope a module list belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Nixos,
    Home,
}

impl ModuleKind {
    fn scope(self) -> &'static str {
        match self {
            ModuleKind::Nixos => "nixos",
            ModuleKind::Home => "homeManager",
        }
    }

    /// The qualifiers a value may use, longest first, e.g.
    /// `self.modules.nixos.` and `flake.modules.nixos.`.
    fn prefixes(self) -> [String; 2] {
        [
            format!("self.modules.{}.", self.scope()),
            format!("flake.modules.{}.", self.scope()),
        ]
    }
}

/// Runs a package-name search and calls back with results on the main thread.
/// Supplied by the app so the widgets stay free of nixd plumbing.
pub type PackageSearch = Rc<dyn Fn(String, Box<dyn FnOnce(Vec<String>)>)>;

/// Extra context a control needs beyond its own record: the flake's available
/// module names, split by kind, and the package search hook.
#[derive(Default, Clone)]
pub struct BuildContext {
    pub nixos_modules: Vec<String>,
    pub hm_modules: Vec<String>,
    pub search: Option<PackageSearch>,
}

/// The editing control backing one option. Each variant knows how to render
/// its current state back to Nix and whether it still holds the original value.
pub enum Editor {
    Switch(gtk::Switch),
    Dropdown {
        combo: gtk::DropDown,
        choices: Vec<String>,
    },
    Spin(gtk::SpinButton),
    Slider(gtk::Scale),
    Text(gtk::Entry),
    Multiline {
        view: gtk::TextView,
        root: gtk::ScrolledWindow,
    },
    Path(gtk::Entry),
    Package {
        root: gtk::Box,
        entry: gtk::Entry,
    },
    List {
        root: gtk::Box,
        entries: Rc<RefCell<Vec<gtk::Entry>>>,
        packages: bool,
    },
    ModuleList {
        root: gtk::Box,
        entries: Rc<RefCell<Vec<gtk::Entry>>>,
        prefix: String,
    },
    Color(gtk::ColorDialogButton),
    Submodule {
        root: gtk::Box,
        children: Vec<(Record, Editor)>,
    },
    Raw {
        view: gtk::TextView,
        root: gtk::ScrolledWindow,
    },
}

impl Editor {
    pub fn root(&self) -> gtk::Widget {
        match self {
            Editor::Switch(w) => w.clone().upcast(),
            Editor::Dropdown { combo, .. } => combo.clone().upcast(),
            Editor::Spin(w) => w.clone().upcast(),
            Editor::Slider(w) => w.clone().upcast(),
            Editor::Text(w) => w.clone().upcast(),
            Editor::Multiline { root, .. } => root.clone().upcast(),
            Editor::Path(w) => w.clone().upcast(),
            Editor::Package { root, .. } => root.clone().upcast(),
            Editor::List { root, .. } => root.clone().upcast(),
            Editor::ModuleList { root, .. } => root.clone().upcast(),
            Editor::Color(w) => w.clone().upcast(),
            Editor::Submodule { root, .. } => root.clone().upcast(),
            Editor::Raw { root, .. } => root.clone().upcast(),
        }
    }

    /// Whether this editor holds a text input (or another width-hungry control)
    /// and should be stretched to fill the space left of its row label.
    pub fn wants_width(&self) -> bool {
        matches!(
            self,
            Editor::Text(_)
                | Editor::Path(_)
                | Editor::Multiline { .. }
                | Editor::Raw { .. }
                | Editor::Package { .. }
                | Editor::List { .. }
                | Editor::ModuleList { .. }
                | Editor::Submodule { .. }
                | Editor::Slider(_)
        )
    }

    /// Render the editor's current state back to Nix source.
    pub fn to_nix(&self) -> String {
        match self {
            Editor::Switch(w) => if w.is_active() { "true" } else { "false" }.to_string(),
            Editor::Dropdown { combo, choices } => choices
                .get(combo.selected() as usize)
                .cloned()
                .unwrap_or_else(|| quote("")),
            Editor::Spin(w) => number_literal(w.value(), w.digits()),
            Editor::Slider(w) => number_literal(w.value(), 0),
            Editor::Text(w) => string_literal(&w.text()),
            Editor::Multiline { view, .. } => {
                let buffer = view.buffer();
                string_literal(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false))
            }
            Editor::Path(w) => path_literal(&w.text()),
            Editor::Package { entry, .. } => package_literal(&entry.text()),
            Editor::List {
                entries, packages, ..
            } => {
                let raw: Vec<String> = entries
                    .borrow()
                    .iter()
                    .map(|e| e.text().to_string())
                    .collect();
                if *packages {
                    serialize_packages(&raw)
                } else {
                    let items: Vec<String> = raw
                        .iter()
                        .map(|t| expr_literal(t))
                        .filter(|t| !t.is_empty() && t != "\"\"")
                        .collect();
                    value::serialize_list(&items)
                }
            }
            Editor::ModuleList {
                entries, prefix, ..
            } => {
                let names: Vec<String> = entries
                    .borrow()
                    .iter()
                    .map(|e| e.text().trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect();
                serialize_modules(&names, prefix)
            }
            Editor::Color(w) => {
                let rgba = w.rgba();
                format!(
                    "\"#{:02x}{:02x}{:02x}\"",
                    (rgba.red() * 255.0).round() as u8,
                    (rgba.green() * 255.0).round() as u8,
                    (rgba.blue() * 255.0).round() as u8
                )
            }
            Editor::Submodule { .. } => "{ }".to_string(),
            Editor::Raw { view, .. } => {
                let buffer = view.buffer();
                buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string()
            }
        }
    }

    /// Whether the editor still holds the original value, ignoring formatting
    /// differences, so an untouched control never triggers a rewrite.
    pub fn unchanged(&self, original: &str) -> bool {
        let original = unwrap_modifiers(original);
        let original = original.as_str();
        match self {
            Editor::Switch(w) => w.is_active() == truthy(original),
            Editor::Spin(w) => match original.trim().parse::<f64>() {
                Ok(v) => (w.value() - v).abs() < 1e-9,
                Err(_) => self.to_nix().trim() == original.trim(),
            },
            Editor::Slider(w) => match original.trim().parse::<f64>() {
                Ok(v) => (w.value() - v).abs() < 1e-9,
                Err(_) => self.to_nix().trim() == original.trim(),
            },
            Editor::Text(w) => w.text() == entry_text(original),
            Editor::Path(w) => w.text() == entry_text(original),
            Editor::Multiline { view, .. } => {
                let buffer = view.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                text.as_str() == parse_string_literal(original)
            }
            Editor::List { packages, .. } => match canonical_list(original, *packages) {
                Some(canonical) => self.to_nix().trim() == canonical.trim(),
                None => self.to_nix().trim() == original.trim(),
            },
            Editor::ModuleList { prefix, .. } => match canonical_module_list(original, prefix) {
                Some(canonical) => self.to_nix().trim() == canonical.trim(),
                None => self.to_nix().trim() == original.trim(),
            },
            Editor::Package { entry, .. } => {
                package_literal(&entry.text()) == package_literal(&entry_text(original))
            }
            Editor::Submodule { children, .. } => children
                .iter()
                .all(|(record, editor)| editor.unchanged(&record.value)),
            _ => self.to_nix().trim() == original.trim(),
        }
    }

    /// The edits this control would make, empty when it is unchanged. A
    /// submodule emits one edit per nested control.
    pub fn edits(&self, record: &Record) -> Vec<PendingEdit> {
        if let Editor::Submodule { children, .. } = self {
            let mut out = Vec::new();
            for (child, editor) in children {
                out.extend(editor.edits(child));
            }
            return out;
        }
        if self.unchanged(&record.value) {
            Vec::new()
        } else {
            vec![PendingEdit {
                file: record.file.clone(),
                start: record.byte_start,
                end: record.byte_end,
                value: self.to_nix(),
            }]
        }
    }
}

/// Build the appropriate control for a record. `type_str` is the nixd type when
/// it is known; the `@option widget` takes precedence, then the type, then the
/// shape of the current value.
pub fn build(record: &Record, type_str: Option<&str>, ctx: &BuildContext) -> Editor {
    let spec = &record.spec;
    let value_text = unwrap_modifiers(&record.value);
    let value_text = value_text.as_str();
    match choose_widget(record, type_str) {
        WidgetKind::Switch => Editor::Switch(build_switch(value_text)),
        WidgetKind::Dropdown | WidgetKind::Radio => {
            let choices = resolve_choices(spec, type_str);
            build_dropdown(&choices, value_text)
        }
        WidgetKind::Spin => Editor::Spin(build_spin(spec, value_text)),
        WidgetKind::Slider => Editor::Slider(build_slider(spec, value_text)),
        WidgetKind::Text => Editor::Text(build_entry(value_text, spec, false)),
        WidgetKind::Multiline => {
            let (view, root) = build_multiline(value_text, spec);
            Editor::Multiline { view, root }
        }
        WidgetKind::Path => Editor::Path(build_entry(value_text, spec, false)),
        WidgetKind::Package => build_package_editor(value_text, spec, ctx.search.as_ref()),
        WidgetKind::Packages => build_list(value_text, true, ctx.search.as_ref()),
        WidgetKind::List => build_list(value_text, false, None),
        WidgetKind::NixosModules => {
            build_module_list(value_text, ModuleKind::Nixos, &ctx.nixos_modules, record)
        }
        WidgetKind::HmModules => {
            build_module_list(value_text, ModuleKind::Home, &ctx.hm_modules, record)
        }
        WidgetKind::Color => Editor::Color(build_color(value_text)),
        WidgetKind::Submodule => build_submodule(record, type_str),
        WidgetKind::Raw => {
            let (view, root) = build_raw(value_text, spec);
            Editor::Raw { view, root }
        }
    }
}

/// Whether a record is annotated as a single package or a package list.
pub fn is_package(record: &Record) -> bool {
    matches!(
        record.spec.widget,
        Some(WidgetKind::Package) | Some(WidgetKind::Packages)
    )
}

fn choose_widget(record: &Record, type_str: Option<&str>) -> WidgetKind {
    let spec = &record.spec;
    let value_text = &record.value;
    if let Some(widget) = spec.widget {
        if widget == WidgetKind::Spin && spec.min.is_some() && spec.max.is_some() {
            return WidgetKind::Slider;
        }
        return widget;
    }
    if let Some(widget) = import_widget_kind(record) {
        return widget;
    }
    if let Some(widget) = type_str.and_then(widget_from_type) {
        return widget;
    }
    widget_from_kind(value::classify(value_text))
}

/// A top-level `imports` list becomes a dedicated module-import control: NixOS
/// modules for a `modules.nixos.*` origin, home-manager modules for
/// `modules.homeManager.*`.
fn import_widget_kind(record: &Record) -> Option<WidgetKind> {
    if record.segments.len() != 1 || record.segments[0] != "imports" {
        return None;
    }
    if record.origin.starts_with("modules.nixos.") {
        Some(WidgetKind::NixosModules)
    } else if record.origin.starts_with("modules.homeManager.") {
        Some(WidgetKind::HmModules)
    } else {
        None
    }
}

fn widget_from_type(type_str: &str) -> Option<WidgetKind> {
    let t = type_str.to_lowercase();
    if t.contains("one of") {
        Some(WidgetKind::Dropdown)
    } else if t.contains("list") && t.contains("package") {
        Some(WidgetKind::Packages)
    } else if t.contains("list") {
        Some(WidgetKind::List)
    } else if t.contains("package") {
        Some(WidgetKind::Package)
    } else if t.contains("path") {
        Some(WidgetKind::Path)
    } else if t.contains("bool") {
        Some(WidgetKind::Switch)
    } else if t.contains("float") || t.contains("number") || t.contains("int") {
        Some(WidgetKind::Spin)
    } else if t.contains("submodule") || t.contains("attribute set") || t.contains("attrs") {
        Some(WidgetKind::Submodule)
    } else if t.contains("string") {
        Some(WidgetKind::Text)
    } else {
        None
    }
}

fn widget_from_kind(kind: value::ValueKind) -> WidgetKind {
    match kind {
        value::ValueKind::Bool => WidgetKind::Switch,
        value::ValueKind::Int | value::ValueKind::Float => WidgetKind::Spin,
        value::ValueKind::String => WidgetKind::Text,
        value::ValueKind::List => WidgetKind::List,
        value::ValueKind::AttrSet => WidgetKind::Submodule,
        _ => WidgetKind::Raw,
    }
}

fn resolve_choices(spec: &OptionSpec, type_str: Option<&str>) -> Vec<String> {
    if let Some(choices) = &spec.choices {
        return choices.clone();
    }
    type_str.and_then(enum_choices).unwrap_or_default()
}

/// Pull the quoted members out of a `one of "a", "b"` type clause.
fn enum_choices(type_str: &str) -> Option<Vec<String>> {
    let lower = type_str.to_lowercase();
    let index = lower.find("one of")?;
    let mut out = Vec::new();
    let mut chars = type_str[index..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut item = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                item.push(c);
            }
            out.push(item);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn build_switch(value_text: &str) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(truthy(value_text));
    switch.set_halign(gtk::Align::Start);
    switch
}

fn build_dropdown(choices: &[String], value_text: &str) -> Editor {
    let current = value_text.trim();
    let mut labels: Vec<String> = choices.to_vec();
    let mut literals: Vec<String> = choices.iter().map(|c| choice_literal(c)).collect();
    if !literals.iter().any(|l| l == current) {
        for existing in literals.iter_mut() {
            if existing.is_empty() {
                *existing = current.to_string();
                break;
            }
        }
        if !literals.iter().any(|l| l == current) {
            labels.push(value_text.trim().to_string());
            literals.push(current.to_string());
        }
    }
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let combo = gtk::DropDown::from_strings(&refs);
    let selected = literals.iter().position(|l| l == current).unwrap_or(0);
    combo.set_selected(selected as u32);
    combo.set_halign(gtk::Align::Start);
    Editor::Dropdown {
        combo,
        choices: literals,
    }
}

fn build_spin(spec: &OptionSpec, value_text: &str) -> gtk::SpinButton {
    let min = spec.min.unwrap_or(-1.0e15);
    let max = spec.max.unwrap_or(1.0e15);
    let step = spec.step.unwrap_or(1.0);
    let default_digits = if value_text.contains('.') { 2 } else { 0 };
    let spin = gtk::SpinButton::with_range(min, max, step);
    spin.set_digits(spec.digits.unwrap_or(default_digits));
    spin.set_value(value_text.trim().parse::<f64>().unwrap_or(0.0));
    spin.set_halign(gtk::Align::Start);
    spin
}

fn build_slider(spec: &OptionSpec, value_text: &str) -> gtk::Scale {
    let min = spec.min.unwrap_or(0.0);
    let max = spec.max.unwrap_or(100.0);
    let step = spec.step.unwrap_or(1.0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_value(value_text.trim().parse::<f64>().unwrap_or(min));
    scale.set_draw_value(true);
    scale.set_hexpand(true);
    scale
}

fn build_entry(value_text: &str, spec: &OptionSpec, monospace: bool) -> gtk::Entry {
    let unquoted = entry_text(value_text);
    let entry = gtk::Entry::new();
    entry.set_text(&unquoted);
    entry.set_hexpand(true);
    entry.set_width_chars(44);
    if monospace {
        entry.add_css_class("monospace");
    }
    if let Some(placeholder) = &spec.placeholder {
        entry.set_placeholder_text(Some(placeholder));
    }
    entry
}

/// Strip the surrounding quotes from a simple double-quoted string literal.
fn unescape(text: &str) -> String {
    let inner = &text[1..text.len() - 1];
    inner
        .replace("\\\"", "\"")
        .replace("\\${", "${")
        .replace("\\\\", "\\")
}

/// The text an entry shows for a value: a quoted literal is unquoted, anything
/// else is trimmed as-is.
fn entry_text(value: &str) -> String {
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        unescape(value)
    } else {
        value.trim().to_string()
    }
}

/// Decode a `"..."` or `''...''` string literal into its raw text.
fn parse_string_literal(text: &str) -> String {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix("''").and_then(|s| s.strip_suffix("''")) {
        return inner.trim_matches('\n').to_string();
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        return unescape(t);
    }
    t.to_string()
}

fn build_multiline(value_text: &str, spec: &OptionSpec) -> (gtk::TextView, gtk::ScrolledWindow) {
    let view = gtk::TextView::new();
    view.buffer().set_text(&parse_string_literal(value_text));
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.set_left_margin(4);
    view.set_right_margin(4);
    let rows = spec.rows.unwrap_or(4).max(2);
    let root = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .min_content_width(440)
        .min_content_height((rows as i32) * 24)
        .has_frame(true)
        .build();
    (view, root)
}

fn build_raw(value_text: &str, spec: &OptionSpec) -> (gtk::TextView, gtk::ScrolledWindow) {
    let view = gtk::TextView::new();
    view.buffer().set_text(value_text);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.set_left_margin(4);
    view.set_right_margin(4);
    let rows = spec.rows.unwrap_or(5).max(2);
    let root = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .min_content_width(440)
        .min_content_height((rows as i32) * 22)
        .has_frame(true)
        .build();
    (view, root)
}

fn build_color(value_text: &str) -> gtk::ColorDialogButton {
    let dialog = gtk::ColorDialog::new();
    let button = gtk::ColorDialogButton::new(Some(dialog));
    let raw = value_text.trim().trim_matches('"');
    if let Ok(rgba) = gtk::gdk::RGBA::parse(raw) {
        button.set_rgba(&rgba);
    }
    button.set_halign(gtk::Align::Start);
    button
}

fn build_list(value_text: &str, packages: bool, search: Option<&PackageSearch>) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    let entries: Rc<RefCell<Vec<gtk::Entry>>> = Rc::new(RefCell::new(Vec::new()));

    let add = gtk::Button::with_label("Add item");
    add.set_halign(gtk::Align::Start);
    {
        let root = root.clone();
        let entries = entries.clone();
        let search = search.cloned();
        add.connect_clicked(move |_| add_list_row(&root, &entries, "", packages, search.as_ref()));
    }
    root.append(&add);

    for item in value::list_items(value_text).unwrap_or_default() {
        add_list_row(&root, &entries, &item, packages, search);
    }

    Editor::List {
        root,
        entries,
        packages,
    }
}

fn add_list_row(
    root: &gtk::Box,
    entries: &Rc<RefCell<Vec<gtk::Entry>>>,
    text: &str,
    packages: bool,
    search: Option<&PackageSearch>,
) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let entry = gtk::Entry::new();
    entry.set_text(text);
    entry.set_hexpand(true);
    if packages {
        entry.add_css_class("monospace");
        entry.set_editable(false);
        entry.set_tooltip_text(Some("Choose a package with the search button"));
    }
    if let Some(search) = search {
        row.append(&attach_package_search(&entry, search));
    }
    let remove = gtk::Button::with_label("Remove");
    {
        let root = root.clone();
        let entries = entries.clone();
        let row = row.clone();
        let entry = entry.clone();
        remove.connect_clicked(move |_| {
            root.remove(&row);
            entries.borrow_mut().retain(|e| *e != entry);
        });
    }
    row.append(&entry);
    row.append(&remove);
    root.append(&row);
    entries.borrow_mut().push(entry);
}

/// A single package value: an entry plus a search button that opens a popover
/// of nixd-suggested package names.
fn build_package_editor(
    value_text: &str,
    spec: &OptionSpec,
    search: Option<&PackageSearch>,
) -> Editor {
    let entry = build_entry(value_text, spec, true);
    entry.set_editable(false);
    entry.set_tooltip_text(Some("Choose a package with the search button"));
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    root.set_hexpand(true);
    if let Some(search) = search {
        root.append(&attach_package_search(&entry, search));
    }
    root.append(&entry);
    Editor::Package { root, entry }
}

/// A search button backed by the app's package search. Picking a result writes
/// it into `entry`.
fn attach_package_search(entry: &gtk::Entry, search: &PackageSearch) -> gtk::MenuButton {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list)
        .min_content_height(160)
        .max_content_height(280)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Search packages")
        .build();
    let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    pop_box.set_size_request(280, -1);
    pop_box.append(&search_entry);
    pop_box.append(&scrolled);
    let popover = gtk::Popover::builder().child(&pop_box).build();
    let button = gtk::MenuButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Search packages")
        .popover(&popover)
        .build();
    button.add_css_class("flat");

    let entry = entry.clone();
    let search: PackageSearch = Rc::clone(search);
    let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    search_entry.connect_search_changed(move |field| {
        let current = generation.get() + 1;
        generation.set(current);
        let term = field.text().to_string();
        if term.trim().len() < 2 {
            clear_rows(&list);
            return;
        }
        let entry = entry.clone();
        let list = list.clone();
        let popover = popover.clone();
        let search = PackageSearch::clone(&search);
        let generation = generation.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            if generation.get() != current {
                return;
            }
            let entry = entry.clone();
            let list = list.clone();
            let popover = popover.clone();
            search(
                term,
                Box::new(move |names| {
                    clear_rows(&list);
                    for name in names.into_iter().take(50) {
                        let row = gtk::Button::with_label(&name);
                        row.add_css_class("flat");
                        row.set_halign(gtk::Align::Start);
                        let entry = entry.clone();
                        let popover = popover.clone();
                        row.connect_clicked(move |_| {
                            entry.set_text(&name);
                            popover.popdown();
                        });
                        list.append(&row);
                    }
                }),
            );
        });
    });
    button
}

fn clear_rows(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn build_module_list(
    value_text: &str,
    kind: ModuleKind,
    choices: &[String],
    _record: &Record,
) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    let entries: Rc<RefCell<Vec<gtk::Entry>>> = Rc::new(RefCell::new(Vec::new()));
    let prefixes = kind.prefixes();

    if !choices.is_empty() {
        let labels: Vec<&str> = choices.iter().map(String::as_str).collect();
        let chooser = gtk::DropDown::from_strings(&labels);
        chooser.set_halign(gtk::Align::Start);
        let add = gtk::Button::with_label("Add module");
        {
            let root = root.clone();
            let entries = entries.clone();
            let chooser = chooser.clone();
            let choices = choices.to_vec();
            add.connect_clicked(move |_| {
                if let Some(name) = choices.get(chooser.selected() as usize) {
                    add_module_row(&root, &entries, name);
                }
            });
        }
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.append(&chooser);
        bar.append(&add);
        root.append(&bar);
    }

    let mut prefix = String::new();
    for item in module_items(value_text).unwrap_or_default() {
        let raw = item.trim();
        let name = prefixes
            .iter()
            .find_map(|p| raw.strip_prefix(p.as_str()))
            .unwrap_or(raw);
        if prefix.is_empty() {
            if let Some(p) = prefixes.iter().find(|p| raw.starts_with(p.as_str())) {
                prefix = p.clone();
            }
        }
        add_module_row(&root, &entries, name);
    }

    Editor::ModuleList {
        root,
        entries,
        prefix,
    }
}

fn add_module_row(root: &gtk::Box, entries: &Rc<RefCell<Vec<gtk::Entry>>>, name: &str) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let label = gtk::Entry::new();
    label.set_text(name);
    label.set_editable(false);
    label.set_hexpand(true);
    let remove = gtk::Button::with_label("Remove");
    {
        let root = root.clone();
        let entries = entries.clone();
        let row = row.clone();
        let label = label.clone();
        remove.connect_clicked(move |_| {
            root.remove(&row);
            entries.borrow_mut().retain(|e| *e != label);
        });
    }
    row.append(&label);
    row.append(&remove);
    root.append(&row);
    entries.borrow_mut().push(label);
}

fn build_submodule(record: &Record, type_str: Option<&str>) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let mut children = Vec::new();
    for mut child in scan::scan_value(&record.value) {
        offset_record(&mut child, record.byte_start, &record.file);
        let editor = build(&child, type_str, &BuildContext::default());
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let label = child
            .spec
            .label
            .clone()
            .or_else(|| child.segments.last().cloned())
            .unwrap_or_else(|| child.path());
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label(label)
            .css_classes(["dim-label"])
            .build();
        row.append(&heading);
        row.append(&editor.root());
        root.append(&row);
        children.push((child, editor));
    }
    Editor::Submodule { root, children }
}

/// Shift a fragment record's spans into whole-file offsets.
fn offset_record(record: &mut Record, base: usize, file: &str) {
    record.byte_start += base;
    record.byte_end += base;
    if let Some((start, end)) = record.comment_span {
        record.comment_span = Some((start + base, end + base));
    }
    record.file = file.to_string();
}

/// Serialize a hand-entered value for a chosen widget kind. Used by the
/// add-option flow to build the assignment it inserts.
pub fn literal_for_widget(widget: WidgetKind, value: &str) -> String {
    let t = value.trim();
    match widget {
        WidgetKind::Switch => {
            if matches!(t, "true" | "false") {
                t.to_string()
            } else {
                "false".to_string()
            }
        }
        WidgetKind::Spin | WidgetKind::Slider => {
            if t.parse::<f64>().is_ok() {
                t.to_string()
            } else {
                "0".to_string()
            }
        }
        WidgetKind::Text | WidgetKind::Multiline | WidgetKind::Color => string_literal(value),
        WidgetKind::Path => path_literal(value),
        WidgetKind::Dropdown | WidgetKind::Radio => choice_literal(value),
        WidgetKind::Packages => {
            let items = value::list_items(value).unwrap_or_else(|| vec![value.trim().to_string()]);
            serialize_packages(&items)
        }
        WidgetKind::Package => package_literal(value),
        WidgetKind::List
        | WidgetKind::NixosModules
        | WidgetKind::HmModules
        | WidgetKind::Raw
        | WidgetKind::Submodule => expr_literal(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_escapes_strings() {
        assert_eq!(quote("hello"), "\"hello\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote("${x}"), "\"\\${x}\"");
    }

    #[test]
    fn multiline_uses_indented_string() {
        assert_eq!(string_literal("one\ntwo"), "''\none\ntwo\n''");
        assert_eq!(string_literal("one"), "\"one\"");
    }

    #[test]
    fn distinguishes_choice_literals() {
        assert_eq!(choice_literal("auto"), "\"auto\"");
        assert_eq!(choice_literal("true"), "true");
        assert_eq!(choice_literal("42"), "42");
    }

    #[test]
    fn keeps_valid_expressions_verbatim() {
        assert_eq!(expr_literal("pkgs.git"), "pkgs.git");
        assert_eq!(expr_literal("\"nix-command\""), "\"nix-command\"");
        assert_eq!(expr_literal("Europe/Amsterdam"), "Europe/Amsterdam");
        assert_eq!(expr_literal("two words"), "two words");
        assert_eq!(expr_literal("("), "\"(\"");
    }

    #[test]
    fn number_formatting_respects_digits() {
        assert_eq!(number_literal(8.0, 0), "8");
        assert_eq!(number_literal(1.5, 2), "1.50");
    }

    #[test]
    fn parses_enum_choices_from_type() {
        assert_eq!(
            enum_choices("one of \"a\", \"b\", \"c\""),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(enum_choices("list of string"), None);
    }

    #[test]
    fn type_mapping_covers_common_kinds() {
        assert_eq!(widget_from_type("boolean"), Some(WidgetKind::Switch));
        assert_eq!(
            widget_from_type("list of package"),
            Some(WidgetKind::Packages)
        );
        assert_eq!(widget_from_type("path"), Some(WidgetKind::Path));
        assert_eq!(widget_from_type("list of string"), Some(WidgetKind::List));
        assert_eq!(widget_from_type("freeform"), None);
    }

    #[test]
    fn path_literals_are_recognised() {
        assert_eq!(path_literal("./foo"), "./foo");
        assert_eq!(path_literal("/etc/x"), "/etc/x");
        assert_eq!(path_literal("foo"), "\"foo\"");
    }

    #[test]
    fn serialises_add_option_values() {
        assert_eq!(literal_for_widget(WidgetKind::Switch, "true"), "true");
        assert_eq!(literal_for_widget(WidgetKind::Spin, "8"), "8");
        assert_eq!(literal_for_widget(WidgetKind::Text, "hello"), "\"hello\"");
        assert_eq!(
            literal_for_widget(WidgetKind::Packages, "[ pkgs.git pkgs.vim ]"),
            "[ pkgs.git pkgs.vim ]"
        );
    }

    #[test]
    fn qualifies_single_bare_packages() {
        assert_eq!(literal_for_widget(WidgetKind::Package, "git"), "pkgs.git");
        assert_eq!(
            literal_for_widget(WidgetKind::Package, "pkgs.git"),
            "pkgs.git"
        );
        assert_eq!(package_literal("git"), "pkgs.git");
        assert_eq!(package_literal("pkgs.vim"), "pkgs.vim");
    }

    #[test]
    fn wraps_bare_packages_in_with_pkgs() {
        assert_eq!(
            literal_for_widget(WidgetKind::Packages, "[ git vim ]"),
            "with pkgs; [ git vim ]"
        );
        assert_eq!(
            serialize_packages(&["git".into(), "python3".into()]),
            "with pkgs; [ git python3 ]"
        );
        assert_eq!(
            serialize_packages(&["pkgs.git".into(), "vim".into()]),
            "[ pkgs.git vim ]"
        );
        assert!(is_bare_package("gcc-13"));
        assert!(!is_bare_package("pkgs.git"));
        assert!(!is_bare_package("git.override {}"));
    }

    #[test]
    fn canonicalises_multiline_lists() {
        let list = "[\n      \"nix-command\"\n      \"flakes\"\n    ]";
        assert_eq!(
            canonical_list(list, false),
            Some("[ \"nix-command\" \"flakes\" ]".to_string())
        );
        let packages = "[\n        git\n        vim\n      ]";
        assert_eq!(
            canonical_list(packages, true),
            Some("with pkgs; [ git vim ]".to_string())
        );
        assert_eq!(canonical_list("not a list", false), None);
    }

    #[test]
    fn canonicalises_module_lists_ignoring_scope() {
        let original = "[\n  self.modules.nixos.common\n  self.modules.nixos.audio\n]";
        assert_eq!(
            canonical_module_list(original, "self.modules.nixos."),
            Some("[ self.modules.nixos.common self.modules.nixos.audio ]".to_string())
        );
        assert_eq!(
            canonical_module_list("[ common ]", ""),
            Some("[ common ]".to_string())
        );
    }

    #[test]
    fn unwraps_value_modifiers() {
        assert_eq!(unwrap_modifiers("lib.mkDefault true"), "true");
        assert_eq!(unwrap_modifiers("lib.mkForce pkgs.git"), "pkgs.git");
        assert_eq!(unwrap_modifiers("lib.mkOverride 70 [ \"a\" ]"), "[ \"a\" ]");
        assert_eq!(unwrap_modifiers("  true  "), "true");
        assert_eq!(unwrap_modifiers("lib.mkDefault"), "lib.mkDefault");
    }

    #[test]
    fn reads_nix_truthiness() {
        assert!(truthy("true"));
        assert!(truthy("1"));
        assert!(truthy("lib.mkDefault true"));
        assert!(truthy("pkgs.linuxPackages_latest"));
        assert!(!truthy("false"));
        assert!(!truthy("0"));
        assert!(!truthy("null"));
    }

    #[test]
    fn canonicalises_single_module_reference() {
        let original = "self.modules.homeManager.homeconfig-den";
        assert_eq!(
            canonical_module_list(original, "self.modules.homeManager."),
            Some("[ self.modules.homeManager.homeconfig-den ]".to_string())
        );
        assert_eq!(
            module_items("[ a b ]"),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            module_items("self.modules.nixos.foo"),
            Some(vec!["self.modules.nixos.foo".to_string()])
        );
        assert_eq!(module_items("1 + 2"), None);
    }

    #[test]
    fn package_and_entry_text_round_trip() {
        assert_eq!(entry_text("\"en_US.UTF-8\""), "en_US.UTF-8");
        assert_eq!(entry_text(" pkgs.git "), "pkgs.git");
        assert_eq!(package_literal(&entry_text("\"git\"")), "pkgs.git");
        assert_eq!(package_literal(&entry_text("pkgs.git")), "pkgs.git");
    }

    #[test]
    fn serialises_module_lists() {
        let names = vec!["audio".to_string(), "desktop".to_string()];
        assert_eq!(
            serialize_modules(&names, "flake.modules.nixos."),
            "[ flake.modules.nixos.audio flake.modules.nixos.desktop ]"
        );
        assert_eq!(
            serialize_modules(&names, "self.modules.homeManager."),
            "[ self.modules.homeManager.audio self.modules.homeManager.desktop ]"
        );
        assert_eq!(serialize_modules(&names, ""), "[ audio desktop ]");
        assert_eq!(serialize_modules(&[], "flake.modules.nixos."), "[]");
    }

    #[test]
    fn maps_imports_to_module_widgets() {
        let mut record = Record {
            segments: vec!["imports".to_string()],
            origin: "modules.nixos.audio".to_string(),
            ..Record::default()
        };
        assert_eq!(import_widget_kind(&record), Some(WidgetKind::NixosModules));

        record.origin = "modules.homeManager.desktop".to_string();
        assert_eq!(import_widget_kind(&record), Some(WidgetKind::HmModules));

        record.origin = String::new();
        assert_eq!(import_widget_kind(&record), None);

        record.origin = "modules.nixos.audio".to_string();
        record.segments = vec!["services".to_string(), "imports".to_string()];
        assert_eq!(import_widget_kind(&record), None);
    }
}
