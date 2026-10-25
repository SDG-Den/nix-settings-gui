use super::basic::build_raw;
use super::context::{BuildContext, ChangeHook};
use super::dispatch::build;
use super::editor::Editor;
use super::icon::{build_icon, icon_dir};
use super::nix::{attr_key, entry_text, string_literal};
use crate::dsl::{OptionSpec, WidgetKind};
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[cfg(test)]
mod display_tests;

/// The editor shape of one table column, from `columnTypes`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ColumnKind {
    Value(value::ValueKind),
    Icon,
}

/// Paths a table cell needs to resolve files relative to its module.
pub(crate) struct CellContext {
    pub(crate) module_dir: PathBuf,
    pub(crate) icon_dir: PathBuf,
}

impl CellContext {
    fn new(record: &Record, ctx: &BuildContext) -> Self {
        let module_dir = Path::new(&record.file)
            .parent()
            .map(super::nix::normalize_path)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| super::nix::normalize_path(Path::new(&ctx.base)));
        let icon_dir = icon_dir(&record.spec, &module_dir);
        Self {
            module_dir,
            icon_dir,
        }
    }
}
/// One cell of a table row: a keyed value with an inferred editor.
#[derive(Clone)]
pub(crate) struct TableCell {
    /// The column name this cell belongs to.
    pub(crate) column: String,
    pub(crate) key: gtk::Entry,
    pub(crate) editor: Editor,
    pub(crate) orig_key: String,
    pub(crate) orig_value: String,
}

/// One editable row of a table.
#[derive(Clone)]
pub(crate) struct TableRow {
    pub(crate) key_entry: gtk::Entry,
    pub(crate) orig_key: String,
    pub(crate) cells: Vec<TableCell>,
    pub(crate) spec: OptionSpec,
}

pub(crate) fn table_row_identity_locked(list: bool, key_editable: bool) -> bool {
    !list && !key_editable
}

pub(crate) fn table_first_cell_static(
    list: bool,
    is_first: bool,
    first_value_editable: bool,
) -> bool {
    list && is_first && !first_value_editable
}

/// A table over an attribute set of sets (`table-attrset`) or a list of sets
/// (`table-list`), with automatic per-cell widgets and built-in row buttons.
pub(crate) fn build_table(record: &Record, ctx: &BuildContext, list: bool) -> Editor {
    let columns = record.spec.columns.clone().unwrap_or_default();
    if columns.is_empty() {
        let (view, root) = build_raw(&record.value, &record.spec);
        return Editor::Raw { view, root };
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    root.set_halign(gtk::Align::Fill);
    let key_editable = record.spec.key_editable;
    let first_value_editable = record.spec.first_value_editable;

    let scan_rows = scan::scan_table(&record.value, list);
    let first_row = scan_rows.first();
    let type_hints = column_type_hints(record.spec.column_types.as_deref(), &columns, first_row);
    let widens: Vec<bool> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| column_widens(first_row, column, type_hints[index]))
        .collect();
    let cell_ctx = CellContext::new(record, ctx);

    // One grid for the whole table: each column sizes to the widest cell it
    // holds, so the header and every row share the same x positions.
    let grid = (!key_editable).then(|| {
        let grid = gtk::Grid::new();
        grid.set_column_spacing(6);
        grid.set_row_spacing(6);
        grid.set_hexpand(true);
        grid.set_halign(gtk::Align::Fill);
        grid
    });
    // The attrset key occupies column 0 in `table-attrset`; `table-list` rows
    // start with their first cell instead.
    let base = if list { 0 } else { 1 };
    if let Some(grid) = &grid {
        for (index, column) in columns.iter().enumerate() {
            if list && !first_value_editable && index == 0 {
                continue;
            }
            let caption = gtk::Label::builder()
                .label(column)
                .xalign(0.0)
                .hexpand(widens[index])
                .css_classes(["dim-label", "caption"])
                .build();
            grid.attach(&caption, (base + index) as i32, 0, 1, 1);
        }
        root.append(grid);
    }

    let rows: Rc<RefCell<Vec<TableRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    for scan_row in &scan_rows {
        if scan_row.spec.hidden && !ctx.advanced {
            continue;
        }
        let parent = row_parent(&root, &grid, &rows);
        let row = make_table_row(
            &parent,
            &rows,
            &columns,
            list,
            key_editable,
            first_value_editable,
            &type_hints,
            &widens,
            &cell_ctx,
            scan_row,
            ctx,
            None,
        );
        rows.borrow_mut().push(row);
    }

    let add = gtk::Button::with_label("Add row");
    add.set_halign(gtk::Align::Start);
    {
        let root = root.clone();
        let rows = rows.clone();
        let columns = columns.clone();
        let ctx = ctx.clone();
        let on_change = on_change.clone();
        let grid = grid.clone();
        let type_hints = type_hints.clone();
        let widens = widens.clone();
        add.connect_clicked(move |_| {
            let key = if !key_editable && !list {
                let existing: Vec<String> = rows
                    .borrow()
                    .iter()
                    .map(|row| row.key_entry.text().to_string())
                    .collect();
                let mut n = 1;
                let mut candidate = "row".to_string();
                while existing.contains(&candidate) {
                    n += 1;
                    candidate = format!("row{n}");
                }
                candidate
            } else {
                String::new()
            };
            let scan_row = scan::TableRowScan {
                key,
                fields: columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        let hint = type_hints.get(index).copied().flatten();
                        (column.clone(), empty_cell_literal(hint))
                    })
                    .collect(),
                spec: OptionSpec::default(),
            };
            let update = on_change.borrow().clone();
            let parent = row_parent(&root, &grid, &rows);
            let row = make_table_row(
                &parent,
                &rows,
                &columns,
                list,
                key_editable,
                first_value_editable,
                &type_hints,
                &widens,
                &cell_ctx,
                &scan_row,
                &ctx,
                update.as_ref(),
            );
            rows.borrow_mut().push(row);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    root.append(&add);

    Editor::Table {
        root,
        rows,
        list,
        key_editable,
        columns,
        on_change,
    }
}

/// Where the next row lands: a shared grid column slot when the table has one,
/// else its own horizontal box.
pub(crate) enum TableParent<'a> {
    Rows(&'a gtk::Box),
    Grid { grid: &'a gtk::Grid, row: i32 },
}

pub(crate) fn row_parent<'a>(
    root: &'a gtk::Box,
    grid: &'a Option<gtk::Grid>,
    rows: &Rc<RefCell<Vec<TableRow>>>,
) -> TableParent<'a> {
    match grid {
        Some(grid) => TableParent::Grid {
            grid,
            row: rows.borrow().len() as i32 + 1,
        },
        None => TableParent::Rows(root),
    }
}

/// How a row takes itself out of the table when its remove button is hit.
pub(crate) enum RowDetach {
    Boxes { parent: gtk::Box, row_box: gtk::Box },
    Grid { grid: gtk::Grid, row: i32 },
}

impl RowDetach {
    pub(crate) fn detach(&self) {
        match self {
            RowDetach::Boxes { parent, row_box } => parent.remove(row_box),
            RowDetach::Grid { grid, row } => grid_remove_row(grid, *row),
        }
    }
}

/// Drop every widget of `row` from a grid and shift the rows below it up, so
/// row indices stay dense after a removal.
pub(crate) fn grid_remove_row(grid: &gtk::Grid, row: i32) {
    let Some(manager) = grid.layout_manager() else {
        return;
    };
    let children = grid.observe_children();
    let mut doomed = Vec::new();
    let mut shifts = Vec::new();
    for index in 0..children.n_items() {
        let Some(object) = children.item(index) else {
            continue;
        };
        let Ok(child) = object.downcast::<gtk::Widget>() else {
            continue;
        };
        let Ok(cell) = manager
            .layout_child(&child)
            .downcast::<gtk::GridLayoutChild>()
        else {
            continue;
        };
        let child_row = cell.row();
        if child_row == row {
            doomed.push(child);
        } else if child_row > row {
            shifts.push((child, cell.column(), child_row));
        }
    }
    for child in doomed {
        grid.remove(&child);
    }
    shifts.sort_by_key(|(_, _, child_row)| *child_row);
    for (child, column, child_row) in shifts {
        grid.remove(&child);
        grid.attach(&child, column, child_row - 1, 1, 1);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_table_row(
    parent: &TableParent<'_>,
    rows: &Rc<RefCell<Vec<TableRow>>>,
    columns: &[String],
    list: bool,
    key_editable: bool,
    first_value_editable: bool,
    type_hints: &[Option<ColumnKind>],
    widens: &[bool],
    cell_ctx: &CellContext,
    scan_row: &scan::TableRowScan,
    ctx: &BuildContext,
    update: Option<&Rc<dyn Fn()>>,
) -> TableRow {
    let key_entry = gtk::Entry::new();
    key_entry.set_width_chars(12);
    if !list {
        key_entry.set_text(&scan_row.key);
    }

    // The row's widgets in column order; grid tables attach one per slot,
    // `keyEditable` tables collect them into their own horizontal box.
    let mut slots: Vec<gtk::Widget> = Vec::new();
    let mut slot_widens: Vec<bool> = Vec::new();

    if !list {
        if table_row_identity_locked(list, key_editable) {
            let heading_text = scan_row
                .spec
                .label
                .clone()
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| scan_row.key.clone());
            let heading = gtk::Label::builder()
                .label(heading_text)
                .xalign(0.0)
                .tooltip_text("Row key")
                .build();
            slots.push(heading.upcast::<gtk::Widget>());
        } else {
            key_entry.set_tooltip_text(Some("Row key"));
            slots.push(key_entry.clone().upcast::<gtk::Widget>());
        }
        slot_widens.push(false);
        if let Some(update) = update {
            let update = update.clone();
            key_entry.connect_changed(move |_| update());
        }
    }

    let mut cells = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let hint = type_hints.get(index).copied().flatten();
        let widen = widens.get(index).copied().unwrap_or(false);
        let (field_key, field_value) = scan_row
            .fields
            .iter()
            .find(|(key, _)| key == column)
            .cloned()
            .unwrap_or_else(|| (column.clone(), empty_cell_literal(hint)));
        let is_first = index == 0;

        let cell_key = gtk::Entry::new();
        cell_key.set_text(&field_key);
        cell_key.set_width_chars(10);

        let editor = build_cell(&field_value, hint, ctx, cell_ctx);
        if key_editable {
            slots.push(cell_key.clone().upcast::<gtk::Widget>());
            slots.push(gtk::Label::new(Some("=")).upcast::<gtk::Widget>());
            slots.push(editor.root());
            slot_widens.extend([false, false, widen]);
        } else if table_first_cell_static(list, is_first, first_value_editable) {
            let label = gtk::Label::builder()
                .label(entry_text(&field_value))
                .xalign(0.0)
                .build();
            let caption = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            caption.append(&label);
            caption.append(&gtk::Label::new(Some(":")));
            slots.push(caption.upcast::<gtk::Widget>());
            slot_widens.push(widen);
        } else {
            slots.push(editor.root());
            slot_widens.push(widen);
        }

        if let Some(update) = update {
            let update_key = update.clone();
            cell_key.connect_changed(move |_| update_key());
            editor.connect_changed(update);
        }
        cells.push(TableCell {
            column: column.clone(),
            key: cell_key,
            editor,
            orig_key: field_key,
            orig_value: field_value,
        });
    }

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove row"));
    slots.push(remove.clone().upcast::<gtk::Widget>());
    slot_widens.push(false);

    let detach = match parent {
        TableParent::Rows(parent_root) => {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row_box.set_hexpand(true);
            for slot in &slots {
                row_box.append(slot);
            }
            parent_root.append(&row_box);
            RowDetach::Boxes {
                parent: (**parent_root).clone(),
                row_box,
            }
        }
        TableParent::Grid { grid, row } => {
            for (index, slot) in slots.iter().enumerate() {
                slot.set_hexpand(slot_widens.get(index).copied().unwrap_or(false));
                slot.set_valign(gtk::Align::Center);
                grid.attach(slot, index as i32, *row, 1, 1);
            }
            RowDetach::Grid {
                grid: (**grid).clone(),
                row: *row,
            }
        }
    };
    {
        let rows = rows.clone();
        let key_entry = key_entry.clone();
        let update = update.cloned();
        remove.connect_clicked(move |_| {
            detach.detach();
            rows.borrow_mut().retain(|row| row.key_entry != key_entry);
            if let Some(cb) = update.as_ref() {
                cb();
            }
        });
    }

    TableRow {
        key_entry,
        orig_key: scan_row.key.clone(),
        cells,
        spec: scan_row.spec.clone(),
    }
}

/// The scanned source text of one column in the first row, `null` when the
/// column has no sample.
pub(crate) fn column_sample<'a>(
    first_row: Option<&'a scan::TableRowScan>,
    column: &str,
) -> &'a str {
    first_row
        .and_then(|row| row.fields.iter().find(|(key, _)| key == column))
        .map(|(_, value)| value.as_str())
        .unwrap_or("null")
}

pub(crate) fn column_expands(first_row: Option<&scan::TableRowScan>, column: &str) -> bool {
    let sample = column_sample(first_row, column);
    !matches!(
        value::classify(sample),
        value::ValueKind::Bool | value::ValueKind::Int | value::ValueKind::Float
    )
}

/// Whether a column gives its extra width back to the window: text cells take
/// slack, switches and spins stay compact. The declared column type decides
/// when the sample is still `null`.
pub(crate) fn column_widens(
    first_row: Option<&scan::TableRowScan>,
    column: &str,
    hint: Option<ColumnKind>,
) -> bool {
    match hint {
        None => column_expands(first_row, column),
        Some(ColumnKind::Icon) => true,
        Some(ColumnKind::Value(hint)) => matches!(
            cell_widget_kind(column_sample(first_row, column), Some(hint)),
            WidgetKind::Text
        ),
    }
}

/// The editor for one table cell. A real value always wins; a `null` or
/// bare-expression cell falls back to the column's declared type.
pub(crate) fn cell_widget_kind(value: &str, hint: Option<value::ValueKind>) -> WidgetKind {
    match value::classify(value) {
        value::ValueKind::Bool => WidgetKind::Switch,
        value::ValueKind::Int | value::ValueKind::Float => WidgetKind::Spin,
        value::ValueKind::Null | value::ValueKind::Other => match hint {
            Some(value::ValueKind::Bool) => WidgetKind::Switch,
            Some(value::ValueKind::Int | value::ValueKind::Float) => WidgetKind::Spin,
            _ => WidgetKind::Text,
        },
        _ => WidgetKind::Text,
    }
}

/// The scaffold value written for a freshly added cell, so the row opens with
/// the right editor and serialises a sane default when saved untouched.
pub(crate) fn empty_cell_literal(hint: Option<ColumnKind>) -> String {
    match hint {
        Some(ColumnKind::Value(value::ValueKind::Bool)) => "false",
        Some(ColumnKind::Value(value::ValueKind::Int | value::ValueKind::Float)) => "0",
        Some(ColumnKind::Value(value::ValueKind::String)) => "\"\"",
        _ => "null",
    }
    .to_string()
}

/// One `columnTypes` token, matched case-insensitively. Anything unrecognised
/// yields no hint.
pub(crate) fn column_type_hint(token: &str) -> Option<ColumnKind> {
    match token.trim().to_ascii_lowercase().as_str() {
        "bool" | "boolean" => Some(ColumnKind::Value(value::ValueKind::Bool)),
        "int" | "integer" => Some(ColumnKind::Value(value::ValueKind::Int)),
        "float" => Some(ColumnKind::Value(value::ValueKind::Float)),
        "str" | "string" | "raw" => Some(ColumnKind::Value(value::ValueKind::String)),
        "icon" | "image" => Some(ColumnKind::Icon),
        _ => None,
    }
}

/// Per-column type hints, taken from `columnTypes` when declared and
/// otherwise sampled from the first scanned row. Extra types are ignored and
/// missing ones leave the column unhinted.
pub(crate) fn column_type_hints(
    column_types: Option<&[String]>,
    columns: &[String],
    first_row: Option<&scan::TableRowScan>,
) -> Vec<Option<ColumnKind>> {
    match column_types {
        Some(types) => (0..columns.len())
            .map(|index| types.get(index).and_then(|token| column_type_hint(token)))
            .collect(),
        None => columns
            .iter()
            .map(
                |column| match value::classify(column_sample(first_row, column)) {
                    kind @ (value::ValueKind::Bool
                    | value::ValueKind::Int
                    | value::ValueKind::Float
                    | value::ValueKind::String) => Some(ColumnKind::Value(kind)),
                    _ => None,
                },
            )
            .collect(),
    }
}

/// A cell editor inferred from the cell's value: a switch for a boolean, a spin
/// for a number, a text field otherwise.
pub(crate) fn build_cell(
    value: &str,
    hint: Option<ColumnKind>,
    ctx: &BuildContext,
    cell: &CellContext,
) -> Editor {
    if hint == Some(ColumnKind::Icon) {
        return build_icon(value, &cell.module_dir, &cell.icon_dir, ctx);
    }
    let kind = cell_widget_kind(
        value,
        hint.and_then(|hint| match hint {
            ColumnKind::Value(kind) => Some(kind),
            ColumnKind::Icon => None,
        }),
    );
    let spec = OptionSpec {
        widget: Some(kind),
        ..OptionSpec::default()
    };
    let record = Record {
        value: value.to_string(),
        spec,
        ..Record::default()
    };
    build(&record, None, ctx)
}

/// Rebuild the `@table-item` comment for a row from its scanned spec, so a
/// save does not wipe the label/hidden/visibleWhen directives.
fn option_spec_to_comment(spec: &OptionSpec) -> String {
    let mut parts = Vec::new();
    if let Some(label) = &spec.label {
        parts.push(format!("label = {};", string_literal(label)));
    }
    if spec.hidden {
        parts.push("hidden = true;".to_string());
    }
    if let Some(visibility) = &spec.visible_when {
        parts.push(format!(
            "visibleWhen.path = {};",
            string_literal(&visibility.path)
        ));
        if let Some(equals) = &visibility.equals {
            parts.push(format!("visibleWhen.equals = {equals};"));
        }
        if let Some(not_equals) = &visibility.not_equals {
            parts.push(format!("visibleWhen.notEquals = {not_equals};"));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("/* @table-item {} */", parts.join(" "))
}

/// Serialize a table editor's rows back to an attribute set or a list.
pub(crate) fn table_nix(editor: &Editor) -> String {
    let Editor::Table {
        rows,
        list,
        key_editable,
        columns,
        ..
    } = editor
    else {
        return "{ }".to_string();
    };
    let mut parts = Vec::new();
    for row in rows.borrow().iter() {
        let mut fields = Vec::new();
        for (index, cell) in row.cells.iter().enumerate() {
            let key = if *key_editable {
                cell.key.text().trim().to_string()
            } else {
                columns
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| cell.column.clone())
            };
            fields.push(format!("{} = {};", attr_key(&key), cell.editor.to_nix()));
        }
        if *list {
            parts.push(format!("{{ {} }}", fields.join(" ")));
        } else {
            let raw_key = row.key_entry.text();
            let key_text = raw_key.trim();
            let key = if key_text.is_empty() {
                if row.orig_key.trim().is_empty() {
                    "row".to_string()
                } else {
                    row.orig_key.clone()
                }
            } else {
                key_text.to_string()
            };
            let comment = option_spec_to_comment(&row.spec);
            let entry = format!("{} = {{ {} }};", attr_key(&key), fields.join(" "));
            if comment.is_empty() {
                parts.push(entry);
            } else {
                parts.push(format!("{comment} {entry}"));
            }
        }
    }
    if *list {
        format!("[ {} ]", parts.join(" "))
    } else {
        format!("{{ {} }}", parts.join(" "))
    }
}
pub(crate) fn connect_table_row(row: &TableRow, update: &Rc<dyn Fn()>) {
    let update_entry = update.clone();
    row.key_entry.connect_changed(move |_| update_entry());
    for cell in &row.cells {
        let update_key = update.clone();
        cell.key.connect_changed(move |_| update_key());
        cell.editor.connect_changed(update);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_expand_follows_cell_editor_kind() {
        let row = scan::TableRowScan {
            key: "youtube".into(),
            fields: vec![
                ("enable".into(), "true".into()),
                ("name".into(), "\"YouTube\"".into()),
                ("icon".into(), "null".into()),
            ],
            spec: OptionSpec::default(),
        };
        assert!(!column_expands(Some(&row), "enable"));
        assert!(column_expands(Some(&row), "name"));
        assert!(column_expands(Some(&row), "icon"));
        assert!(column_expands(None, "url"));
    }

    #[test]
    fn craftapps_rows_lock_names_and_edit_first_cell() {
        assert!(table_row_identity_locked(false, false));
        assert!(!table_row_identity_locked(false, true));
        assert!(!table_row_identity_locked(true, false));
        assert!(table_first_cell_static(true, true, false));
        assert!(!table_first_cell_static(false, true, false));
        assert!(!table_first_cell_static(true, false, false));
        assert!(!table_first_cell_static(true, true, true));
    }

    #[test]
    fn cell_widget_kind_prefers_value_then_type_hint() {
        let bool_hint = Some(value::ValueKind::Bool);
        assert_eq!(cell_widget_kind("null", bool_hint), WidgetKind::Switch);
        assert_eq!(
            cell_widget_kind("null", Some(value::ValueKind::Int)),
            WidgetKind::Spin
        );
        assert_eq!(
            cell_widget_kind("null", Some(value::ValueKind::Float)),
            WidgetKind::Spin
        );
        assert_eq!(cell_widget_kind("null", None), WidgetKind::Text);
        assert_eq!(cell_widget_kind("\"x\"", bool_hint), WidgetKind::Text);
        assert_eq!(cell_widget_kind("true", None), WidgetKind::Switch);
        assert_eq!(
            cell_widget_kind("42", Some(value::ValueKind::String)),
            WidgetKind::Spin
        );
    }

    fn value_hint(kind: value::ValueKind) -> Option<ColumnKind> {
        Some(ColumnKind::Value(kind))
    }

    #[test]
    fn empty_cell_literals_follow_the_column_hint() {
        assert_eq!(
            empty_cell_literal(value_hint(value::ValueKind::Bool)),
            "false".to_string()
        );
        assert_eq!(
            empty_cell_literal(value_hint(value::ValueKind::Int)),
            "0".to_string()
        );
        assert_eq!(
            empty_cell_literal(value_hint(value::ValueKind::Float)),
            "0".to_string()
        );
        assert_eq!(
            empty_cell_literal(value_hint(value::ValueKind::String)),
            "\"\"".to_string()
        );
        assert_eq!(
            empty_cell_literal(Some(ColumnKind::Icon)),
            "null".to_string()
        );
        assert_eq!(empty_cell_literal(None), "null".to_string());
    }

    #[test]
    fn column_type_tokens_map_to_value_kinds() {
        assert_eq!(column_type_hint("bool"), value_hint(value::ValueKind::Bool));
        assert_eq!(
            column_type_hint("Boolean"),
            value_hint(value::ValueKind::Bool)
        );
        assert_eq!(column_type_hint("int"), value_hint(value::ValueKind::Int));
        assert_eq!(
            column_type_hint("INTEGER"),
            value_hint(value::ValueKind::Int)
        );
        assert_eq!(
            column_type_hint("float"),
            value_hint(value::ValueKind::Float)
        );
        assert_eq!(
            column_type_hint("str"),
            value_hint(value::ValueKind::String)
        );
        assert_eq!(
            column_type_hint("string"),
            value_hint(value::ValueKind::String)
        );
        assert_eq!(
            column_type_hint("raw"),
            value_hint(value::ValueKind::String)
        );
        assert_eq!(column_type_hint("icon"), Some(ColumnKind::Icon));
        assert_eq!(column_type_hint("image"), Some(ColumnKind::Icon));
        assert_eq!(column_type_hint("nope"), None);
    }

    #[test]
    fn column_type_hints_align_positionally_with_columns() {
        let columns = ["enable".to_string(), "value".to_string()];
        let declared = ["bool".to_string(), "str".to_string()];
        assert_eq!(
            column_type_hints(Some(&declared), &columns, None),
            vec![
                value_hint(value::ValueKind::Bool),
                value_hint(value::ValueKind::String)
            ]
        );
        let short = ["bool".to_string()];
        assert_eq!(
            column_type_hints(Some(&short), &columns, None),
            vec![value_hint(value::ValueKind::Bool), None]
        );
        let row = scan::TableRowScan {
            key: "llm".into(),
            fields: vec![
                ("enable".into(), "true".into()),
                ("value".into(), "\"null\"".into()),
            ],
            spec: OptionSpec::default(),
        };
        assert_eq!(
            column_type_hints(None, &columns, Some(&row)),
            vec![
                value_hint(value::ValueKind::Bool),
                value_hint(value::ValueKind::String)
            ]
        );
        assert_eq!(column_type_hints(None, &columns, None), vec![None, None]);
    }
}
