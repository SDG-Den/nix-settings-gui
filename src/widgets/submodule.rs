use super::basic::build_raw;
use super::context::{BuildContext, ChangeHook};
use super::dispatch::{build, full_path};
use super::editor::Editor;
use super::names::{field_caption, field_help};
use crate::dsl::WidgetKind;
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::prelude::*;
use std::rc::Rc;
pub(crate) fn build_submodule(
    record: &Record,
    type_str: Option<&str>,
    ctx: &BuildContext,
) -> Editor {
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
        child_ctx.path_prefix = full_path(record, ctx);
        let editor = build(&child, type_str, &child_ctx);
        root.append(&build_child_row(&child, &editor, depth, ctx.expand_details));
        children.push((child, editor));
    }
    Editor::Submodule { root, children }
}

pub(crate) fn flow_box(max_columns: u32) -> gtk::FlowBox {
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
pub(crate) fn build_horizontal_submodule(
    record: &Record,
    type_str: Option<&str>,
    ctx: &BuildContext,
) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let flow = flow_box(record.spec.max_columns.unwrap_or(4));

    let mut children = Vec::new();
    for child in scan_children(record, ctx.advanced) {
        let mut child_ctx = ctx.clone();
        child_ctx.path_prefix = full_path(record, ctx);
        let editor = build(&child, type_str, &child_ctx);
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
        let details = super::InfoDetails::new(
            child.spec.description.as_deref(),
            &child.origin,
            ctx.expand_details,
        );
        cell.append(&details.root);
        cell.append(&editor.root());
        flow.append(&cell);
        children.push((child, editor));
    }
    root.append(&flow);
    Editor::HorizontalSubmodule { root, children }
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

pub(crate) fn build_position_dropdown(value_text: &str, tooltip: Option<&str>) -> Editor {
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

pub(crate) fn build_extra_editor(record: &Record, ctx: &BuildContext) -> Editor {
    let name = record
        .segments
        .last()
        .map(String::as_str)
        .unwrap_or_default();
    if name == "position" {
        build_position_dropdown(&record.value, record.spec.description.as_deref())
    } else {
        build_bespoke_child(record, ctx)
    }
}

pub(crate) fn build_pair_row(
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
            .label(
                record
                    .spec
                    .label
                    .clone()
                    .unwrap_or_else(|| field_caption(&name)),
            )
            .css_classes(["dim-label"])
            .build();
        if let Some(help) = record
            .spec
            .description
            .as_deref()
            .or_else(|| field_help(&name))
        {
            caption.set_tooltip_text(Some(help));
        }
        cell.append(&caption);
        cell.append(&editor.root());
        row.append(&cell);
    }
    row.upcast()
}
pub(crate) fn autohide_delay_pair<'a>(
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

pub(crate) fn horizontal_run(extras: &[&(Record, Editor)], index: usize) -> usize {
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

pub(crate) fn append_flow_group(root: &gtk::Box, group: &[&(Record, Editor)]) {
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
            let label = record.spec.label.clone().unwrap_or_else(|| {
                record
                    .segments
                    .last()
                    .cloned()
                    .unwrap_or_else(|| record.path())
            });
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

pub(crate) fn append_settings_rows(
    root: &gtk::Box,
    extras: &[(Record, Editor)],
    ctx: &BuildContext,
) {
    let visible: Vec<&(Record, Editor)> = extras
        .iter()
        .filter(|(record, _)| !(record.spec.hidden && !ctx.advanced))
        .collect();
    let mut index = 0;
    while index < visible.len() {
        let (record, editor) = visible[index];
        let run = horizontal_run(&visible, index);
        if run >= 2 {
            append_flow_group(root, &visible[index..index + run]);
            index += run;
            continue;
        }
        match autohide_delay_pair(&visible, index) {
            Some((delay_record, delay_editor)) => {
                root.append(&build_pair_row(record, editor, delay_record, delay_editor));
                index += 2;
            }
            None => {
                root.append(&build_child_row(record, editor, 1, ctx.expand_details));
                index += 1;
            }
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

/// One nested control: heading, editor, and a collapsible details row, indented
/// by `depth * 12` pixels (capped at four levels).
pub(crate) fn build_child_row(
    record: &Record,
    editor: &Editor,
    depth: usize,
    expanded: bool,
) -> gtk::Widget {
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
    let details =
        super::InfoDetails::new(record.spec.description.as_deref(), &record.origin, expanded);
    row.append(&details.root);
    row.upcast()
}

/// Shift a fragment record's spans into whole-file offsets.
pub(crate) fn offset_record(record: &mut Record, base: usize, file: &str) {
    record.byte_start += base;
    record.byte_end += base;
    if let Some((start, end)) = record.comment_span {
        record.comment_span = Some((start + base, end + base));
    }
    record.file = file.to_string();
}
#[cfg(test)]
mod tests {
    use super::*;

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
}
