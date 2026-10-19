use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::dsl::OptionSpec;
use crate::model::Record;
use crate::value;
use crate::widgets::{
    attr_key, build_bespoke_child, build_entry, build_spin, build_switch, hook_notify,
    scan_children, BuildContext, ChangeHook, Editor,
};

use self::PluginFieldKind::{Bool, Float, Int, Text};

/// One plugin entry of the `plugins` widget: the plugin id, its enable
/// switch, and the entry's remaining settings behind a cog pop-out.
struct PluginRow {
    key: gtk::Entry,
    orig_key: String,
    #[allow(dead_code)]
    name_label: gtk::Label,
    enable: Editor,
    enable_orig: String,
    enable_present: bool,
    children: Vec<(Record, Editor)>,
    #[allow(dead_code)]
    settings: Option<(gtk::MenuButton, gtk::Popover)>,
    row_box: gtk::Box,
    remove: gtk::Button,
}

/// The rows of a `plugins` editor, shared by construction and wiring.
type PluginRows = Rc<RefCell<Vec<PluginRow>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluginFieldKind {
    Bool,
    Int,
    Float,
    Text,
}

/// One known setting of a known plugin: its dotted path under the plugin,
/// the heading shown above the control, and the control the setting uses.
#[derive(Debug, Clone, Copy)]
struct PluginFieldSpec {
    path: &'static str,
    label: &'static str,
    kind: PluginFieldKind,
}

/// One entry of the plugin registry: the id as written in configuration, its
/// display name, and the settings the app knows how to label.
#[derive(Debug, Clone, Copy)]
struct PluginSpec {
    id: &'static str,
    name: &'static str,
    fields: &'static [PluginFieldSpec],
}

const fn pspec(
    id: &'static str,
    name: &'static str,
    fields: &'static [PluginFieldSpec],
) -> PluginSpec {
    PluginSpec { id, name, fields }
}

const fn pfield(path: &'static str, label: &'static str, kind: PluginFieldKind) -> PluginFieldSpec {
    PluginFieldSpec { path, label, kind }
}

const PLUGIN_REGISTRY_A: &[PluginSpec] = &[
    pspec("dankMenu", "Dank Menu", &[]),
    pspec(
        "dankKDEConnect",
        "KDE Connect",
        &[pfield("settings.scanSubdirectories", "Scan subdirectories", Bool)],
    ),
    pspec(
        "nixMonitor",
        "Nix Monitor",
        &[
            pfield("settings.showGenerations", "Show generations", Bool),
            pfield("settings.showStoreSize", "Show store size", Bool),
            pfield("settings.updateInterval", "Update interval", Int),
            pfield("settings.updateCheckInterval", "Check interval", Int),
            pfield("settings.gcThresholdGB", "GC threshold", Int),
        ],
    ),
];

const PLUGIN_REGISTRY_B: &[PluginSpec] = &[
    pspec(
        "localServices",
        "Local Services",
        &[
            pfield("settings.ignoredPorts", "Ignored ports", Text),
            pfield("settings.exactWatches", "Exact watches", Text),
            pfield("settings.labels", "Labels", Text),
        ],
    ),
    pspec(
        "scratchpadHelper",
        "Scratchpad Helper",
        &[
            pfield("settings.hideBarWhenEmpty", "Hide bar when empty", Bool),
            pfield("settings.namedManagerEnabled", "Named manager", Bool),
        ],
    ),
    pspec("developerUtilities", "Developer Utilities", &[]),
];

const PLUGIN_REGISTRY_C: &[PluginSpec] = &[
    pspec(
        "screenCaptureToolbar",
        "Screen Capture Toolbar",
        &[
            pfield("settings.enableAudioRecorder", "Audio recorder", Bool),
            pfield("settings.multiMonitorScreenshot", "Multi monitor", Bool),
            pfield("settings.showNotify", "Show notify", Bool),
            pfield("settings.showRecPill", "Show record pill", Bool),
            pfield("settings.toolbarOpacity", "Toolbar opacity", Float),
            pfield("settings.pillOpacity", "Pill opacity", Float),
        ],
    ),
    pspec(
        "commandRunner",
        "Command Runner",
        &[
            pfield("settings.terminal", "Terminal", Text),
            pfield("settings.trigger", "Trigger", Text),
            pfield("settings.execFlag", "Exec flag", Text),
        ],
    ),
    pspec("webSearch", "Web Search", &[]),
];

/// Help text for a setting path, stored as path and tooltip.
struct Help(&'static str, &'static str);

const HELP_A: &[Help] = &[
    Help("settings.scanSubdirectories", "Index nested plugin dirs"),
    Help("settings.showGenerations", "List system generations"),
    Help("settings.showStoreSize", "Show the nix store size"),
    Help("settings.updateInterval", "Seconds between refreshes"),
    Help("settings.updateCheckInterval", "Seconds between checks"),
    Help("settings.gcThresholdGB", "Store size that suggests gc"),
    Help("settings.ignoredPorts", "Comma separated ports to skip"),
    Help("settings.exactWatches", "Paths watched exactly"),
    Help("settings.labels", "Labels as an attrset"),
    Help("settings.hideBarWhenEmpty", "Collapse the bar when idle"),
];

const HELP_B: &[Help] = &[
    Help("settings.namedManagerEnabled", "Allow named scratchpads"),
    Help("settings.enableAudioRecorder", "Include microphone audio"),
    Help("settings.multiMonitorScreenshot", "Capture every monitor"),
    Help("settings.showNotify", "Notify after capture"),
    Help("settings.showRecPill", "Show the record pill"),
    Help("settings.toolbarOpacity", "Opacity of the toolbar"),
    Help("settings.pillOpacity", "Opacity of the pill"),
    Help("settings.terminal", "Terminal for commands"),
    Help("settings.trigger", "Key that opens the runner"),
    Help("settings.execFlag", "Flag for exec mode"),
];

const HELP_TABLES: [&[Help]; 2] = [HELP_A, HELP_B];

/// The help text recorded for a setting path, if any.
fn plugin_field_help(path: &str) -> Option<&'static str> {
    HELP_TABLES
        .iter()
        .find_map(|table| table.iter().find(|help| help.0 == path))
        .map(|help| help.1)
}

/// Every registry table, so lookup and iteration share one source of truth.
const PLUGIN_TABLES: [&[PluginSpec]; 3] = [
    PLUGIN_REGISTRY_A,
    PLUGIN_REGISTRY_B,
    PLUGIN_REGISTRY_C,
];

/// The registry entry for a plugin id, if it is known.
fn plugin_spec(id: &str) -> Option<&'static PluginSpec> {
    PLUGIN_TABLES
        .iter()
        .find_map(|table| table.iter().find(|spec| spec.id == id))
}

/// The registry entry for one dotted setting path under a plugin.
fn plugin_field(id: &str, path: &str) -> Option<&'static PluginFieldSpec> {
    plugin_spec(id)
        .and_then(|spec| spec.fields.iter().find(|field| field.path == path))
}

/// A heading from an identifier: split camel-case boundaries, turn snake case
/// into spaces, and capitalize the first letter.
fn humanize(raw: &str) -> String {
    let mut out = String::new();
    for (index, ch) in raw.chars().enumerate() {
        if ch == '_' {
            out.push(' ');
        } else if ch.is_ascii_uppercase() && index > 0 && out.ends_with(char::is_lowercase) {
            out.push(' ');
            out.push(ch);
        } else if index == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The `plugins` widget: a list of plugin rows plus a footer that adds a
/// plugin chosen from a dropdown of ids not already present.
pub struct PluginList {
    root: gtk::Box,
    rows: PluginRows,
    on_change: ChangeHook,
}

impl PluginList {
    pub fn root(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn to_nix(&self) -> String {
        plugins_nix(&self.rows.borrow())
    }

    pub fn unchanged(&self) -> bool {
        self.rows.borrow().iter().all(|row| {
            row.key.text() == row.orig_key
                && row.enable.unchanged(&row.enable_orig)
                && row
                    .children
                    .iter()
                    .all(|(record, editor)| editor.unchanged(&record.value))
        })
    }

    pub fn connect_changed(&self, update: &Rc<dyn Fn()>) {
        *self.on_change.borrow_mut() = Some(update.clone());
        for row in self.rows.borrow().iter() {
            connect_plugin_row(row, update);
        }
    }
}

/// The picker state shared between the footer and the remove handler, so the
/// dropdown re-offers a plugin once its row is gone.
#[derive(Clone)]
struct Picker {
    dropdown: gtk::DropDown,
    add: gtk::Button,
    rows: PluginRows,
    source: Rc<Vec<String>>,
    available: Rc<RefCell<Vec<String>>>,
}

impl Picker {
    fn refresh(&self) {
        let taken: Vec<String> = self
            .rows
            .borrow()
            .iter()
            .map(|row| row.key.text().to_string())
            .collect();
        let available = available_ids(&self.source, &taken);
        let labels: Vec<String> = available.iter().map(|id| display_name(id)).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.dropdown
            .set_model(Some(&gtk::StringList::new(&refs)));
        self.add.set_sensitive(!available.is_empty());
        *self.available.borrow_mut() = available;
    }
}

/// Build the plugins editor: one row per plugin in the value, plus a footer
/// dropdown and button that add a plugin not already listed.
pub fn build(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_hexpand(true);
    let rows: PluginRows = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    let dropdown = gtk::DropDown::from_strings(&[]);
    let add = gtk::Button::with_label("Add plugin");
    let picker = Picker {
        dropdown: dropdown.clone(),
        add: add.clone(),
        rows: rows.clone(),
        source: Rc::new(picker_source(record)),
        available: Rc::new(RefCell::new(Vec::new())),
    };

    for (id, leaves, shown) in &plugin_groups(record, ctx) {
        let row = make_plugin_row(id, leaves, ctx);
        attach_plugin_row(&container, &rows, row, &on_change, &picker, *shown);
    }
    picker.refresh();

    {
        let dropdown = dropdown.clone();
        let picker = picker.clone();
        let container = container.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let ctx = ctx.clone();
        add.connect_clicked(move |_| {
            let index = dropdown.selected() as usize;
            let Some(id) = picker.available.borrow().get(index).cloned() else {
                return;
            };
            scaffold_plugin_row(&id, &container, &rows, &on_change, &ctx, &picker);
            picker.refresh();
        });
    }

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    footer.set_halign(gtk::Align::Start);
    footer.append(&dropdown);
    footer.append(&add);

    root.append(&container);
    root.append(&footer);

    Editor::Plugins {
        list: PluginList {
            root,
            rows,
            on_change,
        },
    }
}

/// Group the plugin value's flattened leaves by plugin id, rebasing each
/// leaf's path under its plugin. The bool reports whether the row is shown.
fn plugin_groups(record: &Record, ctx: &BuildContext) -> Vec<(String, Vec<Record>, bool)> {
    let mut groups: Vec<(String, Vec<Record>, bool)> = Vec::new();
    for child in scan_children(record, true) {
        let Some(id) = child.segments.first().cloned() else { continue };
        let visible = ctx.advanced || !child.spec.hidden;
        let leaves = if child.segments.len() > 1 {
            let mut leaf = child;
            leaf.segments.remove(0);
            vec![leaf]
        } else if value::classify(&child.value) == value::ValueKind::AttrSet {
            scan_children(&child, true)
        } else {
            Vec::new()
        };
        match groups.iter_mut().find(|(gid, _, _)| *gid == id) {
            Some((_, existing, shown)) => { existing.extend(leaves); *shown &= visible; }
            None => groups.push((id, leaves, visible)),
        }
    }
    groups
}

/// Serialize a setting's rebased path as a dotted attribute path, quoting
/// each segment that is not a plain identifier.
fn plugin_path_nix(record: &Record) -> String {
    record
        .segments
        .iter()
        .map(|segment| attr_key(segment))
        .collect::<Vec<String>>()
        .join(".")
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
                fields.push(format!("{} = {};", plugin_path_nix(record), editor.to_nix()));
            }
            format!("{} = {{ {} }};", attr_key(&row.key.text()), fields.join(" "))
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

/// The enable control of one plugin: the switch, the value it started from
/// (a registry default when the binding is absent), and whether the binding
/// exists in configuration.
fn plugin_enable(id: &str, leaves: &[Record]) -> (Editor, String, bool) {
    let record = leaves.iter().find(|field| field.segments.as_slice() == ["enable"]);
    let present = record.is_some();
    let fallback = if plugin_spec(id).is_some() { "true" } else { "false" };
    let original = record
        .map(|field| field.value.clone())
        .unwrap_or_else(|| fallback.to_string());
    let value = record.map(|field| field.value.as_str()).unwrap_or(fallback);
    (Editor::Switch(build_switch(value)), original, present)
}

/// A plugin setting's control: the registry decides between a switch, a
/// number field, and a text field; anything unknown falls back to the value's
/// own shape.
fn build_plugin_child(id: &str, record: &Record, ctx: &BuildContext) -> Editor {
    let path = record.segments.join(".");
    match plugin_field(id, &path).map(|field| field.kind) {
        Some(PluginFieldKind::Bool) => Editor::Switch(build_switch(&record.value)),
        Some(PluginFieldKind::Int | PluginFieldKind::Float) => {
            let spec = OptionSpec {
                min: Some(0.0),
                ..OptionSpec::default()
            };
            Editor::Spin(build_spin(&spec, &record.value))
        }
        Some(PluginFieldKind::Text) => {
            Editor::Text(build_entry(&record.value, &OptionSpec::default(), false))
        }
        None => build_bespoke_child(record, ctx),
    }
}

/// The caption for one plugin setting: the template `@option label` wins over
/// the registry label, and the leaf name is the last resort.
fn setting_label(record: &Record, id: &str) -> String {
    let path = record.segments.join(".");
    let leaf = record.segments.last().cloned().unwrap_or_else(|| path.clone());
    record
        .spec
        .label
        .clone()
        .or_else(|| plugin_field(id, &path).map(|field| field.label.to_string()))
        .unwrap_or_else(|| humanize(&leaf))
}

/// One setting of a plugin row: a heading, the field's own label, and help
/// text as its tooltip.
fn build_plugin_setting_row(id: &str, record: &Record, editor: &Editor) -> gtk::Widget {
    let path = record.segments.join(".");
    let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label(setting_label(record, id))
        .css_classes(["dim-label", "caption"])
        .build();
    if let Some(help) = record
        .spec
        .description
        .as_deref()
        .or(plugin_field_help(&path))
    {
        heading.set_tooltip_text(Some(help));
    }
    cell.append(&heading);
    cell.append(&editor.root());
    cell.upcast()
}

/// The settings of one plugin behind a cog pop-out, hidden fields dropped
/// while Advanced is off.
fn plugin_settings_popover(
    id: &str,
    children: &[(Record, Editor)],
    ctx: &BuildContext,
) -> (gtk::MenuButton, gtk::Popover) {
    let detail = gtk::Box::new(gtk::Orientation::Vertical, 8);
    detail.set_margin_top(12);
    detail.set_margin_bottom(12);
    detail.set_margin_start(12);
    detail.set_margin_end(12);

    let name = plugin_spec(id)
        .map(|spec| spec.name.to_string())
        .unwrap_or_else(|| humanize(id));
    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label(name)
        .css_classes(["heading"])
        .build();
    detail.append(&heading);
    let caption = gtk::Label::builder()
        .xalign(0.0)
        .label(format!("{id} plugin"))
        .css_classes(["dim-label", "caption"])
        .build();
    detail.append(&caption);
    detail.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    for (record, editor) in children {
        if record.spec.hidden && !ctx.advanced {
            continue;
        }
        detail.append(&build_plugin_setting_row(id, record, editor));
    }

    let popover = gtk::Popover::builder().child(&detail).build();
    let cog = gtk::MenuButton::builder()
        .icon_name("emblem-system-symbolic")
        .css_classes(["flat"])
        .tooltip_text("Plugin settings")
        .popover(&popover)
        .build();
    (cog, popover)
}

/// Assemble one plugin row from its leaves: the name (static for known ids,
/// an editable entry otherwise), a right-aligned cog/enable/remove cluster,
/// and the structural handles.
fn make_plugin_row(id: &str, leaves: &[Record], ctx: &BuildContext) -> PluginRow {
    let (enable, enable_orig, enable_present) = plugin_enable(id, leaves);
    let children: Vec<(Record, Editor)> = leaves
        .iter()
        .filter(|field| field.segments.as_slice() != ["enable"])
        .map(|field| (field.clone(), build_plugin_child(id, field, ctx)))
        .collect();

    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row_box.set_hexpand(true);
    row_box.set_valign(gtk::Align::Center);

    let known = plugin_spec(id).is_some();
    let name_label = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(display_name(id))
        .build();
    name_label.set_tooltip_text(Some(id));

    let key = gtk::Entry::new();
    key.set_width_chars(16);
    key.set_text(id);
    key.set_tooltip_text(Some("Plugin id (not in the registry)"));

    if known {
        row_box.append(&name_label);
    } else {
        key.set_hexpand(true);
        row_box.append(&key);
    }

    let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    trailing.set_halign(gtk::Align::End);
    trailing.set_valign(gtk::Align::Center);

    let settings = (!children.is_empty()).then(|| plugin_settings_popover(id, &children, ctx));
    if let Some((cog, _)) = &settings {
        trailing.append(cog);
    }
    trailing.append(&enable.root());

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove plugin"));
    trailing.append(&remove);

    row_box.append(&trailing);

    PluginRow {
        key,
        orig_key: id.to_string(),
        name_label,
        enable,
        enable_orig,
        enable_present,
        children,
        settings,
        row_box,
        remove,
    }
}

/// Wire a row's remove button to drop it from the container and the row list.
/// The change hook is read at click time so rows added before the app
/// connects still notify.
fn wire_plugin_remove(
    row: &PluginRow,
    container: &gtk::Box,
    rows: &PluginRows,
    on_change: &ChangeHook,
    picker: &Picker,
) {
    let container = container.clone();
    let rows = rows.clone();
    let row_box = row.row_box.clone();
    let key = row.key.clone();
    let on_change = on_change.clone();
    let picker = picker.clone();
    row.remove.connect_clicked(move |_| {
        container.remove(&row_box);
        rows.borrow_mut().retain(|r| r.key != key);
        picker.refresh();
        if let Some(cb) = on_change.borrow().as_ref() {
            cb();
        }
    });
}

/// Register a row with the row list and wire its remove button. `show` keeps
/// a hidden row in the data while leaving it out of the container.
fn attach_plugin_row(
    container: &gtk::Box,
    rows: &PluginRows,
    row: PluginRow,
    on_change: &ChangeHook,
    picker: &Picker,
    show: bool,
) {
    wire_plugin_remove(&row, container, rows, on_change, picker);
    if show {
        container.append(&row.row_box);
    }
    rows.borrow_mut().push(row);
}

/// Add a fresh plugin row for `id`: an unset original key marks it for
/// writing, and the hook notifies the change handler.
fn scaffold_plugin_row(
    id: &str,
    container: &gtk::Box,
    rows: &PluginRows,
    on_change: &ChangeHook,
    ctx: &BuildContext,
    picker: &Picker,
) {
    let mut row = make_plugin_row(id, &[], ctx);
    row.orig_key = "\0".to_string();
    connect_plugin_row(&row, &hook_notify(on_change));
    attach_plugin_row(container, rows, row, on_change, picker, true);
    if let Some(cb) = on_change.borrow().as_ref() {
        cb();
    }
}

/// The ids the footer dropdown offers: the template's `choices` when present,
/// otherwise the registry ids in table order.
fn picker_source(record: &Record) -> Vec<String> {
    if let Some(choices) = &record.spec.choices {
        if !choices.is_empty() {
            return choices.clone();
        }
    }
    PLUGIN_TABLES
        .iter()
        .flat_map(|table| table.iter())
        .map(|spec| spec.id.to_string())
        .collect()
}

/// The display name for a picker id: the registry name, or a humanized id.
fn display_name(id: &str) -> String {
    plugin_spec(id)
        .map(|spec| spec.name.to_string())
        .unwrap_or_else(|| humanize(id))
}

/// The source ids minus the ones already present, so a plugin is offered once.
fn available_ids(source: &[String], taken: &[String]) -> Vec<String> {
    source
        .iter()
        .filter(|id| !taken.iter().any(|present| present == *id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::WidgetKind;

    fn template_file(relative: &str) -> std::path::PathBuf {
        match std::env::var("OJUNY_TEMPLATE") {
            Ok(dir) => std::path::PathBuf::from(dir).join(relative),
            Err(_) => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../ojuny-template")
                .join(relative),
        }
    }

    fn plugins_record(text: &str) -> Record {
        crate::scan::scan_file(std::path::Path::new("fixture.nix"), text)
            .records
            .into_iter()
            .find(|record| record.spec.widget == Some(WidgetKind::Plugins))
            .expect("a plugins record")
    }

    #[test]
    fn picker_source_prefers_template_choices() {
        let mut record = Record::default();
        assert_eq!(picker_source(&record).len(), 9);
        record.spec.choices = Some(vec!["dankTodo".to_string(), "nixMonitor".to_string()]);
        assert_eq!(picker_source(&record), ["dankTodo", "nixMonitor"]);
        record.spec.choices = Some(vec![]);
        assert_eq!(picker_source(&record).len(), 9);
    }

    #[test]
    fn availability_filter_drops_present_ids() {
        let source = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let taken = vec!["b".to_string()];
        assert_eq!(available_ids(&source, &taken), ["a", "c"]);
        assert_eq!(available_ids(&source, &[]).len(), 3);
        assert!(available_ids(&source, &source).is_empty());
    }

    #[test]
    fn setting_label_prefers_template_annotation() {
        let mut record = Record {
            segments: vec!["settings".to_string(), "updateInterval".to_string()],
            value: "3600".to_string(),
            ..Record::default()
        };
        assert_eq!(setting_label(&record, "nixMonitor"), "Update interval");
        assert_eq!(setting_label(&record, "unknown"), "Update Interval");
        record.segments = vec!["rebuildCommand".to_string()];
        assert_eq!(setting_label(&record, "nixMonitor"), "Rebuild Command");
        record.spec.label = Some("Refresh every".to_string());
        assert_eq!(setting_label(&record, "nixMonitor"), "Refresh every");
    }

    #[test]
    fn groups_scan_template_plugins() {
        let path = template_file("modules/06-utility/02-plugins.nix");
        if !path.exists() {
            eprintln!("skipping: sibling ojuny-template checkout not available");
            return;
        }
        let text = std::fs::read_to_string(path).expect("02-plugins.nix is readable");
        let record = plugins_record(&text);
        let groups = plugin_groups(&record, &BuildContext::default());
        assert_eq!(groups.len(), 9, "plugin count");
        let (_, leaves, shown) = groups
            .iter()
            .find(|(id, _, _)| id == "nixMonitor")
            .expect("nixMonitor");
        assert!(*shown);
        let paths: Vec<String> = leaves.iter().map(|leaf| leaf.segments.join(".")).collect();
        assert!(paths.contains(&"enable".to_string()));
        assert_eq!(
            paths.iter().filter(|path| path.starts_with("settings.")).count(),
            5
        );
        assert!(
            paths.iter().any(|path| path.starts_with("config.")),
            "{paths:?}"
        );
    }

    #[test]
    fn parses_multiline_choices_annotation() {
        let text = r#"{ ... }: {
  flake.modules.nixos.x = { ... }: {
    /* @option widget = "plugins"; label = "Plugins";
       choices = [ "dankTodo" "nixMonitor" ]; */
    desktop.ojuny.dms.plugins = {
      nixMonitor = { enable = true; };
    };
  };
}"#;
        let record = plugins_record(text);
        assert_eq!(
            record.spec.choices,
            Some(vec!["dankTodo".to_string(), "nixMonitor".to_string()])
        );
        assert_eq!(picker_source(&record), ["dankTodo", "nixMonitor"]);
    }

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn enable_falls_back_by_registry_membership() {
        if gtk::init().is_err() {
            eprintln!("skipping plugin enable check: no display");
            return;
        }
        let (known, orig, present) = plugin_enable("nixMonitor", &[]);
        assert!(matches!(known, Editor::Switch(_)));
        assert_eq!(orig, "true");
        assert!(!present);
        let (unknown, orig, present) = plugin_enable("myThing", &[]);
        assert!(matches!(unknown, Editor::Switch(_)));
        assert_eq!(orig, "false");
        assert!(!present);
    }

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn plugin_list_round_trips_untouched_value() {
        if gtk::init().is_err() {
            eprintln!("skipping plugin list round-trip: no display");
            return;
        }
        let source = "{ nixMonitor = { enable = true; settings = { showStoreSize = true; }; }; }";
        let record = Record {
            value: source.to_string(),
            ..Record::default()
        };
        let editor = build(&record, &BuildContext::default());
        assert!(editor.unchanged(source));
        assert!(editor.edits(&record).is_empty());
        assert_eq!(
            editor.to_nix(),
            "{ nixMonitor = { enable = true; settings.showStoreSize = true; }; }"
        );
    }
}
