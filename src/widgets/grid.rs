use super::context::{BuildContext, ChangeHook};
use super::dms_catalog::{card_defaults, widget_description, widget_icon, GridSurface};
use super::editor::Editor;
use super::fields::dim_line_with_help;
use super::grid_details::{
    grid_clear_all, grid_refresh_adder, grid_refresh_details, grid_reset_layout,
};
use super::grid_gestures::wire_grid_tile;
use super::grid_layout::{
    drop_repack, grid_tile_fields_nix, repack, CellRect, TileInput, GRID_DEFAULT_ORDER, GRID_GAP,
    GRID_ROW_UNIT,
};
use super::names::{field_caption, field_help, widget_display_name};
use super::nix::attr_key;
use super::submodule::{build_bespoke_child, build_child_row, scan_direct_children};
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
/// One editable card setting and its editor.
#[derive(Clone)]
pub(crate) struct GridField {
    pub(crate) record: Record,
    pub(crate) editor: Editor,
}

/// A tile on the fixed-width placement grid of a `grid-configuration` control.
#[derive(Clone)]
pub(crate) struct GridTile {
    pub(crate) id: String,
    pub(crate) added: bool,
    pub(crate) order: i64,
    pub(crate) cell: CellRect,
    pub(crate) orig: CellRect,
    pub(crate) enabled: Option<gtk::Switch>,
    pub(crate) orig_enabled: Option<bool>,
    pub(crate) fields: Vec<GridField>,
    pub(crate) tile: gtk::ToggleButton,
    pub(crate) label: gtk::Label,
    pub(crate) overlay: gtk::Overlay,
    pub(crate) right_strip: gtk::Box,
    pub(crate) bottom_strip: gtk::Box,
    pub(crate) corner_strip: gtk::Box,
    pub(crate) chrome: gtk::Box,
    pub(crate) remove_button: gtk::Button,
    pub(crate) size_badge: gtk::Label,
}

/// The per-page shape of a grid control, derived from the record path.
#[derive(Clone, Copy)]
pub(crate) struct GridProfile {
    pub(crate) surface: GridSurface,
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
    "audioOutput",
    "audioInput",
    "builtin_vpn",
    "builtin_cups",
    "builtin_tailscale",
    "builtin_display_profiles",
    "diskUsage",
    "nightMode",
    "darkMode",
    "colorPicker",
    "doNotDisturb",
    "idleInhibitor",
    "runningApps",
    "quickTiles",
    "plugin_dankKDEConnect",
];
pub(crate) const DASH_GRID_REGISTRY: &[&str] = &[
    "clock",
    "weather",
    "notifications",
    "calendar",
    "media",
    "sysmon",
    "cpu",
    "memory",
    "network",
    "disk",
    "battery",
    "user",
    "wellbeing",
];
pub(crate) const WEATHER_GRID_REGISTRY: &[&str] = &[
    "conditions",
    "hourly",
    "sun",
    "moon",
    "humidity",
    "wind",
    "daily",
    "pressure",
    "precipitation",
    "visibility",
    "uv",
];

impl GridProfile {
    pub(crate) fn for_path(path: &str) -> Option<Self> {
        if path.contains("controlCenterWidgets") {
            Some(Self {
                surface: GridSurface::ControlCenter,
                columns: 8,
                max_rows: 64,
                has_enabled: true,
                has_footer: true,
                registry: CC_GRID_REGISTRY,
            })
        } else if path.contains("dashCards") {
            Some(Self {
                surface: GridSurface::Dashboard,
                columns: 6,
                max_rows: 64,
                has_enabled: false,
                has_footer: false,
                registry: DASH_GRID_REGISTRY,
            })
        } else if path.contains("weather.widgets") {
            Some(Self {
                surface: GridSurface::Weather,
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
            surface: GridSurface::Generic,
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
#[derive(Clone)]
pub(crate) struct GridLayoutState {
    pub(crate) profile: GridProfile,
    pub(crate) tiles: Rc<RefCell<Vec<GridTile>>>,
    pub(crate) orig_ids: Vec<String>,
    pub(crate) selected: Rc<RefCell<Option<usize>>>,
    pub(crate) extras: Vec<(Record, Editor)>,
    pub(crate) on_change: ChangeHook,
    pub(crate) canvas: gtk::Grid,
    pub(crate) drop_preview: gtk::Box,
    pub(crate) details: gtk::Box,
    pub(crate) adder_slot: gtk::Box,
    pub(crate) footer_flow: Option<gtk::FlowBox>,
    pub(crate) size_caption: gtk::Label,
    pub(crate) status: gtk::Label,
    pub(crate) advanced: bool,
    pub(crate) drag_index: Rc<RefCell<Option<usize>>>,
    pub(crate) size_index: Rc<RefCell<Option<usize>>>,
    pub(crate) pinned: Rc<RefCell<Vec<CellRect>>>,
    pub(crate) steps: Rc<Cell<(f64, f64)>>,
    pub(crate) cancelled: Rc<Cell<bool>>,
    pub(crate) programmatic: Rc<Cell<bool>>,
    pub(crate) gesture_moved: Rc<Cell<bool>>,
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
    canvas.add_css_class("dms-card-canvas");
    canvas.set_column_homogeneous(true);
    canvas.set_row_homogeneous(true);
    canvas.set_column_spacing(GRID_GAP as u32);
    canvas.set_row_spacing(GRID_GAP as u32);
    canvas.set_hexpand(true);
    let drop_preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
    drop_preview.add_css_class("dms-drop-preview");
    drop_preview.set_can_target(false);
    drop_preview.set_visible(false);
    let details = gtk::Box::new(gtk::Orientation::Vertical, 8);
    details.set_hexpand(true);
    let adder_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let size_caption = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let status = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label", "error"])
        .build();
    let footer_flow = profile.has_footer.then(|| {
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(8);
        flow.set_column_spacing(8);
        flow.set_row_spacing(8);
        flow
    });

    let extras_expander = gtk::Expander::new(Some("More settings"));
    extras_expander.set_expanded(false);
    let extras_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    extras_expander.set_child(Some(&extras_box));

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

    let mut orig_ids = Vec::new();
    let mut extras = Vec::new();
    let mut built = Vec::new();
    for child in scan_direct_children(record, ctx.advanced) {
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
                extras_box.append(&build_child_row(&child, &editor, 1, ctx.expand_details));
            }
            (child, editor)
        })
        .collect();
    root.append(&adder_slot);
    root.append(&details);
    if extras_box.first_child().is_some() {
        root.append(&extras_expander);
    }

    let state = Rc::new(GridLayoutState {
        profile,
        tiles: Rc::new(RefCell::new(built)),
        orig_ids,
        selected: Rc::new(RefCell::new(None)),
        extras,
        on_change: on_change.clone(),
        canvas: canvas.clone(),
        drop_preview: drop_preview.clone(),
        details: details.clone(),
        adder_slot: adder_slot.clone(),
        footer_flow,
        size_caption: size_caption.clone(),
        status: status.clone(),
        advanced: ctx.advanced,
        drag_index: Rc::new(RefCell::new(None)),
        size_index: Rc::new(RefCell::new(None)),
        pinned: Rc::new(RefCell::new(Vec::new())),
        steps: Rc::new(Cell::new((1.0, 1.0))),
        cancelled: Rc::new(Cell::new(false)),
        programmatic: Rc::new(Cell::new(false)),
        gesture_moved: Rc::new(Cell::new(false)),
    });

    {
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let reset = gtk::Button::with_label("Reset sizes");
        reset.set_tooltip_text(Some("Reset every card to its default size."));
        let clear = gtk::Button::with_label("Clear all");
        clear.add_css_class("destructive-action");
        let armed = Rc::new(Cell::new(false));
        {
            let state = state.clone();
            reset.connect_clicked(move |_| grid_reset_layout(&state));
        }
        {
            let state = state.clone();
            let armed = armed.clone();
            let clear_button = clear.clone();
            clear.connect_clicked(move |_| {
                if armed.get() {
                    grid_clear_all(&state);
                    armed.set(false);
                    clear_button.set_label("Clear all");
                    return;
                }
                armed.set(true);
                clear_button.set_label("Confirm clear");
            });
        }
        toolbar.append(&reset);
        toolbar.append(&clear);
        root.insert_child_after(&toolbar, Some(&canvas));
    }
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
    tile.fields.iter().any(|field| {
        field.record.segments.last().map(String::as_str) == Some("footer")
            && field.editor.to_nix().trim() == "true"
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
        let css = "
            .grid-resize {
                background-color: alpha(@window_fg_color, 0.25);
                border-radius: 3px;
            }
            .dms-card {
                background-color: alpha(@window_fg_color, 0.05);
                border: 1px solid alpha(@window_fg_color, 0.10);
                border-radius: 14px;
                padding: 8px;
            }
            .dms-card:hover { background-color: alpha(@window_fg_color, 0.09); }
            .dms-card:checked {
                border: 2px solid @accent_bg_color;
                background-color: alpha(@accent_bg_color, 0.12);
            }
            .dms-card-size {
                background-color: alpha(@window_fg_color, 0.85);
                color: @window_bg_color;
                border-radius: 8px;
                padding: 2px 7px;
                font-size: 11px;
                font-weight: 700;
            }
            .dms-card-canvas {
                background-color: alpha(@window_fg_color, 0.03);
                border-radius: 18px;
                padding: 8px;
            }
            .dms-drop-preview {
                background-color: alpha(@accent_bg_color, 0.15);
                border: 2px solid alpha(@accent_bg_color, 0.8);
                border-radius: 14px;
            }
        ";
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

fn field_default(surface: GridSurface, id: &str, name: &str) -> Option<&'static str> {
    match name {
        "small" | "footer" | "footerEnd" | "footerFill" => Some("false"),
        "footerW" => Some("1"),
        _ => card_defaults(surface, id)
            .into_iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value),
    }
}

fn build_grid_popout(fields: &[GridField]) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_position(gtk::PositionType::Right);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_size_request(300, -1);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    if fields.is_empty() {
        let empty = gtk::Label::builder()
            .xalign(0.0)
            .label("No extra settings.")
            .css_classes(["dim-label"])
            .build();
        content.append(&empty);
    } else {
        for field in fields {
            let name = field.record.segments.last().cloned().unwrap_or_default();
            content.append(&dim_line_with_help(
                &field_caption(&name),
                field_help(&name),
                &field.editor.root(),
            ));
        }
    }
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(120)
        .max_content_height(420)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    popover.set_child(Some(&scroll));
    popover
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
    let mut fields: Vec<GridField> = Vec::new();
    let mut present = Vec::new();
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
                present.push(name);
                let editor = build_bespoke_child(&child, ctx);
                fields.push(GridField {
                    record: child,
                    editor,
                });
            }
        }
    }
    for (name, literal) in card_defaults(profile.surface, id) {
        if present.iter().any(|field| field.eq_ignore_ascii_case(name)) {
            continue;
        }
        let record = Record {
            segments: vec![name.to_string()],
            value: literal.to_string(),
            ..Record::default()
        };
        let editor = build_bespoke_child(&record, ctx);
        fields.push(GridField { record, editor });
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
        .css_classes(["heading"])
        .build();
    let subtitle = gtk::Label::builder()
        .xalign(0.5)
        .wrap(true)
        .label(widget_description(id))
        .css_classes(["dim-label", "caption"])
        .build();
    let icon = gtk::Image::from_icon_name(widget_icon(id));
    icon.set_pixel_size(28);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.set_valign(gtk::Align::Center);
    card.set_halign(gtk::Align::Center);
    card.append(&icon);
    card.append(&label);
    card.append(&subtitle);
    let button = gtk::ToggleButton::new();
    button.add_css_class("dms-card");
    button.set_hexpand(true);
    button.set_vexpand(true);
    button.set_child(Some(&card));

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&button));
    let (right_strip, bottom_strip, corner_strip) = grid_resize_strips();
    overlay.add_overlay(&bottom_strip);
    overlay.add_overlay(&right_strip);
    overlay.add_overlay(&corner_strip);
    right_strip.set_visible(false);
    bottom_strip.set_visible(false);
    corner_strip.set_visible(false);

    let chrome = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    chrome.add_css_class("dms-card-chrome");
    chrome.set_halign(gtk::Align::End);
    chrome.set_valign(gtk::Align::Start);
    chrome.set_margin_top(6);
    chrome.set_margin_end(6);
    let options_button = gtk::Button::from_icon_name("emblem-system-symbolic");
    options_button.add_css_class("flat");
    options_button.set_tooltip_text(Some("Widget settings"));
    let remove_button = gtk::Button::from_icon_name("user-trash-symbolic");
    remove_button.add_css_class("flat");
    remove_button.set_tooltip_text(Some("Remove card"));
    chrome.append(&options_button);
    chrome.append(&remove_button);
    chrome.set_visible(false);
    overlay.add_overlay(&chrome);
    let popout = build_grid_popout(&fields);
    popout.set_parent(&options_button);
    let for_options = popout;
    options_button.connect_clicked(move |_| for_options.popup());

    let size_badge = gtk::Label::new(None);
    size_badge.add_css_class("dms-card-size");
    size_badge.set_halign(gtk::Align::Start);
    size_badge.set_valign(gtk::Align::Start);
    size_badge.set_margin_top(6);
    size_badge.set_margin_start(6);
    size_badge.set_visible(false);
    overlay.add_overlay(&size_badge);

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
        chrome,
        remove_button,
        size_badge,
    };
    grid_update_tile_visual(&tile);
    tile
}
pub(crate) fn grid_select(state: &Rc<GridLayoutState>, index: usize) {
    if *state.selected.borrow() == Some(index) {
        return;
    }
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
    grid_set_status(state, "");
    grid_hide_drop_preview(state);
}

/// Repaint the canvas after a gesture ends. Reparenting tiles inside the
/// gesture's own signal handler unmaps a widget GTK still considers mapped, so
/// the commit runs from an idle callback instead.
pub(crate) fn grid_schedule_refresh(state: &Rc<GridLayoutState>) {
    let state = state.clone();
    glib::idle_add_local_once(move || {
        grid_reattach(&state);
        grid_update_size_caption(&state);
        grid_refresh_details(&state);
        state.notify();
    });
}

/// Recompute the committed cells of every non-footer tile. `interacting` is
/// forced first in the placement order. Returns false when the layout would
/// exceed `max_rows`, in which case no cell is changed.
type RepackInputs = (Vec<usize>, Vec<usize>, Vec<TileInput>);

fn grid_repack_inputs(
    state: &Rc<GridLayoutState>,
    interacting: Option<usize>,
    pinned: bool,
) -> RepackInputs {
    let tiles = state.tiles.borrow();
    let pinned_cells = pinned.then(|| state.pinned.borrow().clone());
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
        .map(|&i| {
            let cell = if Some(i) == interacting {
                tiles[i].cell
            } else {
                pinned_cells
                    .as_ref()
                    .and_then(|cells| cells.get(i).copied())
                    .unwrap_or(tiles[i].cell)
            };
            TileInput {
                cell: Some(cell),
                w: cell.w,
                h: cell.h,
            }
        })
        .collect();
    let order_local: Vec<usize> = order
        .iter()
        .map(|i| indices.iter().position(|x| x == i).unwrap_or(0))
        .collect();
    (indices, order_local, inputs)
}

fn grid_apply_cells(state: &Rc<GridLayoutState>, indices: &[usize], cells: &[CellRect]) {
    let mut tiles = state.tiles.borrow_mut();
    for (position, &index) in indices.iter().enumerate() {
        tiles[index].cell = cells[position];
    }
}

pub(crate) fn grid_repack(
    state: &Rc<GridLayoutState>,
    interacting: Option<usize>,
    gravity: bool,
) -> bool {
    let (indices, order, inputs) = grid_repack_inputs(state, interacting, false);
    let Some(cells) = repack(
        &inputs,
        &order,
        state.profile.columns,
        state.profile.max_rows,
        gravity,
    ) else {
        return false;
    };
    grid_apply_cells(state, &indices, &cells);
    true
}

/// Repack the preview of an interaction from the cells pinned at its start.
pub(crate) fn grid_repack_preview(state: &Rc<GridLayoutState>, interacting: usize) -> bool {
    let (indices, order, inputs) = grid_repack_inputs(state, Some(interacting), true);
    let Some(cells) = repack(
        &inputs,
        &order,
        state.profile.columns,
        state.profile.max_rows,
        true,
    ) else {
        return false;
    };
    grid_apply_cells(state, &indices, &cells);
    true
}

/// Repack with a dragged tile kept at its drop target while the others make
/// room, matching DMS `dropInto` instead of letting gravity teleport it.
pub(crate) fn grid_repack_drop(state: &Rc<GridLayoutState>, index: usize) -> bool {
    let Some(origin) = state.pinned.borrow().get(index).copied() else {
        return grid_repack(state, Some(index), true);
    };
    let (indices, order, inputs) = grid_repack_inputs(state, Some(index), true);
    let Some(dragged) = indices.iter().position(|&i| i == index) else {
        return grid_repack(state, Some(index), true);
    };
    let Some(cells) = drop_repack(
        &inputs,
        &order,
        state.profile.columns,
        state.profile.max_rows,
        dragged,
        origin,
    ) else {
        return false;
    };
    grid_apply_cells(state, &indices, &cells);
    true
}

pub(crate) fn grid_show_drop_preview(state: &Rc<GridLayoutState>, cell: CellRect) {
    if state.drop_preview.parent().is_some() {
        state.canvas.remove(&state.drop_preview);
    }
    state.drop_preview.set_visible(true);
    state.canvas.attach(
        &state.drop_preview,
        cell.col as i32,
        cell.row as i32,
        cell.w as i32,
        cell.h as i32,
    );
}

pub(crate) fn grid_hide_drop_preview(state: &Rc<GridLayoutState>) {
    if state.drop_preview.parent().is_some() {
        state.canvas.remove(&state.drop_preview);
    }
    state.drop_preview.set_visible(false);
}

pub(crate) fn grid_reattach(state: &Rc<GridLayoutState>) {
    grid_hide_drop_preview(state);
    while let Some(child) = state.canvas.first_child() {
        state.canvas.remove(&child);
    }
    if let Some(flow) = &state.footer_flow {
        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }
    }
    let tiles = state.tiles.borrow();
    if tiles.is_empty() {
        let empty = gtk::Label::builder()
            .label("No cards yet. Use Add widget to start.")
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build();
        state
            .canvas
            .attach(&empty, 0, 0, state.profile.columns as i32, 2);
    }
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
    if let Some(tile) = state
        .selected
        .borrow()
        .and_then(|index| state.tiles.borrow().get(index).cloned())
    {
        if tile.size_badge.is_visible() {
            tile.size_badge
                .set_text(&format!("{} x {}", tile.cell.w, tile.cell.h));
        }
    }
}

pub(crate) fn grid_set_status(state: &Rc<GridLayoutState>, message: &str) {
    state.status.set_text(message);
}

pub(crate) fn grid_update_strips(state: &Rc<GridLayoutState>) {
    let selected = *state.selected.borrow();
    let tiles = state.tiles.borrow();
    for (index, tile) in tiles.iter().enumerate() {
        let selected = selected == Some(index);
        let footer = state.profile.has_footer && grid_tile_is_footer(tile);
        tile.right_strip.set_visible(selected && !footer);
        tile.bottom_strip.set_visible(selected && !footer);
        tile.corner_strip.set_visible(selected && !footer);
        tile.chrome.set_visible(selected);
        tile.size_badge.set_visible(selected && !footer);
        if selected && !footer {
            tile.size_badge
                .set_text(&format!("{} x {}", tile.cell.w, tile.cell.h));
        }
    }
}
pub(crate) fn grid_tile_nix(tile: &GridTile, profile: GridProfile) -> String {
    let tile_surface = profile.surface;
    let fields: Vec<(String, String)> = tile
        .fields
        .iter()
        .filter_map(|field| {
            let value = field.editor.to_nix();
            let name = field.record.segments.last().cloned().unwrap_or_default();
            if field_default(tile_surface, &tile.id, &name)
                .is_some_and(|default| default.trim() == value.trim())
            {
                return None;
            }
            Some((name, value))
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
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn grid_builds_cards_and_keeps_geometry_out_of_extras() {
        if gtk::init().is_err() {
            eprintln!("skipping grid runtime check: no display");
            return;
        }
        let record = Record {
            segments: vec!["controlCenterWidgets".into()],
            value: "{ user = { enabled = true; _order = 10; col = 0; row = 0; w = 4; h = 1; }; battery = { enabled = true; _order = 20; col = 4; row = 0; w = 1; h = 1; small = true; }; columns = 8; }".into(),
            ..Record::default()
        };
        let editor = build_grid_configuration(&record, &BuildContext::default());
        let Editor::Grid { state, .. } = editor else {
            panic!("grid control expected");
        };
        let tiles = state.tiles.borrow();
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].id, "user");
        assert_eq!(tiles[1].id, "battery");
        assert_eq!(tiles[0].fields.len(), 6);
        assert_eq!(tiles[1].fields.len(), 3);
        assert_eq!(tiles[1].fields[0].record.segments, vec!["small"]);
        assert_eq!(state.extras.len(), 1);
        assert_eq!(state.extras[0].0.segments, vec!["columns"]);
        drop(tiles);
        let nix = grid_nix(&state);
        assert!(!nix.contains("batteryStyle"));
        assert!(!nix.contains("background"));
        assert!(nix.contains("small = true"));
    }

    #[test]
    #[ignore = "needs a display; run with `cargo test -- --ignored`"]
    fn grid_reattach_mapped_tiles_does_not_abort() {
        if gtk::init().is_err() {
            eprintln!("skipping grid runtime check: no display");
            return;
        }
        let record = Record {
            segments: vec!["controlCenterWidgets".into()],
            value: "{ user = { enabled = true; _order = 10; col = 0; row = 0; w = 4; h = 1; }; battery = { enabled = true; _order = 20; col = 4; row = 0; w = 1; h = 1; }; wifi = { enabled = true; _order = 30; col = 2; row = 1; w = 4; h = 1; }; }".into(),
            ..Record::default()
        };
        let editor = build_grid_configuration(&record, &BuildContext::default());
        let Editor::Grid { root, state } = editor else {
            panic!("grid control expected");
        };
        let window = gtk::Window::new();
        window.set_default_size(900, 700);
        window.set_child(Some(&root));
        window.present();
        pump_main_loop(5);
        let gesture = {
            let tiles = state.tiles.borrow();
            let controllers = tiles[0].tile.observe_controllers();
            let mut found = None;
            for index in 0..controllers.n_items() {
                let Some(object) = controllers.item(index) else {
                    continue;
                };
                if let Ok(gesture) = object.downcast::<gtk::GestureDrag>() {
                    found = Some(gesture);
                    break;
                }
            }
            found.expect("move gesture")
        };
        gesture.emit_by_name::<()>("drag-begin", &[&0.0f64, &0.0f64]);
        pump_main_loop(5);
        for step in 0..24 {
            gesture.emit_by_name::<()>("drag-update", &[&(step as f64), &(step as f64)]);
            pump_main_loop(2);
        }
        gesture.emit_by_name::<()>("drag-end", &[&24.0f64, &24.0f64]);
        pump_main_loop(5);
        window.destroy();
    }

    fn pump_main_loop(iterations: usize) {
        let context = glib::MainContext::default();
        for _ in 0..iterations {
            while context.pending() {
                context.iteration(false);
            }
        }
    }

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
