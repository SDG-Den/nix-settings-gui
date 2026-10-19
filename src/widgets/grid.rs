use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::grid_details::{grid_refresh_adder, grid_refresh_details};
use super::grid_gestures::wire_grid_tile;
use super::grid_layout::{
    grid_tile_fields_nix, repack, CellRect, TileInput, GRID_DEFAULT_ORDER, GRID_GAP, GRID_ROW_UNIT,
};
use super::names::widget_display_name;
use super::nix::attr_key;
use super::submodule::{build_bespoke_child, build_child_row, scan_children};
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
/// A tile on the fixed-width placement grid of a `grid-configuration` control.
pub(crate) struct GridTile {
    pub(crate) id: String,
    pub(crate) added: bool,
    pub(crate) order: i64,
    pub(crate) cell: CellRect,
    pub(crate) orig: CellRect,
    pub(crate) enabled: Option<gtk::Switch>,
    pub(crate) orig_enabled: Option<bool>,
    pub(crate) fields: Vec<(Record, Editor)>,
    pub(crate) tile: gtk::ToggleButton,
    pub(crate) label: gtk::Label,
    pub(crate) overlay: gtk::Overlay,
    pub(crate) right_strip: gtk::Box,
    pub(crate) bottom_strip: gtk::Box,
    pub(crate) corner_strip: gtk::Box,
}

/// The per-page shape of a grid control, derived from the record path.
#[derive(Clone, Copy)]
pub(crate) struct GridProfile {
    pub(crate) columns: i64,
    pub(crate) max_rows: i64,
    pub(crate) has_enabled: bool,
    pub(crate) has_footer: bool,
    pub(crate) registry: &'static [&'static str],
}

pub(crate) const CC_GRID_REGISTRY: &[&str] = &[
    "user",
    "battery",
    "settings",
    "lock",
    "power",
    "brightnessSlider",
    "volumeSlider",
    "inputVolumeSlider",
    "wifi",
    "bluetooth",
    "builtin_vpn",
    "diskUsage",
    "nightMode",
    "doNotDisturb",
    "idleInhibitor",
    "runningApps",
];
pub(crate) const DASH_GRID_REGISTRY: &[&str] = &[
    "clock",
    "weather",
    "notifications",
    "calendar",
    "media",
    "sysmon",
    "network",
    "user",
];
pub(crate) const WEATHER_GRID_REGISTRY: &[&str] = &[
    "conditions",
    "hourly",
    "humidity",
    "wind",
    "daily",
    "precipitation",
    "uv",
];

impl GridProfile {
    pub(crate) fn for_path(path: &str) -> Option<Self> {
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

    pub(crate) fn generic() -> Self {
        Self {
            columns: 6,
            max_rows: 64,
            has_enabled: false,
            has_footer: false,
            registry: &[],
        }
    }
}
/// Shared state of one grid control. The editor, the canvas, the details pane
/// and every gesture closure hold the same `Rc`, so tile indices are always
/// resolved against the live list rather than captured at build time.
pub(crate) struct GridLayoutState {
    pub(crate) profile: GridProfile,
    pub(crate) tiles: Rc<RefCell<Vec<GridTile>>>,
    pub(crate) orig_ids: Vec<String>,
    pub(crate) selected: Rc<RefCell<Option<usize>>>,
    pub(crate) extras: Vec<(Record, Editor)>,
    pub(crate) on_change: ChangeHook,
    pub(crate) canvas: gtk::Grid,
    pub(crate) details: gtk::Box,
    pub(crate) adder_slot: gtk::Box,
    pub(crate) footer_flow: Option<gtk::FlowBox>,
    pub(crate) size_caption: gtk::Label,
    pub(crate) advanced: bool,
    pub(crate) drag_index: Rc<RefCell<Option<usize>>>,
    pub(crate) size_index: Rc<RefCell<Option<usize>>>,
    pub(crate) pinned: Rc<RefCell<Vec<CellRect>>>,
    pub(crate) steps: Rc<Cell<(f64, f64)>>,
    pub(crate) cancelled: Rc<Cell<bool>>,
    pub(crate) programmatic: Rc<Cell<bool>>,
}
pub(crate) fn build_grid_configuration(record: &Record, ctx: &BuildContext) -> Editor {
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
impl GridLayoutState {
    pub(crate) fn notify(&self) {
        if let Some(callback) = self.on_change.borrow().as_ref() {
            callback();
        }
    }
}
/// Whether a tile belongs to the footer strip rather than the canvas.
pub(crate) fn grid_tile_is_footer(tile: &GridTile) -> bool {
    tile.fields.iter().any(|(record, editor)| {
        record.segments.last().map(String::as_str) == Some("footer")
            && editor.to_nix().trim() == "true"
    })
}

pub(crate) fn grid_index_of(
    state: &Rc<GridLayoutState>,
    button: &gtk::ToggleButton,
) -> Option<usize> {
    state
        .tiles
        .borrow()
        .iter()
        .position(|tile| tile.tile == *button)
}

/// One-time stylesheet that makes the invisible resize strips visible.
pub(crate) fn install_grid_css() {
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
pub(crate) fn grid_resize_strips() -> (gtk::Box, gtk::Box, gtk::Box) {
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

pub(crate) fn grid_update_tile_visual(tile: &GridTile) {
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
pub(crate) fn make_grid_tile(
    id: &str,
    value_text: &str,
    profile: GridProfile,
    ctx: &BuildContext,
) -> GridTile {
    let mut cell = CellRect {
        col: 0,
        row: 0,
        w: 1,
        h: 1,
    };
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
pub(crate) fn grid_select(state: &Rc<GridLayoutState>, index: usize) {
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

pub(crate) fn grid_restore_pinned(state: &Rc<GridLayoutState>) {
    let pinned = state.pinned.borrow().clone();
    let mut tiles = state.tiles.borrow_mut();
    for (tile, cell) in tiles.iter_mut().zip(pinned.iter()) {
        tile.cell = *cell;
    }
}

pub(crate) fn grid_cancel(state: &Rc<GridLayoutState>) {
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
pub(crate) fn grid_repack(
    state: &Rc<GridLayoutState>,
    interacting: Option<usize>,
    gravity: bool,
) -> bool {
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

pub(crate) fn grid_reattach(state: &Rc<GridLayoutState>) {
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
    state
        .canvas
        .set_size_request(-1, rows as i32 * GRID_ROW_UNIT);
}

pub(crate) fn grid_update_size_caption(state: &Rc<GridLayoutState>) {
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

pub(crate) fn grid_update_strips(state: &Rc<GridLayoutState>) {
    let selected = *state.selected.borrow();
    let tiles = state.tiles.borrow();
    for (index, tile) in tiles.iter().enumerate() {
        let show =
            selected == Some(index) && !(state.profile.has_footer && grid_tile_is_footer(tile));
        tile.right_strip.set_visible(show);
        tile.bottom_strip.set_visible(show);
        tile.corner_strip.set_visible(show);
    }
}
pub(crate) fn grid_tile_nix(tile: &GridTile, profile: GridProfile) -> String {
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

pub(crate) fn grid_nix(state: &GridLayoutState) -> String {
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_profiles_resolve_by_path() {
        let cc = GridProfile::for_path("programs.ojuny.dms.controlCenterWidgets").unwrap();
        assert_eq!((cc.columns, cc.has_enabled, cc.has_footer), (8, true, true));
        let dash = GridProfile::for_path("desktop.ojuny.dms.dashCards").unwrap();
        assert_eq!(
            (dash.columns, dash.has_enabled, dash.has_footer),
            (6, false, false)
        );
        let weather = GridProfile::for_path("programs.ojuny.dms.weather.widgets").unwrap();
        assert_eq!((weather.columns, weather.has_enabled), (4, false));
        assert!(GridProfile::for_path("a.b").is_none());
    }
}
