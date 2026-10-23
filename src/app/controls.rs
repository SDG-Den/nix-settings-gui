use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gtk::prelude::*;

use crate::dsl::Visibility;
use crate::metadata::{self, Target};
use crate::model::Record;
use crate::widgets::{self, Editor};

use super::state::{App, ControlRow, MenuPage, Msg};

/// Bolt shown beside an option's label when its applyCommand runs on change.
fn live_marker(record: &Record) -> gtk::Image {
    let icon = match gtk::gdk::Display::default() {
        Some(display)
            if gtk::IconTheme::for_display(&display).has_icon("flash-symbolic") =>
        {
            "flash-symbolic"
        }
        _ => "emblem-important-symbolic",
    };
    let marker = gtk::Image::from_icon_name(icon);
    marker.set_pixel_size(14);
    marker.set_valign(gtk::Align::Center);
    marker.add_css_class("accent");
    if let Some(command) = record.spec.apply_command.as_deref() {
        marker.set_tooltip_text(Some(&format!("Applies immediately: {command}")));
    }
    marker
}

/// Show the bolt only while live-apply is on and the row has a command.
pub(super) fn refresh_live_markers(app: &Rc<App>) {
    let enabled = app.config.borrow().live_apply;
    for page in app.pages.borrow().iter() {
        for control in &page.controls {
            let container = matches!(
                &*control.editor.borrow(),
                Editor::Submodule { .. } | Editor::HorizontalSubmodule { .. }
            );
            control.live.set_visible(
                enabled && !container && control.record.spec.apply_command.is_some(),
            );
        }
    }
}

pub(super) fn control_cmp(
    a: &ControlRow,
    b: &ControlRow,
    ranks: &HashMap<String, usize>,
) -> std::cmp::Ordering {
    crate::model::record_order(&a.record, a.src_index, &b.record, b.src_index, ranks)
}

/// A control stays inline (label left, widget right) only for a `switch`, a
/// `spin`, and a `dropdown` whose longest choice is short. Everything else
/// stacks the widget below the label and fills the width.
fn control_is_inline(editor: &Editor) -> bool {
    match editor {
        Editor::Switch(_) | Editor::Spin(_) => true,
        Editor::Dropdown { choices, .. } => {
            choices.iter().map(|c| c.chars().count()).max().unwrap_or(0) < 24
        }
        Editor::Text(_) | Editor::Path(_) | Editor::Multiline { .. } | Editor::Package { .. } => {
            false
        }
        _ => false,
    }
}

/// The flake's available module names per kind, collected from scan origins.
pub(super) fn build_context(app: &Rc<App>) -> widgets::BuildContext {
    let mut ctx = widgets::BuildContext::default();
    {
        let scan = app.scan.borrow();
        if let Some(scan) = scan.as_ref() {
            for origin in scan.origin_menu.keys() {
                if let Some(name) = origin.strip_prefix("modules.nixos.") {
                    push_unique(&mut ctx.nixos_modules, name);
                } else if let Some(name) = origin.strip_prefix("modules.homeManager.") {
                    push_unique(&mut ctx.hm_modules, name);
                }
            }
            let mut presets = HashMap::new();
            for menu in &scan.menus {
                if !menu.widget_presets.is_empty() {
                    presets.insert(menu.file.clone(), menu.widget_presets.clone());
                }
            }
            ctx.presets = std::rc::Rc::new(presets);
        }
    }
    ctx.nixos_modules.sort();
    ctx.hm_modules.sort();
    ctx.search = Some(package_search(app));
    ctx.plugin_registry = Some(plugin_registry_hook(app));
    ctx.apply = Some(apply_hook(app));
    ctx
}

/// Apply hook: queues the option's apply command, so the command runs on the
/// app's schedule instead of the widget's.
fn apply_hook(app: &Rc<App>) -> widgets::ApplyHook {
    let app = app.clone();
    Rc::new(move |request: crate::apply::ApplyRequest| {
        super::apply::schedule(&app, request);
    })
}

/// Package search hook: queries nixd off the main thread and delivers the
/// results back on the main thread.
fn package_search(app: &Rc<App>) -> widgets::PackageSearch {
    let service = app.package_service.clone();
    let flake = app.config.borrow().flake_path.clone().unwrap_or_default();
    Rc::new(move |term: String, done: Box<dyn FnOnce(Vec<String>)>| {
        service.search(flake.clone(), term, done);
    })
}

/// Plugin registry hook: serves a cached id list instantly, or queues the
/// callback behind one worker-thread `nix eval` of the registry extractor.
fn plugin_registry_hook(app: &Rc<App>) -> widgets::PluginRegistryHook {
    let state = app.plugin_registry.clone();
    let tx = app.tx.clone();
    let base = app.base.borrow().clone();
    Rc::new(move |done| {
        if let Some(cached) = state.cache.borrow().as_ref() {
            done(cached.clone());
            return;
        }
        state.pending.borrow_mut().push(done);
        if state.loading.replace(true) {
            return;
        }
        let tx = tx.clone();
        let base = base.clone();
        std::thread::spawn(move || {
            let result = crate::metadata::dms_plugins(&base);
            let _ = tx.send_blocking(Msg::PluginRegistry(result));
        });
    })
}

fn push_unique(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|n| n == name) {
        list.push(name.to_string());
    }
}

pub(super) fn build_control_row(
    app: &Rc<App>,
    record: &Record,
    src_index: usize,
) -> Rc<ControlRow> {
    let mut ctx = build_context(app);
    ctx.advanced = app.advanced.get();
    let editor = widgets::build(record, None, &ctx);
    let inline = control_is_inline(&editor)
        && record
            .spec
            .description
            .as_ref()
            .map_or(true, |text| text.is_empty());
    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Fill);

    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    labels.set_hexpand(true);
    let label_text = record
        .spec
        .label
        .clone()
        .or_else(|| record.segments.last().cloned())
        .unwrap_or_else(|| record.path());
    let name = gtk::Label::builder()
        .label(label_text)
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    let live = live_marker(record);
    live.set_visible(
        app.config.borrow().live_apply
            && record.spec.apply_command.is_some()
            && !matches!(&editor, Editor::Submodule { .. } | Editor::HorizontalSubmodule { .. }),
    );
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    title.set_hexpand(true);
    title.append(&name);
    title.append(&live);
    labels.append(&title);

    let desc = gtk::Label::builder()
        .label(record.spec.description.clone().unwrap_or_default())
        .xalign(0.0)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim-label", "caption"])
        .width_chars(48)
        .build();
    desc.set_visible(!desc.label().is_empty());

    let dirty = gtk::Label::builder()
        .label("•")
        .xalign(0.0)
        .visible(false)
        .css_classes(["dim-label"])
        .build();
    labels.append(&dirty);

    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    slot.set_valign(gtk::Align::Center);
    if inline {
        slot.set_halign(gtk::Align::End);
    } else {
        slot.set_hexpand(true);
        slot.set_halign(gtk::Align::Fill);
    }

    let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
    reset.add_css_class("flat");
    reset.set_tooltip_text(Some("Reset to default"));
    reset.set_valign(gtk::Align::Center);
    reset.set_halign(gtk::Align::End);

    slot.append(&editor.root());
    slot.append(&reset);

    if inline {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.set_hexpand(true);
        line.set_halign(gtk::Align::Fill);
        line.append(&labels);
        line.append(&slot);
        row.append(&line);
    } else {
        row.append(&labels);
        row.append(&slot);
    }
    row.append(&desc);

    if !record.origin.is_empty() {
        let origin = gtk::Label::builder()
            .label(format!("imported from {}", record.origin))
            .xalign(0.0)
            .hexpand(true)
            .halign(gtk::Align::Fill)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build();
        row.append(&origin);
    }

    row.set_tooltip_text(Some(&record.path()));

    let control = Rc::new(ControlRow {
        record: record.clone(),
        editor: Rc::new(RefCell::new(editor)),
        row,
        slot,
        desc,
        dirty,
        live,
        reset,
        src_index,
        dep_visible: Cell::new(true),
    });

    {
        let app = app.clone();
        let weak = Rc::downgrade(&control);
        control.reset.clone().connect_clicked(move |_| {
            if let Some(control) = weak.upgrade() {
                reset_to_default(&app, &control);
            }
        });
    }

    connect_dirty(app, &control);
    control
}

/// Set the dirty bullet whenever any editor widget changes, based on whether
/// the editor still matches the record's original value, and re-evaluate any
/// visibility dependencies that key off the changed editor.
///
/// The same callback runs the option's apply command, so a change that the
/// widget reports through its shared change hook reaches the command too.
pub(super) fn connect_dirty(app: &Rc<App>, row: &ControlRow) {
    let dirty = row.dirty.clone();
    let original = row.record.value.clone();
    let weak = Rc::downgrade(&row.editor);
    let app = app.clone();
    let apply_command = row.record.spec.apply_command.clone();
    let apply_path = row.record.path();
    let update: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(editor) = weak.upgrade() {
            let unchanged = editor.borrow().unchanged(&original);
            dirty.set_visible(!unchanged);
            if let Some(command) = apply_command.clone() {
                apply_now(&app, &apply_path, &command, &editor.borrow());
            }
        }
        refresh_dependencies(&app);
    });
    connect_editor_signals(&row.editor.borrow(), &update);
}

/// Queue the apply command of an option with the value its editor holds now.
/// An option that holds other options has no value of its own to apply.
fn apply_now(app: &Rc<App>, path: &str, command: &str, editor: &Editor) {
    if matches!(
        editor,
        Editor::Submodule { .. } | Editor::HorizontalSubmodule { .. }
    ) {
        return;
    }
    super::apply::schedule(
        app,
        crate::apply::ApplyRequest {
            path: path.to_string(),
            command: command.to_string(),
            value: editor.to_nix(),
        },
    );
}

/// Connect `update` to the change signal of every widget owned by the editor,
/// recursing into submodule children.
fn connect_editor_signals(editor: &Editor, update: &Rc<dyn Fn()>) {
    editor.connect_changed(update);
}

/// Recompute `dep_visible` for every control from the current values of the
/// options it depends on, then re-apply the combined visibility. Dependency
/// scope is one page: `visibleWhen.path` is looked up by full option path
/// within the same menu.
fn refresh_dependencies(app: &Rc<App>) {
    let current = app.current_file.borrow().clone();
    for page in app.pages.borrow().iter() {
        if page.file == current {
            evaluate_dependencies(&page.controls);
        }
    }
    apply_control_filter(app);
}

/// Current value of `path` in `values`, tolerating the `config.` prefix the
/// scanner records for annotated assignments inside a module's `config = { }`
/// wrapper: annotation paths omit that wrapper, so both spellings are tried.
fn resolve_dependency_value<'a>(
    values: &'a HashMap<String, String>,
    path: &str,
) -> Option<&'a String> {
    values
        .get(path)
        .or_else(|| values.get(&format!("config.{path}")))
        .or_else(|| path.strip_prefix("config.").and_then(|s| values.get(s)))
}

/// Whether `vis` holds for the governing value found in `values`.
fn dependency_visible(vis: &Visibility, values: &HashMap<String, String>) -> bool {
    let current = resolve_dependency_value(values, &vis.path).map(String::as_str);
    vis.holds(current)
}

/// Set each control's `dep_visible` from its `visibleWhen` condition, looking
/// the governing value up through `resolve_dependency_value`. A governing
/// option that is absent fails open, so a stale path never hides a setting.
pub(super) fn evaluate_dependencies(controls: &[Rc<ControlRow>]) {
    let mut values: HashMap<String, String> = HashMap::new();
    for control in controls {
        values
            .entry(control.record.path())
            .or_insert_with(|| control.editor.borrow().to_nix());
    }
    for control in controls {
        let visible = match &control.record.spec.visible_when {
            Some(vis) => dependency_visible(vis, &values),
            None => true,
        };
        control.dep_visible.set(visible);
    }
}

fn visible_group_names(
    rows: impl IntoIterator<Item = ((bool, String), bool)>,
) -> HashSet<(bool, String)> {
    let mut set = HashSet::new();
    for ((advanced, group), row_visible) in rows {
        if row_visible && !group.is_empty() {
            set.insert((advanced, group));
        }
    }
    set
}

pub(super) fn apply_control_filter(app: &Rc<App>) {
    let query = app.search.text().to_lowercase();
    let current = app.current_file.borrow().clone();
    for page in app.pages.borrow().iter() {
        if page.file != current {
            continue;
        }
        let mut first_match: Option<&Rc<ControlRow>> = None;
        let mut visible_rows: Vec<((bool, String), bool)> = Vec::new();
        for control in &page.controls {
            control.row.remove_css_class("search-hit");
            let matches = query.is_empty()
                || control.record.path().to_lowercase().contains(&query)
                || control
                    .record
                    .spec
                    .label
                    .as_deref()
                    .map(|l| l.to_lowercase().contains(&query))
                    .unwrap_or(false);
            let visible = matches && control.dep_visible.get();
            control.row.set_visible(visible);
            if visible && !query.is_empty() && first_match.is_none() {
                first_match = Some(control);
            }
            let advanced = control.record.spec.advanced;
            let group = control.record.spec.group.clone().unwrap_or_default();
            visible_rows.push(((advanced, group), visible));
        }
        let visible_groups = visible_group_names(visible_rows);
        for (advanced, group_name, label) in &page.group_headers {
            let visible = visible_groups.contains(&(*advanced, group_name.clone()));
            label.set_visible(visible);
        }
        for (group_name, button) in &page.group_inserts {
            let has_header = !group_name.is_empty()
                && page
                    .group_headers
                    .iter()
                    .any(|(adv, name, _)| !*adv && name == group_name);
            if has_header {
                let visible = visible_groups.contains(&(false, group_name.clone()));
                button.set_visible(visible);
            }
        }
        if let Some(control) = first_match {
            control.row.add_css_class("search-hit");
            scroll_to_control(page, &control.row);
        }
    }
}

/// Scroll the page so `row` sits just below the top of the viewport.
fn scroll_to_control(page: &MenuPage, row: &gtk::Box) {
    let Ok(scrolled) = page.root.clone().downcast::<gtk::ScrolledWindow>() else {
        return;
    };
    if let Some(bounds) = row.compute_bounds(&page.content) {
        scrolled
            .vadjustment()
            .set_value(scroll_offset(bounds.y() as f64));
    }
}

fn scroll_offset(bounds_y: f64) -> f64 {
    bounds_y - 12.0
}

/// Rebuild one control's editor from the option registry's default value.
fn reset_to_default(app: &Rc<App>, control: &ControlRow) {
    let Some(target) = Target::parse(&app.target.borrow()) else {
        app.status.set_text("No option target for this config");
        return;
    };
    let base = app.base.borrow().clone();
    let path = control.record.segments.clone();
    match metadata::default_expression(&base, &target, &path) {
        Ok(default) if !default.trim().is_empty() => {
            let mut record = control.record.clone();
            record.value = default;
            let editor = widgets::build(&record, None, &build_context(app));
            control.set_editor(editor);
            connect_dirty(app, control);
            app.status.set_text("Reset to default");
            apply_reset(app, control);
        }
        Ok(_) => app.status.set_text("No default available"),
        Err(e) => app.status.set_text(&format!("Cannot resolve default: {e}")),
    }
}

/// Apply the reset value right away, so a control with an apply command does
/// not wait for the next edit to reach the running system.
fn apply_reset(app: &Rc<App>, control: &ControlRow) {
    let Some(command) = control.record.spec.apply_command.clone() else {
        return;
    };
    let value = control.editor.borrow().to_nix();
    super::apply::schedule(
        app,
        crate::apply::ApplyRequest {
            path: control.record.path(),
            command,
            value,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan;

    #[test]
    fn offsets_scroll_above_the_target() {
        assert_eq!(scroll_offset(120.0), 108.0);
        assert_eq!(scroll_offset(0.0), -12.0);
    }

    #[test]
    fn hides_group_when_all_rows_invisible() {
        let visible = visible_group_names([
            ((false, "Firefox".to_string()), false),
            ((false, "Firefox".to_string()), false),
        ]);
        assert!(!visible.contains(&(false, "Firefox".to_string())));
    }

    #[test]
    fn shows_group_when_one_row_visible() {
        let visible = visible_group_names([
            ((false, "Chrome".to_string()), false),
            ((false, "Chrome".to_string()), true),
        ]);
        assert!(visible.contains(&(false, "Chrome".to_string())));
    }

    #[test]
    fn separates_same_group_name_by_section() {
        let visible = visible_group_names([
            ((false, "Theme".to_string()), false),
            ((true, "Theme".to_string()), true),
        ]);
        assert!(!visible.contains(&(false, "Theme".to_string())));
        assert!(visible.contains(&(true, "Theme".to_string())));
    }

    #[test]
    fn empty_rows_yield_empty_visible_set() {
        let visible = visible_group_names(Vec::<((bool, String), bool)>::new());
        assert!(visible.is_empty());
    }

    #[test]
    fn resolves_dependency_paths_across_config_wrapper() {
        let mut values = HashMap::new();
        values.insert(
            "config.ojuny.browser.selected".to_string(),
            "\"firefox\"".to_string(),
        );
        assert_eq!(
            resolve_dependency_value(&values, "ojuny.browser.selected").map(String::as_str),
            Some("\"firefox\"")
        );
        values.insert("programs.steam.enable".to_string(), "false".to_string());
        assert_eq!(
            resolve_dependency_value(&values, "programs.steam.enable").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            resolve_dependency_value(&values, "config.programs.steam.enable").map(String::as_str),
            Some("false")
        );
        assert_eq!(resolve_dependency_value(&values, "missing.path"), None);
    }

    #[test]
    fn visible_when_resolves_config_wrapped_scan_paths() {
        let text = "{ config = {\n  # @option widget = \"switch\"; label = \"Set gaps per edge\";\n  ojuny.style.customGaps = false;\n  # @option widget = \"spin\"; label = \"Gap size\"; visibleWhen.path = \"ojuny.style.customGaps\"; visibleWhen.equals = false;\n  ojuny.style.gaps = 4;\n}; }";
        let records = scan::scan_value(text);
        let mut values: HashMap<String, String> = records
            .iter()
            .map(|record| (record.path(), record.value.clone()))
            .collect();
        let gated = records
            .iter()
            .find(|record| record.spec.visible_when.is_some())
            .unwrap();
        let vis = gated.spec.visible_when.as_ref().unwrap();
        assert_eq!(vis.path, "ojuny.style.customGaps");
        assert!(values.contains_key("config.ojuny.style.customGaps"));
        assert!(dependency_visible(vis, &values));
        values.insert(
            "config.ojuny.style.customGaps".to_string(),
            "true".to_string(),
        );
        assert!(!dependency_visible(vis, &values));
    }
}
