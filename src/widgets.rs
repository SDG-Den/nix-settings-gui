use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::dsl::{OptionSpec, WidgetKind};
use crate::edit::PendingEdit;
use crate::model::Record;
use crate::scan;
use crate::value;

/// A late-bound change callback, set when the app wires a table editor's rows
/// so dynamically added rows can notify the same handler.
type ChangeHook = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

mod schemes {
    include!(concat!(env!("OUT_DIR"), "/schemes.rs"));
}

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

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

/// Serialize a package list as the editor writes it into the existing value
/// span. The span is the inner list (the file's own `with pkgs;` sits outside
/// it), so this stays a plain list and never adds a wrapper.
fn package_items(entries: &[String]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|e| expr_literal(e))
        .filter(|e| !e.is_empty() && e != "\"\"")
        .collect();
    value::serialize_list(&items)
}

/// Serialize a standalone package assignment. Bare names need `pkgs` in scope,
/// so they go under `with pkgs;`; already-qualified expressions stay verbatim.
/// Used when inserting a brand-new option, which has no wrapper of its own.
fn serialize_packages(entries: &[String]) -> String {
    let cleaned: Vec<String> = entries
        .iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    if !cleaned.is_empty() && cleaned.iter().all(|e| is_bare_package(e)) {
        format!("with pkgs; {}", package_items(entries))
    } else {
        package_items(entries)
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
        Some(package_items(&items))
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
    /// Whether the Advanced toggle is on, so nested hidden controls show.
    pub advanced: bool,
    /// Nesting depth of the control being built, used for submodule indent.
    pub depth: usize,
}

/// One cell of a table row: a keyed value with an inferred editor.
pub(crate) struct TableCell {
    /// The column name this cell belongs to.
    column: String,
    key: gtk::Entry,
    editor: Editor,
    orig_key: String,
    orig_value: String,
}

/// One editable row of a table.
pub(crate) struct TableRow {
    key_entry: gtk::Entry,
    orig_key: String,
    cells: Vec<TableCell>,
}

/// One freeform field of a bespoke widget row: a named value with an inferred
/// editor.
pub(crate) struct AttrsetField {
    name: String,
    editor: Editor,
    orig_value: String,
    orig_present: bool,
}

/// One row of an id-keyed attribute set editor (bar widget lists, dock
/// widgets, menu entries).
pub(crate) struct AttrsetRow {
    key: gtk::Entry,
    orig_key: String,
    fields: Vec<AttrsetField>,
    row_box: gtk::Box,
    remove: gtk::Button,
}

/// One bind of the `keybinds` widget: the bind name, a `modifier+keys`
/// preview, and the bind's fields.
pub(crate) struct KeybindRow {
    key: gtk::Entry,
    orig_key: String,
    modifier: gtk::Entry,
    keys: gtk::Entry,
    orig_modifier: String,
    orig_keys: String,
    fields: Vec<AttrsetField>,
    row_box: gtk::Box,
    remove: gtk::Button,
}

/// One monitor rule of the `monitors` widget: core geometry editors plus the
/// rule's remaining fields and the original source text per field.
pub(crate) struct MonitorRow {
    name: gtk::Entry,
    width: gtk::SpinButton,
    height: gtk::SpinButton,
    refresh: gtk::SpinButton,
    x: gtk::SpinButton,
    y: gtk::SpinButton,
    scale: gtk::SpinButton,
    rr: gtk::Entry,
    extras: Vec<AttrsetField>,
    orig: Vec<(String, String)>,
    form_box: gtk::Box,
    remove: gtk::Button,
}

/// One plugin entry of the `plugins` widget: the plugin name, its enable
/// switch, and the entry's remaining fields as nested child rows.
pub(crate) struct PluginRow {
    key: gtk::Entry,
    orig_key: String,
    enable: Editor,
    enable_present: bool,
    children: Vec<(Record, Editor)>,
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
    HorizontalSubmodule {
        root: gtk::Box,
        children: Vec<(Record, Editor)>,
    },
    Table {
        root: gtk::Box,
        rows: Rc<RefCell<Vec<TableRow>>>,
        list: bool,
        key_editable: bool,
        columns: Vec<String>,
        on_change: ChangeHook,
    },
    Theme {
        root: gtk::Box,
        current: Rc<RefCell<String>>,
        list: gtk::ListBox,
    },
    Keybinds {
        root: gtk::Box,
        rows: Rc<RefCell<Vec<KeybindRow>>>,
        on_change: ChangeHook,
    },
    Monitors {
        root: gtk::Box,
        rows: Rc<RefCell<Vec<MonitorRow>>>,
        on_change: ChangeHook,
    },
    Menu {
        root: gtk::Box,
        rows: Rc<RefCell<Vec<AttrsetRow>>>,
        on_change: ChangeHook,
    },
    Plugins {
        root: gtk::Box,
        rows: Rc<RefCell<Vec<PluginRow>>>,
        on_change: ChangeHook,
    },
    Bar {
        root: gtk::Box,
        left: Rc<RefCell<Vec<AttrsetRow>>>,
        right: Rc<RefCell<Vec<AttrsetRow>>>,
        center: Box<Editor>,
        center_orig: String,
        left_present: bool,
        right_present: bool,
        center_present: bool,
        extras: Vec<(Record, Editor)>,
        on_change: ChangeHook,
    },
    Dock {
        root: gtk::Box,
        widgets: Rc<RefCell<Vec<AttrsetRow>>>,
        widgets_present: bool,
        extras: Vec<(Record, Editor)>,
        on_change: ChangeHook,
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
            Editor::HorizontalSubmodule { root, .. } => root.clone().upcast(),
            Editor::Table { root, .. } => root.clone().upcast(),
            Editor::Theme { root, .. } => root.clone().upcast(),
            Editor::Keybinds { root, .. } => root.clone().upcast(),
            Editor::Monitors { root, .. } => root.clone().upcast(),
            Editor::Menu { root, .. } => root.clone().upcast(),
            Editor::Plugins { root, .. } => root.clone().upcast(),
            Editor::Bar { root, .. } => root.clone().upcast(),
            Editor::Dock { root, .. } => root.clone().upcast(),
            Editor::Raw { root, .. } => root.clone().upcast(),
        }
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
                    package_items(&raw)
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
            Editor::HorizontalSubmodule { .. } => "{ }".to_string(),
            Editor::Table { .. } => table_nix(self),
            Editor::Theme { current, .. } => quote(&current.borrow()),
            Editor::Keybinds { rows, .. } => keybinds_nix(&rows.borrow()),
            Editor::Monitors { rows, .. } => monitors_nix(&rows.borrow()),
            Editor::Menu { rows, .. } => attrset_rows_nix(&rows.borrow()),
            Editor::Plugins { rows, .. } => plugins_nix(&rows.borrow()),
            Editor::Bar {
                left,
                right,
                center,
                center_orig: _,
                left_present,
                right_present,
                center_present,
                extras,
                ..
            } => bar_nix(
                left,
                right,
                center,
                *left_present,
                *right_present,
                *center_present,
                extras,
            ),
            Editor::Dock {
                widgets,
                widgets_present,
                extras,
                ..
            } => dock_nix(widgets, *widgets_present, extras),
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
            Editor::HorizontalSubmodule { children, .. } => children
                .iter()
                .all(|(record, editor)| editor.unchanged(&record.value)),
            Editor::Table {
                rows,
                list,
                key_editable,
                ..
            } => rows.borrow().iter().all(|row| {
                (*list || row.key_entry.text() == row.orig_key)
                    && row.cells.iter().all(|cell| {
                        cell.editor.unchanged(&cell.orig_value)
                            && (!*key_editable || cell.key.text() == cell.orig_key)
                    })
            }),
            Editor::Keybinds { rows, .. } => rows.borrow().iter().all(|row| {
                row.key.text() == row.orig_key
                    && csv_list(&row.modifier.text()) == csv_list(&row.orig_modifier)
                    && csv_list(&row.keys.text()) == csv_list(&row.orig_keys)
                    && row.fields.iter().all(|f| f.editor.unchanged(&f.orig_value))
            }),
            Editor::Monitors { rows, .. } => rows.borrow().iter().all(monitor_row_unchanged),
            Editor::Menu { rows, .. } => attrset_rows_unchanged(rows),
            Editor::Plugins { rows, .. } => rows.borrow().iter().all(|row| {
                row.key.text() == row.orig_key
                    && row
                        .children
                        .iter()
                        .all(|(record, editor)| editor.unchanged(&record.value))
            }),
            Editor::Bar {
                left,
                right,
                center,
                center_orig,
                extras,
                ..
            } => {
                attrset_rows_unchanged(left)
                    && attrset_rows_unchanged(right)
                    && center.unchanged(center_orig)
                    && extras
                        .iter()
                        .all(|(record, editor)| editor.unchanged(&record.value))
            }
            Editor::Dock {
                widgets, extras, ..
            } => {
                attrset_rows_unchanged(widgets)
                    && extras
                        .iter()
                        .all(|(record, editor)| editor.unchanged(&record.value))
            }
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
        if let Editor::HorizontalSubmodule { children, .. } = self {
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

    /// Connect `update` to the change signal of every widget owned by the
    /// editor, recursing into submodule children.
    pub fn connect_changed(&self, update: &Rc<dyn Fn()>) {
        match self {
            Editor::Switch(widget) => {
                let update = update.clone();
                widget.connect_active_notify(move |_| update());
            }
            Editor::Dropdown { combo, .. } => {
                let update = update.clone();
                combo.connect_selected_notify(move |_| update());
            }
            Editor::Spin(widget) => {
                let update = update.clone();
                widget.connect_value_changed(move |_| update());
            }
            Editor::Slider(widget) => {
                let update = update.clone();
                widget.connect_value_changed(move |_| update());
            }
            Editor::Text(widget) | Editor::Path(widget) => {
                let update = update.clone();
                widget.connect_changed(move |_| update());
            }
            Editor::Package { entry, .. } => {
                let update = update.clone();
                entry.connect_changed(move |_| update());
            }
            Editor::Multiline { view, .. } | Editor::Raw { view, .. } => {
                let update = update.clone();
                view.buffer().connect_changed(move |_| update());
            }
            Editor::List { entries, .. } | Editor::ModuleList { entries, .. } => {
                for entry in entries.borrow().iter() {
                    let update = update.clone();
                    entry.connect_changed(move |_| update());
                }
            }
            Editor::Color(widget) => {
                let update = update.clone();
                widget.connect_rgba_notify(move |_| update());
            }
            Editor::Submodule { children, .. } | Editor::HorizontalSubmodule { children, .. } => {
                for (_, editor) in children {
                    editor.connect_changed(update);
                }
            }
            Editor::Table {
                rows, on_change, ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in rows.borrow().iter() {
                    connect_table_row(row, update);
                }
            }
            Editor::Theme { list, .. } => {
                let update = update.clone();
                list.connect_row_selected(move |_, _| update());
            }
            Editor::Keybinds {
                rows, on_change, ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in rows.borrow().iter() {
                    connect_keybind_row(row, update);
                }
            }
            Editor::Monitors {
                rows, on_change, ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in rows.borrow().iter() {
                    connect_monitor_row(row, update);
                }
            }
            Editor::Menu {
                rows, on_change, ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in rows.borrow().iter() {
                    connect_attrset_row(row, update);
                }
            }
            Editor::Plugins {
                rows, on_change, ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in rows.borrow().iter() {
                    connect_plugin_row(row, update);
                }
            }
            Editor::Bar {
                left,
                right,
                center,
                extras,
                on_change,
                ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in left.borrow().iter() {
                    connect_attrset_row(row, update);
                }
                for row in right.borrow().iter() {
                    connect_attrset_row(row, update);
                }
                center.connect_changed(update);
                for (_, editor) in extras {
                    editor.connect_changed(update);
                }
            }
            Editor::Dock {
                widgets,
                extras,
                on_change,
                ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for row in widgets.borrow().iter() {
                    connect_attrset_row(row, update);
                }
                for (_, editor) in extras {
                    editor.connect_changed(update);
                }
            }
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
        WidgetKind::Submodule => build_submodule(record, type_str, ctx),
        WidgetKind::HorizontalSubmodule => build_horizontal_submodule(record, type_str, ctx),
        WidgetKind::TableAttrset => build_table(record, ctx, false),
        WidgetKind::TableList => build_table(record, ctx, true),
        WidgetKind::Theme => build_theme(value_text),
        WidgetKind::Keybinds => build_keybinds(record),
        WidgetKind::Monitors => build_monitors(record),
        WidgetKind::Menu => build_menu(record, ctx),
        WidgetKind::Plugins => build_plugins(record, ctx),
        WidgetKind::Bar => build_bar(record, ctx),
        WidgetKind::Dock => build_dock(record, ctx),
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
        if matches!(widget, WidgetKind::List | WidgetKind::Packages)
            && value::list_items(value_text).is_none()
        {
            return WidgetKind::Raw;
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

fn build_submodule(record: &Record, type_str: Option<&str>, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let depth = ctx.depth + 1;
    let mut children = Vec::new();

    let scanned = scan_children(record, ctx.advanced);
    let ranks = crate::model::group_ranks(&scanned);
    let mut indexed: Vec<(usize, Record)> = scanned.into_iter().enumerate().collect();
    indexed.sort_by(|(ai, a), (bi, b)| crate::model::record_order(a, *ai, b, *bi, &ranks));

    for (_index, child) in indexed {
        let mut child_ctx = ctx.clone();
        child_ctx.depth = depth;
        let editor = build(&child, type_str, &child_ctx);
        root.append(&build_child_row(&child, &editor, depth));
        children.push((child, editor));
    }
    Editor::Submodule { root, children }
}

/// A row of labelled controls that wraps to an equal-width grid. Non-nestable:
/// children keep declaration order and do not indent further.
fn build_horizontal_submodule(
    record: &Record,
    type_str: Option<&str>,
    ctx: &BuildContext,
) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_column_spacing(12);
    flow.set_row_spacing(6);
    flow.set_max_children_per_line(record.spec.max_columns.unwrap_or(4).max(1));
    flow.set_valign(gtk::Align::Start);

    let mut children = Vec::new();
    for child in scan_children(record, ctx.advanced) {
        let editor = build(&child, type_str, ctx);
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.set_hexpand(true);
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
        cell.append(&heading);
        cell.append(&editor.root());
        flow.append(&cell);
        children.push((child, editor));
    }
    root.append(&flow);
    Editor::HorizontalSubmodule { root, children }
}

/// A table over an attribute set of sets (`table-attrset`) or a list of sets
/// (`table-list`), with automatic per-cell widgets and built-in row buttons.
fn build_table(record: &Record, ctx: &BuildContext, list: bool) -> Editor {
    let columns = record.spec.columns.clone().unwrap_or_default();
    if columns.is_empty() {
        let (view, root) = build_raw(&record.value, &record.spec);
        return Editor::Raw { view, root };
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let key_editable = record.spec.key_editable;
    let first_value_editable = record.spec.first_value_editable;

    if !key_editable {
        let mut names = columns.clone();
        if !first_value_editable && !names.is_empty() {
            names.remove(0);
        }
        if !names.is_empty() {
            let header = gtk::Label::builder()
                .xalign(0.0)
                .label(names.join("   "))
                .css_classes(["dim-label", "caption"])
                .build();
            root.append(&header);
        }
    }

    let rows: Rc<RefCell<Vec<TableRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    for scan_row in scan::scan_table(&record.value, list) {
        if scan_row.spec.hidden && !ctx.advanced {
            continue;
        }
        let row = make_table_row(
            &root,
            &rows,
            &columns,
            list,
            key_editable,
            first_value_editable,
            &scan_row,
            ctx,
            None,
        );
        rows.borrow_mut().push(row);
    }

    let add = gtk::Button::with_label("Add row");
    add.set_halign(gtk::Align::Start);
    {
        let root = root.clone();
        let rows = rows.clone();
        let columns = columns.clone();
        let ctx = ctx.clone();
        let on_change = on_change.clone();
        add.connect_clicked(move |_| {
            let scan_row = scan::TableRowScan {
                key: String::new(),
                fields: columns
                    .iter()
                    .map(|column| (column.clone(), "null".to_string()))
                    .collect(),
                spec: OptionSpec::default(),
            };
            let update = on_change.borrow().clone();
            let row = make_table_row(
                &root,
                &rows,
                &columns,
                list,
                key_editable,
                first_value_editable,
                &scan_row,
                &ctx,
                update.as_ref(),
            );
            rows.borrow_mut().push(row);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    root.append(&add);

    Editor::Table {
        root,
        rows,
        list,
        key_editable,
        columns,
        on_change,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_table_row(
    root: &gtk::Box,
    rows: &Rc<RefCell<Vec<TableRow>>>,
    columns: &[String],
    list: bool,
    key_editable: bool,
    first_value_editable: bool,
    scan_row: &scan::TableRowScan,
    ctx: &BuildContext,
    update: Option<&Rc<dyn Fn()>>,
) -> TableRow {
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row_box.set_hexpand(true);

    let key_entry = gtk::Entry::new();
    key_entry.set_width_chars(12);
    if !list {
        key_entry.set_text(&scan_row.key);
        key_entry.set_tooltip_text(Some("Row key"));
        row_box.append(&key_entry);
        if let Some(update) = update {
            let update = update.clone();
            key_entry.connect_changed(move |_| update());
        }
    }

    let mut cells = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let (field_key, field_value) = scan_row
            .fields
            .iter()
            .find(|(key, _)| key == column)
            .cloned()
            .unwrap_or_else(|| (column.clone(), "null".to_string()));
        let is_first = index == 0;

        let cell_key = gtk::Entry::new();
        cell_key.set_text(&field_key);
        cell_key.set_width_chars(10);
        let show_editor = if key_editable {
            row_box.append(&cell_key);
            row_box.append(&gtk::Label::new(Some("=")));
            true
        } else if is_first && !first_value_editable {
            let label = gtk::Label::builder()
                .label(entry_text(&field_value))
                .xalign(0.0)
                .build();
            row_box.append(&label);
            row_box.append(&gtk::Label::new(Some(":")));
            false
        } else {
            true
        };

        let editor = build_cell(&field_value, ctx);
        if show_editor {
            row_box.append(&editor.root());
        }
        if let Some(update) = update {
            let update_key = update.clone();
            cell_key.connect_changed(move |_| update_key());
            editor.connect_changed(update);
        }
        cells.push(TableCell {
            column: column.clone(),
            key: cell_key,
            editor,
            orig_key: field_key,
            orig_value: field_value,
        });
    }

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove row"));
    {
        let root = root.clone();
        let rows = rows.clone();
        let row_box = row_box.clone();
        let key_entry = key_entry.clone();
        let update = update.cloned();
        remove.connect_clicked(move |_| {
            root.remove(&row_box);
            rows.borrow_mut().retain(|row| row.key_entry != key_entry);
            if let Some(cb) = update.as_ref() {
                cb();
            }
        });
    }
    row_box.append(&remove);

    root.append(&row_box);
    TableRow {
        key_entry,
        orig_key: scan_row.key.clone(),
        cells,
    }
}

/// A cell editor inferred from the cell's value: a switch for a boolean, a spin
/// for a number, a text field otherwise.
fn build_cell(value: &str, ctx: &BuildContext) -> Editor {
    let kind = match value::classify(value) {
        value::ValueKind::Bool => WidgetKind::Switch,
        value::ValueKind::Int | value::ValueKind::Float => WidgetKind::Spin,
        _ => WidgetKind::Text,
    };
    let spec = OptionSpec {
        widget: Some(kind),
        ..OptionSpec::default()
    };
    let record = Record {
        value: value.to_string(),
        spec,
        ..Record::default()
    };
    build(&record, None, ctx)
}

/// Serialize a table editor's rows back to an attribute set or a list.
fn table_nix(editor: &Editor) -> String {
    let Editor::Table {
        rows,
        list,
        key_editable,
        columns,
        ..
    } = editor
    else {
        return "{ }".to_string();
    };
    let mut parts = Vec::new();
    for row in rows.borrow().iter() {
        let mut fields = Vec::new();
        for (index, cell) in row.cells.iter().enumerate() {
            let key = if *key_editable {
                cell.key.text().trim().to_string()
            } else {
                columns
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| cell.column.clone())
            };
            fields.push(format!("{} = {};", attr_key(&key), cell.editor.to_nix()));
        }
        if *list {
            parts.push(format!("{{ {} }}", fields.join(" ")));
        } else {
            parts.push(format!(
                "{} = {{ {} }};",
                attr_key(&row.key_entry.text()),
                fields.join(" ")
            ));
        }
    }
    if *list {
        format!("[ {} ]", parts.join(" "))
    } else {
        format!("{{ {} }}", parts.join(" "))
    }
}

/// A Nix attribute name, kept bare when it is a plain identifier.
fn attr_key(raw: &str) -> String {
    let t = raw.trim();
    let valid = !t.is_empty()
        && t.chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        t.to_string()
    } else {
        quote(t)
    }
}

fn connect_table_row(row: &TableRow, update: &Rc<dyn Fn()>) {
    let update_entry = update.clone();
    row.key_entry.connect_changed(move |_| update_entry());
    for cell in &row.cells {
        let update_key = update.clone();
        cell.key.connect_changed(move |_| update_key());
        cell.editor.connect_changed(update);
    }
}

/// A scheme selector backed by the base16 palettes bundled at build time. Each
/// row shows the scheme's sixteen colors and its name; the written value is the
/// short scheme name. Degrades to a plain text field when no palettes are
/// bundled (plain `cargo test`).
fn build_theme(value_text: &str) -> Editor {
    if schemes::SCHEMES.is_empty() {
        let spec = OptionSpec::default();
        return Editor::Text(build_entry(value_text, &spec, false));
    }

    install_swatch_css();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let current = Rc::new(RefCell::new(entry_text(value_text)));
    let list = gtk::ListBox::new();
    list.add_css_class("theme-list");
    list.set_selection_mode(gtk::SelectionMode::Single);

    for (index, (name, palette)) in schemes::SCHEMES.iter().enumerate() {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_margin_top(4);
        content.set_margin_bottom(4);
        content.set_margin_start(6);
        content.set_margin_end(6);
        let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for (ci, _) in palette.iter().enumerate() {
            let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            swatch.set_size_request(14, 14);
            swatch.add_css_class("theme-swatch");
            swatch.add_css_class(&format!("s{index}-c{ci}"));
            swatches.append(&swatch);
        }
        content.append(&swatches);
        content.append(&gtk::Label::new(Some(name)));
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&content));
        list.append(&row);
        if *current.borrow() == *name {
            list.select_row(Some(&row));
        }
    }

    {
        let current = current.clone();
        list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                if let Some((name, _)) = schemes::SCHEMES.get(row.index() as usize) {
                    *current.borrow_mut() = (*name).to_string();
                }
            }
        });
    }

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(160)
        .child(&list)
        .build();
    scrolled.set_hexpand(true);
    root.append(&scrolled);
    Editor::Theme {
        root,
        current,
        list,
    }
}

/// Install the one-time stylesheet that paints each palette swatch.
fn install_swatch_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let mut css = String::new();
        for (si, (_, palette)) in schemes::SCHEMES.iter().enumerate() {
            for (ci, color) in palette.iter().enumerate() {
                css.push_str(&format!(
                    ".theme-swatch.s{si}-c{ci} {{ background-color: {color}; }}\n"
                ));
            }
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_data(&css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

/// Split a comma-separated field text into its trimmed, non-empty items.
fn csv_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The `modifier+keys` preview shown on a keybind row.
fn bind_preview(modifier: &str, keys: &str) -> String {
    let mut parts: Vec<String> = csv_list(modifier)
        .into_iter()
        .map(|m| m.to_uppercase())
        .collect();
    parts.extend(csv_list(keys).into_iter().map(|k| k.to_uppercase()));
    parts.join("+")
}

/// A comma-separated list field rendered as a Nix list of strings.
fn csv_list_nix(text: &str) -> String {
    let items: Vec<String> = csv_list(text).iter().map(|t| quote(t)).collect();
    value::serialize_list(&items)
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.trim())
}

/// An editor for one freeform field value, inferring the widget from the value.
fn build_field_cell(source: &str, ctx: &BuildContext) -> Editor {
    let kind = match value::classify(source) {
        value::ValueKind::Bool => WidgetKind::Switch,
        value::ValueKind::Int | value::ValueKind::Float => WidgetKind::Spin,
        _ => WidgetKind::Text,
    };
    let spec = OptionSpec {
        widget: Some(kind),
        ..OptionSpec::default()
    };
    let record = Record {
        value: source.to_string(),
        spec,
        ..Record::default()
    };
    build(&record, None, ctx)
}

/// One freeform field of a bespoke row. A missing (`null` or empty) source
/// becomes an empty leaf editor, so the literal `null` is never shown.
fn make_attrset_field(name: &str, raw: &str, ctx: &BuildContext) -> AttrsetField {
    let t = raw.trim();
    let present = !t.is_empty() && t != "null";
    let source = if present { t } else { "" };
    AttrsetField {
        name: name.to_string(),
        editor: build_field_cell(source, ctx),
        orig_value: source.to_string(),
        orig_present: present,
    }
}

/// Whether an editor holds something beyond its neutral value.
fn field_has_content(editor: &Editor) -> bool {
    match editor {
        Editor::Switch(w) => w.is_active(),
        Editor::Spin(w) => w.value() != 0.0,
        Editor::Text(w) => !w.text().is_empty(),
        Editor::Path(w) => !w.text().is_empty(),
        _ => true,
    }
}

fn attrset_field_nix(field: &AttrsetField) -> Option<String> {
    if !field.orig_present && !field_has_content(&field.editor) {
        return None;
    }
    Some(format!(
        "{} = {};",
        attr_key(&field.name),
        field.editor.to_nix()
    ))
}

/// One row of an id-keyed attribute set editor, with a key entry, one inferred
/// editor per field, and a remove button.
fn make_attrset_row(
    key: &str,
    fields: &[(String, String)],
    indent_by_key: bool,
    ctx: &BuildContext,
) -> AttrsetRow {
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row_box.set_hexpand(true);
    if indent_by_key {
        let depth = key.matches('.').count().min(4);
        row_box.set_margin_start((depth * 12) as i32);
    }

    let key_entry = gtk::Entry::new();
    key_entry.set_width_chars(14);
    key_entry.set_text(key);
    key_entry.set_tooltip_text(Some("Id"));
    row_box.append(&key_entry);

    let mut attr_fields = Vec::new();
    for (name, raw) in fields {
        let field = make_attrset_field(name, raw, ctx);
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let label = gtk::Label::builder()
            .xalign(0.0)
            .label(&field.name)
            .css_classes(["dim-label", "caption"])
            .build();
        cell.append(&label);
        cell.append(&field.editor.root());
        row_box.append(&cell);
        attr_fields.push(field);
    }

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove"));
    row_box.append(&remove);

    AttrsetRow {
        key: key_entry,
        orig_key: key.to_string(),
        fields: attr_fields,
        row_box,
        remove,
    }
}

fn attrset_rows_nix(rows: &[AttrsetRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let fields: Vec<String> = row.fields.iter().filter_map(attrset_field_nix).collect();
            format!(
                "{} = {{ {} }};",
                attr_key(&row.key.text()),
                fields.join(" ")
            )
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

fn attrset_rows_unchanged(rows: &Rc<RefCell<Vec<AttrsetRow>>>) -> bool {
    rows.borrow().iter().all(|row| {
        row.key.text() == row.orig_key
            && row.fields.iter().all(|f| f.editor.unchanged(&f.orig_value))
    })
}

fn connect_attrset_row(row: &AttrsetRow, update: &Rc<dyn Fn()>) {
    let update_key = update.clone();
    row.key.connect_changed(move |_| update_key());
    for field in &row.fields {
        field.editor.connect_changed(update);
    }
}

/// Wire a row's remove button and append it to the row container. The change
/// hook is read at click time so rows added before the app connects still
/// notify.
fn attach_attrset_row(
    container: &gtk::Box,
    rows: &Rc<RefCell<Vec<AttrsetRow>>>,
    row: AttrsetRow,
    on_change: &ChangeHook,
) {
    {
        let container = container.clone();
        let rows = rows.clone();
        let row_box = row.row_box.clone();
        let key = row.key.clone();
        let on_change = on_change.clone();
        row.remove.connect_clicked(move |_| {
            container.remove(&row_box);
            rows.borrow_mut().retain(|r| r.key != key);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    container.append(&row.row_box);
    rows.borrow_mut().push(row);
}

/// Build the shared id-keyed attribute set editor: scanned rows, an add
/// button that scaffolds a row from `scaffold` fields, and optional key-based
/// indentation for dotted ids.
fn build_attrset_rows(
    value: &str,
    ctx: &BuildContext,
    scaffold: &[(&str, &str)],
    indent_by_key: bool,
    on_change: &ChangeHook,
) -> (gtk::Box, Rc<RefCell<Vec<AttrsetRow>>>) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_hexpand(true);
    let rows = Rc::new(RefCell::new(Vec::new()));

    for scan_row in scan::scan_table(value, false) {
        let row = make_attrset_row(&scan_row.key, &scan_row.fields, indent_by_key, ctx);
        attach_attrset_row(&container, &rows, row, on_change);
    }

    let add = gtk::Button::with_label("Add");
    add.set_halign(gtk::Align::Start);
    {
        let container = container.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let ctx = ctx.clone();
        let scaffold: Vec<(String, String)> = scaffold
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        add.connect_clicked(move |_| {
            let mut row = make_attrset_row("", &scaffold, indent_by_key, &ctx);
            row.orig_key = "\0".to_string();
            attach_attrset_row(&container, &rows, row, &on_change);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }

    root.append(&container);
    root.append(&add);
    (root, rows)
}

/// An editor for a bespoke widget's child record. Annotated or bare attribute
/// sets render as raw text so the widget can rewrite its value without
/// dropping nested content.
fn build_bespoke_child(record: &Record, ctx: &BuildContext) -> Editor {
    let composite = matches!(
        record.spec.widget,
        Some(WidgetKind::Submodule) | Some(WidgetKind::HorizontalSubmodule)
    ) || (record.spec.widget.is_none()
        && value::classify(&record.value) == value::ValueKind::AttrSet);
    if composite {
        let (view, root) = build_raw(&record.value, &record.spec);
        Editor::Raw { view, root }
    } else {
        build(record, None, ctx)
    }
}

/// One bind of the `keybinds` widget: name and preview on the main line, the
/// bind's fields behind an expander.
fn make_keybind_row(scan_row: &scan::TableRowScan, ctx: &BuildContext) -> KeybindRow {
    let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row_box.set_hexpand(true);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let key_entry = gtk::Entry::new();
    key_entry.set_width_chars(14);
    key_entry.set_text(&scan_row.key);
    key_entry.set_tooltip_text(Some("Bind name"));
    main.append(&key_entry);

    let modifier = gtk::Entry::new();
    modifier.set_width_chars(16);
    modifier.set_tooltip_text(Some("Modifiers, comma separated"));
    modifier.set_text(&list_text(
        field(&scan_row.fields, "modifier").unwrap_or(""),
    ));
    main.append(&modifier);

    let keys = gtk::Entry::new();
    keys.set_width_chars(12);
    keys.set_tooltip_text(Some("Keys, comma separated"));
    keys.set_text(&list_text(field(&scan_row.fields, "keys").unwrap_or("")));
    main.append(&keys);

    let preview = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .label(bind_preview(&modifier.text(), &keys.text()))
        .css_classes(["dim-label", "caption"])
        .build();
    main.append(&preview);

    let expander = gtk::Expander::new(Some("Edit"));
    let detail = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let mut fields = Vec::new();
    for name in ["dispatcher", "args", "description", "group", "subcat"] {
        let field = make_attrset_field(name, field(&scan_row.fields, name).unwrap_or(""), ctx);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let label = gtk::Label::builder()
            .xalign(0.0)
            .width_chars(12)
            .label(name)
            .css_classes(["dim-label", "caption"])
            .build();
        line.append(&label);
        line.append(&field.editor.root());
        detail.append(&line);
        fields.push(field);
    }
    expander.set_child(Some(&detail));
    main.append(&expander);

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove bind"));
    main.append(&remove);

    row_box.append(&main);

    {
        let modifier_watch = modifier.clone();
        let keys_watch = keys.clone();
        let preview_a = preview.clone();
        modifier.connect_changed(move |_| {
            preview_a.set_label(&bind_preview(&modifier_watch.text(), &keys_watch.text()));
        });
        let modifier_watch = modifier.clone();
        let keys_watch = keys.clone();
        keys.connect_changed(move |_| {
            preview.set_label(&bind_preview(&modifier_watch.text(), &keys_watch.text()));
        });
    }

    KeybindRow {
        key: key_entry,
        orig_key: scan_row.key.clone(),
        modifier,
        keys,
        orig_modifier: field(&scan_row.fields, "modifier")
            .unwrap_or("")
            .to_string(),
        orig_keys: field(&scan_row.fields, "keys").unwrap_or("").to_string(),
        fields,
        row_box,
        remove,
    }
}

/// The comma-separated text shown in a modifier/keys entry.
fn list_text(source: &str) -> String {
    value::list_items(source)
        .map(|items| {
            items
                .iter()
                .map(|item| parse_string_literal(item))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn keybinds_nix(rows: &[KeybindRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut fields = vec![
                format!("modifier = {};", csv_list_nix(&row.modifier.text())),
                format!("keys = {};", csv_list_nix(&row.keys.text())),
            ];
            for field in &row.fields {
                if let Some(line) = attrset_field_nix(field) {
                    fields.push(line);
                }
            }
            format!(
                "{} = {{ {} }};",
                attr_key(&row.key.text()),
                fields.join(" ")
            )
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

fn connect_keybind_row(row: &KeybindRow, update: &Rc<dyn Fn()>) {
    for entry in [&row.key, &row.modifier, &row.keys] {
        let update = update.clone();
        entry.connect_changed(move |_| update());
    }
    for field in &row.fields {
        field.editor.connect_changed(update);
    }
}

fn build_keybinds(record: &Record) -> Editor {
    let ctx = BuildContext::default();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_hexpand(true);
    let rows: Rc<RefCell<Vec<KeybindRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    let mut scanned = scan::scan_table(&record.value, false);
    scanned.sort_by(|a, b| {
        let key = |row: &scan::TableRowScan| {
            (
                field(&row.fields, "group").unwrap_or("").to_string(),
                field(&row.fields, "subcat").unwrap_or("").to_string(),
                row.key.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    for scan_row in scanned {
        let row = make_keybind_row(&scan_row, &ctx);
        {
            let container = container.clone();
            let rows = rows.clone();
            let on_change = on_change.clone();
            let row_box = row.row_box.clone();
            let key = row.key.clone();
            row.remove.connect_clicked(move |_| {
                container.remove(&row_box);
                rows.borrow_mut().retain(|r| r.key != key);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        }
        container.append(&row.row_box);
        rows.borrow_mut().push(row);
    }

    let add = gtk::Button::with_label("Add bind");
    add.set_halign(gtk::Align::Start);
    {
        let container = container.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        add.connect_clicked(move |_| {
            let scan_row = scan::TableRowScan {
                key: String::new(),
                fields: vec![
                    ("modifier".to_string(), "[]".to_string()),
                    ("keys".to_string(), "[]".to_string()),
                ],
                spec: OptionSpec::default(),
            };
            let mut row = make_keybind_row(&scan_row, &ctx);
            row.orig_key = "\0".to_string();
            {
                let container = container.clone();
                let rows = rows.clone();
                let on_change = on_change.clone();
                let row_box = row.row_box.clone();
                let key = row.key.clone();
                row.remove.connect_clicked(move |_| {
                    container.remove(&row_box);
                    rows.borrow_mut().retain(|r| r.key != key);
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                });
            }
            container.append(&row.row_box);
            rows.borrow_mut().push(row);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }

    root.append(&container);
    root.append(&add);
    Editor::Keybinds {
        root,
        rows,
        on_change,
    }
}

/// The geometry a monitor rectangle shows on the canvas. Size falls back to
/// 1920x1080 when unset; `sized` marks the fallback.
fn monitor_geometry(row: &MonitorRow) -> (f64, f64, f64, f64, String, bool) {
    let width = row.width.value();
    let height = row.height.value();
    let sized = width > 0.0 && height > 0.0;
    let width = if width > 0.0 { width } else { 1920.0 };
    let height = if height > 0.0 { height } else { 1080.0 };
    (
        row.x.value(),
        row.y.value(),
        width,
        height,
        row.name.text().to_string(),
        sized,
    )
}

fn monitor_orig<'a>(row: &'a MonitorRow, name: &str) -> &'a str {
    row.orig
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or("")
}

fn monitor_row_unchanged(row: &MonitorRow) -> bool {
    let text_same =
        |entry: &gtk::Entry, name: &str| entry.text() == entry_text(monitor_orig(row, name));
    let spin_same =
        |spin: &gtk::SpinButton, name: &str| match monitor_orig(row, name).parse::<f64>() {
            Ok(value) => (spin.value() - value).abs() < 1e-9,
            Err(_) => spin.value() == 0.0,
        };
    text_same(&row.name, "name")
        && spin_same(&row.width, "width")
        && spin_same(&row.height, "height")
        && spin_same(&row.refresh, "refresh")
        && spin_same(&row.x, "x")
        && spin_same(&row.y, "y")
        && spin_same(&row.scale, "scale")
        && text_same(&row.rr, "rr")
        && row
            .extras
            .iter()
            .all(|field| field.editor.unchanged(&field.orig_value))
}

fn monitor_row_nix(row: &MonitorRow) -> String {
    let mut parts = Vec::new();
    let orig_has =
        |name: &str| !monitor_orig(row, name).is_empty() && monitor_orig(row, name) != "null";
    let name = row.name.text().trim().to_string();
    if !name.is_empty() || orig_has("name") {
        parts.push(format!("name = {};", quote(&name)));
    }
    let number = |parts: &mut Vec<String>, key: &str, spin: &gtk::SpinButton| {
        if spin.value() != 0.0 || orig_has(key) {
            parts.push(format!("{} = {};", key, number_literal(spin.value(), 0)));
        }
    };
    number(&mut parts, "width", &row.width);
    number(&mut parts, "height", &row.height);
    number(&mut parts, "refresh", &row.refresh);
    number(&mut parts, "x", &row.x);
    number(&mut parts, "y", &row.y);
    if row.scale.value() != 0.0 || orig_has("scale") {
        parts.push(format!("scale = {};", number_literal(row.scale.value(), 2)));
    }
    let rr = row.rr.text().trim().to_string();
    if !rr.is_empty() || orig_has("rr") {
        parts.push(format!("rr = {};", quote(&rr)));
    }
    for field in &row.extras {
        if let Some(line) = attrset_field_nix(field) {
            parts.push(line);
        }
    }
    format!("{{ {} }}", parts.join(" "))
}

fn monitors_nix(rows: &[MonitorRow]) -> String {
    let parts: Vec<String> = rows.iter().map(monitor_row_nix).collect();
    value::serialize_list(&parts)
}

fn connect_monitor_row(row: &MonitorRow, update: &Rc<dyn Fn()>) {
    let update_entry = update.clone();
    row.name.connect_changed(move |_| update_entry());
    let update_rr = update.clone();
    row.rr.connect_changed(move |_| update_rr());
    for spin in [
        &row.width,
        &row.height,
        &row.refresh,
        &row.x,
        &row.y,
        &row.scale,
    ] {
        let update = update.clone();
        spin.connect_value_changed(move |_| update());
    }
    for field in &row.extras {
        field.editor.connect_changed(update);
    }
}

const MONITOR_EXTRAS: &[(&str, bool)] = &[
    ("vrr", true),
    ("hdr", true),
    ("hdr_force", true),
    ("disable", true),
    ("primary", true),
    ("make", false),
    ("model", false),
    ("serial", false),
    ("icc", false),
    ("custom", false),
    ("hdr_min_lum", false),
    ("hdr_max_lum", false),
    ("hdr_max_avg_lum", false),
];

/// Attach a caption and a control to a monitor form grid.
fn attach_cell(grid: &gtk::Grid, row: i32, col: i32, label: &str, widget: &impl IsA<gtk::Widget>) {
    let caption = gtk::Label::builder()
        .xalign(1.0)
        .label(label)
        .css_classes(["dim-label", "caption"])
        .build();
    grid.attach(&caption, col, row, 1, 1);
    grid.attach(widget, col + 1, row, 1, 1);
}

/// One monitor rule: core geometry on a grid, the remaining fields behind an
/// "Extra rules" expander, and a remove button.
fn make_monitor_row(scan_row: &scan::TableRowScan, ctx: &BuildContext) -> MonitorRow {
    let get = |name: &str| field(&scan_row.fields, name).unwrap_or("");
    let number_spin = |value: &str, min: f64, max: f64, step: f64| {
        let spin = gtk::SpinButton::with_range(min, max, step);
        spin.set_digits(0);
        if let Ok(parsed) = value.parse::<f64>() {
            spin.set_value(parsed);
        }
        spin
    };

    let name = gtk::Entry::new();
    name.set_text(entry_text(get("name")).as_str());
    name.set_tooltip_text(Some("Output name"));

    let width = number_spin(get("width"), 0.0, 16384.0, 1.0);
    let height = number_spin(get("height"), 0.0, 16384.0, 1.0);
    let refresh = number_spin(get("refresh"), 0.0, 1000.0, 1.0);
    let x = number_spin(get("x"), -16384.0, 16384.0, 1.0);
    let y = number_spin(get("y"), -16384.0, 16384.0, 1.0);
    let scale = gtk::SpinButton::with_range(0.0, 16.0, 0.05);
    scale.set_digits(2);
    if let Ok(parsed) = get("scale").parse::<f64>() {
        scale.set_value(parsed);
    }
    let rr = gtk::Entry::new();
    rr.set_tooltip_text(Some("Transform, 0-7"));
    rr.set_text(entry_text(get("rr")).as_str());

    let form_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    form_box.set_margin_top(4);
    form_box.set_margin_bottom(4);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(6);
    attach_cell(&grid, 0, 0, "Name", &name);
    attach_cell(&grid, 0, 2, "Refresh", &refresh);
    attach_cell(&grid, 1, 0, "Width", &width);
    attach_cell(&grid, 1, 2, "Height", &height);
    attach_cell(&grid, 2, 0, "X", &x);
    attach_cell(&grid, 2, 2, "Y", &y);
    attach_cell(&grid, 3, 0, "Scale", &scale);
    attach_cell(&grid, 3, 2, "RR", &rr);
    form_box.append(&grid);

    let mut extras = Vec::new();
    let expander = gtk::Expander::new(Some("Extra rules"));
    let extra_grid = gtk::Grid::new();
    extra_grid.set_column_spacing(12);
    extra_grid.set_row_spacing(6);
    for (index, (field_name, is_bool)) in MONITOR_EXTRAS.iter().enumerate() {
        let raw = get(field_name);
        let source = if raw.is_empty() || raw == "null" {
            if *is_bool {
                "false"
            } else {
                "0"
            }
        } else {
            raw
        };
        let editor = build_field_cell(source, ctx);
        let present = !raw.is_empty() && raw != "null";
        let caption = gtk::Label::builder()
            .xalign(1.0)
            .label(*field_name)
            .css_classes(["dim-label", "caption"])
            .build();
        let row_index = index as i32;
        extra_grid.attach(&caption, 0, row_index, 1, 1);
        extra_grid.attach(&editor.root(), 1, row_index, 1, 1);
        extras.push(AttrsetField {
            name: (*field_name).to_string(),
            editor,
            orig_value: source.to_string(),
            orig_present: present,
        });
    }
    expander.set_child(Some(&extra_grid));
    form_box.append(&expander);

    let remove = gtk::Button::with_label("Remove monitor");
    remove.add_css_class("destructive-action");
    remove.set_halign(gtk::Align::Start);
    form_box.append(&remove);

    let orig = scan_row
        .fields
        .iter()
        .map(|(name, value)| {
            let t = value.trim();
            let normalized = if t.is_empty() || t == "null" {
                String::new()
            } else {
                t.to_string()
            };
            (name.clone(), normalized)
        })
        .collect();

    MonitorRow {
        name,
        width,
        height,
        refresh,
        x,
        y,
        scale,
        rr,
        extras,
        orig,
        form_box,
        remove,
    }
}

#[derive(Clone, Copy)]
struct MonitorView {
    scale: f64,
    min_x: f64,
    min_y: f64,
}

fn build_monitors(record: &Record) -> Editor {
    let ctx = BuildContext::default();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let rows = Rc::new(RefCell::new(Vec::new()));
    let selected = Rc::new(Cell::new(0usize));
    let view = Rc::new(Cell::new(MonitorView {
        scale: 1.0,
        min_x: 0.0,
        min_y: 0.0,
    }));

    let canvas = gtk::DrawingArea::builder()
        .content_width(640)
        .content_height(240)
        .build();
    canvas.set_hexpand(true);
    canvas.set_draw_func({
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        move |_, cr, width, height| {
            let rows = rows.borrow();
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;
            for row in rows.iter() {
                let (x, y, w, h, _, _) = monitor_geometry(row);
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + w);
                max_y = max_y.max(y + h);
            }
            if min_x == f64::MAX {
                return;
            }
            let pad = 16.0;
            let span_x = (max_x - min_x).max(1.0);
            let span_y = (max_y - min_y).max(1.0);
            let scale = f64::min(
                (width as f64 - 2.0 * pad) / span_x,
                (height as f64 - 2.0 * pad) / span_y,
            )
            .max(0.001);
            view.set(MonitorView {
                scale,
                min_x,
                min_y,
            });

            for (index, row) in rows.iter().enumerate() {
                let (x, y, w, h, label, sized) = monitor_geometry(row);
                let px = pad + (x - min_x) * scale;
                let py = pad + (y - min_y) * scale;
                let pw = (w * scale).max(24.0);
                let ph = (h * scale).max(16.0);
                if index == selected.get() {
                    cr.set_source_rgb(0.27, 0.45, 0.75);
                } else {
                    cr.set_source_rgb(0.42, 0.42, 0.47);
                }
                cr.rectangle(px, py, pw, ph);
                cr.fill_preserve().ok();
                if sized {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.6);
                    cr.set_line_width(1.0);
                } else {
                    cr.set_source_rgb(0.95, 0.65, 0.25);
                    cr.set_line_width(2.0);
                    cr.set_dash(&[4.0, 4.0], 0.0);
                }
                cr.stroke().ok();
                cr.set_dash(&[], 0.0);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.move_to(px + 6.0, py + 16.0);
                let _ = cr.show_text(&label);
            }
        }
    });
    root.append(&canvas);

    let form = gtk::Box::new(gtk::Orientation::Vertical, 6);
    form.set_hexpand(true);
    root.append(&form);

    for (index, scan_row) in scan::scan_table(&record.value, true)
        .into_iter()
        .enumerate()
    {
        let row = make_monitor_row(&scan_row, &ctx);
        row.form_box.set_visible(index == 0);
        {
            let form = form.clone();
            let rows = rows.clone();
            let on_change = on_change.clone();
            let row_box = row.form_box.clone();
            row.remove.connect_clicked(move |_| {
                form.remove(&row_box);
                rows.borrow_mut().retain(|r| r.form_box != row_box);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        }
        form.append(&row.form_box);
        rows.borrow_mut().push(row);
    }

    {
        let click = gtk::GestureClick::new();
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        let redraw = canvas.clone();
        click.connect_pressed(move |_, _, px, py| {
            let point = view.get();
            let hit = rows.borrow().iter().position(|row| {
                let (x, y, w, h, _, _) = monitor_geometry(row);
                let left = 16.0 + (x - point.min_x) * point.scale;
                let top = 16.0 + (y - point.min_y) * point.scale;
                let width = (w * point.scale).max(24.0);
                let height = (h * point.scale).max(16.0);
                px >= left && px <= left + width && py >= top && py <= top + height
            });
            if let Some(index) = hit {
                selected.set(index);
                for (row_index, row) in rows.borrow().iter().enumerate() {
                    row.form_box.set_visible(row_index == index);
                }
                redraw.queue_draw();
            }
        });
        canvas.add_controller(click);
    }

    {
        let drag = gtk::GestureDrag::new();
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        let redraw = canvas.clone();
        let on_change = on_change.clone();
        let last = Rc::new(Cell::new((0.0f64, 0.0f64)));
        drag.connect_drag_begin({
            let last = last.clone();
            move |_, _, _| last.set((0.0, 0.0))
        });
        drag.connect_drag_update(move |gesture, _, _| {
            let (offset_x, offset_y) = gesture.offset().unwrap_or((0.0, 0.0));
            let (prev_x, prev_y) = last.get();
            last.set((offset_x, offset_y));
            let scale = view.get().scale;
            if let Some(row) = rows.borrow().get(selected.get()) {
                row.x.set_value(row.x.value() + (offset_x - prev_x) / scale);
                row.y.set_value(row.y.value() + (offset_y - prev_y) / scale);
                redraw.queue_draw();
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            }
        });
        canvas.add_controller(drag);
    }

    let add = gtk::Button::with_label("Add monitor");
    add.set_halign(gtk::Align::Start);
    {
        let form = form.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let canvas = canvas.clone();
        let selected = selected.clone();
        add.connect_clicked(move |_| {
            let scan_row = scan::TableRowScan {
                key: String::new(),
                fields: vec![("name".to_string(), "\"\"".to_string())],
                spec: OptionSpec::default(),
            };
            let row = make_monitor_row(&scan_row, &ctx);
            let index = rows.borrow().len();
            row.form_box.set_visible(true);
            {
                let form = form.clone();
                let rows = rows.clone();
                let on_change = on_change.clone();
                let row_box = row.form_box.clone();
                row.remove.connect_clicked(move |_| {
                    form.remove(&row_box);
                    rows.borrow_mut().retain(|r| r.form_box != row_box);
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                });
            }
            form.append(&row.form_box);
            rows.borrow_mut().push(row);
            selected.set(index);
            for (row_index, other) in rows.borrow().iter().enumerate() {
                other.form_box.set_visible(row_index == index);
            }
            canvas.queue_draw();
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    root.append(&add);

    Editor::Monitors {
        root,
        rows,
        on_change,
    }
}

fn build_menu(record: &Record, ctx: &BuildContext) -> Editor {
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let (root, rows) =
        build_attrset_rows(&record.value, ctx, &[("label", "\"\"")], true, &on_change);
    Editor::Menu {
        root,
        rows,
        on_change,
    }
}

fn build_plugins(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_hexpand(true);
    let rows: Rc<RefCell<Vec<PluginRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    for child in scan_children(record, true) {
        let visible = !(child.spec.hidden && !ctx.advanced);
        let key = child.segments.last().cloned().unwrap_or_default();
        let fields = scan_children(&child, true);
        let enable_record = fields
            .iter()
            .find(|f| f.segments.as_slice() == ["enable"])
            .cloned();
        let enable_present = enable_record.is_some();
        let enable = match &enable_record {
            Some(record) => build(record, None, ctx),
            None => Editor::Switch(build_switch("false")),
        };
        let children: Vec<(Record, Editor)> = fields
            .iter()
            .filter(|f| f.segments.as_slice() != ["enable"])
            .map(|f| (f.clone(), build_bespoke_child(f, ctx)))
            .collect();

        let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        row_box.set_hexpand(true);
        let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let key_entry = gtk::Entry::new();
        key_entry.set_width_chars(16);
        key_entry.set_text(&key);
        key_entry.set_tooltip_text(Some("Plugin name"));
        main.append(&key_entry);
        main.append(&enable.root());
        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Remove plugin"));
        main.append(&remove);
        row_box.append(&main);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_margin_start(18);
        for (field_record, editor) in &children {
            if field_record.spec.hidden && !ctx.advanced {
                continue;
            }
            body.append(&build_child_row(field_record, editor, 1));
        }
        row_box.append(&body);
        if visible {
            container.append(&row_box);
        }

        {
            let container = container.clone();
            let rows = rows.clone();
            let on_change = on_change.clone();
            let row_box = row_box.clone();
            let key = key_entry.clone();
            remove.connect_clicked(move |_| {
                container.remove(&row_box);
                rows.borrow_mut().retain(|r| r.key != key);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        }

        rows.borrow_mut().push(PluginRow {
            key: key_entry,
            orig_key: key,
            enable,
            enable_present,
            children,
        });
    }

    let add = gtk::Button::with_label("Add plugin");
    add.set_halign(gtk::Align::Start);
    {
        let container = container.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        add.connect_clicked(move |_| {
            let key_entry = gtk::Entry::new();
            key_entry.set_width_chars(16);
            key_entry.set_tooltip_text(Some("Plugin name"));
            let enable = Editor::Switch(build_switch("true"));
            let remove = gtk::Button::from_icon_name("user-trash-symbolic");
            remove.add_css_class("flat");
            let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            main.append(&key_entry);
            main.append(&enable.root());
            main.append(&remove);
            let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
            row_box.append(&main);
            let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
            row_box.append(&body);
            key_entry.set_text("");
            {
                let container = container.clone();
                let rows = rows.clone();
                let on_change = on_change.clone();
                let row_box = row_box.clone();
                let key = key_entry.clone();
                remove.connect_clicked(move |_| {
                    container.remove(&row_box);
                    rows.borrow_mut().retain(|r| r.key != key);
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                });
            }
            container.append(&row_box);
            let row = PluginRow {
                key: key_entry,
                orig_key: "\0".to_string(),
                enable,
                enable_present: false,
                children: Vec::new(),
            };
            rows.borrow_mut().push(row);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }

    root.append(&container);
    root.append(&add);
    Editor::Plugins {
        root,
        rows,
        on_change,
    }
}

fn plugins_nix(rows: &[PluginRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut fields = Vec::new();
            let enabled = match &row.enable {
                Editor::Switch(w) => w.is_active(),
                _ => true,
            };
            if row.enable_present || enabled {
                fields.push(format!("enable = {};", row.enable.to_nix()));
            }
            for (record, editor) in &row.children {
                let name = record.segments.last().cloned().unwrap_or_default();
                fields.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
            }
            format!(
                "{} = {{ {} }};",
                attr_key(&row.key.text()),
                fields.join(" ")
            )
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

fn connect_plugin_row(row: &PluginRow, update: &Rc<dyn Fn()>) {
    let update_key = update.clone();
    row.key.connect_changed(move |_| update_key());
    row.enable.connect_changed(update);
    for (_, editor) in &row.children {
        editor.connect_changed(update);
    }
}

fn build_bar(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    let mut left_value = None;
    let mut right_value = None;
    let mut center_record = None;
    let mut extras = Vec::new();
    for child in scan_children(record, true) {
        match child.segments.last().map(String::as_str) {
            Some("leftWidgets") => left_value = Some(child.value.clone()),
            Some("rightWidgets") => right_value = Some(child.value.clone()),
            Some("centerWidgets") => center_record = Some(child),
            _ => extras.push(child),
        }
    }

    let left_present = left_value.is_some();
    let right_present = right_value.is_some();
    let center_present = center_record.is_some();

    let heading = |text: &str| {
        gtk::Label::builder()
            .xalign(0.0)
            .label(text)
            .css_classes(["heading"])
            .build()
    };

    root.append(&heading("Left widgets"));
    let (left_root, left) = build_attrset_rows(
        left_value.as_deref().unwrap_or("{ }"),
        ctx,
        &[("enabled", "true")],
        false,
        &on_change,
    );
    root.append(&left_root);

    root.append(&heading("Center widgets"));
    let (center, center_orig) = match center_record {
        Some(record) => {
            let editor = Box::new(build(&record, None, ctx));
            let value = record.value.clone();
            (editor, value)
        }
        None => (Box::new(build_list("[]", false, None)), "[]".to_string()),
    };
    root.append(&center.root());

    root.append(&heading("Right widgets"));
    let (right_root, right) = build_attrset_rows(
        right_value.as_deref().unwrap_or("{ }"),
        ctx,
        &[("enabled", "true")],
        false,
        &on_change,
    );
    root.append(&right_root);

    let extras: Vec<(Record, Editor)> = extras
        .into_iter()
        .map(|record| (record.clone(), build_bespoke_child(&record, ctx)))
        .collect();
    if extras
        .iter()
        .any(|(record, _)| !(record.spec.hidden && !ctx.advanced))
    {
        root.append(&heading("Settings"));
        for (record, editor) in &extras {
            if record.spec.hidden && !ctx.advanced {
                continue;
            }
            root.append(&build_child_row(record, editor, 1));
        }
    }

    Editor::Bar {
        root,
        left,
        right,
        center,
        center_orig,
        left_present,
        right_present,
        center_present,
        extras,
        on_change,
    }
}

fn bar_nix(
    left: &Rc<RefCell<Vec<AttrsetRow>>>,
    right: &Rc<RefCell<Vec<AttrsetRow>>>,
    center: &Editor,
    left_present: bool,
    right_present: bool,
    center_present: bool,
    extras: &[(Record, Editor)],
) -> String {
    let mut parts = Vec::new();
    let left_rows = left.borrow();
    let right_rows = right.borrow();
    if left_present || !left_rows.is_empty() {
        parts.push(format!("leftWidgets = {};", attrset_rows_nix(&left_rows)));
    }
    if center_present {
        parts.push(format!("centerWidgets = {};", center.to_nix()));
    }
    if right_present || !right_rows.is_empty() {
        parts.push(format!("rightWidgets = {};", attrset_rows_nix(&right_rows)));
    }
    for (record, editor) in extras {
        let name = record.segments.last().cloned().unwrap_or_default();
        parts.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
    }
    format!("{{ {} }}", parts.join(" "))
}

fn build_dock(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    let mut widgets_value = None;
    let mut extras = Vec::new();
    for child in scan_children(record, true) {
        if child.segments.last().map(String::as_str) == Some("widgets") {
            widgets_value = Some(child.value.clone());
        } else {
            extras.push(child);
        }
    }
    let widgets_present = widgets_value.is_some();

    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label("Widgets")
        .css_classes(["heading"])
        .build();
    root.append(&heading);
    let (widgets_root, widgets) = build_attrset_rows(
        widgets_value.as_deref().unwrap_or("{ }"),
        ctx,
        &[("enabled", "true")],
        false,
        &on_change,
    );
    root.append(&widgets_root);

    if !extras.is_empty() {
        let settings = gtk::Label::builder()
            .xalign(0.0)
            .label("Settings")
            .css_classes(["heading"])
            .build();
        root.append(&settings);
    }
    let extras: Vec<(Record, Editor)> = extras
        .into_iter()
        .map(|record| {
            let visible = !(record.spec.hidden && !ctx.advanced);
            let editor = build_bespoke_child(&record, ctx);
            if visible {
                root.append(&build_child_row(&record, &editor, 1));
            }
            (record, editor)
        })
        .collect();

    Editor::Dock {
        root,
        widgets,
        widgets_present,
        extras,
        on_change,
    }
}

fn dock_nix(
    widgets: &Rc<RefCell<Vec<AttrsetRow>>>,
    widgets_present: bool,
    extras: &[(Record, Editor)],
) -> String {
    let mut parts = Vec::new();
    let rows = widgets.borrow();
    if widgets_present || !rows.is_empty() {
        parts.push(format!("widgets = {};", attrset_rows_nix(&rows)));
    }
    for (record, editor) in extras {
        let name = record.segments.last().cloned().unwrap_or_default();
        parts.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
    }
    format!("{{ {} }}", parts.join(" "))
}

/// Scan a submodule value's direct children into offset records, dropping
/// hidden controls while the Advanced toggle is off.
fn scan_children(record: &Record, advanced: bool) -> Vec<Record> {
    let mut scanned = Vec::new();
    for mut child in scan::scan_value(&record.value) {
        offset_record(&mut child, record.byte_start, &record.file);
        if child.spec.hidden && !advanced {
            continue;
        }
        scanned.push(child);
    }
    scanned
}

/// One nested control: heading, editor, and an "imported from" footer, indented
/// by `depth * 12` pixels (capped at four levels).
fn build_child_row(record: &Record, editor: &Editor, depth: usize) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.set_margin_start((depth.min(4) * 12) as i32);
    let label = record
        .spec
        .label
        .clone()
        .or_else(|| record.segments.last().cloned())
        .unwrap_or_else(|| record.path());
    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label(label)
        .css_classes(["dim-label"])
        .build();
    row.append(&heading);
    row.append(&editor.root());
    if !record.origin.is_empty() {
        let origin = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .label(format!("imported from {}", record.origin))
            .css_classes(["dim-label", "caption"])
            .build();
        row.append(&origin);
    }
    row.upcast()
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
        | WidgetKind::Submodule
        | WidgetKind::HorizontalSubmodule
        | WidgetKind::TableAttrset
        | WidgetKind::TableList
        | WidgetKind::Keybinds
        | WidgetKind::Monitors
        | WidgetKind::Theme
        | WidgetKind::Menu
        | WidgetKind::Plugins
        | WidgetKind::Bar
        | WidgetKind::Dock => expr_literal(value),
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
            Some("[ git vim ]".to_string())
        );
        assert_eq!(canonical_list("not a list", false), None);
    }

    #[test]
    fn package_edits_keep_the_existing_with_wrapper() {
        let original = "[ git vim ]";
        assert_eq!(canonical_list(original, true), Some(original.to_string()));
        assert_eq!(
            package_items(&["git".into(), "vim".into(), "neovim".into()]),
            "[ git vim neovim ]"
        );
        assert!(!package_items(&["git".into()]).contains("with pkgs;"));
    }

    #[test]
    fn list_over_non_list_value_falls_back_to_raw() {
        let mut record = Record {
            value: "builtins.attrValues self.modules.homeManager".to_string(),
            segments: vec!["home-manager".to_string(), "sharedModules".to_string()],
            ..Record::default()
        };
        record.spec.widget = Some(WidgetKind::List);
        assert_eq!(choose_widget(&record, None), WidgetKind::Raw);

        record.value = "[ git vim ]".to_string();
        assert_eq!(choose_widget(&record, None), WidgetKind::List);
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

    #[test]
    fn splits_comma_separated_lists() {
        assert_eq!(csv_list("SUPER, SHIFT"), ["SUPER", "SHIFT"]);
        assert_eq!(csv_list(" , , "), [] as [&str; 0]);
        assert_eq!(csv_list(""), [] as [&str; 0]);
    }

    #[test]
    fn previews_modifier_plus_keys() {
        assert_eq!(
            bind_preview("SUPER, SHIFT", "Return, a"),
            "SUPER+SHIFT+RETURN+A"
        );
        assert_eq!(bind_preview("", ""), "");
    }

    #[test]
    fn reads_list_text_from_nix() {
        assert_eq!(list_text("[ \"SUPER\" \"SHIFT\" ]"), "SUPER, SHIFT");
        assert_eq!(list_text("[]"), "");
        assert_eq!(list_text("null"), "");
    }

    #[test]
    fn serialises_csv_lists_as_nix_lists() {
        assert_eq!(csv_list_nix("SUPER, SHIFT"), "[ \"SUPER\" \"SHIFT\" ]");
        assert_eq!(csv_list_nix(""), "[]");
    }

    #[test]
    fn quotes_dotted_attr_keys() {
        assert_eq!(attr_key("dank-rs.cal.simple"), "\"dank-rs.cal.simple\"");
        assert_eq!(attr_key("plain"), "plain");
        assert_eq!(attr_key(""), "\"\"");
    }

    #[test]
    fn parses_bespoke_widget_names() {
        assert_eq!(WidgetKind::parse("keybinds"), Some(WidgetKind::Keybinds));
        assert_eq!(WidgetKind::parse("monitors"), Some(WidgetKind::Monitors));
        assert_eq!(WidgetKind::parse("theme"), Some(WidgetKind::Theme));
        assert_eq!(WidgetKind::parse("menu"), Some(WidgetKind::Menu));
        assert_eq!(WidgetKind::parse("plugins"), Some(WidgetKind::Plugins));
        assert_eq!(WidgetKind::parse("bar"), Some(WidgetKind::Bar));
        assert_eq!(WidgetKind::parse("dock"), Some(WidgetKind::Dock));
        assert_eq!(WidgetKind::parse("nope"), None);
    }

    #[test]
    fn scan_children_filters_hidden_unless_advanced() {
        let text = r#"{
  /* @option widget = "submodule"; label = "Nested"; */
  nested = {
    /* @option label = "Visible"; */
    visible = true;
    /* @option label = "Hidden"; hidden = true; */
    hidden = false;
  };
}"#;
        let record = crate::scan::scan_value(text)
            .into_iter()
            .find(|r| r.path() == "nested")
            .unwrap();

        let visible_only = scan_children(&record, false);
        assert_eq!(visible_only.len(), 1);
        assert_eq!(
            visible_only[0].segments.last().map(String::as_str),
            Some("visible")
        );

        let all = scan_children(&record, true);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn parses_bundled_palette_stub() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/palettes/base16-stub.yaml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        let (name, colors) = build_support::parse_scheme_text(&text, "base16-stub").unwrap();
        assert_eq!(name, "stub");
        assert_eq!(colors[0], "#111111");
        assert_eq!(colors[15], "#552200");
    }
}
