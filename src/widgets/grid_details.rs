use super::context::BuildContext;
use super::fields::dim_line;
use super::grid::{
    grid_reattach, grid_repack, grid_select, grid_tile_is_footer, grid_update_size_caption,
    grid_update_strips, make_grid_tile, GridLayoutState, GridProfile,
};
use super::grid_gestures::wire_grid_tile;
use super::grid_layout::{grid_first_fit, CellRect};
use super::names::{field_caption, widget_display_name};
use gtk::prelude::*;
use std::rc::Rc;
/// The add bar: a dropdown of registry ids not yet on the canvas, an optional
/// custom id entry in advanced mode, and an "Add widget" button. Rebuilt on
/// every change so a deleted widget reappears in the dropdown.
pub(crate) fn build_grid_adder(
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
pub(crate) fn grid_resize_step(state: &Rc<GridLayoutState>, dw: i64, dh: i64) {
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

pub(crate) fn grid_delete(state: &Rc<GridLayoutState>) {
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

pub(crate) fn grid_refresh_adder(state: &Rc<GridLayoutState>) {
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

pub(crate) fn grid_scaffold_value(profile: GridProfile, order: i64, cell: CellRect) -> String {
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

pub(crate) fn grid_add_tile(state: &Rc<GridLayoutState>, id: &str) {
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

pub(crate) fn grid_refresh_details(state: &Rc<GridLayoutState>) {
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
        state
            .details
            .append(&dim_line(&field_caption(&name), &root));
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
