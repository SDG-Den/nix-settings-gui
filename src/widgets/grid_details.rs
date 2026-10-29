use super::context::BuildContext;
use super::dms_catalog::{card_defaults, card_size, widget_description, widget_icon};
use super::fields::{dim_line, dim_line_with_help};
use super::grid::{
    grid_reattach, grid_repack, grid_select, grid_set_footer, grid_set_status, grid_tile_is_footer,
    grid_update_size_caption, grid_update_strips, make_grid_tile, GridLayoutState, GridProfile,
};
use super::grid_gestures::wire_grid_tile;
use super::grid_layout::{grid_clamp_cell, grid_first_fit, CellRect};
use super::names::{field_help, widget_display_name};
use super::nix::attr_key;
use gtk::prelude::*;
use std::rc::Rc;
/// The add bar: a dropdown of registry ids not yet on the canvas, an optional
/// custom id entry in advanced mode, and an "Add widget" button. Rebuilt on
/// every change so a deleted widget reappears in the dropdown.
pub(crate) fn build_grid_adder(
    advanced: bool,
    available: &[String],
    has_footer: bool,
    on_add: Rc<dyn Fn(&str, bool)>,
) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let footer = Rc::new(std::cell::Cell::new(false));
    let button = gtk::MenuButton::builder()
        .label("Add widget")
        .halign(gtk::Align::Start)
        .build();
    let popover = gtk::Popover::new();
    popover.set_position(gtk::PositionType::Bottom);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_size_request(320, -1);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search widgets"));
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(160)
        .max_content_height(280)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    content.append(&search);
    content.append(&scroll);
    popover.set_child(Some(&content));
    button.set_popover(Some(&popover));
    bar.append(&button);

    let all: Vec<String> = available.to_vec();
    let repopulate: Rc<dyn Fn()> = {
        let list = list.clone();
        let search = search.clone();
        let all = all.clone();
        Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let needle = search.text().trim().to_lowercase();
            for id in &all {
                let label = widget_display_name(id);
                if !needle.is_empty()
                    && !label.to_lowercase().contains(&needle)
                    && !id.to_lowercase().contains(&needle)
                {
                    continue;
                }
                let row = gtk::ListBoxRow::new();
                row.set_widget_name(id);
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                line.set_margin_top(6);
                line.set_margin_bottom(6);
                line.set_margin_start(8);
                line.set_margin_end(8);
                let icon = gtk::Image::from_icon_name(widget_icon(id));
                icon.set_pixel_size(24);
                line.append(&icon);
                let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
                text.set_hexpand(true);
                let title = gtk::Label::builder().xalign(0.0).label(label).build();
                let subtitle = gtk::Label::builder()
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .label(widget_description(id))
                    .css_classes(["dim-label", "caption"])
                    .build();
                text.append(&title);
                text.append(&subtitle);
                line.append(&text);
                let add = gtk::Image::from_icon_name("list-add-symbolic");
                line.append(&add);
                row.set_child(Some(&line));
                list.append(&row);
            }
        })
    };
    popover.connect_map({
        let repopulate = repopulate.clone();
        move |_| repopulate()
    });
    search.connect_search_changed({
        let repopulate = repopulate.clone();
        move |_| repopulate()
    });
    {
        let on_add = on_add.clone();
        let popover = popover.clone();
        let footer = footer.clone();
        list.connect_row_activated(move |_, row| {
            let id = row.widget_name().to_string();
            if !id.is_empty() {
                on_add(&id, footer.get());
            }
            popover.popdown();
        });
    }
    if has_footer {
        let check = gtk::CheckButton::with_label("Footer");
        check.set_tooltip_text(Some("Add the next widget to the footer strip"));
        let footer = footer.clone();
        check.connect_toggled(move |check| footer.set(check.is_active()));
        bar.append(&check);
    }
    if advanced {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Custom id"));
        entry.set_width_chars(14);
        bar.append(&entry);
        let add = gtk::Button::with_label("Add custom");
        let entry_add = entry.clone();
        let on_add = on_add.clone();
        let footer = footer.clone();
        add.connect_clicked(move |_| {
            let id = entry_add.text().trim().to_string();
            if !id.is_empty() {
                on_add(&id, footer.get());
                entry_add.set_text("");
            }
        });
        bar.append(&add);
    }
    bar
}

pub(crate) fn grid_clear_all(state: &Rc<GridLayoutState>) {
    state.tiles.borrow_mut().clear();
    *state.selected.borrow_mut() = None;
    grid_repack(state, None, true);
    grid_reattach(state);
    grid_refresh_adder(state);
    grid_refresh_details(state);
    state.notify();
}

/// Reset every card to its catalog size and first-fit it again, keeping the
/// card's fields and unknown settings.
pub(crate) fn grid_reset_layout(state: &Rc<GridLayoutState>) {
    let profile = state.profile;
    let mut taken: Vec<CellRect> = Vec::new();
    {
        let mut tiles = state.tiles.borrow_mut();
        for tile in tiles.iter_mut() {
            if profile.has_footer && grid_tile_is_footer(tile) {
                continue;
            }
            let spec = card_size(profile.surface, &tile.id);
            let w = spec.w.clamp(spec.min_w, spec.max_w.min(profile.columns));
            let h = spec.h.clamp(spec.min_h, spec.max_h);
            let cell = grid_first_fit(&taken, profile.columns, w, h);
            taken.push(cell);
            tile.cell = cell;
            tile.cell.w = w;
            tile.cell.h = h;
            if let Some(switch) = &tile.enabled {
                switch.set_active(true);
            }
        }
    }
    grid_repack(state, None, true);
    grid_reattach(state);
    grid_update_size_caption(state);
    grid_set_status(state, "");
    state.notify();
}

pub(crate) fn grid_reset_tile(state: &Rc<GridLayoutState>, index: usize) {
    let profile = state.profile;
    let Some(tile) = state.tiles.borrow().get(index).cloned() else {
        return;
    };
    let spec = card_size(profile.surface, &tile.id);
    let previous = tile.cell;
    let w = spec.w.clamp(spec.min_w, spec.max_w.min(profile.columns));
    let h = spec.h.clamp(spec.min_h, spec.max_h);
    {
        let mut tiles = state.tiles.borrow_mut();
        if let Some(tile) = tiles.get_mut(index) {
            tile.cell.w = w;
            tile.cell.h = h;
            if let Some(switch) = &tile.enabled {
                switch.set_active(true);
            }
        }
    }
    if !grid_repack(state, Some(index), false) {
        if let Some(tile) = state.tiles.borrow_mut().get_mut(index) {
            tile.cell = previous;
        }
        grid_repack(state, Some(index), false);
        grid_set_status(state, "No room to reset this card's size.");
    } else {
        grid_set_status(state, "");
    }
    grid_reattach(state);
    grid_update_size_caption(state);
    grid_refresh_details(state);
    state.notify();
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
        grid_set_status(
            state,
            "No room to grow here — the grid is full at the bottom.",
        );
    } else {
        grid_set_status(state, "");
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
    let on_add: Rc<dyn Fn(&str, bool)> =
        Rc::new(move |id, footer| grid_add_tile(&add_state, id, footer));
    state.adder_slot.append(&build_grid_adder(
        state.advanced,
        &available,
        state.profile.has_footer,
        on_add,
    ));
}

pub(crate) fn grid_scaffold_value(
    profile: GridProfile,
    id: &str,
    order: i64,
    cell: CellRect,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if profile.has_enabled {
        parts.push("enabled = true;".to_string());
    }
    parts.push(format!("_order = {};", order));
    parts.push(format!(
        "col = {}; row = {}; w = {}; h = {};",
        cell.col, cell.row, cell.w, cell.h
    ));
    for (name, value) in card_defaults(profile.surface, id) {
        parts.push(format!("{} = {};", attr_key(name), value));
    }
    format!("{{ {} }}", parts.join(" "))
}

pub(crate) fn grid_add_tile(state: &Rc<GridLayoutState>, id: &str, footer: bool) {
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
    let spec = card_size(profile.surface, id);
    let w = spec.w.clamp(spec.min_w, spec.max_w.min(profile.columns));
    let h = spec.h.clamp(spec.min_h, spec.max_h);
    let cell = if footer {
        CellRect {
            col: 0,
            row: 0,
            w: 1,
            h: 1,
        }
    } else {
        let tiles = state.tiles.borrow();
        let taken: Vec<CellRect> = tiles
            .iter()
            .filter(|tile| !(profile.has_footer && grid_tile_is_footer(tile)))
            .map(|tile| tile.cell)
            .collect();
        grid_first_fit(&taken, profile.columns, w, h)
    };
    let value = grid_scaffold_value(profile, id, order, cell);
    let mut tile = make_grid_tile(id, &value, profile, &BuildContext::default());
    if footer {
        tile.cell = cell;
        tile.fields
            .retain(|field| field.record.segments.last().map(String::as_str) != Some("footer"));
        grid_set_footer(&mut tile, true);
    }
    tile.added = true;
    wire_grid_tile(state, &tile);
    let index = {
        let mut tiles = state.tiles.borrow_mut();
        tiles.push(tile);
        tiles.len() - 1
    };
    if grid_repack(state, None, false) {
        grid_set_status(state, "");
    } else {
        grid_set_status(state, "No room to add this widget — the grid is full.");
    }
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
    let heading_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let icon = gtk::Image::from_icon_name(widget_icon(&tile.id));
    icon.set_pixel_size(28);
    heading_row.append(&icon);
    let heading_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let heading = gtk::Label::builder()
        .xalign(0.0)
        .label(widget_display_name(&tile.id))
        .css_classes(["heading"])
        .build();
    let caption = gtk::Label::builder()
        .xalign(0.0)
        .label(widget_description(&tile.id))
        .css_classes(["dim-label", "caption"])
        .build();
    heading_text.append(&heading);
    heading_text.append(&caption);
    heading_row.append(&heading_text);
    state.details.append(&heading_row);
    if let Some(switch) = &tile.enabled {
        state.details.append(&dim_line_with_help(
            "Enabled",
            field_help("enabled"),
            switch,
        ));
    }
    if state.profile.has_footer {
        let footer = grid_tile_is_footer(tile);
        let toggle = gtk::Button::with_label(if footer {
            "Move to canvas"
        } else {
            "Move to footer"
        });
        toggle.add_css_class("flat");
        state.details.append(&toggle);
        {
            let st = state.clone();
            let index = selected.unwrap_or(0);
            toggle.connect_clicked(move |_| {
                {
                    let mut tiles = st.tiles.borrow_mut();
                    let Some(tile) = tiles.get_mut(index) else {
                        return;
                    };
                    let footer = grid_tile_is_footer(tile);
                    grid_set_footer(tile, !footer);
                }
                grid_repack(&st, None, false);
                grid_reattach(&st);
                grid_refresh_details(&st);
                st.notify();
            });
        }
    }

    let columns = state.profile.columns;
    let max_rows = state.profile.max_rows;
    let cell = tile.cell;
    let position = gtk::Expander::new(Some("Exact position"));
    position.set_expanded(false);
    let position_grid = gtk::Grid::new();
    position_grid.set_column_spacing(12);
    position_grid.set_row_spacing(6);
    let column_spin = gtk::SpinButton::with_range(0.0, (columns - 1).max(0) as f64, 1.0);
    let row_spin = gtk::SpinButton::with_range(0.0, max_rows as f64, 1.0);
    let width_spin = gtk::SpinButton::with_range(1.0, columns.max(1) as f64, 1.0);
    let height_spin = gtk::SpinButton::with_range(1.0, max_rows.max(1) as f64, 1.0);
    for (spin, value) in [
        (&column_spin, cell.col),
        (&row_spin, cell.row),
        (&width_spin, cell.w),
        (&height_spin, cell.h),
    ] {
        spin.set_digits(0);
        spin.set_value(value as f64);
    }
    position_grid.attach(&dim_line("Column", &column_spin), 0, 0, 1, 1);
    position_grid.attach(&dim_line("Row", &row_spin), 0, 1, 1, 1);
    position_grid.attach(&dim_line("Width", &width_spin), 0, 2, 1, 1);
    position_grid.attach(&dim_line("Height", &height_spin), 0, 3, 1, 1);
    position.set_child(Some(&position_grid));
    state.details.append(&position);
    {
        let st = state.clone();
        let spins = (
            column_spin.clone(),
            row_spin.clone(),
            width_spin.clone(),
            height_spin.clone(),
        );
        let apply = move || {
            if st.programmatic.get() {
                return;
            }
            let Some(index) = *st.selected.borrow() else {
                return;
            };
            let previous = match st.tiles.borrow().get(index) {
                Some(tile) => tile.cell,
                None => return,
            };
            let desired = CellRect {
                col: spins.0.value() as i64,
                row: spins.1.value() as i64,
                w: spins.2.value() as i64,
                h: spins.3.value() as i64,
            };
            let clamped = grid_clamp_cell(desired, st.profile.columns);
            {
                let mut tiles = st.tiles.borrow_mut();
                if let Some(tile) = tiles.get_mut(index) {
                    tile.cell = clamped;
                }
            }
            if !grid_repack(&st, Some(index), false) {
                {
                    let mut tiles = st.tiles.borrow_mut();
                    if let Some(tile) = tiles.get_mut(index) {
                        tile.cell = previous;
                    }
                }
                grid_repack(&st, Some(index), false);
                grid_set_status(&st, "No room to place there — the grid is full.");
            } else {
                grid_set_status(&st, "");
            }
            grid_reattach(&st);
            grid_update_size_caption(&st);
            st.notify();
        };
        for spin in [&column_spin, &row_spin, &width_spin, &height_spin] {
            let apply = apply.clone();
            spin.connect_value_changed(move |_| apply());
        }
    }

    let width_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    width_line.append(&state.size_caption);
    let shrink_w = gtk::Button::from_icon_name("list-remove-symbolic");
    shrink_w.add_css_class("flat");
    shrink_w.set_tooltip_text(Some("Shrink width by one cell"));
    let grow_w = gtk::Button::from_icon_name("list-add-symbolic");
    grow_w.add_css_class("flat");
    grow_w.set_tooltip_text(Some("Grow width by one cell"));
    width_line.append(&shrink_w);
    width_line.append(&grow_w);
    state.details.append(&width_line);

    let height_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let height_caption = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .label(format!("Height: {} rows", cell.h))
        .build();
    height_line.append(&height_caption);
    let shrink_h = gtk::Button::from_icon_name("list-remove-symbolic");
    shrink_h.add_css_class("flat");
    shrink_h.set_tooltip_text(Some("Shrink height by one cell"));
    let grow_h = gtk::Button::from_icon_name("list-add-symbolic");
    grow_h.add_css_class("flat");
    grow_h.set_tooltip_text(Some("Grow height by one cell"));
    height_line.append(&shrink_h);
    height_line.append(&grow_h);
    state.details.append(&height_line);

    state.details.append(&state.status);

    let reset = gtk::Button::with_label("Reset card size");
    reset.add_css_class("flat");
    state.details.append(&reset);
    {
        let st = state.clone();
        let index = selected.unwrap_or(0);
        reset.connect_clicked(move |_| grid_reset_tile(&st, index));
    }

    {
        let st = state.clone();
        shrink_w.connect_clicked(move |_| grid_resize_step(&st, -1, 0));
    }
    {
        let st = state.clone();
        grow_w.connect_clicked(move |_| grid_resize_step(&st, 1, 0));
    }
    {
        let st = state.clone();
        shrink_h.connect_clicked(move |_| grid_resize_step(&st, 0, -1));
    }
    {
        let st = state.clone();
        grow_h.connect_clicked(move |_| grid_resize_step(&st, 0, 1));
    }
    drop(tiles);
    grid_update_size_caption(state);
    grid_update_strips(state);
}
