use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::fields::{attrset_field_nix, build_field_cell, field, AttrsetField};
use super::monitor_geometry::{effective_size, monitor_geometry, monitor_orig, resolve_release};
use super::monitor_picker::open_monitor_picker;
use super::nix::{entry_text, number_literal, quote};
use crate::dsl::OptionSpec;
use crate::model::Record;
use crate::monitor_detect::DetectedMode;
use crate::scan;
use crate::value;
use gtk::prelude::*;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
/// One monitor rule of the `monitors` widget: core geometry editors plus the
/// rule's remaining fields and the original source text per field.
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
    pub(crate) remove: gtk::Button,
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

/// One monitor rule: core geometry on a grid, the remaining fields behind an
/// "Extra rules" expander, and a remove button.
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
pub(crate) struct MonitorView {
    pub(crate) scale: f64,
    pub(crate) min_x: f64,
    pub(crate) min_y: f64,
}
pub(crate) fn attach_monitor_row(
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

pub(crate) fn build_monitors(record: &Record) -> Editor {
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
