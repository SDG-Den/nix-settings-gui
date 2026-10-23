use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::fields::{attrset_field_nix, build_field_cell, field, AttrsetField};
use super::monitor_geometry::{
    monitor_orig, resolve_release, row_footprint, scene_bounds, Viewport, SNAP_TOLERANCE, VIEW_PAD,
};
use super::monitor_picker::open_monitor_picker;
use super::nix::{attr_key, entry_text, number_literal, quote};
use crate::dsl::OptionSpec;
use crate::model::Record;
use crate::monitor_detect::DetectedMode;
use crate::scan;
use crate::value;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
/// One monitor rule of the `monitors` widget: core geometry editors plus the
/// rule's remaining fields and the original source text per field.
#[derive(Clone)]
pub(crate) struct MonitorRow {
    pub(crate) name: gtk::Entry,
    pub(crate) width: gtk::SpinButton,
    pub(crate) height: gtk::SpinButton,
    pub(crate) refresh: gtk::SpinButton,
    pub(crate) x: gtk::SpinButton,
    pub(crate) y: gtk::SpinButton,
    pub(crate) scale: gtk::SpinButton,
    pub(crate) rr: Editor,
    pub(crate) extras: Vec<AttrsetField>,
    pub(crate) orig: Vec<(String, String)>,
    pub(crate) form_box: gtk::Box,
}
pub(crate) const MONITOR_TRANSFORMS: &[(&str, &str)] = &[
    ("0", "No transform"),
    ("1", "90° counter-clockwise"),
    ("2", "180° counter-clockwise"),
    ("3", "270° counter-clockwise"),
    ("4", "180° vertical flip"),
    ("5", "Flip + 90° counter-clockwise"),
    ("6", "Flip + 180° counter-clockwise"),
    ("7", "Flip + 270° counter-clockwise"),
];

pub(crate) fn build_transform(value_text: &str) -> Editor {
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
        choices: MONITOR_TRANSFORMS
            .iter()
            .map(|(value, _)| (*value).to_string())
            .collect(),
    }
}
pub(crate) fn monitor_row_unchanged(row: &MonitorRow) -> bool {
    let text_same =
        |entry: &gtk::Entry, name: &str| entry.text() == entry_text(monitor_orig(row, name));
    let spin_same = |spin: &gtk::SpinButton, name: &str, default: f64| match monitor_orig(row, name)
        .parse::<f64>()
    {
        Ok(value) => (spin.value() - value).abs() < 1e-9,
        Err(_) => (spin.value() - default).abs() < 1e-9,
    };
    let rr_same = monitor_orig(row, "rr").is_empty() || row.rr.unchanged(monitor_orig(row, "rr"));
    text_same(&row.name, "name")
        && spin_same(&row.width, "width", 0.0)
        && spin_same(&row.height, "height", 0.0)
        && spin_same(&row.refresh, "refresh", 0.0)
        && spin_same(&row.x, "x", 0.0)
        && spin_same(&row.y, "y", 0.0)
        && spin_same(&row.scale, "scale", 1.0)
        && rr_same
        && row
            .extras
            .iter()
            .all(|field| field.editor.unchanged(&field.orig_value))
}

pub(crate) fn monitor_row_nix(row: &MonitorRow) -> String {
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
    for key in MONITOR_PASSTHROUGH_FIELDS.iter().copied() {
        let raw = monitor_orig(row, key);
        if !raw.is_empty() && raw != "null" {
            parts.push(format!("{} = {};", attr_key(key), raw));
        }
    }
    format!("{{ {} }}", parts.join(" "))
}

pub(crate) fn monitors_nix(rows: &[MonitorRow]) -> String {
    let parts: Vec<String> = rows.iter().map(monitor_row_nix).collect();
    value::serialize_list(&parts)
}

pub(crate) fn connect_monitor_row(row: &MonitorRow, update: &Rc<dyn Fn()>) {
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

pub(crate) const MONITOR_EXTRAS: &[(&str, bool)] = &[
    ("vrr", true),
    ("hdr", true),
    ("hdr_force", true),
    ("disable", true),
    ("primary", true),
    ("icc", false),
    ("hdr_min_lum", false),
    ("hdr_max_lum", false),
    ("hdr_max_avg_lum", false),
];

pub(crate) const MONITOR_MATCH_FIELDS: &[(&str, &str)] =
    &[("make", "Make"), ("model", "Model"), ("serial", "Serial")];

pub(crate) const MONITOR_PASSTHROUGH_FIELDS: &[&str] = &["make", "model", "serial", "custom"];

/// Attach a caption and a control to a monitor form grid.
pub(crate) fn attach_cell(
    grid: &gtk::Grid,
    row: i32,
    col: i32,
    label: &str,
    widget: &impl IsA<gtk::Widget>,
) {
    let caption = gtk::Label::builder()
        .xalign(1.0)
        .label(label)
        .css_classes(["dim-label", "caption"])
        .build();
    grid.attach(&caption, col, row, 1, 1);
    grid.attach(widget, col + 1, row, 1, 1);
}

struct MonitorPalette {
    background: gtk::gdk::RGBA,
    monitor: gtk::gdk::RGBA,
    selected: gtk::gdk::RGBA,
    outline: gtk::gdk::RGBA,
    dashed: gtk::gdk::RGBA,
    text: gtk::gdk::RGBA,
    selected_text: gtk::gdk::RGBA,
}

impl MonitorPalette {
    fn from_style(style: &gtk::StyleContext) -> Self {
        let color = |names: &[&str], fallback: (f32, f32, f32, f32)| {
            names
                .iter()
                .find_map(|name| style.lookup_color(name))
                .unwrap_or_else(|| {
                    gtk::gdk::RGBA::new(fallback.0, fallback.1, fallback.2, fallback.3)
                })
        };
        Self {
            background: color(
                &["window_bg_color", "theme_bg_color"],
                (0.08, 0.08, 0.10, 1.0),
            ),
            monitor: color(
                &["card_bg_color", "theme_base_color", "theme_bg_color"],
                (0.42, 0.42, 0.47, 1.0),
            ),
            selected: color(
                &["accent_bg_color", "theme_selected_bg_color"],
                (0.27, 0.45, 0.75, 1.0),
            ),
            outline: color(
                &["window_fg_color", "theme_fg_color", "theme_text_color"],
                (1.0, 1.0, 1.0, 1.0),
            ),
            dashed: color(
                &[
                    "warning_bg_color",
                    "warning_color",
                    "theme_selected_bg_color",
                ],
                (0.95, 0.65, 0.25, 1.0),
            ),
            text: color(
                &["card_fg_color", "window_fg_color", "theme_fg_color"],
                (1.0, 1.0, 1.0, 1.0),
            ),
            selected_text: color(
                &["accent_fg_color", "theme_selected_fg_color"],
                (1.0, 1.0, 1.0, 1.0),
            ),
        }
    }
}

fn set_color(cr: &gtk::cairo::Context, color: &gtk::gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        color.alpha() as f64 * alpha,
    );
}

fn monitor_flag(row: &MonitorRow, name: &str) -> bool {
    row.extras
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.editor.to_nix() == "true")
        .unwrap_or(false)
}

fn draw_monitor_label(
    cr: &gtk::cairo::Context,
    label: &str,
    px: f64,
    py: f64,
    pw: f64,
    ph: f64,
    color: &gtk::gdk::RGBA,
    alpha: f64,
) {
    if label.is_empty() || pw <= 1.0 || ph <= 1.0 {
        return;
    }
    cr.save().ok();
    cr.rectangle(px, py, pw, ph);
    cr.clip();
    set_color(cr, color, alpha);
    let font_size = 14.0;
    cr.set_font_size(font_size);
    if let Ok(extents) = cr.text_extents(label) {
        let max_width = (pw - 8.0).max(1.0);
        let max_height = (ph - 8.0).max(1.0);
        let scale = (max_width / extents.width().max(1.0))
            .min(max_height / extents.height().max(1.0))
            .min(1.0);
        if scale < 1.0 {
            cr.set_font_size((font_size * scale).max(9.0));
        }
    }
    if let Ok(extents) = cr.text_extents(label) {
        let x = px + (pw - extents.width()) / 2.0 - extents.x_bearing();
        let y = py + (ph - extents.height()) / 2.0 - extents.y_bearing();
        cr.move_to(x, y);
        let _ = cr.show_text(label);
    }
    cr.restore().ok();
}

/// One monitor rule: core geometry and the disable/primary toggles on a grid,
/// a read-only "Applies to (match)" block when make/model/serial are set, and
/// the remaining effect fields behind an "Advanced options" expander.
pub(crate) fn make_monitor_row(scan_row: &scan::TableRowScan, ctx: &BuildContext) -> MonitorRow {
    let get = |name: &str| field(&scan_row.fields, name).unwrap_or("");
    let number_spin = |value: &str, default: f64, min: f64, max: f64, step: f64| {
        let spin = gtk::SpinButton::with_range(min, max, step);
        spin.set_digits(0);
        spin.set_value(value.parse::<f64>().unwrap_or(default));
        spin
    };

    let name = gtk::Entry::new();
    name.set_text(entry_text(get("name")).as_str());
    name.set_tooltip_text(Some(
        "Matcher: this rule applies to the output with this name. Regex supported.",
    ));

    let width = number_spin(get("width"), 0.0, 0.0, 16384.0, 1.0);
    let height = number_spin(get("height"), 0.0, 0.0, 16384.0, 1.0);
    let refresh = number_spin(get("refresh"), 0.0, 0.0, 1000.0, 1.0);
    let x = number_spin(get("x"), 0.0, -16384.0, 16384.0, 1.0);
    let y = number_spin(get("y"), 0.0, -16384.0, 16384.0, 1.0);
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

    let mut extras = Vec::new();
    let expander = gtk::Expander::new(Some("Advanced options"));
    let extra_grid = gtk::Grid::new();
    extra_grid.set_column_spacing(12);
    extra_grid.set_row_spacing(6);
    let mut advanced_index = 0i32;
    for (field_name, is_bool) in MONITOR_EXTRAS.iter().copied() {
        let raw = get(field_name);
        let source = if raw.is_empty() || raw == "null" {
            if is_bool {
                "false"
            } else {
                "0"
            }
        } else {
            raw
        };
        let editor = build_field_cell(source, ctx);
        let present = !raw.is_empty() && raw != "null";
        if field_name == "disable" || field_name == "primary" {
            let (column, caption) = if field_name == "disable" {
                (0, "Disable")
            } else {
                (2, "Primary")
            };
            attach_cell(&grid, 4, column, caption, &editor.root());
        } else {
            let caption = gtk::Label::builder()
                .xalign(1.0)
                .label(field_name)
                .css_classes(["dim-label", "caption"])
                .build();
            extra_grid.attach(&caption, 0, advanced_index, 1, 1);
            extra_grid.attach(&editor.root(), 1, advanced_index, 1, 1);
            advanced_index += 1;
        }
        extras.push(AttrsetField {
            name: field_name.to_string(),
            editor,
            orig_value: source.to_string(),
            orig_present: present,
        });
    }
    form_box.append(&grid);

    let mut match_rows = Vec::new();
    for (key, caption_text) in MONITOR_MATCH_FIELDS.iter().copied() {
        let raw = get(key);
        if raw.is_empty() || raw == "null" {
            continue;
        }
        let value = entry_text(raw);
        if value.is_empty() {
            continue;
        }
        match_rows.push((caption_text, value));
    }
    if !match_rows.is_empty() {
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .label("Applies to (match)")
            .css_classes(["heading"])
            .build();
        form_box.append(&heading);
        let match_grid = gtk::Grid::new();
        match_grid.set_column_spacing(12);
        match_grid.set_row_spacing(6);
        for (row_index, (caption_text, value)) in match_rows.iter().enumerate() {
            let value_label = gtk::Label::builder().xalign(0.0).label(value).build();
            attach_cell(&match_grid, row_index as i32, 0, caption_text, &value_label);
        }
        form_box.append(&match_grid);
    }

    expander.set_child(Some(&extra_grid));
    form_box.append(&expander);

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
    }
}
#[derive(Clone, Copy)]
pub(crate) enum DragMode {
    None,
    Pan {
        origin_x: f64,
        origin_y: f64,
    },
    Monitor {
        start_px: f64,
        start_py: f64,
        grab_dx: f64,
        grab_dy: f64,
    },
}

pub(crate) struct MonitorCanvas {
    pub(crate) canvas: gtk::DrawingArea,
    pub(crate) view: Rc<Cell<Viewport>>,
    pub(crate) last_size: Rc<Cell<(f64, f64)>>,
}

impl Clone for MonitorCanvas {
    fn clone(&self) -> Self {
        MonitorCanvas {
            canvas: self.canvas.clone(),
            view: self.view.clone(),
            last_size: self.last_size.clone(),
        }
    }
}

impl MonitorCanvas {
    pub(crate) fn refit(&self, rows: &[MonitorRow]) {
        if let Some((min_x, min_y, max_x, max_y)) = scene_bounds(rows) {
            let (w, h) = self.last_size.get();
            self.view
                .set(Viewport::fit(min_x, min_y, max_x, max_y, w, h));
        }
        self.canvas.queue_draw();
    }
    pub(crate) fn redraw(&self) {
        self.canvas.queue_draw();
    }
}

pub(crate) fn attach_monitor_row(
    form: &gtk::Box,
    rows: &Rc<RefCell<Vec<MonitorRow>>>,
    mc: &MonitorCanvas,
    row: MonitorRow,
) -> usize {
    let redraw: Rc<dyn Fn()> = {
        let mc = mc.clone();
        Rc::new(move || mc.redraw())
    };
    connect_monitor_row(&row, &redraw);
    form.append(&row.form_box);
    let index = rows.borrow().len();
    rows.borrow_mut().push(row);
    index
}

pub(crate) fn build_monitors(record: &Record) -> Editor {
    let ctx = BuildContext::default();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let rows = Rc::new(RefCell::new(Vec::new()));
    let selected = Rc::new(Cell::new(0usize));
    let view = Rc::new(Cell::new(Viewport {
        scale: 1.0,
        origin_x: 0.0,
        origin_y: 0.0,
    }));
    let last_size = Rc::new(Cell::new((640.0f64, 400.0f64)));

    let canvas = gtk::DrawingArea::builder()
        .content_width(640)
        .content_height(400)
        .build();
    canvas.set_hexpand(true);
    canvas.set_size_request(-1, 400);
    canvas.set_draw_func({
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        let last_size = last_size.clone();
        move |area, cr, width, height| {
            let (last_w, last_h) = last_size.get();
            if last_w != width as f64 || last_h != height as f64 {
                last_size.set((width as f64, height as f64));
                let rows_snapshot = rows.borrow();
                if let Some((min_x, min_y, max_x, max_y)) = scene_bounds(&rows_snapshot) {
                    view.set(Viewport::fit(
                        min_x,
                        min_y,
                        max_x,
                        max_y,
                        width as f64,
                        height as f64,
                    ));
                }
            }
            let palette = MonitorPalette::from_style(&area.style_context());
            set_color(cr, &palette.background, 1.0);
            cr.rectangle(0.0, 0.0, width as f64, height as f64);
            cr.fill().ok();
            let v = view.get();
            let rows = rows.borrow();
            for (index, row) in rows.iter().enumerate() {
                let (x, y, w, h, label, sized) = row_footprint(row);
                let (px, py, pw, ph) = v.rect_on_screen(x, y, w, h);
                let is_selected = index == selected.get();
                let disabled = monitor_flag(row, "disable");
                let primary = monitor_flag(row, "primary");
                let label = if primary { format!("* {label}") } else { label };
                if disabled {
                    if is_selected {
                        set_color(cr, &palette.selected, 0.35);
                    } else {
                        set_color(cr, &palette.monitor, 0.35);
                    }
                } else if is_selected {
                    set_color(cr, &palette.selected, 1.0);
                } else {
                    set_color(cr, &palette.monitor, 1.0);
                }
                cr.rectangle(px, py, pw, ph);
                cr.fill_preserve().ok();
                if sized {
                    set_color(cr, &palette.outline, if disabled { 0.3 } else { 0.6 });
                    cr.set_line_width(1.0);
                } else {
                    set_color(cr, &palette.dashed, if disabled { 0.35 } else { 1.0 });
                    cr.set_line_width(2.0);
                    cr.set_dash(&[4.0, 4.0], 0.0);
                }
                cr.stroke().ok();
                cr.set_dash(&[], 0.0);
                let (text_color, text_alpha) = if disabled {
                    (&palette.text, 0.45)
                } else if is_selected {
                    (&palette.selected_text, 1.0)
                } else {
                    (&palette.text, 1.0)
                };
                draw_monitor_label(cr, &label, px, py, pw, ph, text_color, text_alpha);
            }
        }
    });
    root.append(&canvas);

    let mc = MonitorCanvas {
        canvas: canvas.clone(),
        view: view.clone(),
        last_size: last_size.clone(),
    };

    let last_pointer = Rc::new(Cell::new((320.0f64, 200.0f64)));
    {
        let last_pointer = last_pointer.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, x, y| last_pointer.set((x, y)));
        canvas.add_controller(motion);
    }
    {
        let canvas = canvas.clone();
        let view = view.clone();
        let last_pointer = last_pointer.clone();
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let redraw = canvas.clone();
        scroll.connect_scroll(move |_, _dx, dy| {
            if dy == 0.0 {
                return glib::Propagation::Proceed;
            }
            let factor = if dy < 0.0 { 1.1 } else { 1.0 / 1.1 };
            let v = view.get();
            let new_scale = (v.scale * factor).clamp(0.02, 8.0);
            let (px, py) = last_pointer.get();
            let cfg_x = v.to_config_x(px);
            let cfg_y = v.to_config_y(py);
            view.set(Viewport {
                scale: new_scale,
                origin_x: cfg_x - (px - VIEW_PAD) / new_scale,
                origin_y: cfg_y - (py - VIEW_PAD) / new_scale,
            });
            redraw.queue_draw();
            glib::Propagation::Stop
        });
        canvas.add_controller(scroll);
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.set_halign(gtk::Align::Start);
    let recenter = gtk::Button::with_label("Re-center view");
    let recenter_monitor = gtk::Button::with_label("Re-center monitor");
    let remove = gtk::Button::with_label("Remove monitor");
    remove.add_css_class("destructive-action");
    let add = gtk::Button::with_label("Add monitor");
    actions.append(&recenter);
    actions.append(&recenter_monitor);
    actions.append(&remove);
    actions.append(&add);
    root.append(&actions);
    {
        let mc_recenter = mc.clone();
        let rows = rows.clone();
        recenter.connect_clicked(move |_| {
            mc_recenter.refit(&rows.borrow());
        });
    }
    {
        let rows = rows.clone();
        let selected = selected.clone();
        let on_change = on_change.clone();
        let mc = mc.clone();
        recenter_monitor.connect_clicked(move |_| {
            let rows = rows.borrow();
            let Some(row) = rows.get(selected.get()) else {
                return;
            };
            let (_, _, w, h, _, _) = row_footprint(row);
            let others: Vec<(f64, f64, f64, f64)> = rows
                .iter()
                .filter(|other| !std::ptr::eq(*other, row))
                .map(|other| {
                    let (ox, oy, ow, oh, _, _) = row_footprint(other);
                    (ox, oy, ow, oh)
                })
                .collect();
            let (nx, ny) = resolve_release(0.0, 0.0, w, h, &others, SNAP_TOLERANCE);
            row.x.set_value(nx.round());
            row.y.set_value(ny.round());
            drop(rows);
            mc.redraw();
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }

    let form = gtk::Box::new(gtk::Orientation::Vertical, 6);
    form.set_hexpand(true);
    root.append(&form);

    for (index, scan_row) in scan::scan_table(&record.value, true)
        .into_iter()
        .enumerate()
    {
        let row = make_monitor_row(&scan_row, &ctx);
        row.form_box.set_visible(index == 0);
        attach_monitor_row(&form, &rows, &mc, row);
    }

    {
        let click = gtk::GestureClick::new();
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        let mc = mc.clone();
        click.connect_pressed(move |_, _, px, py| {
            let v = view.get();
            let hit = rows.borrow().iter().position(|row| {
                let (x, y, w, h, _, _) = row_footprint(row);
                let (left, top, width, height) = v.rect_on_screen(x, y, w, h);
                px >= left && px <= left + width && py >= top && py <= top + height
            });
            if let Some(index) = hit {
                selected.set(index);
                for (row_index, row) in rows.borrow().iter().enumerate() {
                    row.form_box.set_visible(row_index == index);
                }
                mc.redraw();
            }
        });
        canvas.add_controller(click);
    }

    {
        let drag = gtk::GestureDrag::new();
        let drag_mode = Rc::new(Cell::new(DragMode::None));
        let rows = rows.clone();
        let selected = selected.clone();
        let view = view.clone();
        let on_change = on_change.clone();
        let mc = mc.clone();
        drag.connect_drag_begin({
            let rows = rows.clone();
            let selected = selected.clone();
            let view = view.clone();
            let drag_mode = drag_mode.clone();
            let mc = mc.clone();
            move |_, x, y| {
                let v = view.get();
                let rows = rows.borrow();
                let hit = rows.iter().position(|row| {
                    let (rx, ry, rw, rh, _, _) = row_footprint(row);
                    let (left, top, width, height) = v.rect_on_screen(rx, ry, rw, rh);
                    x >= left && x <= left + width && y >= top && y <= top + height
                });
                drag_mode.set(match hit {
                    Some(index) => {
                        selected.set(index);
                        for (i, r) in rows.iter().enumerate() {
                            r.form_box.set_visible(i == index);
                        }
                        let row = &rows[index];
                        let cfg_x = v.to_config_x(x);
                        let cfg_y = v.to_config_y(y);
                        DragMode::Monitor {
                            start_px: x,
                            start_py: y,
                            grab_dx: cfg_x - row.x.value(),
                            grab_dy: cfg_y - row.y.value(),
                        }
                    }
                    None => DragMode::Pan {
                        origin_x: v.origin_x,
                        origin_y: v.origin_y,
                    },
                });
                mc.redraw();
            }
        });
        drag.connect_drag_update({
            let rows = rows.clone();
            let selected = selected.clone();
            let view = view.clone();
            let on_change = on_change.clone();
            let mc = mc.clone();
            let drag_mode = drag_mode.clone();
            move |gesture, _, _| {
                let (offset_x, offset_y) = gesture.offset().unwrap_or((0.0, 0.0));
                match drag_mode.get() {
                    DragMode::Monitor {
                        start_px,
                        start_py,
                        grab_dx,
                        grab_dy,
                    } => {
                        let v = view.get();
                        let px = start_px + offset_x;
                        let py = start_py + offset_y;
                        let new_x = (v.to_config_x(px) - grab_dx).round();
                        let new_y = (v.to_config_y(py) - grab_dy).round();
                        let rows = rows.borrow();
                        let Some(row) = rows.get(selected.get()) else {
                            return;
                        };
                        row.x.set_value(new_x);
                        row.y.set_value(new_y);
                        drop(rows);
                        mc.redraw();
                        if let Some(cb) = on_change.borrow().as_ref() {
                            cb();
                        }
                    }
                    DragMode::Pan { origin_x, origin_y } => {
                        let v = view.get();
                        view.set(Viewport {
                            scale: v.scale,
                            origin_x: origin_x - offset_x / v.scale,
                            origin_y: origin_y - offset_y / v.scale,
                        });
                        mc.redraw();
                    }
                    DragMode::None => {}
                }
            }
        });
        drag.connect_drag_end({
            let rows = rows.clone();
            let selected = selected.clone();
            let on_change = on_change.clone();
            let mc = mc.clone();
            move |_, _, _| {
                if let DragMode::Monitor { .. } = drag_mode.get() {
                    let rows = rows.borrow();
                    let Some(row) = rows.get(selected.get()) else {
                        return;
                    };
                    let (x, y, w, h, _, _) = row_footprint(row);
                    let others: Vec<(f64, f64, f64, f64)> = rows
                        .iter()
                        .filter(|other| !std::ptr::eq(*other, row))
                        .map(|other| {
                            let (ox, oy, ow, oh, _, _) = row_footprint(other);
                            (ox, oy, ow, oh)
                        })
                        .collect();
                    let (nx, ny) = resolve_release(x, y, w, h, &others, SNAP_TOLERANCE);
                    row.x.set_value(nx.round());
                    row.y.set_value(ny.round());
                    drop(rows);
                    mc.redraw();
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                }
                drag_mode.set(DragMode::None);
            }
        });
        canvas.add_controller(drag);
    }

    {
        let form = form.clone();
        let rows = rows.clone();
        let selected = selected.clone();
        let on_change = on_change.clone();
        let mc = mc.clone();
        remove.connect_clicked(move |_| {
            let index = selected.get();
            let removed = {
                let mut rows = rows.borrow_mut();
                if index >= rows.len() {
                    return;
                }
                rows.remove(index)
            };
            form.remove(&removed.form_box);
            let count = rows.borrow().len();
            let next = if count == 0 { 0 } else { index.min(count - 1) };
            selected.set(next);
            for (position, item) in rows.borrow().iter().enumerate() {
                item.form_box.set_visible(position == next);
            }
            mc.refit(&rows.borrow());
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    {
        let form = form.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let mc = mc.clone();
        let selected = selected.clone();
        add.connect_clicked(move |button| {
            let anchor = button.clone().upcast::<gtk::Widget>();
            let existing_names: Vec<String> = rows
                .borrow()
                .iter()
                .map(|row| row.name.text().to_string())
                .collect();
            let (tx, rx) =
                async_channel::bounded::<Option<Vec<crate::monitor_detect::DetectedMonitor>>>(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(crate::monitor_detect::detect());
            });
            let form = form.clone();
            let rows = rows.clone();
            let on_change = on_change.clone();
            let mc = mc.clone();
            let selected = selected.clone();
            let ctx = ctx.clone();
            open_monitor_picker(&anchor, existing_names, rx, move |picked| {
                let scan_row = monitor_scaffold(&rows.borrow(), picked.as_ref());
                let row = make_monitor_row(&scan_row, &ctx);
                let index = attach_monitor_row(&form, &rows, &mc, row);
                selected.set(index);
                for (position, item) in rows.borrow().iter().enumerate() {
                    item.form_box.set_visible(position == index);
                }
                mc.refit(&rows.borrow());
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        });
    }

    Editor::Monitors {
        root,
        rows,
        on_change,
    }
}

pub(crate) fn monitor_scaffold(
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
            let (rx, _, rw, _, _, _) = row_footprint(row);
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
