use super::bar::{bar_nix, dock_nix};
use super::context::ChangeHook;
use super::fields::csv_list;
use super::grid::{grid_nix, GridLayoutState};
use super::keybinds::{connect_keybind_row, keybinds_nix, KeybindRow};
use super::menu::{connect_menu_row, menu_rows_nix, menu_rows_unchanged, MenuRow};
use super::monitors::{connect_monitor_row, monitor_row_unchanged, monitors_nix, MonitorRow};
use super::nix::{
    canonical_list, canonical_module_list, entry_text, expr_literal, number_literal, package_items,
    package_literal, parse_string_literal, path_literal, quote, serialize_modules, string_literal,
    truthy, unwrap_modifiers,
};
use super::table::{connect_table_row, table_nix, TableRow};
use super::tile_list::WidgetTileList;
use super::tile_nix::tiles_unchanged;
use crate::edit::PendingEdit;
use crate::model::Record;
use crate::value;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
/// The editing control backing one option. Each variant knows how to render
/// its current state back to Nix and whether it still holds the original value.
#[derive(Clone)]
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
    Wallpaper {
        root: gtk::Box,
        ui: Rc<RefCell<super::wallpaper::WallpaperUi>>,
        on_change: ChangeHook,
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
            Editor::Wallpaper { root, .. } => root.clone().upcast(),
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
            Editor::Slider(w) => number_literal(w.value(), w.digits() as u32),
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
            Editor::Wallpaper { ui, .. } => ui.borrow().nix_value(),
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

    /// The value a live-apply command should receive. Most controls use their
    /// Nix literal; the wallpaper picker resolves files to direct paths.
    pub fn apply_value(&self) -> String {
        match self {
            Editor::Wallpaper { ui, .. } => ui.borrow().apply_literal(),
            _ => self.to_nix(),
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
                            && tile.enabled.as_ref().map(|switch| switch.is_active())
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
            Editor::Wallpaper { on_change, .. } => {
                *on_change.borrow_mut() = Some(update.clone());
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
                extras, on_change, ..
            } => {
                *on_change.borrow_mut() = Some(update.clone());
                for (_, editor) in extras {
                    editor.connect_changed(update);
                }
            }
            Editor::Dock {
                extras, on_change, ..
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
