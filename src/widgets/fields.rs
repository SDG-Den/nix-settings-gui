use super::context::BuildContext;
use super::dispatch::build;
use super::editor::Editor;
use super::nix::attr_key;
use crate::dsl::{OptionSpec, WidgetKind};
use crate::model::Record;
use crate::value;
use gtk::prelude::*;
/// One freeform field of a bespoke widget row: a named value with an inferred
/// editor.
pub(crate) struct AttrsetField {
    pub(crate) name: String,
    pub(crate) editor: Editor,
    pub(crate) orig_value: String,
    pub(crate) orig_present: bool,
}
/// Split a comma-separated field text into its trimmed, non-empty items.
pub(crate) fn csv_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}
pub(crate) fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.trim())
}

/// An editor for one freeform field value, inferring the widget from the value.
pub(crate) fn build_field_cell(source: &str, ctx: &BuildContext) -> Editor {
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
pub(crate) fn make_attrset_field(name: &str, raw: &str, ctx: &BuildContext) -> AttrsetField {
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

/// One freeform field of a bespoke row, edited as a fixed dropdown. `choices`
/// are the dropdown labels (not Nix literals); the caller maps the selected
/// label back to a value, so entries that serialize to nothing stay distinct.
pub(crate) fn make_dropdown_field(
    name: &str,
    raw: &str,
    choices: &[String],
    selected: usize,
) -> AttrsetField {
    let t = raw.trim();
    let present = !t.is_empty() && t != "null";
    let source = if present { t } else { "" };
    let editor = Editor::Dropdown {
        combo: build_dropdown_combo(choices, selected),
        choices: choices.to_vec(),
    };
    AttrsetField {
        name: name.to_string(),
        editor,
        orig_value: source.to_string(),
        orig_present: present,
    }
}

fn build_dropdown_combo(choices: &[String], selected: usize) -> gtk::DropDown {
    let labels: Vec<&str> = choices.iter().map(String::as_str).collect();
    let combo = gtk::DropDown::from_strings(&labels);
    combo.set_selected(selected as u32);
    combo.set_halign(gtk::Align::Start);
    combo
}

/// Whether an editor holds something beyond its neutral value.
pub(crate) fn field_has_content(editor: &Editor) -> bool {
    match editor {
        Editor::Switch(w) => w.is_active(),
        Editor::Spin(w) => w.value() != 0.0,
        Editor::Text(w) => !w.text().is_empty(),
        Editor::Path(w) => !w.text().is_empty(),
        _ => true,
    }
}

pub(crate) fn field_nix(
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

pub(crate) fn attrset_field_nix(field: &AttrsetField) -> Option<String> {
    field_nix(
        &field.name,
        &field.editor,
        &field.orig_value,
        field.orig_present,
    )
}
/// A horizontal line of a dim caption label and a child control, used in the
/// keybind edit popup.
pub(crate) fn dim_line(name: &str, child: &impl gtk::prelude::IsA<gtk::Widget>) -> gtk::Box {
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
pub(crate) fn dim_line_with_help(
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
pub(crate) fn editor_snapshot(editor: &Editor) -> String {
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
/// ever text, switch, spin or dropdown editors, so the remaining variants stay
/// untouched.
pub(crate) fn editor_restore(editor: &Editor, value: &str) {
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

pub(crate) fn capture_field_values(fields: &[AttrsetField]) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|field| (field.name.clone(), editor_snapshot(&field.editor)))
        .collect()
}

pub(crate) fn restore_field_values(fields: &[AttrsetField], values: &[(String, String)]) {
    for (name, value) in values {
        if let Some(field) = fields.iter().find(|field| field.name == *name) {
            editor_restore(&field.editor, value);
        }
    }
}

/// The Cancel/Apply bar both pop-outs place at the bottom of their box.
pub(crate) fn popout_button_bar() -> (gtk::Box, gtk::Button, gtk::Button) {
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
