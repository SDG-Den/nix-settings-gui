use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::dsl::{OptionSpec, PresetFieldSpec, WidgetKind, WidgetPresetSpec};
use crate::edit::PendingEdit;
use crate::model::Record;
use crate::monitor_detect::{DetectedMode, DetectedMonitor};
use crate::scan;
use crate::value;

/// A late-bound change callback, set when the app wires a table editor's rows
/// so dynamically added rows can notify the same handler.
pub(crate) type ChangeHook = Rc<RefCell<Option<Rc<dyn Fn()>>>>;

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

/// File path -> presets declared by @widget-preset markers in that file.
pub type PresetMap = std::rc::Rc<
    std::collections::HashMap<String, Vec<(crate::dsl::WidgetPresetSpec, Vec<crate::dsl::PresetFieldSpec>)>>,
>;

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
    pub presets: PresetMap,
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

/// Which widget list a tile belongs to; drives the picker pool and the
/// serialization form (center is a plain string list).
#[derive(Debug, Clone, Copy, PartialEq)]
enum TileSection {
    BarLeft,
    BarCenter,
    BarRight,
    Dock,
}

impl TileSection {
    fn picker_filter(self) -> &'static str {
        match self {
            TileSection::BarLeft => "left",
            TileSection::BarCenter => "center",
            TileSection::BarRight => "right",
            TileSection::Dock => "dock",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            TileSection::BarLeft => "Left widgets",
            TileSection::BarCenter => "Center widgets",
            TileSection::BarRight => "Right widgets",
            TileSection::Dock => "Widgets",
        }
    }

    fn is_center(self) -> bool {
        matches!(self, TileSection::BarCenter)
    }
}

/// One draggable widget tile. Order in the enclosing Vec is the position.
pub(crate) struct WidgetTile {
    /// Current attrset key; editable in the cog pop-out for unknown ids.
    id: String,
    orig_id: String,
    /// The widget's enabled switch, never rendered on the tile face; the face
    /// shows a dimmed body and a toggle dot instead.
    enabled: gtk::Switch,
    orig_enabled: String,
    /// Tuning fields: every scanned field except "enabled" and "_order".
    fields: Rc<RefCell<Vec<AttrsetField>>>,
    /// Original "_order" raw text, empty for tiles added this session.
    orig_order: String,
    /// gtk::ListBoxRow child; row widget is always the tile face box.
    row: gtk::ListBoxRow,
}

/// A whole tile list: heading, tiles, and add buttons.
pub(crate) struct WidgetTileList {
    root: gtk::Box,
    tiles: Rc<RefCell<Vec<WidgetTile>>>,
    /// Original id sequence as scanned, for unchanged detection.
    orig_ids: Vec<String>,
}

/// The values a pop-out held when it opened, so Cancel can write them back
/// into the live row widgets.
struct PopoutSnapshot {
    name: String,
    modifier: String,
    keys: String,
    fields: Vec<(String, String)>,
}

/// One menu row's pop-out state, cloned into the shared edit/cancel wiring of
/// the menu pop-out window.
#[derive(Clone)]
struct MenuPopout {
    popup_box: gtk::Box,
    fields: Rc<RefCell<Vec<AttrsetField>>>,
    snapshot: Rc<RefCell<Option<PopoutSnapshot>>>,
}

impl MenuPopout {
    /// Capture the row's current field values as the restore point.
    fn capture(&self) {
        let values = capture_field_values(&self.fields.borrow());
        *self.snapshot.borrow_mut() = Some(PopoutSnapshot {
            name: String::new(),
            modifier: String::new(),
            keys: String::new(),
            fields: values,
        });
    }

    /// Put the captured values back into the row's live editors.
    fn cancel(&self) {
        let taken = self.snapshot.borrow_mut().take();
        if let Some(snapshot) = taken {
            restore_field_values(&self.fields.borrow(), &snapshot.fields);
        }
    }

    /// Drop the restore point without touching the editors, as Apply does.
    fn discard(&self) {
        self.snapshot.borrow_mut().take();
    }
}

pub(crate) struct MenuRow {
    section: String,
    key: gtk::Entry,
    orig_key: String,
    icon: String,
    label: gtk::Entry,
    orig_label: String,
    aliases: gtk::Entry,
    orig_aliases: String,
    popup_fields: Rc<RefCell<Vec<AttrsetField>>>,
    popup_box: gtk::Box,
    row_box: gtk::Box,
    remove: gtk::Button,
    snapshot: Rc<RefCell<Option<PopoutSnapshot>>>,
}

/// One bind of the `keybinds` widget: a display label and description on the
/// main line, the bind's full field set in a hidden edit popup.
pub(crate) struct KeybindRow {
    key: gtk::Entry,
    orig_key: String,
    modifier: gtk::Entry,
    keys: gtk::Entry,
    orig_modifier: String,
    orig_keys: String,
    fields: Rc<RefCell<Vec<AttrsetField>>>,
    row_box: gtk::Box,
    remove: gtk::Button,
    popup: gtk::Window,
    snapshot: Rc<RefCell<Option<PopoutSnapshot>>>,
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
    rr: Editor,
    extras: Vec<AttrsetField>,
    orig: Vec<(String, String)>,
    form_box: gtk::Box,
    remove: gtk::Button,
}

/// One rectangle of the placement grid, in whole cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CellRect {
    col: i64,
    row: i64,
    w: i64,
    h: i64,
}

/// Spacing between grid cells in pixels; also the pixel gap a drag delta is
/// divided by when it is rounded to whole cells.
const GRID_GAP: i64 = 8;
/// Pixel height reserved per grid row on the canvas.
const GRID_ROW_UNIT: i32 = 72;
/// Placement priority used when a tile has no `_order` field.
const GRID_DEFAULT_ORDER: i64 = 1000;

/// A tile on the fixed-width placement grid of a `grid-configuration` control.
struct GridTile {
    id: String,
    added: bool,
    order: i64,
    cell: CellRect,
    orig: CellRect,
    enabled: Option<gtk::Switch>,
    orig_enabled: Option<bool>,
    fields: Vec<(Record, Editor)>,
    tile: gtk::ToggleButton,
    label: gtk::Label,
    overlay: gtk::Overlay,
    right_strip: gtk::Box,
    bottom_strip: gtk::Box,
    corner_strip: gtk::Box,
}

/// The per-page shape of a grid control, derived from the record path.
#[derive(Clone, Copy)]
struct GridProfile {
    columns: i64,
    max_rows: i64,
    has_enabled: bool,
    has_footer: bool,
    registry: &'static [&'static str],
}

const CC_GRID_REGISTRY: &[&str] = &[
    "user", "battery", "settings", "lock", "power", "brightnessSlider",
    "volumeSlider", "inputVolumeSlider", "wifi", "bluetooth", "builtin_vpn",
    "diskUsage", "nightMode", "doNotDisturb", "idleInhibitor", "runningApps",
];
const DASH_GRID_REGISTRY: &[&str] = &[
    "clock", "weather", "notifications", "calendar", "media", "sysmon",
    "network", "user",
];
const WEATHER_GRID_REGISTRY: &[&str] = &[
    "conditions", "hourly", "humidity", "wind", "daily", "precipitation", "uv",
];

impl GridProfile {
    fn for_path(path: &str) -> Option<Self> {
        if path.contains("controlCenterWidgets") {
            Some(Self {
                columns: 8,
                max_rows: 64,
                has_enabled: true,
                has_footer: true,
                registry: CC_GRID_REGISTRY,
            })
        } else if path.contains("dashCards") {
            Some(Self {
                columns: 6,
                max_rows: 64,
                has_enabled: false,
                has_footer: false,
                registry: DASH_GRID_REGISTRY,
            })
        } else if path.contains("weather.widgets") {
            Some(Self {
                columns: 4,
                max_rows: 64,
                has_enabled: false,
                has_footer: false,
                registry: WEATHER_GRID_REGISTRY,
            })
        } else {
            None
        }
    }

    fn generic() -> Self {
        Self {
            columns: 6,
            max_rows: 64,
            has_enabled: false,
            has_footer: false,
            registry: &[],
        }
    }
}

/// One tile fed to `repack`: its desired cell (None = first fit) and size.
struct TileInput {
    cell: Option<CellRect>,
    w: i64,
    h: i64,
}

/// Shared state of one grid control. The editor, the canvas, the details pane
/// and every gesture closure hold the same `Rc`, so tile indices are always
/// resolved against the live list rather than captured at build time.
pub(crate) struct GridLayoutState {
    profile: GridProfile,
    tiles: Rc<RefCell<Vec<GridTile>>>,
    orig_ids: Vec<String>,
    selected: Rc<RefCell<Option<usize>>>,
    extras: Vec<(Record, Editor)>,
    on_change: ChangeHook,
    canvas: gtk::Grid,
    details: gtk::Box,
    adder_slot: gtk::Box,
    footer_flow: Option<gtk::FlowBox>,
    size_caption: gtk::Label,
    advanced: bool,
    drag_index: Rc<RefCell<Option<usize>>>,
    size_index: Rc<RefCell<Option<usize>>>,
    pinned: Rc<RefCell<Vec<CellRect>>>,
    steps: Rc<Cell<(f64, f64)>>,
    cancelled: Rc<Cell<bool>>,
    programmatic: Rc<Cell<bool>>,
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
        entry: Option<gtk::Entry>,
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
        rows: Rc<RefCell<Vec<MenuRow>>>,
        on_change: ChangeHook,
    },
    Plugins {
        list: crate::plugin_list::PluginList,
    },
    Bar {
        root: gtk::Box,
        left: WidgetTileList,
        right: WidgetTileList,
        center: WidgetTileList,
        left_present: bool,
        right_present: bool,
        center_present: bool,
        extras: Vec<(Record, Editor)>,
        on_change: ChangeHook,
    },
    Dock {
        root: gtk::Box,
        widgets: WidgetTileList,
        widgets_present: bool,
        extras: Vec<(Record, Editor)>,
        on_change: ChangeHook,
    },
    Grid {
        root: gtk::Box,
        state: Rc<GridLayoutState>,
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
            Editor::Plugins { list, .. } => list.root(),
            Editor::Bar { root, .. } => root.clone().upcast(),
            Editor::Dock { root, .. } => root.clone().upcast(),
            Editor::Grid { root, .. } => root.clone().upcast(),
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
            Editor::Menu { rows, .. } => menu_rows_nix(&rows.borrow()),
            Editor::Plugins { list, .. } => list.to_nix(),
            Editor::Bar {
                left,
                right,
                center,
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
            Editor::Grid { state, .. } => grid_nix(state),
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
                    && row
                        .fields
                        .borrow()
                        .iter()
                        .all(|f| f.editor.unchanged(&f.orig_value))
            }),
            Editor::Monitors { rows, .. } => rows.borrow().iter().all(monitor_row_unchanged),
            Editor::Menu { rows, .. } => menu_rows_unchanged(rows),
            Editor::Plugins { list, .. } => list.unchanged(),
            Editor::Bar {
                left,
                right,
                center,
                extras,
                ..
            } => {
                tiles_unchanged(left)
                    && tiles_unchanged(right)
                    && tiles_unchanged(center)
                    && extras
                        .iter()
                        .all(|(record, editor)| editor.unchanged(&record.value))
            }
            Editor::Dock {
                widgets, extras, ..
            } => {
                tiles_unchanged(widgets)
                    && extras
                        .iter()
                        .all(|(record, editor)| editor.unchanged(&record.value))
            }
            Editor::Grid { state, .. } => {
                let tiles = state.tiles.borrow();
                tiles.len() == state.orig_ids.len()
                    && tiles.iter().zip(state.orig_ids.iter()).all(|(tile, id)| {
                        tile.id == *id
                            && !tile.added
                            && tile.cell == tile.orig
                            && tile
                                .enabled
                                .as_ref()
                                .map(|switch| switch.is_active())
                                == tile.orig_enabled
                            && tile
                                .fields
                                .iter()
                                .all(|(record, editor)| editor.unchanged(&record.value))
                    })
                    && state
                        .extras
                        .iter()
                        .all(|(record, editor)| editor.unchanged(&record.value))
            }
            _ => self.to_nix().trim() == original.trim(),
        }
    }

    /// The selected index of a dropdown editor as a number, 0 for other kinds.
    pub fn current_choice(&self) -> f64 {
        match self {
            Editor::Dropdown { combo, .. } => combo.selected() as f64,
            _ => 0.0,
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
            Editor::Theme { list, entry, .. } => {
                if let Some(entry) = entry {
                    let update = update.clone();
                    entry.connect_changed(move |_| update());
                }
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
                    connect_menu_row(row, update);
                }
            }
            Editor::Plugins { list, .. } => list.connect_changed(update),
            Editor::Bar {
                extras,
                on_change,
                ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for (_, editor) in extras {
                    editor.connect_changed(update);
                }
            }
            Editor::Dock {
                extras,
                on_change,
                ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for (_, editor) in extras {
                    editor.connect_changed(update);
                }
            }
            Editor::Grid { state, .. } => {
                *state.on_change.borrow_mut() = Some(update.clone());
                for (_, editor) in &state.extras {
                    editor.connect_changed(update);
                }
                let tiles = state.tiles.borrow();
                for tile in tiles.iter() {
                    for (_, editor) in &tile.fields {
                        editor.connect_changed(update);
                    }
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
        WidgetKind::GridConfiguration => build_grid_configuration(record, ctx),
        WidgetKind::TableAttrset => build_table(record, ctx, false),
        WidgetKind::TableList => build_table(record, ctx, true),
        WidgetKind::Theme => build_theme(value_text),
        WidgetKind::Keybinds => build_keybinds(record),
        WidgetKind::Monitors => build_monitors(record),
        WidgetKind::Menu => build_menu(record, ctx),
        WidgetKind::Plugins => crate::plugin_list::build(record, ctx),
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

pub(crate) fn build_switch(value_text: &str) -> gtk::Switch {
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

const MONITOR_TRANSFORMS: &[(&str, &str)] = &[
    ("0", "No transform"),
    ("1", "90° counter-clockwise"),
    ("2", "180° counter-clockwise"),
    ("3", "270° counter-clockwise"),
    ("4", "180° vertical flip"),
    ("5", "Flip + 90° counter-clockwise"),
    ("6", "Flip + 180° counter-clockwise"),
    ("7", "Flip + 270° counter-clockwise"),
];

fn build_transform(value_text: &str) -> Editor {
    let labels: Vec<&str> = MONITOR_TRANSFORMS.iter().map(|(_, label)| *label).collect();
    let combo = gtk::DropDown::from_strings(&labels);
    let current = value_text.trim().trim_matches('"');
    let selected = MONITOR_TRANSFORMS
        .iter()
        .position(|(value, _)| *value == current)
        .unwrap_or(0);
    combo.set_selected(selected as u32);
    combo.set_halign(gtk::Align::Start);
    Editor::Dropdown {
        combo,
        choices: MONITOR_TRANSFORMS.iter().map(|(value, _)| (*value).to_string()).collect(),
    }
}

pub(crate) fn build_spin(spec: &OptionSpec, value_text: &str) -> gtk::SpinButton {
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

pub(crate) fn build_entry(value_text: &str, spec: &OptionSpec, monospace: bool) -> gtk::Entry {
    let unquoted = entry_text(value_text);
    let entry = gtk::Entry::new();
    entry.set_text(&unquoted);
    entry.set_hexpand(true);
    if let Some(width) = spec.width_chars {
        entry.set_width_chars(width as i32);
        entry.set_hexpand(false);
        entry.set_halign(gtk::Align::Start);
    }
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

fn flow_box(max_columns: u32) -> gtk::FlowBox {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_column_spacing(12);
    flow.set_row_spacing(6);
    flow.set_max_children_per_line(max_columns.max(1));
    flow.set_valign(gtk::Align::Start);
    flow
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
    let flow = flow_box(record.spec.max_columns.unwrap_or(4));

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
        if let Some(desc) = child.spec.description.as_deref() {
            heading.set_tooltip_text(Some(desc));
        }
        cell.append(&heading);
        if let Some(desc) = child.spec.description.as_deref() {
            let caption = gtk::Label::builder()
                .xalign(0.0)
                .wrap(true)
                .label(desc)
                .css_classes(["dim-label", "caption"])
                .build();
            cell.append(&caption);
        }
        cell.append(&editor.root());
        flow.append(&cell);
        children.push((child, editor));
    }
    root.append(&flow);
    Editor::HorizontalSubmodule { root, children }
}

fn table_row_identity_locked(list: bool, key_editable: bool, has_label: bool) -> bool {
    !list && !key_editable && has_label
}

fn table_first_cell_static(
    list: bool, is_first: bool, first_value_editable: bool,
) -> bool {
    list && is_first && !first_value_editable
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

    let scan_rows = scan::scan_table(&record.value, list);
    let first_row = scan_rows.first();
    let type_hints = column_type_hints(record.spec.column_types.as_deref(), &columns, first_row);
    let widens: Vec<bool> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| column_widens(first_row, column, type_hints[index]))
        .collect();

    // One grid for the whole table: each column sizes to the widest cell it
    // holds, so the header and every row share the same x positions.
    let grid = (!key_editable).then(|| {
        let grid = gtk::Grid::new();
        grid.set_column_spacing(6);
        grid.set_row_spacing(6);
        grid.set_hexpand(true);
        grid
    });
    // The attrset key occupies column 0 in `table-attrset`; `table-list` rows
    // start with their first cell instead.
    let base = if list { 0 } else { 1 };
    if let Some(grid) = &grid {
        for (index, column) in columns.iter().enumerate() {
            if list && !first_value_editable && index == 0 {
                continue;
            }
            let caption = gtk::Label::builder()
                .label(column)
                .xalign(0.0)
                .hexpand(widens[index])
                .css_classes(["dim-label", "caption"])
                .build();
            grid.attach(&caption, (base + index) as i32, 0, 1, 1);
        }
        root.append(grid);
    }

    let rows: Rc<RefCell<Vec<TableRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    for scan_row in &scan_rows {
        if scan_row.spec.hidden && !ctx.advanced {
            continue;
        }
        let parent = row_parent(&root, &grid, &rows);
        let row = make_table_row(
            &parent,
            &rows,
            &columns,
            list,
            key_editable,
            first_value_editable,
            &type_hints,
            &widens,
            scan_row,
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
        let grid = grid.clone();
        let type_hints = type_hints.clone();
        let widens = widens.clone();
        add.connect_clicked(move |_| {
            let scan_row = scan::TableRowScan {
                key: String::new(),
                fields: columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        let hint = type_hints.get(index).copied().flatten();
                        (column.clone(), empty_cell_literal(hint))
                    })
                    .collect(),
                spec: OptionSpec::default(),
            };
            let update = on_change.borrow().clone();
            let parent = row_parent(&root, &grid, &rows);
            let row = make_table_row(
                &parent,
                &rows,
                &columns,
                list,
                key_editable,
                first_value_editable,
                &type_hints,
                &widens,
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

/// Where the next row lands: a shared grid column slot when the table has one,
/// else its own horizontal box.
enum TableParent<'a> {
    Rows(&'a gtk::Box),
    Grid { grid: &'a gtk::Grid, row: i32 },
}

fn row_parent<'a>(
    root: &'a gtk::Box,
    grid: &'a Option<gtk::Grid>,
    rows: &Rc<RefCell<Vec<TableRow>>>,
) -> TableParent<'a> {
    match grid {
        Some(grid) => TableParent::Grid {
            grid,
            row: rows.borrow().len() as i32 + 1,
        },
        None => TableParent::Rows(root),
    }
}

/// How a row takes itself out of the table when its remove button is hit.
enum RowDetach {
    Boxes { parent: gtk::Box, row_box: gtk::Box },
    Grid { grid: gtk::Grid, row: i32 },
}

impl RowDetach {
    fn detach(&self) {
        match self {
            RowDetach::Boxes { parent, row_box } => parent.remove(row_box),
            RowDetach::Grid { grid, row } => grid_remove_row(grid, *row),
        }
    }
}

/// Drop every widget of `row` from a grid and shift the rows below it up, so
/// row indices stay dense after a removal.
fn grid_remove_row(grid: &gtk::Grid, row: i32) {
    let Some(manager) = grid.layout_manager() else {
        return;
    };
    let children = grid.observe_children();
    let mut doomed = Vec::new();
    let mut shifts = Vec::new();
    for index in 0..children.n_items() {
        let Some(object) = children.item(index) else {
            continue;
        };
        let Ok(child) = object.downcast::<gtk::Widget>() else {
            continue;
        };
        let Ok(cell) = manager.layout_child(&child).downcast::<gtk::GridLayoutChild>() else {
            continue;
        };
        let child_row = cell.row();
        if child_row == row {
            doomed.push(child);
        } else if child_row > row {
            shifts.push((child, cell.column(), child_row));
        }
    }
    for child in doomed {
        grid.remove(&child);
    }
    shifts.sort_by_key(|(_, _, child_row)| *child_row);
    for (child, column, child_row) in shifts {
        grid.remove(&child);
        grid.attach(&child, column, child_row - 1, 1, 1);
    }
}

#[allow(clippy::too_many_arguments)]
fn make_table_row(
    parent: &TableParent<'_>,
    rows: &Rc<RefCell<Vec<TableRow>>>,
    columns: &[String],
    list: bool,
    key_editable: bool,
    first_value_editable: bool,
    type_hints: &[Option<value::ValueKind>],
    widens: &[bool],
    scan_row: &scan::TableRowScan,
    ctx: &BuildContext,
    update: Option<&Rc<dyn Fn()>>,
) -> TableRow {
    let key_entry = gtk::Entry::new();
    key_entry.set_width_chars(12);
    if !list {
        key_entry.set_text(&scan_row.key);
    }

    // The row's widgets in column order; grid tables attach one per slot,
    // `keyEditable` tables collect them into their own horizontal box.
    let mut slots: Vec<gtk::Widget> = Vec::new();
    let mut slot_widens: Vec<bool> = Vec::new();

    if !list {
        if table_row_identity_locked(list, key_editable, scan_row.spec.label.is_some()) {
            let heading = gtk::Label::builder()
                .label(scan_row.spec.label.clone().unwrap_or_default())
                .xalign(0.0)
                .tooltip_text("Row key")
                .build();
            slots.push(heading.upcast::<gtk::Widget>());
        } else {
            key_entry.set_tooltip_text(Some("Row key"));
            slots.push(key_entry.clone().upcast::<gtk::Widget>());
        }
        slot_widens.push(false);
        if let Some(update) = update {
            let update = update.clone();
            key_entry.connect_changed(move |_| update());
        }
    }

    let mut cells = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let hint = type_hints.get(index).copied().flatten();
        let widen = widens.get(index).copied().unwrap_or(false);
        let (field_key, field_value) = scan_row
            .fields
            .iter()
            .find(|(key, _)| key == column)
            .cloned()
            .unwrap_or_else(|| (column.clone(), empty_cell_literal(hint)));
        let is_first = index == 0;

        let cell_key = gtk::Entry::new();
        cell_key.set_text(&field_key);
        cell_key.set_width_chars(10);

        let editor = build_cell(&field_value, hint, ctx);
        if key_editable {
            slots.push(cell_key.clone().upcast::<gtk::Widget>());
            slots.push(gtk::Label::new(Some("=")).upcast::<gtk::Widget>());
            slots.push(editor.root());
            slot_widens.extend([false, false, widen]);
        } else if table_first_cell_static(list, is_first, first_value_editable) {
            let label = gtk::Label::builder()
                .label(entry_text(&field_value))
                .xalign(0.0)
                .build();
            let caption = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            caption.append(&label);
            caption.append(&gtk::Label::new(Some(":")));
            slots.push(caption.upcast::<gtk::Widget>());
            slot_widens.push(widen);
        } else {
            slots.push(editor.root());
            slot_widens.push(widen);
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
    slots.push(remove.clone().upcast::<gtk::Widget>());
    slot_widens.push(false);

    let detach = match parent {
        TableParent::Rows(parent_root) => {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row_box.set_hexpand(true);
            for slot in &slots {
                row_box.append(slot);
            }
            parent_root.append(&row_box);
            RowDetach::Boxes {
                parent: (**parent_root).clone(),
                row_box,
            }
        }
        TableParent::Grid { grid, row } => {
            for (index, slot) in slots.iter().enumerate() {
                slot.set_hexpand(slot_widens.get(index).copied().unwrap_or(false));
                grid.attach(slot, index as i32, *row, 1, 1);
            }
            RowDetach::Grid {
                grid: (**grid).clone(),
                row: *row,
            }
        }
    };
    {
        let rows = rows.clone();
        let key_entry = key_entry.clone();
        let update = update.cloned();
        remove.connect_clicked(move |_| {
            detach.detach();
            rows.borrow_mut().retain(|row| row.key_entry != key_entry);
            if let Some(cb) = update.as_ref() {
                cb();
            }
        });
    }

    TableRow {
        key_entry,
        orig_key: scan_row.key.clone(),
        cells,
    }
}

/// The scanned source text of one column in the first row, `null` when the
/// column has no sample.
fn column_sample<'a>(first_row: Option<&'a scan::TableRowScan>, column: &str) -> &'a str {
    first_row
        .and_then(|row| row.fields.iter().find(|(key, _)| key == column))
        .map(|(_, value)| value.as_str())
        .unwrap_or("null")
}

fn column_expands(first_row: Option<&scan::TableRowScan>, column: &str) -> bool {
    let sample = column_sample(first_row, column);
    !matches!(
        value::classify(sample),
        value::ValueKind::Bool | value::ValueKind::Int | value::ValueKind::Float
    )
}

/// Whether a column gives its extra width back to the window: text cells take
/// slack, switches and spins stay compact. The declared column type decides
/// when the sample is still `null`.
fn column_widens(
    first_row: Option<&scan::TableRowScan>,
    column: &str,
    hint: Option<value::ValueKind>,
) -> bool {
    match hint {
        None => column_expands(first_row, column),
        Some(hint) => matches!(
            cell_widget_kind(column_sample(first_row, column), Some(hint)),
            WidgetKind::Text
        ),
    }
}

/// The editor for one table cell. A real value always wins; a `null` or
/// bare-expression cell falls back to the column's declared type.
fn cell_widget_kind(value: &str, hint: Option<value::ValueKind>) -> WidgetKind {
    match value::classify(value) {
        value::ValueKind::Bool => WidgetKind::Switch,
        value::ValueKind::Int | value::ValueKind::Float => WidgetKind::Spin,
        value::ValueKind::Null | value::ValueKind::Other => match hint {
            Some(value::ValueKind::Bool) => WidgetKind::Switch,
            Some(value::ValueKind::Int | value::ValueKind::Float) => WidgetKind::Spin,
            _ => WidgetKind::Text,
        },
        _ => WidgetKind::Text,
    }
}

/// The scaffold value written for a freshly added cell, so the row opens with
/// the right editor and serialises a sane default when saved untouched.
fn empty_cell_literal(hint: Option<value::ValueKind>) -> String {
    match hint {
        Some(value::ValueKind::Bool) => "false",
        Some(value::ValueKind::Int | value::ValueKind::Float) => "0",
        Some(value::ValueKind::String) => "\"\"",
        _ => "null",
    }
    .to_string()
}

/// One `columnTypes` token, matched case-insensitively. Anything unrecognised
/// yields no hint.
fn column_type_hint(token: &str) -> Option<value::ValueKind> {
    match token.trim().to_ascii_lowercase().as_str() {
        "bool" | "boolean" => Some(value::ValueKind::Bool),
        "int" | "integer" => Some(value::ValueKind::Int),
        "float" => Some(value::ValueKind::Float),
        "str" | "string" | "raw" => Some(value::ValueKind::String),
        _ => None,
    }
}

/// Per-column type hints, taken from `columnTypes` when declared and
/// otherwise sampled from the first scanned row. Extra types are ignored and
/// missing ones leave the column unhinted.
fn column_type_hints(
    column_types: Option<&[String]>,
    columns: &[String],
    first_row: Option<&scan::TableRowScan>,
) -> Vec<Option<value::ValueKind>> {
    match column_types {
        Some(types) => (0..columns.len())
            .map(|index| types.get(index).and_then(|token| column_type_hint(token)))
            .collect(),
        None => columns
            .iter()
            .map(|column| match value::classify(column_sample(first_row, column)) {
                kind @ (value::ValueKind::Bool
                | value::ValueKind::Int
                | value::ValueKind::Float
                | value::ValueKind::String) => Some(kind),
                _ => None,
            })
            .collect(),
    }
}

/// A cell editor inferred from the cell's value: a switch for a boolean, a spin
/// for a number, a text field otherwise.
fn build_cell(value: &str, hint: Option<value::ValueKind>, ctx: &BuildContext) -> Editor {
    let kind = cell_widget_kind(value, hint);
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

/// A Nix attribute name, kept bare when it is a plain identifier. The value is
/// NOT trimmed: a key such as `"daily "` keeps its trailing space when quoted,
/// so a written binding still matches the key that was read.
pub(crate) fn attr_key(raw: &str) -> String {
    let valid = !raw.is_empty()
        && raw
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        raw.to_string()
    } else {
        quote(raw)
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
/// short scheme name. With no palettes bundled (plain `cargo test`, or a build
/// without TINTED_SCHEMES_DIR) it degrades to a labeled text entry.
fn build_theme(value_text: &str) -> Editor {
    if schemes::SCHEMES.is_empty() {
        return theme_text_fallback(value_text);
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
        entry: None,
    }
}

fn theme_text_fallback(value_text: &str) -> Editor {
    let entry = build_entry(value_text, &OptionSpec::default(), false);
    let current = Rc::new(RefCell::new(entry.text().to_string()));
    let (entry_watch, current_watch) = (entry.clone(), current.clone());
    entry.connect_changed(move |_| {
        *current_watch.borrow_mut() = entry_watch.text().to_string();
    });
    let warning = gtk::Label::new(Some("No schemes bundled: set TINTED_SCHEMES_DIR and rebuild"));
    warning.set_xalign(0.0);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    root.append(&warning);
    root.append(&entry);
    Editor::Theme {
        root,
        current,
        list: gtk::ListBox::new(),
        entry: Some(entry),
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

/// Install the one-time stylesheet that paints the keybind preview pill.
fn install_keybind_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".keybind-preview {
            background-color: alpha(@window_fg_color, 0.08);
            border-radius: 8px;
            padding: 2px 8px;
        }";
        let provider = gtk::CssProvider::new();
        provider.load_from_data(css);
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

fn field_nix(
    name: &str,
    editor: &Editor,
    _orig_value: &str,
    orig_present: bool,
) -> Option<String> {
    if !orig_present && !field_has_content(editor) {
        return None;
    }
    Some(format!("{} = {};", attr_key(name), editor.to_nix()))
}

fn attrset_field_nix(field: &AttrsetField) -> Option<String> {
    field_nix(
        &field.name,
        &field.editor,
        &field.orig_value,
        field.orig_present,
    )
}

fn menu_aliases_text(raw: &str) -> String {
    raw.split('"').skip(1).step_by(2).collect::<Vec<_>>().join(", ")
}

fn menu_aliases_nix(text: &str) -> Option<String> {
    let items: Vec<String> = text
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| expr_literal(&s))
        .collect();
    (!items.is_empty())
        .then(|| format!("aliases = {};", value::serialize_list(&items)))
}

const WIDGET_NAMES: &[(&str, &str)] = &[
    ("layout", "Layout"),
    ("launcherButton", "App launcher"),
    ("workspaceSwitcher", "Workspace switcher"),
    ("focusedWindow", "Focused window"),
    ("runningApps", "Running apps"),
    ("appsDock", "Apps dock"),
    ("island", "Island"),
    ("clock", "Clock"),
    ("weather", "Weather"),
    ("music", "Media"),
    ("clipboard", "Clipboard manager"),
    ("cpuUsage", "CPU usage"),
    ("memUsage", "Memory usage"),
    ("diskUsage", "Disk usage"),
    ("cpuTemp", "CPU temperature"),
    ("gpuTemp", "GPU temperature"),
    ("systemTray", "System tray"),
    ("privacyIndicator", "Privacy indicator"),
    ("controlCenterButton", "Control Center"),
    ("notificationButton", "Notification Center"),
    ("battery", "Battery"),
    ("vpn", "VPN"),
    ("idleInhibitor", "Idle inhibitor"),
    ("capsLockIndicator", "Caps Lock indicator"),
    ("spacer", "Spacer"),
    ("separator", "Separator"),
    ("network_speed_monitor", "Network speed monitor"),
    ("keyboard_layout_name", "Keyboard layout"),
    ("notepadButton", "Notepad"),
    ("colorPicker", "Color Picker"),
    ("systemUpdate", "System update"),
    ("powerMenuButton", "Power"),
    ("dockLauncher", "Launcher"),
    ("dockTrash", "Trash"),
    ("left", "Left"),
    ("center", "Center"),
    ("right", "Right"),
    ("wifi", "Wi-Fi"),
    ("builtin_vpn", "Built-in VPN"),
    ("builtinVpn", "Built-in VPN"),
    ("uv", "UV"),
];

fn known_widget_name(id: &str) -> Option<&'static str> {
    WIDGET_NAMES
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, name)| *name)
}

fn title_case_id(id: &str) -> String {
    let expanded = id.replace('_', " ");
    let mut out = String::new();
    for (index, ch) in expanded.chars().enumerate() {
        if ch.is_uppercase() && index > 0 && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push(ch);
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => out,
    }
}

fn widget_display_name(id: &str) -> String {
    known_widget_name(id)
        .map(str::to_string)
        .unwrap_or_else(|| title_case_id(id))
}

/// The label a keybind row shows: the template's `@table-item` override when
/// one is set, otherwise the derived title-case name, or "New bind" while the
/// name is still empty. A trailing colon is added only when missing.
fn bind_display_label(key: &str, label_override: Option<&str>) -> String {
    let base = match label_override.map(str::trim).filter(|text| !text.is_empty()) {
        Some(label) => label.to_string(),
        None if key.is_empty() => "New bind".to_string(),
        None => title_case_id(key),
    };
    if base.ends_with(':') || base.ends_with(';') {
        base
    } else {
        format!("{base}:")
    }
}

const WIDGET_FIELD_HELP: &[(&str, &str)] = &[
    ("enabled", "Whether this widget is shown."),
    ("_order", "Sort position among sibling widgets; lower comes first."),
    ("showOccupiedWorkspacesOnly", "Only list workspaces that have open windows."),
    ("showWorkspaceIndex", "Show the workspace number."),
    ("showWorkspacePadding", "Pad single-digit workspace numbers."),
    ("workspacePaddingCount", "Pad workspace numbers up to this width."),
    ("dwlShowAllTags", "Show every dwl tag, not only used ones."),
    ("showAudioPercent", "Show the audio level as a percentage."),
    ("showBatteryIcon", "Show the battery icon."),
    ("showBrightnessIcon", "Show the brightness icon."),
    ("showBrightnessPercent", "Show brightness as a percentage."),
    ("showDoNotDisturbIcon", "Show the do not disturb icon."),
    ("showIdleInhibitorIcon", "Show the idle inhibitor icon."),
    ("showMicIcon", "Show the microphone icon."),
    ("showMicPercent", "Show the mic level as a percentage."),
    ("autoHide", "Hide the bar or dock when a window would overlap it."),
    ("autoHideDelay", "Milliseconds before auto hide engages."),
    ("position", "Screen edge: top or bottom."),
];

fn field_caption(name: &str) -> String {
    if name == "_order" {
        return "Position".to_string();
    }
    title_case_id(name)
}

/// The one-line captions under the five fields of a menu entry pop-out. These
/// fields are GUI-bespoke, so the help text lives here, not in the template.
const MENU_FIELD_HELP: &[(&str, &str)] = &[
    ("provider", "Where the entry gets its items, e.g. the apps menu."),
    ("target", "URL or path opened when the entry is activated."),
    ("action", "Shell command run when the entry is activated."),
    ("checked", "Command whose exit status decides the toggle state."),
    ("when", "Command; the entry is shown only while it succeeds."),
];

/// The one-line captions under the field editors of a keybind pop-out.
const KEYBIND_FIELD_HELP: &[(&str, &str)] = &[
    ("dispatcher", "mango action the key triggers, e.g. focusdir, spawn."),
    ("args", "Argument passed to the dispatcher."),
    ("group", "Coarse category used to sort the main list."),
    ("subcat", "Finer category used within the group."),
];

fn help_text(table: &'static [(&'static str, &'static str)], name: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, help)| *help)
}

fn field_help(name: &str) -> Option<&'static str> {
    help_text(WIDGET_FIELD_HELP, name)
}

fn menu_field_help(name: &str) -> Option<&'static str> {
    help_text(MENU_FIELD_HELP, name)
}

fn keybind_field_help(name: &str) -> Option<&'static str> {
    help_text(KEYBIND_FIELD_HELP, name)
}

fn menu_popup_fields(fields: &[AttrsetField], on_resize: Rc<dyn Fn()>) -> gtk::Box {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for field in fields.iter().take(3) {
        vbox.append(&dim_line_with_help(
            &field.name,
            menu_field_help(&field.name),
            &field.editor.root(),
        ));
    }
    let advanced = gtk::Expander::new(Some("Advanced conditions"));
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for field in fields.iter().skip(3) {
        inner.append(&dim_line_with_help(
            &field.name,
            menu_field_help(&field.name),
            &field.editor.root(),
        ));
    }
    advanced.set_child(Some(&inner));
    {
        let on_resize = on_resize.clone();
        advanced.connect_expanded_notify(move |_| on_resize());
    }
    vbox.append(&advanced);
    vbox
}

fn connect_menu_edit(
    edit: &gtk::Button,
    row_box: &gtk::Box,
    popup: &gtk::Window,
    popup_content: &gtk::Box,
    popout: &MenuPopout,
    active: &Rc<RefCell<Option<MenuPopout>>>,
) {
    let row_box = row_box.clone();
    let popup = popup.clone();
    let popup_content = popup_content.clone();
    let popout = popout.clone();
    let active = active.clone();
    edit.connect_clicked(move |_| {
        let already_open = active
            .borrow()
            .as_ref()
            .is_some_and(|open| open.popup_box == popout.popup_box);
        if !already_open {
            if let Some(previous) = active.replace(Some(popout.clone())) {
                previous.cancel();
            }
            popout.capture();
        }
        while let Some(child) = popup_content.first_child() {
            popup_content.remove(&child);
        }
        if let Some(win) = row_box.root().and_then(|w| w.downcast::<gtk::Window>().ok()) {
            popup.set_transient_for(Some(&win));
        }
        popup_content.append(&popout.popup_box);
        popup.set_default_size(440, -1);
        popup.present();
    });
}

fn make_menu_row(
    section: &str,
    rel_key: &str,
    fields: &[(String, String)],
    ctx: &BuildContext,
    popup: &gtk::Window,
    popup_content: &gtk::Box,
    active: &Rc<RefCell<Option<MenuPopout>>>,
) -> MenuRow {
    let raw_of = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r.trim().to_string())
            .unwrap_or_default()
    };
    let unquote = |raw: &str| raw.trim().trim_matches('"').to_string();

    let key = gtk::Entry::new();
    key.set_width_chars(16);
    key.set_text(rel_key);
    let orig_key = if rel_key.is_empty() {
        section.to_string()
    } else {
        format!("{section}.{rel_key}")
    };

    let icon = unquote(&raw_of("icon"));
    let label = gtk::Entry::new();
    label.set_text(&unquote(&raw_of("label")));
    let orig_label = label.text().to_string();
    let aliases = gtk::Entry::new();
    aliases.set_text(&menu_aliases_text(&raw_of("aliases")));
    let orig_aliases = aliases.text().to_string();

    let popup_fields: Rc<RefCell<Vec<AttrsetField>>> = Rc::new(RefCell::new(
        ["provider", "target", "action", "checked", "when"]
            .iter()
            .map(|name| make_attrset_field(name, &raw_of(name), ctx))
            .collect(),
    ));
    let on_resize: Rc<dyn Fn()> = {
        let popup = popup.clone();
        Rc::new(move || popup.set_default_size(440, -1))
    };
    let popup_box = menu_popup_fields(&popup_fields.borrow(), on_resize);
    let (bar, cancel, apply) = popout_button_bar();
    popup_box.append(&bar);

    let icon_widget: gtk::Widget = if icon.is_empty() {
        gtk::Label::new(Some(&orig_label.chars().next().unwrap_or(' ').to_string()))
            .upcast()
    } else {
        gtk::Image::from_icon_name(&icon).upcast()
    };
    let kind = match (raw_of("target").as_str(), raw_of("action").as_str()) {
        (t, _) if !t.is_empty() && t != "null" => "link",
        (_, a) if !a.is_empty() && a != "null" => "shell",
        _ => "group",
    };
    let caption = gtk::Label::new(Some(kind));
    caption.add_css_class("dim-label");
    caption.add_css_class("caption");

    let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    main.set_hexpand(true);
    main.append(&icon_widget);
    if !rel_key.is_empty() {
        main.append(&key);
    }
    label.set_hexpand(true);
    main.append(&label);
    main.append(&aliases);
    main.append(&caption);

    let edit = gtk::Button::with_label("Edit");
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    main.append(&edit);
    main.append(&remove);

    let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row_box.set_hexpand(true);
    row_box.append(&main);

    let row = MenuRow {
        section: section.to_string(),
        key,
        orig_key,
        icon,
        label,
        orig_label,
        aliases,
        orig_aliases,
        popup_fields,
        popup_box,
        row_box,
        remove,
        snapshot: Rc::new(RefCell::new(None)),
    };

    let popout = MenuPopout {
        popup_box: row.popup_box.clone(),
        fields: row.popup_fields.clone(),
        snapshot: row.snapshot.clone(),
    };
    connect_menu_edit(&edit, &row.row_box, popup, popup_content, &popout, active);
    {
        let popup = popup.clone();
        cancel.connect_clicked(move |_| popup.close());
    }
    {
        let popup = popup.clone();
        let active = active.clone();
        apply.connect_clicked(move |_| {
            popout.discard();
            let mut open = active.borrow_mut();
            if open
                .as_ref()
                .is_some_and(|current| current.popup_box == popout.popup_box)
            {
                *open = None;
            }
            popup.hide();
        });
    }
    row
}

fn menu_rows_nix(rows: &[MenuRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let full = if row.key.text().is_empty() {
                row.section.clone()
            } else {
                format!("{}.{}", row.section, row.key.text())
            };
            let mut fields: Vec<String> = Vec::new();
            if !row.icon.is_empty() {
                fields.push(format!("icon = {};", string_literal(&row.icon)));
            }
            if !row.orig_label.is_empty() || !row.label.text().is_empty() {
                fields.push(format!("label = {};", string_literal(&row.label.text())));
            }
            if let Some(line) = menu_aliases_nix(&row.aliases.text()) {
                fields.push(line);
            }
            for field in row.popup_fields.borrow().iter() {
                if let Some(line) = attrset_field_nix(field) {
                    fields.push(line);
                }
            }
            format!("{} = {{ {} }};", attr_key(&full), fields.join(" "))
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

fn menu_rows_unchanged(rows: &Rc<RefCell<Vec<MenuRow>>>) -> bool {
    rows.borrow().iter().all(|row| {
        let full = if row.key.text().is_empty() {
            row.section.clone()
        } else {
            format!("{}.{}", row.section, row.key.text())
        };
        full == row.orig_key
            && row.label.text() == row.orig_label
            && row.aliases.text() == row.orig_aliases
            && row
                .popup_fields
                .borrow()
                .iter()
                .all(|f| f.editor.unchanged(&f.orig_value))
    })
}

fn connect_menu_row(row: &MenuRow, update: &Rc<dyn Fn()>) {
    let update_key = update.clone();
    row.key.connect_changed(move |_| update_key());
    let update_label = update.clone();
    row.label.connect_changed(move |_| update_label());
    let update_aliases = update.clone();
    row.aliases.connect_changed(move |_| update_aliases());
    for field in row.popup_fields.borrow().iter() {
        field.editor.connect_changed(update);
    }
}

fn attach_menu_row(
    container: &gtk::Box,
    rows: &Rc<RefCell<Vec<MenuRow>>>,
    row: MenuRow,
    on_change: &ChangeHook,
    popup_content: &gtk::Box,
) {
    {
        let container = container.clone();
        let rows = rows.clone();
        let row_box = row.row_box.clone();
        let key = row.key.clone();
        let popup_box = row.popup_box.clone();
        let popup_content = popup_content.clone();
        let snapshot = row.snapshot.clone();
        let on_change = on_change.clone();
        row.remove.connect_clicked(move |_| {
            container.remove(&row_box);
            if popup_box.parent() == Some(popup_content.clone().upcast::<gtk::Widget>()) {
                popup_box.unparent();
            }
            snapshot.borrow_mut().take();
            rows.borrow_mut().retain(|r| r.key != key);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    container.append(&row.row_box);
    rows.borrow_mut().push(row);
}

/// The add bar: a dropdown of registry ids not yet on the canvas, an optional
/// custom id entry in advanced mode, and an "Add widget" button. Rebuilt on
/// every change so a deleted widget reappears in the dropdown.
fn build_grid_adder(
    advanced: bool,
    available: &[String],
    on_add: Rc<dyn Fn(&str)>,
) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let labels: Vec<String> = available.iter().map(|id| widget_display_name(id)).collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let combo = gtk::DropDown::from_strings(&label_refs);
    bar.append(&combo);
    let entry = gtk::Entry::new();
    if advanced {
        entry.set_placeholder_text(Some("Custom id"));
        entry.set_width_chars(14);
        bar.append(&entry);
    }
    let add = gtk::Button::with_label("Add widget");
    bar.append(&add);
    let available = available.to_vec();
    add.connect_clicked(move |_| {
        let custom = entry.text().trim().to_string();
        let selected = available
            .get(combo.selected() as usize)
            .cloned()
            .unwrap_or_default();
        let id = if custom.is_empty() { selected } else { custom };
        if !id.is_empty() {
            on_add(&id);
        }
    });
    bar
}

/// An editor for a bespoke widget's child record. Annotated or bare attribute
/// sets render as raw text so the widget can rewrite its value without
/// dropping nested content.
pub(crate) fn build_bespoke_child(record: &Record, ctx: &BuildContext) -> Editor {
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

fn build_position_dropdown(value_text: &str, tooltip: Option<&str>) -> Editor {
    let labels = ["Top", "Bottom", "Left", "Right"];
    let values = ["0", "1", "2", "3"];
    let selected = value_text.trim().parse::<i64>().unwrap_or(0).clamp(0, 3) as usize;
    let combo = gtk::DropDown::from_strings(&labels);
    combo.set_selected(selected as u32);
    combo.set_halign(gtk::Align::Start);
    if let Some(text) = tooltip {
        combo.set_tooltip_text(Some(text));
    }
    Editor::Dropdown {
        combo,
        choices: values.map(String::from).to_vec(),
    }
}

fn build_extra_editor(record: &Record, ctx: &BuildContext) -> Editor {
    let name = record.segments.last().map(String::as_str).unwrap_or_default();
    if name == "position" {
        build_position_dropdown(&record.value, record.spec.description.as_deref())
    } else {
        build_bespoke_child(record, ctx)
    }
}

fn build_pair_row(
    first: &Record,
    first_editor: &Editor,
    second: &Record,
    second_editor: &Editor,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_start(12);
    for (record, editor) in [(first, first_editor), (second, second_editor)] {
        let name = record.segments.last().cloned().unwrap_or_default();
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let caption = gtk::Label::builder()
            .xalign(0.0)
            .label(record.spec.label.clone().unwrap_or_else(|| field_caption(&name)))
            .css_classes(["dim-label"])
            .build();
        if let Some(help) = record.spec.description.as_deref().or_else(|| field_help(&name)) {
            caption.set_tooltip_text(Some(help));
        }
        cell.append(&caption);
        cell.append(&editor.root());
        row.append(&cell);
    }
    row.upcast()
}

/// A horizontal line of a dim caption label and a child control, used in the
/// keybind edit popup.
fn dim_line(name: &str, child: &impl gtk::prelude::IsA<gtk::Widget>) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::builder()
        .xalign(0.0)
        .width_chars(12)
        .label(name)
        .css_classes(["dim-label", "caption"])
        .build();
    line.append(&label);
    line.append(child);
    line
}

/// A `dim_line` with a wrapped, dim one-liner explaining the field below it.
fn dim_line_with_help(
    name: &str,
    help: Option<&str>,
    child: &impl gtk::prelude::IsA<gtk::Widget>,
) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 2);
    block.append(&dim_line(name, child));
    if let Some(help) = help {
        let caption = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .label(help)
            .css_classes(["dim-label", "caption"])
            .build();
        block.append(&caption);
    }
    block
}

/// The pop-out-editable current value of a field editor, captured so Cancel
/// can write it back.
fn editor_snapshot(editor: &Editor) -> String {
    match editor {
        Editor::Text(w) | Editor::Path(w) => w.text().to_string(),
        Editor::Switch(w) => w.is_active().to_string(),
        Editor::Spin(w) => w.value().to_string(),
        Editor::Dropdown { combo, choices } => choices
            .get(combo.selected() as usize)
            .cloned()
            .unwrap_or_default(),
        other => other.to_nix(),
    }
}

/// Write a captured value back into a field editor. Pop-out fields are only
/// ever text, switch or spin editors, so the remaining variants stay untouched.
fn editor_restore(editor: &Editor, value: &str) {
    match editor {
        Editor::Text(w) | Editor::Path(w) => w.set_text(value),
        Editor::Switch(w) => w.set_active(value == "true"),
        Editor::Spin(w) => {
            if let Ok(number) = value.parse::<f64>() {
                w.set_value(number);
            }
        }
        Editor::Dropdown { combo, choices } => {
            if let Some(index) = choices.iter().position(|choice| choice == value) {
                combo.set_selected(index as u32);
            }
        }
        _ => {}
    }
}

fn capture_field_values(fields: &[AttrsetField]) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|field| (field.name.clone(), editor_snapshot(&field.editor)))
        .collect()
}

fn restore_field_values(fields: &[AttrsetField], values: &[(String, String)]) {
    for (name, value) in values {
        if let Some(field) = fields.iter().find(|field| field.name == *name) {
            editor_restore(&field.editor, value);
        }
    }
}

/// The Cancel/Apply bar both pop-outs place at the bottom of their box.
fn popout_button_bar() -> (gtk::Box, gtk::Button, gtk::Button) {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    bar.append(&spacer);
    bar.append(&cancel);
    bar.append(&apply);
    (bar, cancel, apply)
}

/// Keep a horizontal separator between consecutive keybind rows: drop any
/// existing separators and re-insert them between the current row widgets.
fn sync_keybind_separators(container: &gtk::Box) {
    let mut children: Vec<gtk::Widget> = Vec::new();
    let mut child = container.first_child();
    while let Some(widget) = child {
        children.push(widget.clone());
        child = widget.next_sibling();
    }
    let mut separators: Vec<gtk::Widget> = Vec::new();
    let mut rows: Vec<gtk::Widget> = Vec::new();
    for widget in children {
        if widget.downcast_ref::<gtk::Separator>().is_some() {
            separators.push(widget);
        } else {
            rows.push(widget);
        }
    }
    for separator in separators {
        container.remove(&separator);
    }
    for pair in rows.windows(2) {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(6);
        separator.set_margin_bottom(6);
        container.insert_after(&separator, Some(&pair[0]));
    }
}

/// One bind of the `keybinds` widget: a display label and description on the
/// main line, the preview pill, and an edit popup holding every field.
fn make_keybind_row(scan_row: &scan::TableRowScan, ctx: &BuildContext) -> KeybindRow {
    install_keybind_css();
    let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row_box.set_hexpand(true);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title_stack = gtk::Box::new(gtk::Orientation::Vertical, 2);
    title_stack.set_hexpand(true);

    let label_override = scan_row.spec.label.clone();
    let title = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(bind_display_label(&scan_row.key, label_override.as_deref()))
        .build();
    title_stack.append(&title);

    let desc_caption = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(entry_text(field(&scan_row.fields, "description").unwrap_or("")))
        .css_classes(["dim-label", "caption"])
        .build();
    title_stack.append(&desc_caption);
    main.append(&title_stack);

    let modifier = gtk::Entry::new();
    modifier.set_width_chars(16);
    modifier.set_tooltip_text(Some("Modifiers, comma separated"));
    modifier.set_text(&list_text(field(&scan_row.fields, "modifier").unwrap_or("")));

    let keys = gtk::Entry::new();
    keys.set_width_chars(12);
    keys.set_tooltip_text(Some("Keys, comma separated"));
    keys.set_text(&list_text(field(&scan_row.fields, "keys").unwrap_or("")));

    let preview = gtk::Label::builder()
        .label(bind_preview(&modifier.text(), &keys.text()))
        .css_classes(["keybind-preview", "caption"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .build();
    main.append(&preview);

    let mut field_values = Vec::new();
    for name in ["dispatcher", "args", "description", "group", "subcat"] {
        field_values.push(make_attrset_field(
            name,
            field(&scan_row.fields, name).unwrap_or(""),
            ctx,
        ));
    }
    let fields = Rc::new(RefCell::new(field_values));

    let key = gtk::Entry::new();
    key.set_hexpand(true);
    key.set_text(&scan_row.key);

    let popup = gtk::Window::builder()
        .title(bind_display_label(&scan_row.key, label_override.as_deref()))
        .modal(true)
        .default_width(440)
        .hide_on_close(true)
        .build();
    let popup_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    popup_box.set_margin_top(12);
    popup_box.set_margin_bottom(12);
    popup_box.set_margin_start(12);
    popup_box.set_margin_end(12);

    let popup_desc = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .label(entry_text(field(&scan_row.fields, "description").unwrap_or("")))
        .css_classes(["dim-label", "caption"])
        .build();
    popup_box.append(&popup_desc);
    popup_box.append(&dim_line("name", &key));
    popup_box.append(&dim_line("modifier", &modifier));
    popup_box.append(&dim_line("keys", &keys));
    for f in fields.borrow().iter() {
        popup_box.append(&dim_line_with_help(
            &f.name,
            keybind_field_help(&f.name),
            &f.editor.root(),
        ));
    }
    let (bar, cancel, apply) = popout_button_bar();
    popup_box.append(&bar);
    popup.set_child(Some(&popup_box));

    let edit = gtk::Button::with_label("Edit");
    main.append(&edit);
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

    let row = KeybindRow {
        key,
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
        popup,
        snapshot: Rc::new(RefCell::new(None)),
    };

    {
        let popup = row.popup.clone();
        row.key.connect_changed(move |entry| {
            let label = bind_display_label(&entry.text(), label_override.as_deref());
            title.set_label(&label);
            popup.set_title(Some(&label));
        });
    }
    {
        if let Some(desc) = row.fields.borrow().iter().find(|f| f.name == "description") {
            if let Editor::Text(entry) = &desc.editor {
                let entry_cb = entry.clone();
                entry.connect_changed(move |_| {
                    desc_caption.set_label(&entry_cb.text());
                    popup_desc.set_label(&entry_cb.text());
                });
            }
        }
    }
    {
        let popup = row.popup.clone();
        let snapshot = row.snapshot.clone();
        let fields = row.fields.clone();
        let key = row.key.clone();
        let modifier = row.modifier.clone();
        let keys = row.keys.clone();
        let edit_probe = edit.clone();
        edit.connect_clicked(move |_| {
            if popup.transient_for().is_none() {
                if let Some(parent) = edit_probe.root().and_downcast::<gtk::Window>() {
                    popup.set_transient_for(Some(&parent));
                }
            }
            let values = capture_field_values(&fields.borrow());
            *snapshot.borrow_mut() = Some(PopoutSnapshot {
                name: key.text().to_string(),
                modifier: modifier.text().to_string(),
                keys: keys.text().to_string(),
                fields: values,
            });
            popup.set_default_size(440, -1);
            popup.present();
        });
    }
    {
        let popup = row.popup.clone();
        let snapshot = row.snapshot.clone();
        apply.connect_clicked(move |_| {
            snapshot.borrow_mut().take();
            popup.hide();
        });
    }
    {
        let popup = row.popup.clone();
        cancel.connect_clicked(move |_| popup.close());
    }
    {
        let snapshot = row.snapshot.clone();
        let fields = row.fields.clone();
        let key = row.key.clone();
        let modifier = row.modifier.clone();
        let keys = row.keys.clone();
        row.popup.connect_close_request(move |_| {
            let taken = snapshot.borrow_mut().take();
            if let Some(popout) = taken {
                key.set_text(&popout.name);
                modifier.set_text(&popout.modifier);
                keys.set_text(&popout.keys);
                restore_field_values(&fields.borrow(), &popout.fields);
            }
            glib::Propagation::Proceed
        });
    }
    {
        let popup = row.popup.clone();
        let controller = gtk::EventControllerKey::new();
        controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                popup.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        row.popup.add_controller(controller);
    }

    row
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
            for field in row.fields.borrow().iter() {
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
    for field in row.fields.borrow().iter() {
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
            let popup = row.popup.clone();
            row.remove.connect_clicked(move |_| {
                popup.destroy();
                container.remove(&row_box);
                sync_keybind_separators(&container);
                rows.borrow_mut().retain(|r| r.key != key);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        }
        container.append(&row.row_box);
        sync_keybind_separators(&container);
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
                let popup = row.popup.clone();
                row.remove.connect_clicked(move |_| {
                    popup.destroy();
                    container.remove(&row_box);
                    sync_keybind_separators(&container);
                    rows.borrow_mut().retain(|r| r.key != key);
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                });
            }
            container.append(&row.row_box);
            sync_keybind_separators(&container);
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

/// The footprint a monitor occupies after its `rr` transform is applied: odd
/// transforms (90/270 degrees, flipped or not) swap width and height.
fn effective_size(w: f64, h: f64, rr: f64) -> (f64, f64) {
    match rr as i32 {
        1 | 3 | 5 | 7 => (h, w),
        _ => (w, h),
    }
}

fn monitor_orig<'a>(row: &'a MonitorRow, name: &str) -> &'a str {
    row.orig
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or("")
}

fn snap_axis(value: f64, size: f64, targets: &[f64], tolerance: f64) -> f64 {
    let mut best = value;
    let mut best_distance = tolerance + 1.0;
    for target in targets {
        for candidate in [value, value + size] {
            let distance = (candidate - target).abs();
            if distance <= tolerance && distance < best_distance {
                best_distance = distance;
                best = value + (target - candidate);
            }
        }
    }
    best
}

fn snap_monitor(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    others: &[(f64, f64, f64, f64)],
    tolerance: f64,
) -> (f64, f64) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (ox, oy, ow, oh) in others {
        xs.push(*ox);
        xs.push(ox + ow);
        ys.push(*oy);
        ys.push(oy + oh);
    }
    (snap_axis(x, w, &xs, tolerance), snap_axis(y, h, &ys, tolerance))
}

/// Snap to the nearest edge, then push the rectangle out of any monitor it
/// still overlaps along the axis of smaller penetration, up to eight rounds,
/// and clamp the result to non-negative coordinates.
fn resolve_release(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    others: &[(f64, f64, f64, f64)],
    tolerance: f64,
) -> (f64, f64) {
    let (mut x, mut y) = snap_monitor(x, y, w, h, others, tolerance);
    for _ in 0..8 {
        let mut pushed = false;
        for &(ox, oy, ow, oh) in others {
            let overlap_x = (x + w).min(ox + ow) - x.max(ox);
            let overlap_y = (y + h).min(oy + oh) - y.max(oy);
            if overlap_x <= 0.0 || overlap_y <= 0.0 {
                continue;
            }
            if overlap_x < overlap_y {
                x = if x + w / 2.0 < ox + ow / 2.0 { ox - w } else { ox + ow };
            } else {
                y = if y + h / 2.0 < oy + oh / 2.0 { oy - h } else { oy + oh };
            }
            pushed = true;
        }
        if !pushed {
            break;
        }
    }
    (x.max(0.0), y.max(0.0))
}

fn monitor_row_unchanged(row: &MonitorRow) -> bool {
    let text_same =
        |entry: &gtk::Entry, name: &str| entry.text() == entry_text(monitor_orig(row, name));
    let spin_same = |spin: &gtk::SpinButton, name: &str, default: f64| {
        match monitor_orig(row, name).parse::<f64>() {
            Ok(value) => (spin.value() - value).abs() < 1e-9,
            Err(_) => (spin.value() - default).abs() < 1e-9,
        }
    };
    let rr_same =
        monitor_orig(row, "rr").is_empty() || row.rr.unchanged(monitor_orig(row, "rr"));
    text_same(&row.name, "name")
        && spin_same(&row.width, "width", 0.0)
        && spin_same(&row.height, "height", 0.0)
        && spin_same(&row.refresh, "refresh", 0.0)
        && spin_same(&row.x, "x", 0.0)
        && spin_same(&row.y, "y", 0.0)
        && spin_same(&row.scale, "scale", 1.0)
        && rr_same
        && row.extras.iter().all(|field| field.editor.unchanged(&field.orig_value))
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
    if row.scale.value() != 1.0 || orig_has("scale") {
        parts.push(format!("scale = {};", number_literal(row.scale.value(), 2)));
    }
    let rr = row.rr.to_nix();
    if orig_has("rr") || rr != "0" {
        parts.push(format!("rr = {rr};"));
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
    row.rr.connect_changed(update);
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
    let number_spin = |value: &str, default: f64, min: f64, max: f64, step: f64| {
        let spin = gtk::SpinButton::with_range(min, max, step);
        spin.set_digits(0);
        spin.set_value(value.parse::<f64>().unwrap_or(default));
        spin
    };

    let name = gtk::Entry::new();
    name.set_text(entry_text(get("name")).as_str());
    name.set_tooltip_text(Some("Output name"));

    let width = number_spin(get("width"), 0.0, 0.0, 16384.0, 1.0);
    let height = number_spin(get("height"), 0.0, 0.0, 16384.0, 1.0);
    let refresh = number_spin(get("refresh"), 0.0, 0.0, 1000.0, 1.0);
    let x = number_spin(get("x"), 0.0, 0.0, 16384.0, 1.0);
    let y = number_spin(get("y"), 0.0, 0.0, 16384.0, 1.0);
    let scale = gtk::SpinButton::with_range(0.0, 16.0, 0.05);
    scale.set_digits(2);
    scale.set_value(get("scale").parse::<f64>().unwrap_or(1.0));
    let rr = build_transform(get("rr"));

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
    attach_cell(&grid, 3, 2, "Transform", &rr.root());
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

/// The callback the monitor picker calls with the chosen head and mode, or
/// `None` for the manual scaffold.
type MonitorPick = Rc<dyn Fn(Option<(String, DetectedMode)>)>;

fn open_monitor_picker<F: Fn(Option<(String, DetectedMode)>) + 'static>(
    anchor: &gtk::Widget,
    detected: Vec<DetectedMonitor>,
    on_pick: F,
) {
    if detected.is_empty() {
        on_pick(None);
        return;
    }
    let on_pick: MonitorPick = Rc::new(on_pick);
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeft);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&stack);

    let heads = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    for monitor in detected {
        let mut text = monitor.name.clone();
        if let (Some(make), Some(model)) = (&monitor.make, &monitor.model) {
            if !make.is_empty() && !model.is_empty() {
                text = format!("{}  {} {}", monitor.name, make, model);
            }
        }
        if !monitor.enabled {
            text = format!("{text} (disabled)");
        }
        let label = gtk::Label::builder().label(&text).xalign(0.0).build();
        if !monitor.enabled {
            label.add_css_class("dim-label");
        }
        label.set_margin_top(6);
        label.set_margin_bottom(6);
        label.set_margin_start(6);
        label.set_margin_end(6);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&label));
        list.append(&row);
        let stack = stack.clone();
        let on_pick = on_pick.clone();
        let popover = popover.clone();
        row.connect_activate(move |_| {
            if let Some(existing) = stack.child_by_name("modes") {
                stack.remove(&existing);
            }
            let stage = build_monitor_mode_stage(&monitor, &on_pick, &popover, &stack);
            stack.add_named(&stage, Some("modes"));
            stack.set_visible_child_name("modes");
        });
    }
    heads.append(&list);
    let custom = gtk::Button::with_label("Add monitor manually");
    let on_pick_manual = on_pick.clone();
    let popover_manual = popover.clone();
    custom.connect_clicked(move |_| {
        on_pick_manual(None);
        popover_manual.popdown();
    });
    heads.append(&custom);
    stack.add_named(&heads, Some("heads"));
    stack.set_visible_child_name("heads");

    popover.set_child(Some(&content));
    popover.connect_closed(|popover| popover.unparent());
    popover.popup();
}

/// The stage-two page of the monitor picker: choose one advertised mode for a
/// single head, with Back returning to the head list.
fn build_monitor_mode_stage(
    monitor: &DetectedMonitor,
    on_pick: &MonitorPick,
    popover: &gtk::Popover,
    stack: &gtk::Stack,
) -> gtk::Box {
    let stage = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let caption = gtk::Label::builder()
        .label(&monitor.name)
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    stage.append(&caption);

    let labels: Vec<String> = monitor
        .modes
        .iter()
        .map(|mode| {
            let mut text = format!("{}x{} @{:.0} Hz", mode.width, mode.height, mode.refresh);
            if mode.preferred {
                text.push_str("  (preferred)");
            }
            if mode.current {
                text.push_str("  (current)");
            }
            text
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(|label| label.as_str()).collect();
    let combo = gtk::DropDown::from_strings(&refs);
    let initial = monitor
        .default_mode()
        .and_then(|mode| monitor.modes.iter().position(|m| std::ptr::eq(m, mode)))
        .unwrap_or(0);
    if !labels.is_empty() {
        combo.set_selected(initial as u32);
    }
    stage.append(&combo);

    let preview = gtk::Label::builder().xalign(0.0).build();
    stage.append(&preview);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let back = gtk::Button::with_label("Back");
    let add = gtk::Button::with_label("Add");
    add.add_css_class("suggested-action");
    buttons.append(&back);
    buttons.append(&add);
    stage.append(&buttons);

    {
        let stack = stack.clone();
        back.connect_clicked(move |_| stack.set_visible_child_name("heads"));
    }

    let update_preview = {
        let combo = combo.clone();
        let preview = preview.clone();
        let modes = monitor.modes.clone();
        move || {
            if let Some(mode) = modes.get(combo.selected() as usize) {
                preview.set_label(&format!(
                    "{}x{} @ {:.0} Hz",
                    mode.width, mode.height, mode.refresh
                ));
            }
        }
    };
    update_preview();
    combo.connect_selected_notify({
        let update_preview = update_preview.clone();
        move |_| update_preview()
    });

    {
        let name = monitor.name.clone();
        let modes = monitor.modes.clone();
        let on_pick = on_pick.clone();
        let popover = popover.clone();
        add.connect_clicked(move |_| {
            if let Some(mode) = modes.get(combo.selected() as usize) {
                on_pick(Some((name.clone(), mode.clone())));
            }
            popover.popdown();
        });
    }

    stage
}

fn attach_monitor_row(
    form: &gtk::Box,
    rows: &Rc<RefCell<Vec<MonitorRow>>>,
    on_change: &ChangeHook,
    row: MonitorRow,
) -> usize {
    let row_box = row.form_box.clone();
    {
        let form = form.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        row.remove.connect_clicked(move |_| {
            form.remove(&row_box);
            rows.borrow_mut().retain(|r| r.form_box != row_box);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    form.append(&row.form_box);
    let index = rows.borrow().len();
    rows.borrow_mut().push(row);
    index
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
        .content_height(320)
        .build();
    canvas.set_hexpand(true);
    canvas.set_size_request(-1, 320);
    canvas.set_draw_func({
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        move |_, cr, width, height| {
            cr.set_source_rgb(0.08, 0.08, 0.10);
            cr.rectangle(0.0, 0.0, width as f64, height as f64);
            cr.fill().ok();
            let rows = rows.borrow();
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;
            for row in rows.iter() {
                let (x, y, w, h, _, _) = monitor_geometry(row);
                let (w, h) = effective_size(w, h, row.rr.current_choice());
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
                let (w, h) = effective_size(w, h, row.rr.current_choice());
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
                cr.save().ok();
                let rr = row.rr.current_choice() as i32;
                if rr != 0 {
                    let angle = (rr % 4) as f64 * 90.0_f64.to_radians();
                    cr.translate(px + pw / 2.0, py + ph / 2.0);
                    cr.rotate(-angle);
                    cr.move_to(-pw / 2.0 + 6.0, -ph / 2.0 + 16.0);
                } else {
                    cr.move_to(px + 6.0, py + 16.0);
                }
                let _ = cr.show_text(&label);
                cr.restore().ok();
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
                let (w, h) = effective_size(w, h, row.rr.current_choice());
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
        let on_change = on_change.clone();
        let last = Rc::new(Cell::new((0.0f64, 0.0f64)));
        drag.connect_drag_begin({
            let last = last.clone();
            move |_, _, _| last.set((0.0, 0.0))
        });
        drag.connect_drag_update({
            let rows = rows.clone();
            let selected = selected.clone();
            let view = view.clone();
            let redraw = canvas.clone();
            let on_change = on_change.clone();
            move |gesture, _, _| {
                let (offset_x, offset_y) = gesture.offset().unwrap_or((0.0, 0.0));
                let (prev_x, prev_y) = last.get();
                last.set((offset_x, offset_y));
                let scale = view.get().scale;
                let rows = rows.borrow();
                let Some(row) = rows.get(selected.get()) else {
                    return;
                };
                let (x, y, _, _, _, _) = monitor_geometry(row);
                let candidate_x = x + (offset_x - prev_x) / scale;
                let candidate_y = y + (offset_y - prev_y) / scale;
                row.x.set_value(candidate_x);
                row.y.set_value(candidate_y);
                drop(rows);
                redraw.queue_draw();
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            }
        });
        drag.connect_drag_end({
            let rows = rows.clone();
            let selected = selected.clone();
            let view = view.clone();
            let redraw = canvas.clone();
            let on_change = on_change.clone();
            move |_, _, _| {
                let scale = view.get().scale;
                let rows = rows.borrow();
                let Some(row) = rows.get(selected.get()) else {
                    return;
                };
                let (x, y, w, h, _, _) = monitor_geometry(row);
                let (w, h) = effective_size(w, h, row.rr.current_choice());
                let others: Vec<(f64, f64, f64, f64)> = rows
                    .iter()
                    .filter(|other| !std::ptr::eq(*other, row))
                    .map(|other| {
                        let (ox, oy, ow, oh, _, _) = monitor_geometry(other);
                        let (ow, oh) = effective_size(ow, oh, other.rr.current_choice());
                        (ox, oy, ow, oh)
                    })
                    .collect();
                let (nx, ny) = resolve_release(x, y, w, h, &others, 12.0 / scale);
                row.x.set_value(nx);
                row.y.set_value(ny);
                drop(rows);
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
        add.connect_clicked(move |button| {
            let anchor = button.clone().upcast::<gtk::Widget>();
            let detected = crate::monitor_detect::detect();
            let form = form.clone();
            let rows = rows.clone();
            let on_change = on_change.clone();
            let canvas = canvas.clone();
            let selected = selected.clone();
            let ctx = ctx.clone();
            open_monitor_picker(&anchor, detected, move |picked| {
                let scan_row = monitor_scaffold(&rows.borrow(), picked.as_ref());
                let row = make_monitor_row(&scan_row, &ctx);
                let index = attach_monitor_row(&form, &rows, &on_change, row);
                selected.set(index);
                for (position, item) in rows.borrow().iter().enumerate() {
                    item.form_box.set_visible(position == index);
                }
                canvas.queue_draw();
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        });
    }
    root.append(&add);

    Editor::Monitors {
        root,
        rows,
        on_change,
    }
}

fn monitor_scaffold(
    rows: &[MonitorRow],
    picked: Option<&(String, DetectedMode)>,
) -> scan::TableRowScan {
    let (name, width, height, refresh) = match picked {
        Some((name, mode)) => (name.clone(), mode.width, mode.height, mode.refresh),
        None => (String::new(), 1920, 1080, 60.0),
    };
    let x = rows
        .iter()
        .map(|row| {
            let (rx, _, rw, rh, _, _) = monitor_geometry(row);
            let (rw, _) = effective_size(rw, rh, row.rr.current_choice());
            rx + rw
        })
        .fold(0.0f64, f64::max);
    let mut fields = vec![
        ("width".to_string(), width.to_string()),
        ("height".to_string(), height.to_string()),
        ("refresh".to_string(), format!("{refresh}")),
        ("x".to_string(), (x as i64).to_string()),
        ("y".to_string(), "0".to_string()),
        ("scale".to_string(), "1.0".to_string()),
        ("rr".to_string(), "0".to_string()),
    ];
    if !name.is_empty() {
        fields.insert(0, ("name".to_string(), quote(&name)));
    }
    scan::TableRowScan {
        key: String::new(),
        fields,
        spec: OptionSpec::default(),
    }
}

fn build_menu(record: &Record, ctx: &BuildContext) -> Editor {
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_hexpand(true);
    let rows = Rc::new(RefCell::new(Vec::new()));
    let active: Rc<RefCell<Option<MenuPopout>>> = Rc::new(RefCell::new(None));

    let popup = gtk::Window::builder()
        .hide_on_close(true)
        .default_width(440)
        .title("Entry details")
        .build();
    let popup_content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    popup_content.set_margin_top(12);
    popup_content.set_margin_bottom(12);
    popup_content.set_margin_start(12);
    popup_content.set_margin_end(12);
    popup.set_child(Some(&popup_content));
    {
        let active = active.clone();
        popup.connect_close_request(move |_| {
            let taken = active.borrow_mut().take();
            if let Some(popout) = taken {
                popout.cancel();
            }
            glib::Propagation::Proceed
        });
    }
    {
        let target = popup.clone();
        let controller = gtk::EventControllerKey::new();
        controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                target.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        popup.add_controller(controller);
    }

    let mut sections: Vec<(String, gtk::Box)> = Vec::new();
    for scan_row in scan::scan_table(&record.value, false) {
        let (section, rel) = match scan_row.key.split_once('.') {
            Some((p, c)) => (p.to_string(), c.to_string()),
            None => (scan_row.key.clone(), String::new()),
        };
        let container = match sections.iter().find(|(name, _)| *name == section) {
            Some((_, c)) => c.clone(),
            None => {
                let header = gtk::Label::builder()
                    .xalign(0.0)
                    .label(widget_display_name(&section))
                    .css_classes(["heading"])
                    .build();
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
                vbox.set_hexpand(true);
                vbox.set_margin_start(12);
                root.append(&header);
                root.append(&vbox);
                sections.push((section.clone(), vbox.clone()));
                vbox
            }
        };
        let row = make_menu_row(
            &section,
            &rel,
            &scan_row.fields,
            ctx,
            &popup,
            &popup_content,
            &active,
        );
        attach_menu_row(&container, &rows, row, &on_change, &popup_content);
    }

    let add_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let names: Vec<String> = sections
        .iter()
        .map(|(name, _)| widget_display_name(name))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let parent_menu = gtk::DropDown::from_strings(&refs);
    let child = gtk::Entry::new();
    child.set_placeholder_text(Some("child-id"));
    child.set_width_chars(14);
    let new_label = gtk::Entry::new();
    new_label.set_placeholder_text(Some("Label"));
    new_label.set_hexpand(true);
    let add = gtk::Button::with_label("Add");
    add_box.append(&parent_menu);
    add_box.append(&child);
    add_box.append(&new_label);
    add_box.append(&add);
    root.append(&add_box);
    {
        let child = child.clone();
        let new_label = new_label.clone();
        let parent_menu = parent_menu.clone();
        let sections = sections.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let popup = popup.clone();
        let popup_content = popup_content.clone();
        let ctx = ctx.clone();
        let active = active.clone();
        add.connect_clicked(move |_| {
            let idx = parent_menu.selected() as usize;
            let Some((section, container)) = sections.get(idx) else { return };
            let fields = vec![
                ("label".to_string(), string_literal(&new_label.text())),
                ("icon".to_string(), String::new()),
            ];
            let row = make_menu_row(
                section,
                &child.text(),
                &fields,
                &ctx,
                &popup,
                &popup_content,
                &active,
            );
            let hook: Rc<dyn Fn()> = {
                let on_change = on_change.clone();
                Rc::new(move || {
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                })
            };
            connect_menu_row(&row, &hook);
            attach_menu_row(container, &rows, row, &on_change, &popup_content);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    Editor::Menu {
        root,
        rows,
        on_change,
    }
}

fn autohide_delay_pair<'a>(
    extras: &[&'a (Record, Editor)],
    index: usize,
) -> Option<&'a (Record, Editor)> {
    let (record, _) = extras.get(index)?;
    if record.segments.last().map(String::as_str) != Some("autoHide") {
        return None;
    }
    let (next, _) = extras.get(index + 1)?;
    if next.segments.last().map(String::as_str) == Some("autoHideDelay") {
        extras.get(index + 1).copied()
    } else {
        None
    }
}

fn horizontal_run(extras: &[&(Record, Editor)], index: usize) -> usize {
    let Some((first, _)) = extras.get(index) else {
        return 0;
    };
    if !first.spec.horizontal {
        return 0;
    }
    let group = first.spec.group.as_deref();
    let mut len = 1;
    for (record, _) in &extras[index + 1..] {
        if !record.spec.horizontal || record.spec.group.as_deref() != group {
            break;
        }
        len += 1;
    }
    len
}

fn append_flow_group(root: &gtk::Box, group: &[&(Record, Editor)]) {
    if let Some((first, _)) = group.first() {
        if let Some(name) = &first.spec.group {
            root.append(
                &gtk::Label::builder()
                    .xalign(0.0)
                    .label(name)
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        let flow = flow_box(first.spec.max_columns.unwrap_or(4));
        for (record, editor) in group {
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 2);
            cell.set_hexpand(true);
            let label = record
                .spec
                .label
                .clone()
                .unwrap_or_else(|| record.segments.last().cloned().unwrap_or_else(|| record.path()));
            let caption = gtk::Label::builder()
                .xalign(0.0)
                .label(label)
                .css_classes(["dim-label"])
                .build();
            cell.append(&caption);
            cell.append(&editor.root());
            flow.append(&cell);
        }
        root.append(&flow);
    }
}

fn append_settings_rows(
    root: &gtk::Box,
    extras: &[(Record, Editor)],
    ctx: &BuildContext,
) {
    let visible: Vec<&(Record, Editor)> = extras
        .iter()
        .filter(|(record, _)| !(record.spec.hidden && !ctx.advanced))
        .collect();
    let mut skip = false;
    for (index, (record, editor)) in visible.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        let run = horizontal_run(&visible, index);
        if run >= 2 {
            append_flow_group(root, &visible[index..index + run]);
            continue;
        }
        match autohide_delay_pair(&visible, index) {
            Some((delay_record, delay_editor)) => {
                root.append(&build_pair_row(record, editor, delay_record, delay_editor));
                skip = true;
            }
            None => root.append(&build_child_row(record, editor, 1)),
        }
    }
}

/// Wrap the change hook so a row added after wiring still notifies the app's
/// handler, which the hook only holds once editing has begun.
pub(crate) fn hook_notify(on_change: &ChangeHook) -> Rc<dyn Fn()> {
    let hook = on_change.clone();
    Rc::new(move || {
        if let Some(cb) = hook.borrow().as_ref() {
            cb();
        }
    })
}

/// A tile's serialization inputs, split from the GTK state so order
/// assignment and emission are unit-testable without a display.
struct TileEmit {
    id: String,
    enabled: bool,
    field_lines: Vec<String>,
    orig_order: Option<i64>,
}

/// Derive the `_order` sequence for an emitted list. Original values are
/// collected into an ascending pool and dealt by position, so a no-op save
/// and a pure reorder keep the sparse values; positions past the pool draw
/// 10-step values that never fall behind the pool's tail.
fn assign_tile_orders(orig: &[Option<i64>]) -> Vec<String> {
    let mut pool: Vec<i64> = orig.iter().flatten().copied().collect();
    pool.sort_unstable();
    let mut last = 0i64;
    orig.iter()
        .enumerate()
        .map(|(index, _)| {
            let order = match pool.get(index) {
                Some(value) => *value,
                None => (((index as i64) + 1) * 10).max(last + 10),
            };
            last = order;
            order.to_string()
        })
        .collect()
}

#[cfg(test)]
fn tile_emit_from_scan(row: &scan::TableRowScan) -> TileEmit {
    let mut enabled = true;
    let mut orig_order = None;
    let mut field_lines = Vec::new();
    for (name, raw) in &row.fields {
        let raw = raw.trim();
        match name.as_str() {
            "enabled" => enabled = truthy(raw),
            "_order" => orig_order = raw.parse::<i64>().ok(),
            _ => field_lines.push(format!("{} = {};", attr_key(name), raw)),
        }
    }
    TileEmit {
        id: row.key.clone(),
        enabled,
        field_lines,
        orig_order,
    }
}

fn tile_emit_from_tile(tile: &WidgetTile) -> TileEmit {
    let field_lines = tile
        .fields
        .borrow()
        .iter()
        .filter_map(attrset_field_nix)
        .collect();
    TileEmit {
        id: tile.id.clone(),
        enabled: tile.enabled.is_active(),
        field_lines,
        orig_order: tile.orig_order.trim().parse::<i64>().ok(),
    }
}

/// Emit the attrset form of a widget list: `{ id = { enabled = ...;
/// _order = ...; ... }; ... }`.
fn tiles_nix(entries: &[TileEmit]) -> String {
    let orders = assign_tile_orders(
        &entries
            .iter()
            .map(|entry| entry.orig_order)
            .collect::<Vec<_>>(),
    );
    let parts: Vec<String> = entries
        .iter()
        .zip(orders.iter())
        .map(|(entry, order)| {
            let mut body = format!("enabled = {}; _order = {};", entry.enabled, order);
            for line in &entry.field_lines {
                body.push(' ');
                body.push_str(line);
            }
            format!("{} = {{ {} }};", attr_key(&entry.id), body)
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

/// Emit the center list, which stays a plain string list: `[ "music" ... ]`.
fn center_tiles_nix(ids: &[String]) -> String {
    if ids.is_empty() {
        return "[ ]".to_string();
    }
    let items: Vec<String> = ids.iter().map(|id| quote(id)).collect();
    format!("[ {} ]", items.join(" "))
}

/// Read a plain nix string list into its items; anything else yields nothing.
fn parse_string_list(text: &str) -> Vec<String> {
    use rnix::ast::{Expr, InterpolPart};
    let Ok(root) = rnix::Root::parse(text).ok() else {
        return Vec::new();
    };
    let Some(Expr::List(list)) = root.expr() else {
        return Vec::new();
    };
    list.items()
        .filter_map(|item| match item {
            Expr::Str(string) => match string.normalized_parts().as_slice() {
                [InterpolPart::Literal(text)] => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The tile list matches its scanned original: same id sequence, same
/// enabled state, and every field editor on its original raw text. `_order`
/// never participates; it is derived on save.
fn tiles_unchanged(list: &WidgetTileList) -> bool {
    let tiles = list.tiles.borrow();
    tiles.len() == list.orig_ids.len()
        && tiles
            .iter()
            .zip(list.orig_ids.iter())
            .all(|(tile, orig_id)| {
                tile.orig_id == *orig_id
                    && tile.id == tile.orig_id
                    && tile.enabled.is_active() == truthy(&tile.orig_enabled)
                    && tile
                        .fields
                        .borrow()
                        .iter()
                        .all(|field| field.editor.unchanged(&field.orig_value))
            })
}

fn valid_widget_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|next| next.is_ascii_alphanumeric() || next == '_')
}

/// Build one freeform field from a preset's declared kind and bounds,
/// falling back to value inference when the preset says nothing.
fn make_field_for_preset(
    name: &str,
    raw: &str,
    spec: Option<&PresetFieldSpec>,
    ctx: &BuildContext,
) -> AttrsetField {
    let trimmed = raw.trim();
    let present = !trimmed.is_empty() && trimmed != "null";
    let source = if present { trimmed } else { "" };
    let option_spec = OptionSpec {
        widget: spec.and_then(|spec| spec.widget),
        choices: spec.and_then(|spec| spec.choices.clone()),
        min: spec.and_then(|spec| spec.min),
        max: spec.and_then(|spec| spec.max),
        step: spec.and_then(|spec| spec.step),
        unit: spec.and_then(|spec| spec.unit.clone()),
        ..OptionSpec::default()
    };
    let record = Record {
        value: source.to_string(),
        spec: option_spec,
        ..Record::default()
    };
    AttrsetField {
        name: name.to_string(),
        editor: build(&record, None, ctx),
        orig_value: source.to_string(),
        orig_present: present,
    }
}

/// One pop-out line for a field: preset label and description when the
/// marker supplies them, the shared caption and help otherwise.
fn tile_field_line(field: &AttrsetField, specs: &[PresetFieldSpec]) -> gtk::Box {
    let spec = specs.iter().find(|spec| spec.name == field.name);
    let caption = spec
        .and_then(|spec| spec.label.clone())
        .unwrap_or_else(|| field_caption(&field.name));
    let help = spec
        .and_then(|spec| spec.description.clone())
        .or_else(|| field_help(&field.name).map(str::to_string));
    let line = dim_line(&caption, &field.editor.root());
    if let Some(help) = help {
        line.set_tooltip_text(Some(&help));
    }
    line
}

/// Move a tile to a new position and redraw the rows in list order.
fn move_tile(
    list: &gtk::ListBox,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    from: usize,
    to: isize,
    on_change: &ChangeHook,
) {
    let count = tiles.borrow().len() as isize;
    if count == 0 || from as isize >= count {
        return;
    }
    let to = to.clamp(0, count - 1) as usize;
    if to == from {
        return;
    }
    let tile = {
        let mut guard = tiles.borrow_mut();
        guard.remove(from)
    };
    tiles.borrow_mut().insert(to, tile);
    {
        let guard = tiles.borrow();
        for tile in guard.iter() {
            list.remove(&tile.row);
        }
        for tile in guard.iter() {
            list.append(&tile.row);
        }
    }
    if let Some(cb) = on_change.borrow().as_ref() {
        cb();
    }
}

/// Insertion index for a pointer position: the first row whose midpoint
/// sits below `y`.
fn drop_index(list: &gtk::ListBox, y: f64) -> i32 {
    let children = list.observe_children();
    let mut index = 0;
    for position in 0..children.n_items() {
        let Some(child) = children.item(position) else {
            continue;
        };
        let Ok(widget) = child.downcast::<gtk::Widget>() else {
            continue;
        };
        let Some(bounds) = widget.compute_bounds(list) else {
            continue;
        };
        if y < bounds.y() as f64 + bounds.height() as f64 / 2.0 {
            return index;
        }
        index += 1;
    }
    index
}

fn show_indicator(list: &gtk::ListBox, indicator: &gtk::Separator, index: i32) {
    hide_indicator(list, indicator);
    list.insert(indicator, index);
}

fn hide_indicator(list: &gtk::ListBox, indicator: &gtk::Separator) {
    let Some(parent) = indicator.parent() else {
        return;
    };
    if parent == list.clone().upcast::<gtk::Widget>() {
        list.remove(indicator);
    } else if parent.parent() == Some(list.clone().upcast::<gtk::Widget>()) {
        list.remove(&parent);
    }
}

/// Drop target on a tile list: a 2px separator tracks the pointer between
/// rows, and a drop reorders the tiles.
fn attach_tile_dnd(list: &gtk::ListBox, tiles: &Rc<RefCell<Vec<WidgetTile>>>, on_change: &ChangeHook) {
    let indicator = gtk::Separator::new(gtk::Orientation::Horizontal);
    indicator.set_height_request(2);
    let drop = gtk::DropTarget::new(glib::Type::U32, gtk::gdk::DragAction::MOVE);
    list.add_controller(drop.clone());

    let enter_list = list.clone();
    let enter_indicator = indicator.clone();
    drop.connect_enter(move |_, _, y| {
        hide_indicator(&enter_list, &enter_indicator);
        show_indicator(&enter_list, &enter_indicator, drop_index(&enter_list, y));
        gtk::gdk::DragAction::MOVE
    });

    let motion_list = list.clone();
    let motion_indicator = indicator.clone();
    drop.connect_motion(move |_, _, y| {
        hide_indicator(&motion_list, &motion_indicator);
        show_indicator(&motion_list, &motion_indicator, drop_index(&motion_list, y));
        gtk::gdk::DragAction::MOVE
    });

    let leave_list = list.clone();
    let leave_indicator = indicator.clone();
    drop.connect_leave(move |_| hide_indicator(&leave_list, &leave_indicator));

    let drop_tiles = tiles.clone();
    let drop_hook = on_change.clone();
    let drop_list = list.clone();
    drop.connect_drop(move |_, value, _, y| {
        hide_indicator(&drop_list, &indicator);
        let Ok(source) = value.get::<u32>() else {
            return false;
        };
        let source = source as usize;
        let target = drop_index(&drop_list, y) as usize;
        let count = drop_tiles.borrow().len();
        if source >= count {
            return false;
        }
        let target = if target > source { target - 1 } else { target };
        move_tile(&drop_list, &drop_tiles, source, target as isize, &drop_hook);
        true
    });
}

/// The cog pop-out: id entry for unknown ids, the enabled switch, one line
/// per field, and (for custom ids) an add-setting row. Everything is built
/// eagerly so the field editors connect once, here.
#[allow(clippy::too_many_arguments)]
fn build_tile_popout(
    id: &str,
    preset: Option<&WidgetPresetSpec>,
    fields: &Rc<RefCell<Vec<AttrsetField>>>,
    field_specs: &[PresetFieldSpec],
    enabled: &gtk::Switch,
    label: &gtk::Label,
    row: &gtk::ListBoxRow,
    ctx: &BuildContext,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    on_change: &ChangeHook,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    popover.set_child(Some(&content));

    if preset.is_none() && (ctx.advanced || known_widget_name(id).is_none()) {
        let entry = gtk::Entry::new();
        entry.set_text(id);
        entry.set_hexpand(true);
        let line = dim_line("Id", &entry);
        line.set_tooltip_text(Some("Widget id stored in the configuration file."));
        content.append(&line);
        let (tiles_id, row_id, label_id, hook) = (
            tiles.clone(),
            row.clone(),
            label.clone(),
            on_change.clone(),
        );
        entry.connect_changed(move |entry| {
            let text = entry.text().to_string();
            let mut tiles = tiles_id.borrow_mut();
            if let Some(tile) = tiles.iter_mut().find(|tile| tile.row == row_id) {
                tile.id = text.clone();
                label_id.set_text(&widget_display_name(&text));
            }
            drop(tiles);
            if let Some(cb) = hook.borrow().as_ref() {
                cb();
            }
        });
    }

    let enabled_line = dim_line("Enabled", enabled);
    enabled_line.set_tooltip_text(Some("Whether this widget is shown."));
    content.append(&enabled_line);

    let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&fields_box);
    for field in fields.borrow().iter() {
        fields_box.append(&tile_field_line(field, field_specs));
    }

    if preset.is_none() {
        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = gtk::Entry::new();
        name.set_placeholder_text(Some("name"));
        name.set_width_chars(10);
        let value = gtk::Entry::new();
        value.set_placeholder_text(Some("value"));
        value.set_hexpand(true);
        let add = gtk::Button::with_label("+");
        add.set_tooltip_text(Some("Add setting"));
        add_row.append(&name);
        add_row.append(&value);
        add_row.append(&add);
        content.append(&add_row);

        let (fields_add, lines_add, hook) = (
            fields.clone(),
            fields_box.clone(),
            on_change.clone(),
        );
        let specs_add: Vec<PresetFieldSpec> = field_specs.to_vec();
        let ctx_add = ctx.clone();
        add.connect_clicked(move |_| {
            let field_name = name.text().trim().to_string();
            if field_name.is_empty()
                || !valid_widget_id(&field_name)
                || field_name == "enabled"
                || field_name == "_order"
                || fields_add
                    .borrow()
                    .iter()
                    .any(|field| field.name == field_name)
            {
                return;
            }
            let mut field = make_attrset_field(&field_name, &value.text(), &ctx_add);
            field.orig_present = true;
            field.editor.connect_changed(&hook_notify(&hook));
            lines_add.append(&tile_field_line(&field, &specs_add));
            fields_add.borrow_mut().push(field);
            name.set_text("");
            value.set_text("");
            if let Some(cb) = hook.borrow().as_ref() {
                cb();
            }
        });
    }

    popover
}

/// Build one tile: face (grip, label, enabled dot, cog, remove), the cog
/// pop-out, a right-click move menu, the drag source, and the remove
/// button. `orig_id` is empty for tiles added this session.
#[allow(clippy::too_many_arguments)]
fn make_widget_tile(
    id: &str,
    orig_id: &str,
    orig_enabled: &str,
    orig_order: &str,
    preset: Option<WidgetPresetSpec>,
    fields: Rc<RefCell<Vec<AttrsetField>>>,
    field_specs: Vec<PresetFieldSpec>,
    section: TileSection,
    ctx: &BuildContext,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    list: &gtk::ListBox,
    on_change: &ChangeHook,
) -> WidgetTile {
    let display = preset
        .as_ref()
        .map(|preset| preset.label.clone())
        .unwrap_or_else(|| widget_display_name(id));

    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    body.set_margin_top(4);
    body.set_margin_bottom(4);
    body.set_margin_start(6);
    body.set_margin_end(4);
    row.set_child(Some(&body));

    let grip = gtk::Image::from_icon_name("open-menu-symbolic");
    body.append(&grip);

    let label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(&display)
        .css_classes(["heading"])
        .build();
    label.set_hexpand(true);
    if let Some(description) = preset.as_ref().and_then(|preset| preset.description.as_deref()) {
        label.set_tooltip_text(Some(description));
    }
    body.append(&label);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    body.append(&spacer);

    let enabled = gtk::Switch::new();
    enabled.set_active(truthy(orig_enabled));
    enabled.set_halign(gtk::Align::End);

    if !section.is_center() {
        let dot = gtk::Button::from_icon_name("object-select-symbolic");
        dot.add_css_class("flat");
        dot.set_tooltip_text(Some("Enabled"));
        let (face_body, face_dot) = (body.clone(), dot.clone());
        let face_enabled = enabled.clone();
        let sync_face = Rc::new(move || {
            let active = face_enabled.is_active();
            face_body.set_opacity(if active { 1.0 } else { 0.5 });
            face_dot.set_icon_name(if active {
                "object-select-symbolic"
            } else {
                "edit-clear-symbolic"
            });
        });
        sync_face();
        let (sync_notify, hook) = (sync_face.clone(), on_change.clone());
        enabled.connect_active_notify(move |_| {
            sync_notify();
            if let Some(cb) = hook.borrow().as_ref() {
                cb();
            }
        });
        let toggle = enabled.clone();
        dot.connect_clicked(move |_| toggle.set_active(!toggle.is_active()));
        body.append(&dot);

        let show_cog = match &preset {
            Some(preset) => preset.has_settings || !fields.borrow().is_empty(),
            None => orig_id.is_empty() || !fields.borrow().is_empty(),
        };
        if show_cog {
            let cog = gtk::Button::from_icon_name("emblem-settings-symbolic");
            cog.add_css_class("flat");
            cog.set_tooltip_text(Some("Widget settings"));
            body.append(&cog);
            let popover = build_tile_popout(
                id,
                preset.as_ref(),
                &fields,
                &field_specs,
                &enabled,
                &label,
                &row,
                ctx,
                tiles,
                on_change,
            );
            popover.set_parent(&cog);
            popover.set_position(gtk::PositionType::Right);
            let for_cog = popover;
            cog.connect_clicked(move |_| for_cog.popup());
        }
    }

    for field in fields.borrow().iter() {
        field.editor.connect_changed(&hook_notify(on_change));
    }

    let menu = gtk::Popover::new();
    menu.set_parent(&body);
    menu.set_position(gtk::PositionType::Right);
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(8);
    menu_box.set_margin_end(8);
    menu.set_child(Some(&menu_box));
    for (text, target) in [
        ("Move up", -1isize),
        ("Move down", 1isize),
        ("Move to top", isize::MIN),
        ("Move to bottom", isize::MAX),
    ] {
        let button = gtk::Button::with_label(text);
        button.add_css_class("flat");
        let (tiles_move, row_move, list_move, hook) = (
            tiles.clone(),
            row.clone(),
            list.clone(),
            on_change.clone(),
        );
        let menu_move = menu.clone();
        button.connect_clicked(move |_| {
            menu_move.popdown();
            let from = tiles_move
                .borrow()
                .iter()
                .position(|tile| tile.row == row_move);
            if let Some(from) = from {
                let last = tiles_move.borrow().len() as isize - 1;
                let to = match target {
                    isize::MIN => 0,
                    isize::MAX => last,
                    delta => from as isize + delta,
                };
                move_tile(&list_move, &tiles_move, from, to.clamp(0, last), &hook);
            }
        });
        menu_box.append(&button);
    }
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    body.add_controller(right_click.clone());
    let menu_click = menu.clone();
    right_click.connect_pressed(move |_, _, _, _| menu_click.popup());

    let drag = gtk::DragSource::new();
    drag.set_actions(gtk::gdk::DragAction::MOVE);
    body.add_controller(drag.clone());
    let (drag_tiles, drag_row) = (tiles.clone(), row.clone());
    drag.connect_prepare(move |_, _, _| {
        let index = drag_tiles
            .borrow()
            .iter()
            .position(|tile| tile.row == drag_row)? as u32;
        Some(gtk::gdk::ContentProvider::for_value(&index.to_value()))
    });

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove widget"));
    body.append(&remove);
    let (remove_tiles, remove_row, remove_list, remove_hook) = (
        tiles.clone(),
        row.clone(),
        list.clone(),
        on_change.clone(),
    );
    remove.connect_clicked(move |_| {
        let position = remove_tiles
            .borrow()
            .iter()
            .position(|tile| tile.row == remove_row);
        if let Some(index) = position {
            remove_tiles.borrow_mut().remove(index);
            remove_list.remove(&remove_row);
            if let Some(cb) = remove_hook.borrow().as_ref() {
                cb();
            }
        }
    });

    WidgetTile {
        id: id.to_string(),
        orig_id: orig_id.to_string(),
        enabled,
        orig_enabled: orig_enabled.to_string(),
        orig_order: orig_order.to_string(),
        fields,
        row,
    }
}

/// Scaffold a tile from a catalog preset: `preset.defaults` become the
/// fields, marked as present so they always serialize.
fn add_tile_from_preset(
    preset: &WidgetPresetSpec,
    specs: &[PresetFieldSpec],
    section: TileSection,
    ctx: &BuildContext,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    target: &gtk::ListBox,
    on_change: &ChangeHook,
) {
    if tiles.borrow().iter().any(|tile| tile.id == preset.id) {
        return;
    }
    let fields = if section.is_center() {
        Rc::new(RefCell::new(Vec::new()))
    } else {
        Rc::new(RefCell::new(
            preset
                .defaults
                .iter()
                .map(|(name, lit)| {
                    let spec = specs.iter().find(|spec| spec.name == *name);
                    let mut field = make_field_for_preset(name, &lit.to_nix(), spec, ctx);
                    field.orig_present = true;
                    field
                })
                .collect(),
        ))
    };
    let tile = make_widget_tile(
        &preset.id,
        "",
        "true",
        "",
        Some(preset.clone()),
        fields,
        specs.to_vec(),
        section,
        ctx,
        tiles,
        target,
        on_change,
    );
    target.append(&tile.row);
    tiles.borrow_mut().push(tile);
    if let Some(cb) = on_change.borrow().as_ref() {
        cb();
    }
}

/// Refill the add-picker with this section's presets the list does not
/// already contain, filtered by the search term.
fn populate_widget_picker(
    picker_list: &gtk::ListBox,
    section: TileSection,
    presets: &[(WidgetPresetSpec, Vec<PresetFieldSpec>)],
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    term: &str,
) {
    clear_rows(picker_list);
    let taken: Vec<String> = tiles.borrow().iter().map(|tile| tile.id.clone()).collect();
    let term = term.trim().to_lowercase();
    for (preset, _) in presets
        .iter()
        .filter(|(preset, _)| {
            preset
                .sections
                .iter()
                .any(|value| value == section.picker_filter())
        })
        .filter(|(preset, _)| !taken.iter().any(|id| id == &preset.id))
        .filter(|(preset, _)| {
            term.is_empty()
                || preset.label.to_lowercase().contains(&term)
                || preset.id.to_lowercase().contains(&term)
        })
    {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&preset.id);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_top(6);
        line.set_margin_bottom(6);
        line.set_margin_start(8);
        line.set_margin_end(8);
        if let Some(icon) = &preset.icon {
            let image = gtk::Image::from_icon_name(icon);
            image.set_pixel_size(24);
            line.append(&image);
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let heading = if preset.label.is_empty() {
            widget_display_name(&preset.id)
        } else {
            preset.label.clone()
        };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .label(heading)
            .css_classes(["heading"])
            .build();
        text.append(&label);
        if let Some(description) = &preset.description {
            let dim = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .label(description)
                .css_classes(["dim-label"])
                .build();
            text.append(&dim);
        }
        line.append(&text);
        row.set_child(Some(&line));
        picker_list.append(&row);
    }
}

impl WidgetTileList {
    /// Build one widget tile list: heading, tiles with drag reorder, and
    /// the add buttons. `value` is the raw nix of the list.
    fn build(
        section: TileSection,
        value: &str,
        presets: &[(WidgetPresetSpec, Vec<PresetFieldSpec>)],
        ctx: &BuildContext,
        on_change: &ChangeHook,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        root.set_hexpand(true);
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label(section.heading())
            .css_classes(["heading"])
            .build();
        root.append(&heading);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_hexpand(true);
        let tiles: Rc<RefCell<Vec<WidgetTile>>> = Rc::new(RefCell::new(Vec::new()));
        let mut orig_ids: Vec<String> = Vec::new();

        if section.is_center() {
            for id in parse_string_list(value) {
                orig_ids.push(id.clone());
                let tile = make_widget_tile(
                    &id,
                    &id,
                    "true",
                    "",
                    None,
                    Rc::new(RefCell::new(Vec::new())),
                    Vec::new(),
                    section,
                    ctx,
                    &tiles,
                    &list,
                    on_change,
                );
                list.append(&tile.row);
                tiles.borrow_mut().push(tile);
            }
        } else {
            for scan_row in scan::scan_table(value, false) {
                orig_ids.push(scan_row.key.clone());
                let mut enabled_raw = "true".to_string();
                let mut order_raw = String::new();
                let mut scanned: Vec<(String, String)> = Vec::new();
                for (name, raw) in &scan_row.fields {
                    match name.as_str() {
                        "enabled" => enabled_raw = raw.trim().to_string(),
                        "_order" => order_raw = raw.trim().to_string(),
                        _ => scanned.push((name.clone(), raw.trim().to_string())),
                    }
                }
                let preset_entry = presets.iter().find(|(preset, _)| preset.id == scan_row.key);
                let preset = preset_entry.map(|(preset, _)| preset.clone());
                let specs: Vec<PresetFieldSpec> = preset_entry
                    .map(|(_, specs)| specs.clone())
                    .unwrap_or_default();
                let fields = Rc::new(RefCell::new(
                    scanned
                        .iter()
                        .map(|(name, raw)| {
                            let spec = specs.iter().find(|spec| spec.name == *name);
                            make_field_for_preset(name, raw, spec, ctx)
                        })
                        .collect(),
                ));
                let tile = make_widget_tile(
                    &scan_row.key,
                    &scan_row.key,
                    &enabled_raw,
                    &order_raw,
                    preset,
                    fields,
                    specs,
                    section,
                    ctx,
                    &tiles,
                    &list,
                    on_change,
                );
                list.append(&tile.row);
                tiles.borrow_mut().push(tile);
            }
        }

        attach_tile_dnd(&list, &tiles, on_change);
        root.append(&list);

        let presets = presets.to_vec();
        let picker_popover = gtk::Popover::new();
        picker_popover.set_position(gtk::PositionType::Bottom);
        let picker_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        picker_box.set_margin_top(12);
        picker_box.set_margin_bottom(12);
        picker_box.set_margin_start(12);
        picker_box.set_margin_end(12);
        picker_box.set_size_request(320, -1);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search widgets"));
        let picker_list = gtk::ListBox::new();
        picker_list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .min_content_height(160)
            .max_content_height(280)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&picker_list)
            .build();
        picker_box.append(&search);
        picker_box.append(&scroll);
        picker_popover.set_child(Some(&picker_box));

        let repopulate: Rc<dyn Fn()> = {
            let (picker_list, presets, tiles) = (picker_list.clone(), presets.clone(), tiles.clone());
            let search = search.clone();
            Rc::new(move || {
                populate_widget_picker(&picker_list, section, &presets, &tiles, &search.text());
            })
        };
        picker_popover.connect_map({
            let repopulate = repopulate.clone();
            move |_| repopulate()
        });
        search.connect_search_changed({
            let repopulate = repopulate.clone();
            move |_| repopulate()
        });
        picker_list.connect_row_activated({
            let (presets, tiles) = (presets.clone(), tiles.clone());
            let (target, on_change) = (list.clone(), on_change.clone());
            let ctx = ctx.clone();
            let popover = picker_popover.clone();
            move |_, row| {
                let preset_id = row.widget_name().to_string();
                if let Some((preset, specs)) = presets.iter().find(|(preset, _)| preset.id == preset_id)
                {
                    add_tile_from_preset(preset, specs, section, &ctx, &tiles, &target, &on_change);
                }
                popover.popdown();
            }
        });

        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add = gtk::MenuButton::builder()
            .label("Add widget")
            .halign(gtk::Align::Start)
            .popover(&picker_popover)
            .build();
        let add_custom = gtk::Button::with_label("Add custom…");
        add_custom.add_css_class("flat");
        add_custom.set_halign(gtk::Align::Start);
        add_row.append(&add);
        add_row.append(&add_custom);
        root.append(&add_row);

        let custom_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let custom_entry = gtk::Entry::new();
        custom_entry.set_placeholder_text(Some("widgetId"));
        custom_entry.set_width_chars(16);
        let custom_ok = gtk::Button::with_label("Add");
        custom_row.append(&custom_entry);
        custom_row.append(&custom_ok);
        custom_row.set_visible(false);
        root.append(&custom_row);

        add_custom.connect_clicked({
            let custom_row = custom_row.clone();
            let custom_entry = custom_entry.clone();
            move |_| {
                let show = !custom_row.is_visible();
                custom_row.set_visible(show);
                if show {
                    custom_entry.grab_focus();
                }
            }
        });
        custom_ok.connect_clicked({
            let (custom_entry, custom_row) = (custom_entry.clone(), custom_row.clone());
            let (tiles, on_change) = (tiles.clone(), on_change.clone());
            let (target, ctx) = (list.clone(), ctx.clone());
            move |_| {
                let id = custom_entry.text().trim().to_string();
                if !valid_widget_id(&id) || tiles.borrow().iter().any(|tile| tile.id == id) {
                    return;
                }
                let tile = make_widget_tile(
                    &id,
                    "",
                    "true",
                    "",
                    None,
                    Rc::new(RefCell::new(Vec::new())),
                    Vec::new(),
                    section,
                    &ctx,
                    &tiles,
                    &target,
                    &on_change,
                );
                custom_entry.set_text("");
                custom_row.set_visible(false);
                target.append(&tile.row);
                tiles.borrow_mut().push(tile);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            }
        });

        Self {
            root,
            tiles,
            orig_ids,
        }
    }

    fn root(&self) -> &gtk::Box {
        &self.root
    }
}

fn build_bar(record: &Record, ctx: &BuildContext) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    let mut left_value = None;
    let mut right_value = None;
    let mut center_value = None;
    let mut extras = Vec::new();
    for child in scan_children(record, true) {
        match child.segments.last().map(String::as_str) {
            Some("leftWidgets") => left_value = Some(child.value.clone()),
            Some("rightWidgets") => right_value = Some(child.value.clone()),
            Some("centerWidgets") => center_value = Some(child.value.clone()),
            _ => extras.push(child),
        }
    }

    let left_present = left_value.is_some();
    let right_present = right_value.is_some();
    let center_present = center_value.is_some();
    let presets = ctx.presets.get(&record.file).cloned().unwrap_or_default();

    let left = WidgetTileList::build(
        TileSection::BarLeft,
        left_value.as_deref().unwrap_or("{ }"),
        &presets,
        ctx,
        &on_change,
    );
    let center = WidgetTileList::build(
        TileSection::BarCenter,
        center_value.as_deref().unwrap_or("[ ]"),
        &presets,
        ctx,
        &on_change,
    );
    let right = WidgetTileList::build(
        TileSection::BarRight,
        right_value.as_deref().unwrap_or("{ }"),
        &presets,
        ctx,
        &on_change,
    );
    root.append(left.root());
    root.append(center.root());
    root.append(right.root());

    let extras: Vec<(Record, Editor)> = extras
        .into_iter()
        .map(|record| {
            let editor = build_extra_editor(&record, ctx);
            (record, editor)
        })
        .collect();
    if extras
        .iter()
        .any(|(record, _)| !(record.spec.hidden && !ctx.advanced))
    {
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label("Settings")
            .css_classes(["heading"])
            .build();
        root.append(&heading);
        append_settings_rows(&root, &extras, ctx);
    }

    Editor::Bar {
        root,
        left,
        right,
        center,
        left_present,
        right_present,
        center_present,
        extras,
        on_change,
    }
}

fn bar_nix(
    left: &WidgetTileList,
    right: &WidgetTileList,
    center: &WidgetTileList,
    left_present: bool,
    right_present: bool,
    center_present: bool,
    extras: &[(Record, Editor)],
) -> String {
    let mut parts = Vec::new();
    if left_present || !left.tiles.borrow().is_empty() {
        let entries: Vec<TileEmit> = left.tiles.borrow().iter().map(tile_emit_from_tile).collect();
        parts.push(format!("leftWidgets = {};", tiles_nix(&entries)));
    }
    if center_present || !center.tiles.borrow().is_empty() {
        let ids: Vec<String> = center
            .tiles
            .borrow()
            .iter()
            .map(|tile| tile.id.clone())
            .collect();
        parts.push(format!("centerWidgets = {};", center_tiles_nix(&ids)));
    }
    if right_present || !right.tiles.borrow().is_empty() {
        let entries: Vec<TileEmit> = right.tiles.borrow().iter().map(tile_emit_from_tile).collect();
        parts.push(format!("rightWidgets = {};", tiles_nix(&entries)));
    }
    for (record, editor) in extras {
        let name = record.segments.last().cloned().unwrap_or_default();
        parts.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
    }
    format!("{{ {} }}", parts.join(" "))
}

fn build_grid_configuration(record: &Record, ctx: &BuildContext) -> Editor {
    install_grid_css();
    let profile = GridProfile::for_path(&record.path()).unwrap_or_else(GridProfile::generic);
    let profile = GridProfile {
        columns: record
            .spec
            .grid_columns
            .map(|n| n.max(1))
            .unwrap_or(profile.columns),
        ..profile
    };

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_hexpand(true);
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let canvas = gtk::Grid::new();
    canvas.set_column_homogeneous(true);
    canvas.set_row_homogeneous(true);
    canvas.set_column_spacing(GRID_GAP as u32);
    canvas.set_row_spacing(GRID_GAP as u32);
    canvas.set_hexpand(true);
    let details = gtk::Box::new(gtk::Orientation::Vertical, 8);
    details.set_hexpand(true);
    let adder_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let size_caption = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let footer_flow = profile.has_footer.then(|| {
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(8);
        flow.set_column_spacing(8);
        flow.set_row_spacing(8);
        flow
    });

    root.append(&canvas);
    if let Some(flow) = &footer_flow {
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label("Footer")
            .css_classes(["heading"])
            .build();
        root.append(&heading);
        root.append(flow);
    }
    root.append(&adder_slot);
    root.append(&details);

    let mut orig_ids = Vec::new();
    let mut extras = Vec::new();
    let mut built = Vec::new();
    for child in scan_children(record, ctx.advanced) {
        if value::classify(&child.value) == value::ValueKind::AttrSet {
            let id = child.segments.last().cloned().unwrap_or_default();
            orig_ids.push(id.clone());
            built.push(make_grid_tile(&id, &child.value, profile, ctx));
        } else {
            extras.push(child);
        }
    }

    let extras: Vec<(Record, Editor)> = extras
        .into_iter()
        .map(|child| {
            let visible = !(child.spec.hidden && !ctx.advanced);
            let editor = build_bespoke_child(&child, ctx);
            if visible {
                root.append(&build_child_row(&child, &editor, 1));
            }
            (child, editor)
        })
        .collect();

    let state = Rc::new(GridLayoutState {
        profile,
        tiles: Rc::new(RefCell::new(built)),
        orig_ids,
        selected: Rc::new(RefCell::new(None)),
        extras,
        on_change: on_change.clone(),
        canvas: canvas.clone(),
        details: details.clone(),
        adder_slot: adder_slot.clone(),
        footer_flow,
        size_caption: size_caption.clone(),
        advanced: ctx.advanced,
        drag_index: Rc::new(RefCell::new(None)),
        size_index: Rc::new(RefCell::new(None)),
        pinned: Rc::new(RefCell::new(Vec::new())),
        steps: Rc::new(Cell::new((1.0, 1.0))),
        cancelled: Rc::new(Cell::new(false)),
        programmatic: Rc::new(Cell::new(false)),
    });

    {
        let tiles = state.tiles.borrow();
        for tile in tiles.iter() {
            wire_grid_tile(&state, tile);
        }
    }
    {
        let keys = gtk::EventControllerKey::new();
        let state_keys = state.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                grid_cancel(&state_keys);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        root.add_controller(keys);
    }
    {
        let cancel = gtk::GestureClick::new();
        cancel.set_button(3);
        let state_cancel = state.clone();
        cancel.connect_pressed(move |_, _, _, _| grid_cancel(&state_cancel));
        root.add_controller(cancel);
    }

    grid_reattach(&state);
    grid_refresh_adder(&state);
    grid_refresh_details(&state);
    Editor::Grid { root, state }
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
    let presets = ctx.presets.get(&record.file).cloned().unwrap_or_default();

    let widgets = WidgetTileList::build(
        TileSection::Dock,
        widgets_value.as_deref().unwrap_or("{ }"),
        &presets,
        ctx,
        &on_change,
    );
    root.append(widgets.root());

    let extras: Vec<(Record, Editor)> = extras
        .into_iter()
        .map(|record| {
            let editor = build_extra_editor(&record, ctx);
            (record, editor)
        })
        .collect();
    if extras
        .iter()
        .any(|(record, _)| !(record.spec.hidden && !ctx.advanced))
    {
        let settings = gtk::Label::builder()
            .xalign(0.0)
            .label("Settings")
            .css_classes(["heading"])
            .build();
        root.append(&settings);
        append_settings_rows(&root, &extras, ctx);
    }

    Editor::Dock {
        root,
        widgets,
        widgets_present,
        extras,
        on_change,
    }
}

fn dock_nix(
    widgets: &WidgetTileList,
    widgets_present: bool,
    extras: &[(Record, Editor)],
) -> String {
    let mut parts = Vec::new();
    if widgets_present || !widgets.tiles.borrow().is_empty() {
        let entries: Vec<TileEmit> = widgets
            .tiles
            .borrow()
            .iter()
            .map(tile_emit_from_tile)
            .collect();
        parts.push(format!("widgets = {};", tiles_nix(&entries)));
    }
    for (record, editor) in extras {
        let name = record.segments.last().cloned().unwrap_or_default();
        parts.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
    }
    format!("{{ {} }}", parts.join(" "))
}

impl GridLayoutState {
    fn notify(&self) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback();
        }
    }
}

/// A `width x height` overlap test between two whole-cell rectangles.
fn cells_overlap(a: CellRect, b: CellRect) -> bool {
    a.col < b.col + b.w && b.col < a.col + a.w && a.row < b.row + b.h && b.row < a.row + a.h
}

fn grid_overlaps(taken: &[CellRect], candidate: CellRect) -> bool {
    taken.iter().any(|cell| cells_overlap(*cell, candidate))
}

/// First free slot scanning rows top to bottom; falls back below the stack,
/// matching DMS `firstFit` (GridLayout.js:74-93).
fn grid_first_fit(taken: &[CellRect], columns: i64, w: i64, h: i64) -> CellRect {
    let ceiling = taken
        .iter()
        .map(|cell| cell.row + cell.h)
        .max()
        .unwrap_or(0);
    let mut row = 0;
    while row < ceiling {
        let mut col = 0;
        while col + w <= columns {
            let candidate = CellRect { col, row, w, h };
            if !grid_overlaps(taken, candidate) {
                return candidate;
            }
            col += 1;
        }
        row += 1;
    }
    CellRect {
        col: 0,
        row: ceiling,
        w,
        h,
    }
}

/// Pull every cell upward while the row above it is free, matching DMS
/// `compact` (GridLayout.js:46-53).
fn grid_compact(cells: &mut [CellRect]) {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|&i| (cells[i].row, cells[i].col));
    for &i in &order {
        while cells[i].row > 0 {
            let mut candidate = cells[i];
            candidate.row -= 1;
            let blocked = cells
                .iter()
                .enumerate()
                .any(|(j, cell)| j != i && cells_overlap(*cell, candidate));
            if blocked {
                break;
            }
            cells[i] = candidate;
        }
    }
}

/// Place every tile into a `columns`-wide grid. Tiles with a committed cell
/// keep their column and settle downward on collision; unplaced tiles first
/// fit. `order` is the placement priority (a permutation of `tiles` indices).
/// Returns the final cells, or `None` when the layout would exceed `max_rows`.
fn repack(
    tiles: &[TileInput],
    order: &[usize],
    columns: i64,
    max_rows: i64,
    gravity: bool,
) -> Option<Vec<CellRect>> {
    let mut taken: Vec<CellRect> = Vec::new();
    let mut out: Vec<Option<CellRect>> = vec![None; tiles.len()];
    for &i in order {
        let tile = &tiles[i];
        let w = tile.w.clamp(1, columns.max(1));
        let h = tile.h.max(1);
        let cell = match tile.cell {
            Some(committed) => {
                let col = committed.col.clamp(0, (columns - w).max(0));
                let mut row = committed.row.max(0);
                let mut candidate = CellRect { col, row, w, h };
                while grid_overlaps(&taken, candidate) {
                    row += 1;
                    candidate.row = row;
                }
                candidate
            }
            None => grid_first_fit(&taken, columns, w, h),
        };
        taken.push(cell);
        out[i] = Some(cell);
    }
    let mut cells: Vec<CellRect> = out
        .into_iter()
        .map(|cell| cell.unwrap_or(CellRect { col: 0, row: 0, w: 1, h: 1 }))
        .collect();
    if gravity {
        grid_compact(&mut cells);
    }
    let rows = cells
        .iter()
        .map(|cell| cell.row + cell.h)
        .max()
        .unwrap_or(0);
    if rows > max_rows {
        return None;
    }
    Some(cells)
}

/// Whether a tile belongs to the footer strip rather than the canvas.
fn grid_tile_is_footer(tile: &GridTile) -> bool {
    tile.fields.iter().any(|(record, editor)| {
        record.segments.last().map(String::as_str) == Some("footer")
            && editor.to_nix().trim() == "true"
    })
}

fn grid_index_of(state: &Rc<GridLayoutState>, button: &gtk::ToggleButton) -> Option<usize> {
    state
        .tiles
        .borrow()
        .iter()
        .position(|tile| tile.tile == *button)
}

/// One-time stylesheet that makes the invisible resize strips visible.
fn install_grid_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".grid-resize {
            background-color: alpha(@window_fg_color, 0.25);
            border-radius: 3px;
        }";
        let provider = gtk::CssProvider::new();
        provider.load_from_data(css);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

/// The right, bottom and corner resize handles overlaid on a tile.
fn grid_resize_strips() -> (gtk::Box, gtk::Box, gtk::Box) {
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.add_css_class("grid-resize");
    right.set_size_request(6, -1);
    right.set_halign(gtk::Align::End);
    right.set_valign(gtk::Align::Fill);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bottom.add_css_class("grid-resize");
    bottom.set_size_request(-1, 6);
    bottom.set_halign(gtk::Align::Fill);
    bottom.set_valign(gtk::Align::End);
    let corner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    corner.add_css_class("grid-resize");
    corner.set_size_request(12, 12);
    corner.set_halign(gtk::Align::End);
    corner.set_valign(gtk::Align::End);
    (right, bottom, corner)
}

fn grid_update_tile_visual(tile: &GridTile) {
    let active = tile
        .enabled
        .as_ref()
        .map(|switch| switch.is_active())
        .unwrap_or(true);
    let name = widget_display_name(&tile.id);
    if tile.enabled.is_some() && !active {
        tile.label.set_text(&format!("{name} (off)"));
        tile.tile.set_opacity(0.4);
    } else {
        tile.label.set_text(&name);
        tile.tile.set_opacity(1.0);
    }
}

/// Build one on-canvas tile: parse the grid fields out of its value, keep every
/// other field as a child editor, and wrap the surface in an overlay that
/// carries the resize strips.
fn make_grid_tile(id: &str, value_text: &str, profile: GridProfile, ctx: &BuildContext) -> GridTile {
    let mut cell = CellRect { col: 0, row: 0, w: 1, h: 1 };
    let mut order = GRID_DEFAULT_ORDER;
    let mut enabled_value = true;
    let mut fields: Vec<(Record, Editor)> = Vec::new();
    for child in scan::scan_value(value_text) {
        let name = child.segments.last().cloned().unwrap_or_default();
        let text = child.value.trim().to_string();
        match name.as_str() {
            "col" => cell.col = text.parse().unwrap_or(0),
            "row" => cell.row = text.parse().unwrap_or(0),
            "w" => cell.w = text.parse().unwrap_or(1),
            "h" => cell.h = text.parse().unwrap_or(1),
            "_order" => order = text.parse().unwrap_or(GRID_DEFAULT_ORDER),
            "enabled" if profile.has_enabled => enabled_value = text == "true",
            _ => {
                let editor = build_bespoke_child(&child, ctx);
                fields.push((child, editor));
            }
        }
    }
    cell.w = cell.w.clamp(1, profile.columns.max(1));
    cell.h = cell.h.max(1);
    cell.col = cell.col.clamp(0, (profile.columns - cell.w).max(0));
    cell.row = cell.row.max(0);

    let enabled = profile.has_enabled.then(|| {
        let switch = gtk::Switch::new();
        switch.set_active(enabled_value);
        switch.set_valign(gtk::Align::Center);
        switch
    });

    let label = gtk::Label::builder()
        .xalign(0.5)
        .wrap(true)
        .label(widget_display_name(id))
        .build();
    let button = gtk::ToggleButton::new();
    button.add_css_class("flat");
    button.set_hexpand(true);
    button.set_vexpand(true);
    button.set_child(Some(&label));

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&button));
    let (right_strip, bottom_strip, corner_strip) = grid_resize_strips();
    overlay.add_overlay(&bottom_strip);
    overlay.add_overlay(&right_strip);
    overlay.add_overlay(&corner_strip);
    right_strip.set_visible(false);
    bottom_strip.set_visible(false);
    corner_strip.set_visible(false);

    let tile = GridTile {
        id: id.to_string(),
        added: false,
        order,
        cell,
        orig: cell,
        enabled,
        orig_enabled: profile.has_enabled.then_some(enabled_value),
        fields,
        tile: button,
        label,
        overlay,
        right_strip,
        bottom_strip,
        corner_strip,
    };
    grid_update_tile_visual(&tile);
    tile
}

/// Attach the click, move and resize gestures to one tile. Indices are resolved
/// from the button itself so a delete shifts them safely.
fn wire_grid_tile(state: &Rc<GridLayoutState>, tile: &GridTile) {
    {
        let st = state.clone();
        tile.tile.connect_toggled(move |button| {
            if st.programmatic.get() {
                return;
            }
            let Some(index) = grid_index_of(&st, button) else {
                return;
            };
            if button.is_active() {
                grid_select(&st, index);
            } else if *st.selected.borrow() == Some(index) {
                st.programmatic.set(true);
                button.set_active(true);
                st.programmatic.set(false);
            }
        });
    }
    if let Some(switch) = &tile.enabled {
        let st = state.clone();
        let button = tile.tile.clone();
        let label = tile.label.clone();
        let id = tile.id.clone();
        switch.connect_active_notify(move |switch| {
            let active = switch.is_active();
            let name = widget_display_name(&id);
            if active {
                label.set_text(&name);
                button.set_opacity(1.0);
            } else {
                label.set_text(&format!("{name} (off)"));
                button.set_opacity(0.4);
            }
            st.notify();
        });
    }
    wire_grid_move(state, tile);
    wire_grid_resize(state, tile, &tile.right_strip, true, false);
    wire_grid_resize(state, tile, &tile.bottom_strip, false, true);
    wire_grid_resize(state, tile, &tile.corner_strip, true, true);
}

fn wire_grid_move(state: &Rc<GridLayoutState>, tile: &GridTile) {
    let drag = gtk::GestureDrag::new();
    tile.tile.add_controller(drag.clone());
    let st = state.clone();
    let button = tile.tile.clone();
    drag.connect_drag_begin(move |_, _, _| {
        let Some(index) = grid_index_of(&st, &button) else {
            return;
        };
        *st.drag_index.borrow_mut() = Some(index);
        *st.size_index.borrow_mut() = None;
        st.cancelled.set(false);
        let width = button.width().max(1) as f64;
        let height = button.height().max(1) as f64;
        {
            let tiles = st.tiles.borrow();
            *st.pinned.borrow_mut() = tiles.iter().map(|tile| tile.cell).collect();
            if let Some(tile) = tiles.get(index) {
                let w = tile.cell.w.max(1) as f64;
                let h = tile.cell.h.max(1) as f64;
                st.steps
                    .set(((width + GRID_GAP as f64) / w, (height + GRID_GAP as f64) / h));
            }
        }
        grid_select(&st, index);
    });
    let st = state.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        if st.cancelled.get() {
            return;
        }
        let Some(index) = *st.drag_index.borrow() else {
            return;
        };
        let Some(base) = st.pinned.borrow().get(index).copied() else {
            return;
        };
        let (col_step, row_step) = st.steps.get();
        let columns = st.profile.columns;
        let col = (base.col as f64 + (dx / col_step).round())
            .clamp(0.0, (columns - base.w).max(0) as f64) as i64;
        let row = (base.row as f64 + (dy / row_step).round()).max(0.0) as i64;
        {
            let mut tiles = st.tiles.borrow_mut();
            if let Some(tile) = tiles.get_mut(index) {
                tile.cell.col = col;
                tile.cell.row = row;
            }
        }
        grid_repack(&st, Some(index), false);
        grid_reattach(&st);
        grid_update_size_caption(&st);
    });
    let st = state.clone();
    drag.connect_drag_end(move |_, _, _| {
        if st.cancelled.get() {
            grid_restore_pinned(&st);
        } else {
            grid_repack(&st, None, true);
        }
        grid_reattach(&st);
        grid_update_size_caption(&st);
        *st.drag_index.borrow_mut() = None;
        st.notify();
    });
}

fn wire_grid_resize(
    state: &Rc<GridLayoutState>,
    tile: &GridTile,
    strip: &gtk::Box,
    horizontal: bool,
    vertical: bool,
) {
    let drag = gtk::GestureDrag::new();
    strip.add_controller(drag.clone());
    let st = state.clone();
    let button = tile.tile.clone();
    drag.connect_drag_begin(move |_, _, _| {
        let Some(index) = grid_index_of(&st, &button) else {
            return;
        };
        if *st.selected.borrow() != Some(index) {
            return;
        }
        *st.size_index.borrow_mut() = Some(index);
        *st.drag_index.borrow_mut() = None;
        st.cancelled.set(false);
        let width = button.width().max(1) as f64;
        let height = button.height().max(1) as f64;
        {
            let tiles = st.tiles.borrow();
            *st.pinned.borrow_mut() = tiles.iter().map(|tile| tile.cell).collect();
            if let Some(tile) = tiles.get(index) {
                let w = tile.cell.w.max(1) as f64;
                let h = tile.cell.h.max(1) as f64;
                st.steps
                    .set(((width + GRID_GAP as f64) / w, (height + GRID_GAP as f64) / h));
            }
        }
    });
    let st = state.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        if st.cancelled.get() {
            return;
        }
        let Some(index) = *st.size_index.borrow() else {
            return;
        };
        let Some(base) = st.pinned.borrow().get(index).copied() else {
            return;
        };
        let (col_step, row_step) = st.steps.get();
        let columns = st.profile.columns;
        let last = match st.tiles.borrow().get(index) {
            Some(tile) => tile.cell,
            None => return,
        };
        let mut w = base.w;
        let mut h = base.h;
        if horizontal {
            w = (base.w as f64 + (dx / col_step).round())
                .clamp(1.0, (columns - base.col).max(1) as f64) as i64;
        }
        if vertical {
            h = (base.h as f64 + (dy / row_step).round()).max(1.0) as i64;
        }
        {
            let mut tiles = st.tiles.borrow_mut();
            if let Some(tile) = tiles.get_mut(index) {
                tile.cell.w = w;
                tile.cell.h = h;
            }
        }
        if !grid_repack(&st, Some(index), false) {
            {
                let mut tiles = st.tiles.borrow_mut();
                if let Some(tile) = tiles.get_mut(index) {
                    tile.cell = last;
                }
            }
            grid_repack(&st, Some(index), false);
        }
        grid_reattach(&st);
        grid_update_size_caption(&st);
    });
    let st = state.clone();
    drag.connect_drag_end(move |_, _, _| {
        *st.size_index.borrow_mut() = None;
        if st.cancelled.get() {
            grid_restore_pinned(&st);
        }
        grid_repack(&st, None, false);
        grid_reattach(&st);
        grid_update_size_caption(&st);
        st.notify();
    });
}

fn grid_select(state: &Rc<GridLayoutState>, index: usize) {
    state.programmatic.set(true);
    {
        let tiles = state.tiles.borrow();
        for (i, tile) in tiles.iter().enumerate() {
            tile.tile.set_active(i == index);
        }
    }
    state.programmatic.set(false);
    *state.selected.borrow_mut() = Some(index);
    grid_refresh_details(state);
}

fn grid_restore_pinned(state: &Rc<GridLayoutState>) {
    let pinned = state.pinned.borrow().clone();
    let mut tiles = state.tiles.borrow_mut();
    for (tile, cell) in tiles.iter_mut().zip(pinned.iter()) {
        tile.cell = *cell;
    }
}

fn grid_cancel(state: &Rc<GridLayoutState>) {
    if state.drag_index.borrow().is_none() && state.size_index.borrow().is_none() {
        return;
    }
    state.cancelled.set(true);
    grid_restore_pinned(state);
    grid_repack(state, None, false);
    grid_reattach(state);
}

/// Recompute the committed cells of every non-footer tile. `interacting` is
/// forced first in the placement order. Returns false when the layout would
/// exceed `max_rows`, in which case no cell is changed.
fn grid_repack(state: &Rc<GridLayoutState>, interacting: Option<usize>, gravity: bool) -> bool {
    let (indices, order, inputs) = {
        let tiles = state.tiles.borrow();
        let indices: Vec<usize> = (0..tiles.len())
            .filter(|&i| !(state.profile.has_footer && grid_tile_is_footer(&tiles[i])))
            .collect();
        let mut order = indices.clone();
        order.sort_by_key(|&i| (tiles[i].order, i));
        if let Some(k) = interacting {
            if indices.contains(&k) {
                order.retain(|&i| i != k);
                order.insert(0, k);
            }
        }
        let inputs: Vec<TileInput> = indices
            .iter()
            .map(|&i| TileInput {
                cell: Some(tiles[i].cell),
                w: tiles[i].cell.w,
                h: tiles[i].cell.h,
            })
            .collect();
        let order_local: Vec<usize> = order
            .iter()
            .map(|i| indices.iter().position(|x| x == i).unwrap_or(0))
            .collect();
        (indices, order_local, inputs)
    };
    let Some(cells) = repack(
        &inputs,
        &order,
        state.profile.columns,
        state.profile.max_rows,
        gravity,
    ) else {
        return false;
    };
    let mut tiles = state.tiles.borrow_mut();
    for (position, &index) in indices.iter().enumerate() {
        tiles[index].cell = cells[position];
    }
    true
}

fn grid_reattach(state: &Rc<GridLayoutState>) {
    while let Some(child) = state.canvas.first_child() {
        state.canvas.remove(&child);
    }
    if let Some(flow) = &state.footer_flow {
        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }
    }
    let tiles = state.tiles.borrow();
    let mut rows = 1i64;
    for tile in tiles.iter() {
        if state.profile.has_footer && grid_tile_is_footer(tile) {
            if let Some(flow) = &state.footer_flow {
                flow.append(&tile.overlay);
            }
            continue;
        }
        state.canvas.attach(
            &tile.overlay,
            tile.cell.col as i32,
            tile.cell.row as i32,
            tile.cell.w as i32,
            tile.cell.h as i32,
        );
        rows = rows.max(tile.cell.row + tile.cell.h);
    }
    drop(tiles);
    state.canvas.set_size_request(-1, rows as i32 * GRID_ROW_UNIT);
}

fn grid_update_size_caption(state: &Rc<GridLayoutState>) {
    let text = state
        .selected
        .borrow()
        .and_then(|index| {
            state
                .tiles
                .borrow()
                .get(index)
                .map(|tile| format!("Size: {} x {}", tile.cell.w, tile.cell.h))
        })
        .unwrap_or_default();
    state.size_caption.set_text(&text);
}

fn grid_update_strips(state: &Rc<GridLayoutState>) {
    let selected = *state.selected.borrow();
    let tiles = state.tiles.borrow();
    for (index, tile) in tiles.iter().enumerate() {
        let show = selected == Some(index)
            && !(state.profile.has_footer && grid_tile_is_footer(tile));
        tile.right_strip.set_visible(show);
        tile.bottom_strip.set_visible(show);
        tile.corner_strip.set_visible(show);
    }
}

fn grid_resize_step(state: &Rc<GridLayoutState>, dw: i64, dh: i64) {
    let Some(index) = *state.selected.borrow() else {
        return;
    };
    let columns = state.profile.columns;
    let last = match state.tiles.borrow().get(index) {
        Some(tile) => tile.cell,
        None => return,
    };
    {
        let mut tiles = state.tiles.borrow_mut();
        if let Some(tile) = tiles.get_mut(index) {
            tile.cell.w = (tile.cell.w + dw).clamp(1, (columns - tile.cell.col).max(1));
            tile.cell.h = (tile.cell.h + dh).max(1);
        }
    }
    if !grid_repack(state, Some(index), false) {
        {
            let mut tiles = state.tiles.borrow_mut();
            if let Some(tile) = tiles.get_mut(index) {
                tile.cell = last;
            }
        }
        grid_repack(state, Some(index), false);
    }
    grid_reattach(state);
    grid_update_size_caption(state);
    state.notify();
}

fn grid_delete(state: &Rc<GridLayoutState>) {
    let Some(index) = *state.selected.borrow() else {
        return;
    };
    {
        let mut tiles = state.tiles.borrow_mut();
        if index >= tiles.len() {
            return;
        }
        tiles.remove(index);
    }
    *state.selected.borrow_mut() = None;
    grid_repack(state, None, true);
    grid_reattach(state);
    grid_refresh_adder(state);
    grid_refresh_details(state);
    state.notify();
}

fn grid_refresh_adder(state: &Rc<GridLayoutState>) {
    while let Some(child) = state.adder_slot.first_child() {
        state.adder_slot.remove(&child);
    }
    let used: Vec<String> = state
        .tiles
        .borrow()
        .iter()
        .map(|tile| tile.id.clone())
        .collect();
    let available: Vec<String> = state
        .profile
        .registry
        .iter()
        .filter(|id| !used.contains(&id.to_string()))
        .map(|id| id.to_string())
        .collect();
    let add_state = state.clone();
    let on_add: Rc<dyn Fn(&str)> = Rc::new(move |id| grid_add_tile(&add_state, id));
    state
        .adder_slot
        .append(&build_grid_adder(state.advanced, &available, on_add));
}

fn grid_scaffold_value(profile: GridProfile, order: i64, cell: CellRect) -> String {
    let mut parts: Vec<String> = Vec::new();
    if profile.has_enabled {
        parts.push("enabled = true;".to_string());
    }
    parts.push(format!("_order = {};", order));
    parts.push(format!(
        "col = {}; row = {}; w = {}; h = {};",
        cell.col, cell.row, cell.w, cell.h
    ));
    format!("{{ {} }}", parts.join(" "))
}

fn grid_add_tile(state: &Rc<GridLayoutState>, id: &str) {
    if state.tiles.borrow().iter().any(|tile| tile.id == id) {
        return;
    }
    let profile = state.profile;
    let order = state
        .tiles
        .borrow()
        .iter()
        .map(|tile| tile.order)
        .max()
        .unwrap_or(0)
        + 10;
    let cell = {
        let tiles = state.tiles.borrow();
        let taken: Vec<CellRect> = tiles
            .iter()
            .filter(|tile| !(profile.has_footer && grid_tile_is_footer(tile)))
            .map(|tile| tile.cell)
            .collect();
        grid_first_fit(&taken, profile.columns, 1, 1)
    };
    let value = grid_scaffold_value(profile, order, cell);
    let mut tile = make_grid_tile(id, &value, profile, &BuildContext::default());
    tile.added = true;
    wire_grid_tile(state, &tile);
    let index = {
        let mut tiles = state.tiles.borrow_mut();
        tiles.push(tile);
        tiles.len() - 1
    };
    grid_repack(state, None, false);
    grid_reattach(state);
    grid_refresh_adder(state);
    grid_select(state, index);
    state.notify();
}

fn grid_refresh_details(state: &Rc<GridLayoutState>) {
    while let Some(child) = state.details.first_child() {
        state.details.remove(&child);
    }
    let selected = *state.selected.borrow();
    let tiles = state.tiles.borrow();
    let Some(tile) = selected.and_then(|index| tiles.get(index)) else {
        drop(tiles);
        let hint = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .label("Select a widget to edit it.")
            .css_classes(["dim-label"])
            .build();
        state.details.append(&hint);
        return;
    };
    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label(widget_display_name(&tile.id))
        .css_classes(["heading"])
        .build();
    state.details.append(&heading);
    let caption = gtk::Label::builder()
        .xalign(0.0)
        .label(tile.id.clone())
        .css_classes(["dim-label"])
        .build();
    state.details.append(&caption);
    if let Some(switch) = &tile.enabled {
        state.details.append(&dim_line("Enabled", switch));
    }
    for (record, editor) in &tile.fields {
        let name = record.segments.last().cloned().unwrap_or_default();
        let root = editor.root();
        state.details.append(&dim_line(&field_caption(&name), &root));
    }
    let size_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    size_line.append(&state.size_caption);
    let shrink = gtk::Button::from_icon_name("list-remove-symbolic");
    shrink.add_css_class("flat");
    shrink.set_tooltip_text(Some("Shrink by one cell"));
    let grow = gtk::Button::from_icon_name("list-add-symbolic");
    grow.add_css_class("flat");
    grow.set_tooltip_text(Some("Grow by one cell"));
    size_line.append(&shrink);
    size_line.append(&grow);
    state.details.append(&size_line);
    let delete = gtk::Button::with_label("Delete widget");
    delete.add_css_class("destructive-action");
    state.details.append(&delete);
    {
        let st = state.clone();
        shrink.connect_clicked(move |_| grid_resize_step(&st, -1, 0));
    }
    {
        let st = state.clone();
        grow.connect_clicked(move |_| grid_resize_step(&st, 1, 0));
    }
    {
        let st = state.clone();
        delete.connect_clicked(move |_| grid_delete(&st));
    }
    drop(tiles);
    grid_update_size_caption(state);
    grid_update_strips(state);
}

/// Serialize one tile's fields, placement and `_order`, keeping every field the
/// tile had. `enabled` is written only for profiles that carry it.
fn grid_tile_fields_nix(
    id: &str,
    order: i64,
    cell: CellRect,
    enabled: Option<bool>,
    fields: &[(String, String)],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (name, value) in fields {
        parts.push(format!("{} = {};", attr_key(name), value));
    }
    if let Some(active) = enabled {
        parts.push(format!("enabled = {};", active));
    }
    parts.push(format!("_order = {};", order));
    parts.push(format!(
        "col = {}; row = {}; w = {}; h = {};",
        cell.col, cell.row, cell.w, cell.h
    ));
    format!("{} = {{ {} }};", attr_key(id), parts.join(" "))
}

fn grid_tile_nix(tile: &GridTile, profile: GridProfile) -> String {
    let fields: Vec<(String, String)> = tile
        .fields
        .iter()
        .map(|(record, editor)| {
            (
                record.segments.last().cloned().unwrap_or_default(),
                editor.to_nix(),
            )
        })
        .collect();
    let enabled = if profile.has_enabled {
        Some(
            tile.enabled
                .as_ref()
                .map(|switch| switch.is_active())
                .unwrap_or(true),
        )
    } else {
        None
    };
    grid_tile_fields_nix(&tile.id, tile.order, tile.cell, enabled, &fields)
}

fn grid_nix(state: &GridLayoutState) -> String {
    let mut parts: Vec<String> = {
        let tiles = state.tiles.borrow();
        tiles
            .iter()
            .map(|tile| grid_tile_nix(tile, state.profile))
            .collect()
    };
    for (record, editor) in &state.extras {
        let name = record.segments.last().cloned().unwrap_or_default();
        parts.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
    }
    format!("{{ {} }}", parts.join(" "))
}

/// Scan a submodule value's direct children into offset records, dropping
/// hidden controls while the Advanced toggle is off.
pub(crate) fn scan_children(record: &Record, advanced: bool) -> Vec<Record> {
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
    let name = record.segments.last().cloned().unwrap_or_default();
    let label = record
        .spec
        .label
        .clone()
        .unwrap_or_else(|| field_caption(&name));
    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label(label)
        .css_classes(["dim-label"])
        .build();
    if let Some(help) = record
        .spec
        .description
        .as_deref()
        .or_else(|| field_help(&name))
    {
        heading.set_tooltip_text(Some(help));
    }
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
        | WidgetKind::Dock
        | WidgetKind::GridConfiguration => expr_literal(value),
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
        assert_eq!(attr_key("daily "), "\"daily \"");
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
    fn header_expand_follows_cell_editor_kind() {
        let row = scan::TableRowScan {
            key: "youtube".into(),
            fields: vec![
                ("enable".into(), "true".into()),
                ("name".into(), "\"YouTube\"".into()),
                ("icon".into(), "null".into()),
            ],
            spec: OptionSpec::default(),
        };
        assert!(!column_expands(Some(&row), "enable"));
        assert!(column_expands(Some(&row), "name"));
        assert!(column_expands(Some(&row), "icon"));
        assert!(column_expands(None, "url"));
    }

    #[test]
    fn craftapps_rows_lock_names_and_edit_first_cell() {
        assert!(table_row_identity_locked(false, false, true));
        assert!(!table_row_identity_locked(false, false, false));
        assert!(!table_row_identity_locked(false, true, true));
        assert!(!table_row_identity_locked(true, false, true));
        assert!(table_first_cell_static(true, true, false));
        assert!(!table_first_cell_static(false, true, false));
        assert!(!table_first_cell_static(true, false, false));
        assert!(!table_first_cell_static(true, true, true));
    }

    #[test]
    fn cell_widget_kind_prefers_value_then_type_hint() {
        let bool_hint = Some(value::ValueKind::Bool);
        assert_eq!(cell_widget_kind("null", bool_hint), WidgetKind::Switch);
        assert_eq!(
            cell_widget_kind("null", Some(value::ValueKind::Int)),
            WidgetKind::Spin
        );
        assert_eq!(
            cell_widget_kind("null", Some(value::ValueKind::Float)),
            WidgetKind::Spin
        );
        assert_eq!(cell_widget_kind("null", None), WidgetKind::Text);
        assert_eq!(cell_widget_kind("\"x\"", bool_hint), WidgetKind::Text);
        assert_eq!(cell_widget_kind("true", None), WidgetKind::Switch);
        assert_eq!(
            cell_widget_kind("42", Some(value::ValueKind::String)),
            WidgetKind::Spin
        );
    }

    #[test]
    fn empty_cell_literals_follow_the_column_hint() {
        assert_eq!(
            empty_cell_literal(Some(value::ValueKind::Bool)),
            "false".to_string()
        );
        assert_eq!(
            empty_cell_literal(Some(value::ValueKind::Int)),
            "0".to_string()
        );
        assert_eq!(
            empty_cell_literal(Some(value::ValueKind::Float)),
            "0".to_string()
        );
        assert_eq!(
            empty_cell_literal(Some(value::ValueKind::String)),
            "\"\"".to_string()
        );
        assert_eq!(empty_cell_literal(None), "null".to_string());
    }

    #[test]
    fn column_type_tokens_map_to_value_kinds() {
        assert_eq!(
            column_type_hint("bool"),
            Some(value::ValueKind::Bool)
        );
        assert_eq!(
            column_type_hint("Boolean"),
            Some(value::ValueKind::Bool)
        );
        assert_eq!(column_type_hint("int"), Some(value::ValueKind::Int));
        assert_eq!(
            column_type_hint("INTEGER"),
            Some(value::ValueKind::Int)
        );
        assert_eq!(column_type_hint("float"), Some(value::ValueKind::Float));
        assert_eq!(column_type_hint("str"), Some(value::ValueKind::String));
        assert_eq!(
            column_type_hint("string"),
            Some(value::ValueKind::String)
        );
        assert_eq!(column_type_hint("raw"), Some(value::ValueKind::String));
        assert_eq!(column_type_hint("nope"), None);
    }

    #[test]
    fn column_type_hints_align_positionally_with_columns() {
        let columns = ["enable".to_string(), "value".to_string()];
        let declared = ["bool".to_string(), "str".to_string()];
        assert_eq!(
            column_type_hints(Some(&declared), &columns, None),
            vec![
                Some(value::ValueKind::Bool),
                Some(value::ValueKind::String)
            ]
        );
        let short = ["bool".to_string()];
        assert_eq!(
            column_type_hints(Some(&short), &columns, None),
            vec![Some(value::ValueKind::Bool), None]
        );
        let row = scan::TableRowScan {
            key: "llm".into(),
            fields: vec![
                ("enable".into(), "true".into()),
                ("value".into(), "\"null\"".into()),
            ],
            spec: OptionSpec::default(),
        };
        assert_eq!(
            column_type_hints(None, &columns, Some(&row)),
            vec![
                Some(value::ValueKind::Bool),
                Some(value::ValueKind::String)
            ]
        );
        assert_eq!(column_type_hints(None, &columns, None), vec![None, None]);
    }

    #[test]
    fn bundled_schemes_populated_when_env_provided() {
        if option_env!("TINTED_SCHEMES_DIR").is_none() {
            return;
        }
        assert!(
            schemes::SCHEMES.len() > 200,
            "expected base16 schemes, got {}",
            schemes::SCHEMES.len()
        );
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

    fn template_file(relative: &str) -> std::path::PathBuf {
        match std::env::var("OJUNY_TEMPLATE") {
            Ok(dir) => std::path::PathBuf::from(dir).join(relative),
            Err(_) => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../ojuny-template")
                .join(relative),
        }
    }

    fn scans(value: &str) -> Vec<TileEmit> {
        scan::scan_table(value, false)
            .iter()
            .map(tile_emit_from_scan)
            .collect()
    }

    #[test]
    fn tiles_nix_round_trips_bar_left_list() {
        let source = "{ workspaceSwitcher = { enabled = true; _order = 10; showOccupiedWorkspacesOnly = true; showWorkspaceIndex = true; showWorkspacePadding = true; workspacePaddingCount = 5; dwlShowAllTags = false; }; layout = { enabled = true; _order = 20; }; launcherButton = { enabled = true; _order = 30; }; focusedWindow = { enabled = true; _order = 40; }; }";
        let entries = scans(source);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].orig_order, Some(10));
        assert_eq!(tiles_nix(&entries), source);
        assert_eq!(tiles_nix(&scans(&tiles_nix(&entries))), source);
    }

    #[test]
    fn assigns_orders_from_the_original_pool() {
        assert_eq!(assign_tile_orders(&[Some(20), Some(10)]), ["10", "20"]);
        assert_eq!(
            assign_tile_orders(&[Some(10), None, Some(20)]),
            ["10", "20", "30"]
        );
        assert_eq!(
            assign_tile_orders(&[Some(10), Some(20), None]),
            ["10", "20", "30"]
        );
        assert_eq!(assign_tile_orders(&[None, None]), ["10", "20"]);
        assert_eq!(assign_tile_orders(&[Some(30)]), ["30"]);
        assert_eq!(
            assign_tile_orders(&[Some(10), Some(500), None]),
            ["10", "500", "510"]
        );
    }

    #[test]
    fn center_tiles_nix_round_trips_string_list() {
        let source = r#"[ "music" "clock" "weather" ]"#;
        let ids = parse_string_list(source);
        assert_eq!(ids, ["music", "clock", "weather"]);
        assert_eq!(center_tiles_nix(&ids), source);
        assert_eq!(center_tiles_nix(&[]), "[ ]");
        assert!(parse_string_list("{ }").is_empty());
    }

    #[test]
    fn tiles_nix_emits_dock_keys_in_order() {
        let source = "{ dockLauncher = { enabled = true; _order = 10; }; appsDock = { enabled = true; _order = 20; }; dockTrash = { enabled = true; _order = 30; }; }";
        assert_eq!(tiles_nix(&scans(source)), source);
    }

    fn rect(col: i64, row: i64, w: i64, h: i64) -> CellRect {
        CellRect { col, row, w, h }
    }

    #[test]
    fn repack_settles_committed_tiles_downward() {
        let tiles = vec![
            TileInput { cell: Some(rect(0, 0, 2, 1)), w: 2, h: 1 },
            TileInput { cell: Some(rect(0, 0, 1, 1)), w: 1, h: 1 },
        ];
        let cells = repack(&tiles, &[0, 1], 6, 64, false).unwrap();
        assert_eq!(cells[0], rect(0, 0, 2, 1));
        assert_eq!(cells[1], rect(0, 1, 1, 1));
    }

    #[test]
    fn repack_first_fits_unplaced_tiles() {
        let tiles = vec![
            TileInput { cell: Some(rect(0, 0, 3, 1)), w: 3, h: 1 },
            TileInput { cell: None, w: 1, h: 1 },
        ];
        let cells = repack(&tiles, &[0, 1], 6, 64, false).unwrap();
        assert_eq!(cells[1], rect(3, 0, 1, 1));
    }

    #[test]
    fn repack_clamps_width_to_columns() {
        let tiles = vec![TileInput { cell: Some(rect(0, 0, 99, 1)), w: 99, h: 1 }];
        let cells = repack(&tiles, &[0], 4, 64, false).unwrap();
        assert_eq!(cells[0].w, 4);
    }

    #[test]
    fn repack_reports_overflow_as_none() {
        let tiles = vec![
            TileInput { cell: Some(rect(0, 0, 1, 3)), w: 1, h: 3 },
            TileInput { cell: Some(rect(0, 0, 1, 1)), w: 1, h: 1 },
        ];
        assert!(repack(&tiles, &[0, 1], 1, 3, false).is_none());
    }

    #[test]
    fn compact_pulls_cells_up_to_the_top() {
        let mut cells = vec![rect(0, 5, 1, 1), rect(1, 5, 1, 1)];
        grid_compact(&mut cells);
        assert_eq!(cells[0].row, 0);
        assert_eq!(cells[1].row, 0);
    }

    #[test]
    fn compact_stops_at_a_blocker() {
        let mut cells = vec![rect(0, 0, 2, 1), rect(0, 3, 1, 1)];
        grid_compact(&mut cells);
        assert_eq!(cells[1].row, 1);
    }

    #[test]
    fn first_fit_places_below_the_stack_when_full() {
        let taken = vec![rect(0, 0, 1, 1)];
        assert_eq!(grid_first_fit(&taken, 1, 1, 1), rect(0, 1, 1, 1));
    }

    #[test]
    fn grid_profiles_resolve_by_path() {
        let cc = GridProfile::for_path("programs.ojuny.dms.controlCenterWidgets").unwrap();
        assert_eq!((cc.columns, cc.has_enabled, cc.has_footer), (8, true, true));
        let dash = GridProfile::for_path("desktop.ojuny.dms.dashCards").unwrap();
        assert_eq!((dash.columns, dash.has_enabled, dash.has_footer), (6, false, false));
        let weather = GridProfile::for_path("programs.ojuny.dms.weather.widgets").unwrap();
        assert_eq!((weather.columns, weather.has_enabled), (4, false));
        assert!(GridProfile::for_path("a.b").is_none());
    }

    #[test]
    fn grid_tile_nix_writes_fields_then_placement() {
        let fields = vec![("footer".to_string(), "true".to_string())];
        let out = grid_tile_fields_nix("nightMode", 1000, rect(2, 1, 2, 1), Some(true), &fields);
        assert_eq!(
            out,
            "nightMode = { footer = true; enabled = true; _order = 1000; col = 2; row = 1; w = 2; h = 1; };"
        );
    }

    #[test]
    fn grid_tile_nix_omits_enabled_when_absent() {
        let out = grid_tile_fields_nix("clock", 10, rect(0, 0, 3, 2), None, &[]);
        assert_eq!(out, "clock = { _order = 10; col = 0; row = 0; w = 3; h = 2; };");
    }

    #[test]
    fn grid_tile_nix_quotes_awkward_field_keys() {
        let fields = vec![("daily ".to_string(), "true".to_string())];
        let out = grid_tile_fields_nix("daily ", 1000, rect(0, 0, 1, 1), None, &fields);
        assert_eq!(
            out,
            "\"daily \" = { \"daily \" = true; _order = 1000; col = 0; row = 0; w = 1; h = 1; };"
        );
    }

    #[test]
    fn custom_widget_round_trips() {
        let mut entries = scans("{ layout = { enabled = true; _order = 10; }; }");
        entries.push(TileEmit {
            id: "myThing".to_string(),
            enabled: true,
            field_lines: vec!["foo = \"bar\";".to_string()],
            orig_order: None,
        });
        let out = tiles_nix(&entries);
        assert_eq!(
            out,
            "{ layout = { enabled = true; _order = 10; }; myThing = { enabled = true; _order = 20; foo = \"bar\"; }; }"
        );
        assert_eq!(tiles_nix(&scans(&out)), out);
    }

    #[test]
    fn scans_widget_presets_from_template() {
        use rowan::ast::AstNode;
        let bar_path = template_file("modules/04-gui/01-bar.nix");
        if !bar_path.exists() {
            eprintln!("skipping: sibling ojuny-template checkout not available");
            return;
        }
        let bar = std::fs::read_to_string(bar_path).expect("01-bar.nix is readable");
        let Ok(root) = rnix::Root::parse(&bar).ok() else {
            panic!("01-bar.nix parses");
        };
        let presets = scan::find_widget_presets(&bar, root.syntax());
        assert_eq!(presets.len(), 32, "bar preset count");
        assert_eq!(
            presets.iter().filter(|(preset, _)| preset.has_settings).count(),
            20,
            "bar presets with settings"
        );
        let fields: usize = presets.iter().map(|(_, specs)| specs.len()).sum();
        assert!(fields >= 40, "bar preset field specs: {fields}");

        let dock = std::fs::read_to_string(template_file("modules/04-gui/02-dock.nix"))
            .expect("02-dock.nix is readable");
        let Ok(root) = rnix::Root::parse(&dock).ok() else {
            panic!("02-dock.nix parses");
        };
        assert_eq!(scan::find_widget_presets(&dock, root.syntax()).len(), 3);
    }

    fn grid_cells(grid: &gtk::Grid) -> Vec<(i32, i32)> {
        let Some(manager) = grid.layout_manager() else {
            return Vec::new();
        };
        let children = grid.observe_children();
        let mut out = Vec::new();
        for index in 0..children.n_items() {
            let Some(object) = children.item(index) else {
                continue;
            };
            let Ok(child) = object.downcast::<gtk::Widget>() else {
                continue;
            };
            let Ok(cell) = manager.layout_child(&child).downcast::<gtk::GridLayoutChild>() else {
                continue;
            };
            out.push((cell.column(), cell.row()));
        }
        out
    }

    fn grid_child(grid: &gtk::Grid, column: i32, row: i32) -> Option<gtk::Widget> {
        let manager = grid.layout_manager()?;
        let children = grid.observe_children();
        for index in 0..children.n_items() {
            let child = children.item(index)?.downcast::<gtk::Widget>().ok()?;
            let cell = manager
                .layout_child(&child)
                .downcast::<gtk::GridLayoutChild>()
                .ok()?;
            if cell.column() == column && cell.row() == row {
                return Some(child);
            }
        }
        None
    }

    fn table_grid(record: &Record) -> (Editor, gtk::Grid) {
        let ctx = BuildContext::default();
        let editor = build_table(record, &ctx, false);
        let root = editor.root().downcast::<gtk::Box>().unwrap();
        let grid = root.first_child().unwrap().downcast::<gtk::Grid>().unwrap();
        (editor, grid)
    }

    fn table_record(value: &str, columns: &[&str], types: &[&str]) -> Record {
        Record {
            value: value.to_string(),
            spec: OptionSpec {
                widget: Some(WidgetKind::TableAttrset),
                columns: Some(columns.iter().map(|name| name.to_string()).collect()),
                column_types: Some(types.iter().map(|name| name.to_string()).collect()),
                ..OptionSpec::default()
            },
            ..Record::default()
        }
    }

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn table_grid_aligns_columns_and_reindexes_on_remove() {
        if gtk::init().is_err() {
            eprintln!("skipping table grid runtime check: no display");
            return;
        }
        let record = table_record(
            "{ a = { enable = true; value = \"x\"; }; b = { enable = false; value = \"a much longer value\"; }; }",
            &["enable", "value"],
            &["bool", "str"],
        );
        let (editor, grid) = table_grid(&record);

        // Header captions sit on the same grid columns as every row's cells.
        let cells = grid_cells(&grid);
        assert!(cells.contains(&(1, 0)) && cells.contains(&(2, 0)), "{cells:?}");
        assert!(!cells.contains(&(0, 0)), "no caption over the key column: {cells:?}");
        for row in 1..=2 {
            for column in 0..4 {
                assert!(cells.contains(&(column, row)), "row {row} column {column}: {cells:?}");
            }
        }
        let caption_enable = grid_child(&grid, 1, 0).unwrap();
        let caption_value = grid_child(&grid, 2, 0).unwrap();
        assert!(!caption_enable.hexpands());
        assert!(caption_value.hexpands());

        let enabled = grid_child(&grid, 1, 1)
            .unwrap()
            .downcast::<gtk::Switch>()
            .unwrap();
        assert!(enabled.is_active());
        assert!(!enabled.hexpands());
        let disabled = grid_child(&grid, 1, 2)
            .unwrap()
            .downcast::<gtk::Switch>()
            .unwrap();
        assert!(!disabled.is_active());
        let value = grid_child(&grid, 2, 1)
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        assert!(value.hexpands(), "text cells absorb the extra width");

        // An added row opens with a switch and a text entry, not free text.
        let root = editor.root().downcast::<gtk::Box>().unwrap();
        root.last_child()
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        assert!(grid_child(&grid, 0, 3).is_some(), "row 3 key slot");
        let added = grid_child(&grid, 1, 3)
            .unwrap()
            .downcast::<gtk::Switch>()
            .unwrap();
        assert!(!added.is_active());
        let added_value = grid_child(&grid, 2, 3)
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        assert_eq!(added_value.text(), "");

        grid_child(&grid, 0, 3)
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap()
            .set_text("smoke");
        added.set_active(true);
        let on = editor.to_nix();
        assert!(
            on.contains("smoke = { enable = true; value = \"\"; };"),
            "{on}"
        );
        assert!(!on.contains("enable = \"true\""), "{on}");
        std::fs::write(std::env::temp_dir().join("i01-add-row.nix"), &on).unwrap();

        // Removing a row pulls the rows below it up, so slots stay dense.
        grid_child(&grid, 3, 1)
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        let cells = grid_cells(&grid);
        assert!(!cells.contains(&(0, 3)), "rows shifted up: {cells:?}");
        assert!(cells.contains(&(0, 1)) && cells.contains(&(0, 2)), "{cells:?}");
        let after = editor.to_nix();
        assert!(!after.contains("a = {"), "{after}");
        assert!(after.contains("b = {") && after.contains("smoke = {"), "{after}");

        // Saved off, then re-scanned: the cell is still a switch.
        added.set_active(false);
        let off = editor.to_nix();
        assert!(off.contains("enable = false;"), "{off}");
        let reloaded = Record {
            value: off,
            spec: record.spec.clone(),
            ..Record::default()
        };
        let (_second, second_grid) = table_grid(&reloaded);
        let reopened = grid_child(&second_grid, 1, 2).unwrap();
        assert!(
            reopened.downcast::<gtk::Switch>().is_ok(),
            "a row saved with enable = false re-opens as a switch"
        );
        assert!(grid_child(&second_grid, 1, 1)
            .unwrap()
            .downcast::<gtk::Switch>()
            .is_ok());
    }

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn table_grid_keeps_locked_labels_and_single_column_captions() {
        if gtk::init().is_err() {
            eprintln!("skipping table grid runtime check: no display");
            return;
        }
        let record = table_record(
            "{ /* @table-item label = \"PhotoCraft\"; */ photo = { enable = true; }; }",
            &["enable"],
            &["bool"],
        );
        let (_editor, grid) = table_grid(&record);
        let heading = grid_child(&grid, 0, 1)
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap();
        assert_eq!(heading.label(), "PhotoCraft");
        let caption = grid_child(&grid, 1, 0)
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap();
        assert_eq!(caption.label(), "enable");
        assert!(grid_child(&grid, 1, 1)
            .unwrap()
            .downcast::<gtk::Switch>()
            .is_ok());
        assert!(grid_child(&grid, 2, 1)
            .unwrap()
            .downcast::<gtk::Button>()
            .is_ok());
        assert!(!grid_cells(&grid).contains(&(0, 0)), "no caption over locked labels");
    }

    #[test]
    fn keybind_labels_derive_from_names_and_overrides() {
        assert_eq!(bind_display_label("", None), "New bind:");
        assert_eq!(bind_display_label("focusDown", None), "Focus Down:");
        assert_eq!(bind_display_label("mute", Some("Mute Audio")), "Mute Audio:");
        assert_eq!(bind_display_label("mute", Some("Mute Audio:")), "Mute Audio:");
        assert_eq!(bind_display_label("mute", Some("")), "Mute:");
    }

    #[test]
    fn popout_help_covers_every_field() {
        for name in ["provider", "target", "action", "checked", "when"] {
            assert!(
                menu_field_help(name).is_some(),
                "missing menu help for {name}"
            );
        }
        for name in ["dispatcher", "args", "group", "subcat"] {
            assert!(
                keybind_field_help(name).is_some(),
                "missing keybind help for {name}"
            );
        }
    }

    #[test]
    fn effective_size_swaps_on_odd_transforms() {
        assert_eq!(effective_size(1920.0, 1080.0, 0.0), (1920.0, 1080.0));
        assert_eq!(effective_size(1920.0, 1080.0, 1.0), (1080.0, 1920.0));
        assert_eq!(effective_size(1920.0, 1080.0, 2.0), (1920.0, 1080.0));
        assert_eq!(effective_size(1920.0, 1080.0, 3.0), (1080.0, 1920.0));
        assert_eq!(effective_size(1920.0, 1080.0, 5.0), (1080.0, 1920.0));
        assert_eq!(effective_size(1920.0, 1080.0, 7.0), (1080.0, 1920.0));
    }

    fn rect_overlaps(x: f64, y: f64, w: f64, h: f64, other: &(f64, f64, f64, f64)) -> bool {
        (x + w).min(other.0 + other.2) - x.max(other.0) > 0.0
            && (y + h).min(other.1 + other.3) - y.max(other.1) > 0.0
    }

    #[test]
    fn release_snaps_to_nearby_edge() {
        let others = vec![(0.0, 0.0, 1000.0, 1000.0)];
        let (x, y) = resolve_release(1005.0, 0.0, 1000.0, 1000.0, &others, 12.0);
        assert_eq!((x, y), (1000.0, 0.0));
    }

    #[test]
    fn release_pushes_out_of_overlap() {
        let others = vec![(0.0, 0.0, 1920.0, 1080.0)];
        let (x, y) = resolve_release(100.0, 100.0, 1920.0, 1080.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1920.0, 1080.0, &others[0]));
        assert!(x >= 0.0 && y >= 0.0);
    }

    #[test]
    fn release_never_returns_negative_coordinates() {
        let others = vec![(0.0, 0.0, 1920.0, 1080.0)];
        let (x, y) = resolve_release(0.0, 0.0, 1920.0, 1080.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1920.0, 1080.0, &others[0]));
        assert!(x >= 0.0 && y >= 0.0);
    }

    #[test]
    fn release_resolves_chained_overlaps() {
        let others = vec![(0.0, 0.0, 1000.0, 1000.0), (1000.0, 0.0, 1000.0, 1000.0)];
        let (x, y) = resolve_release(500.0, 500.0, 1000.0, 1000.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1000.0, 1000.0, &others[0]));
        assert!(!rect_overlaps(x, y, 1000.0, 1000.0, &others[1]));
        assert!(x >= 0.0 && y >= 0.0);
    }
}
