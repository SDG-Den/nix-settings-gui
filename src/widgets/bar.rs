use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::nix::attr_key;
use super::submodule::{append_settings_rows, build_extra_editor, scan_children};
use super::tile_list::{TileSection, WidgetTileList};
use super::tile_nix::{center_tiles_nix, tile_emit_from_tile, tiles_nix, TileEmit};
use crate::model::Record;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
pub(crate) fn build_bar(record: &Record, ctx: &BuildContext) -> Editor {
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

pub(crate) fn bar_nix(
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
        let entries: Vec<TileEmit> = left
            .tiles
            .borrow()
            .iter()
            .map(tile_emit_from_tile)
            .collect();
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
        let entries: Vec<TileEmit> = right
            .tiles
            .borrow()
            .iter()
            .map(tile_emit_from_tile)
            .collect();
        parts.push(format!("rightWidgets = {};", tiles_nix(&entries)));
    }
    for (record, editor) in extras {
        let name = record.segments.last().cloned().unwrap_or_default();
        parts.push(format!("{} = {};", attr_key(&name), editor.to_nix()));
    }
    format!("{{ {} }}", parts.join(" "))
}
pub(crate) fn build_dock(record: &Record, ctx: &BuildContext) -> Editor {
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

pub(crate) fn dock_nix(
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
