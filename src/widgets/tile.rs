use super::context::{BuildContext, ChangeHook};
use super::dispatch::build;
use super::dms_catalog::{widget_description, widget_icon};
use super::fields::{dim_line, make_attrset_field, AttrsetField};
use super::names::{field_caption, field_help, known_widget_name, widget_display_name};
use super::submodule::hook_notify;
use super::tile_list::{move_tile_between, DragOrigin, SharedBarSections, TileSection};
use super::tile_nix::valid_widget_id;
use crate::dsl::{OptionSpec, PresetFieldSpec, WidgetPresetSpec};
use crate::model::Record;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
/// One draggable widget tile. Order in the enclosing Vec is the position.
#[derive(Clone)]
pub(crate) struct WidgetTile {
    /// Current attrset key; editable in the cog pop-out for unknown ids.
    pub(crate) id: String,
    pub(crate) orig_id: String,
    /// Tuning fields: every scanned field except "enabled" and "_order".
    pub(crate) fields: Rc<RefCell<Vec<AttrsetField>>>,
    /// Original "_order" raw text, empty for tiles added this session.
    pub(crate) orig_order: String,
    /// gtk::ListBoxRow child; row widget is always the tile face box.
    pub(crate) row: gtk::ListBoxRow,
    pub(crate) section: TileSection,
    pub(crate) orig_section: TileSection,
    pub(crate) enabled: Rc<Cell<bool>>,
    pub(crate) orig_enabled: bool,
}
/// Build one freeform field from a preset's declared kind and bounds,
/// falling back to value inference when the preset says nothing.
pub(crate) fn make_field_for_preset(
    name: &str,
    raw: &str,
    spec: Option<&PresetFieldSpec>,
    ctx: &BuildContext,
) -> AttrsetField {
    let trimmed = raw.trim();
    let present = !trimmed.is_empty() && trimmed != "null";
    let source = if present { trimmed } else { "" };
    let option_spec = OptionSpec {
        widget: spec.and_then(|spec| spec.widget),
        choices: spec.and_then(|spec| spec.choices.clone()),
        min: spec.and_then(|spec| spec.min),
        max: spec.and_then(|spec| spec.max),
        step: spec.and_then(|spec| spec.step),
        unit: spec.and_then(|spec| spec.unit.clone()),
        ..OptionSpec::default()
    };
    let record = Record {
        value: source.to_string(),
        spec: option_spec,
        ..Record::default()
    };
    AttrsetField {
        name: name.to_string(),
        editor: build(&record, None, ctx),
        orig_value: source.to_string(),
        orig_present: present,
    }
}

/// One pop-out line for a field: preset label and description when the
/// marker supplies them, the shared caption and help otherwise.
pub(crate) fn tile_field_line(field: &AttrsetField, specs: &[PresetFieldSpec]) -> gtk::Box {
    let spec = specs.iter().find(|spec| spec.name == field.name);
    let caption = spec
        .and_then(|spec| spec.label.clone())
        .unwrap_or_else(|| field_caption(&field.name));
    let help = spec
        .and_then(|spec| spec.description.clone())
        .or_else(|| field_help(&field.name).map(str::to_string));
    let line = dim_line(&caption, &field.editor.root());
    if let Some(help) = help {
        line.set_tooltip_text(Some(&help));
    }
    line
}

/// Move a tile to a new position and redraw the rows in list order.
pub(crate) fn move_tile(
    list: &gtk::ListBox,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    from: usize,
    to: isize,
    on_change: &ChangeHook,
) {
    let count = tiles.borrow().len() as isize;
    if count == 0 || from as isize >= count {
        return;
    }
    let to = to.clamp(0, count - 1) as usize;
    if to == from {
        return;
    }
    let tile = {
        let mut guard = tiles.borrow_mut();
        guard.remove(from)
    };
    tiles.borrow_mut().insert(to, tile);
    {
        let guard = tiles.borrow();
        for tile in guard.iter() {
            list.remove(&tile.row);
        }
        for tile in guard.iter() {
            list.append(&tile.row);
        }
    }
    if let Some(cb) = on_change.borrow().as_ref() {
        cb();
    }
}

/// Insertion index for a pointer position: the first row whose midpoint
/// sits below `y`.
pub(crate) fn drop_index(list: &gtk::ListBox, y: f64) -> i32 {
    let children = list.observe_children();
    let mut index = 0;
    for position in 0..children.n_items() {
        let Some(child) = children.item(position) else {
            continue;
        };
        let Ok(widget) = child.downcast::<gtk::Widget>() else {
            continue;
        };
        let Some(bounds) = widget.compute_bounds(list) else {
            continue;
        };
        if y < bounds.y() as f64 + bounds.height() as f64 / 2.0 {
            return index;
        }
        index += 1;
    }
    index
}

/// Clear the drop marker classes from every row. The marker is a CSS border on
/// the row itself, so it never inserts extra children into the list.
pub(crate) fn hide_all_indicators(list: &gtk::ListBox) {
    let children = list.observe_children();
    for position in 0..children.n_items() {
        let Some(child) = children.item(position) else {
            continue;
        };
        let Ok(widget) = child.downcast::<gtk::Widget>() else {
            continue;
        };
        widget.remove_css_class("dms-drop-before");
        widget.remove_css_class("dms-drop-after");
    }
}

pub(crate) fn show_indicator(list: &gtk::ListBox, index: i32) {
    hide_all_indicators(list);
    let children = list.observe_children();
    if children.n_items() == 0 {
        return;
    }
    let count = children.n_items() as i32;
    let position = index.max(0);
    if let Some(child) = children.item(position.min(count - 1) as u32) {
        if let Ok(widget) = child.downcast::<gtk::Widget>() {
            if position >= count {
                widget.add_css_class("dms-drop-after");
            } else {
                widget.add_css_class("dms-drop-before");
            }
        }
    }
}

/// Drop target on a tile list: a 2px separator tracks the pointer between
/// rows, and a drop reorders within the list or moves across bar sections.
pub(crate) fn attach_tile_dnd(
    list: &gtk::ListBox,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    section: TileSection,
    shared: Option<SharedBarSections>,
    on_change: &ChangeHook,
) {
    let drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    list.add_controller(drop.clone());

    let enter_list = list.clone();
    drop.connect_enter(move |_, _, y| {
        show_indicator(&enter_list, drop_index(&enter_list, y));
        gtk::gdk::DragAction::MOVE
    });

    let motion_list = list.clone();
    drop.connect_motion(move |_, _, y| {
        show_indicator(&motion_list, drop_index(&motion_list, y));
        gtk::gdk::DragAction::MOVE
    });

    let leave_list = list.clone();
    drop.connect_leave(move |_| hide_all_indicators(&leave_list));

    let drop_tiles = tiles.clone();
    let drop_hook = on_change.clone();
    let drop_list = list.clone();
    let drop_shared = shared.clone();
    drop.connect_drop(move |_, value, _, y| {
        hide_all_indicators(&drop_list);
        let Ok(payload) = value.get::<String>() else {
            return false;
        };
        let Some((source_section, source_index)) =
            payload.split_once(':').and_then(|(section, index)| {
                Some((
                    TileSection::from_key(section)?,
                    index.parse::<usize>().ok()?,
                ))
            })
        else {
            return false;
        };
        let target = drop_index(&drop_list, y) as usize;
        if source_section != section {
            let Some(shared) = drop_shared.as_ref() else {
                return false;
            };
            return move_tile_between(
                shared,
                source_section,
                source_index,
                section,
                target,
                &drop_hook,
            );
        }
        let count = drop_tiles.borrow().len();
        if source_index >= count {
            return false;
        }
        let target = if target > source_index {
            target - 1
        } else {
            target
        };
        move_tile(
            &drop_list,
            &drop_tiles,
            source_index,
            target as isize,
            &drop_hook,
        );
        true
    });
}
/// The cog pop-out: id entry for unknown ids, one line per field, and
/// (for custom ids) an add-setting row. Everything is built eagerly so the
/// field editors connect once, here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_tile_popout(
    id: &str,
    preset: Option<&WidgetPresetSpec>,
    fields: &Rc<RefCell<Vec<AttrsetField>>>,
    field_specs: &[PresetFieldSpec],
    label: &gtk::Label,
    row: &gtk::ListBoxRow,
    ctx: &BuildContext,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    on_change: &ChangeHook,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    popover.set_child(Some(&content));

    if preset.is_none() && (ctx.advanced || known_widget_name(id).is_none()) {
        let entry = gtk::Entry::new();
        entry.set_text(id);
        entry.set_hexpand(true);
        let line = dim_line("Id", &entry);
        line.set_tooltip_text(Some("Widget id stored in the configuration file."));
        content.append(&line);
        let (tiles_id, row_id, label_id, hook) =
            (tiles.clone(), row.clone(), label.clone(), on_change.clone());
        entry.connect_changed(move |entry| {
            let text = entry.text().to_string();
            let mut tiles = tiles_id.borrow_mut();
            if let Some(tile) = tiles.iter_mut().find(|tile| tile.row == row_id) {
                tile.id = text.clone();
                label_id.set_text(&widget_display_name(&text));
            }
            drop(tiles);
            if let Some(cb) = hook.borrow().as_ref() {
                cb();
            }
        });
    }

    let fields_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&fields_box);
    for field in fields.borrow().iter() {
        fields_box.append(&tile_field_line(field, field_specs));
    }

    if preset.is_none() {
        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = gtk::Entry::new();
        name.set_placeholder_text(Some("name"));
        name.set_width_chars(10);
        let value = gtk::Entry::new();
        value.set_placeholder_text(Some("value"));
        value.set_hexpand(true);
        let add = gtk::Button::with_label("+");
        add.set_tooltip_text(Some("Add setting"));
        add_row.append(&name);
        add_row.append(&value);
        add_row.append(&add);
        content.append(&add_row);

        let (fields_add, lines_add, hook) = (fields.clone(), fields_box.clone(), on_change.clone());
        let specs_add: Vec<PresetFieldSpec> = field_specs.to_vec();
        let ctx_add = ctx.clone();
        add.connect_clicked(move |_| {
            let field_name = name.text().trim().to_string();
            if field_name.is_empty()
                || !valid_widget_id(&field_name)
                || field_name == "enabled"
                || field_name == "_order"
                || fields_add
                    .borrow()
                    .iter()
                    .any(|field| field.name == field_name)
            {
                return;
            }
            let mut field = make_attrset_field(&field_name, &value.text(), &ctx_add);
            field.orig_present = true;
            field.editor.connect_changed(&hook_notify(&hook));
            lines_add.append(&tile_field_line(&field, &specs_add));
            fields_add.borrow_mut().push(field);
            name.set_text("");
            value.set_text("");
            if let Some(cb) = hook.borrow().as_ref() {
                cb();
            }
        });
    }

    popover
}

/// Build one tile: face (grip, label, cog, remove), the cog pop-out, a
/// right-click move menu, the drag source, and the remove button. `orig_id`
/// is empty for tiles added this session.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_widget_tile(
    id: &str,
    orig_id: &str,
    orig_order: &str,
    preset: Option<WidgetPresetSpec>,
    fields: Rc<RefCell<Vec<AttrsetField>>>,
    field_specs: Vec<PresetFieldSpec>,
    section: TileSection,
    ctx: &BuildContext,
    tiles: &Rc<RefCell<Vec<WidgetTile>>>,
    list: &gtk::ListBox,
    on_change: &ChangeHook,
    enabled: bool,
    orig_enabled: bool,
    drag_source: Option<DragOrigin>,
    shared: Option<SharedBarSections>,
) -> WidgetTile {
    let display = preset
        .as_ref()
        .map(|preset| preset.label.clone())
        .unwrap_or_else(|| widget_display_name(id));

    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);
    row.add_css_class("dms-widget-row");
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    body.set_margin_start(10);
    body.set_margin_end(10);
    row.set_child(Some(&body));

    let grip = gtk::Image::from_icon_name("open-menu-symbolic");
    grip.add_css_class("dim-label");
    grip.set_tooltip_text(Some("Drag to reorder"));
    let grip_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    grip_box.set_size_request(24, 24);
    grip_box.set_valign(gtk::Align::Center);
    grip_box.append(&grip);
    body.append(&grip_box);

    let icon_name = preset
        .as_ref()
        .and_then(|preset| preset.icon.clone())
        .unwrap_or_else(|| widget_icon(id).to_string());
    let icon = gtk::Image::from_icon_name(&icon_name);
    icon.set_pixel_size(22);
    icon.add_css_class("dms-widget-icon");
    body.append(&icon);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(&display)
        .css_classes(["heading"])
        .build();
    text.append(&label);
    let description = preset
        .as_ref()
        .and_then(|preset| preset.description.clone())
        .unwrap_or_else(|| widget_description(id).to_string());
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(&description)
        .css_classes(["dim-label", "caption"])
        .build();
    text.append(&subtitle);
    body.append(&text);

    let enabled_state = Rc::new(Cell::new(enabled));

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    body.append(&spacer);

    if !section.is_center() {
        let has_fields = !fields.borrow().is_empty();
        let settings_declared = preset.as_ref().is_none_or(|preset| preset.has_settings);
        let editable_unknown =
            preset.is_none() && (ctx.advanced || known_widget_name(id).is_none());
        let show_cog = (has_fields && settings_declared) || editable_unknown;
        if show_cog {
            let cog = gtk::Button::from_icon_name("emblem-system-symbolic");
            cog.add_css_class("flat");
            cog.set_tooltip_text(Some("Widget settings"));
            body.append(&cog);
            let popover = build_tile_popout(
                id,
                preset.as_ref(),
                &fields,
                &field_specs,
                &label,
                &row,
                ctx,
                tiles,
                on_change,
            );
            popover.set_parent(&cog);
            popover.set_position(gtk::PositionType::Right);
            let for_cog = popover;
            cog.connect_clicked(move |_| for_cog.popup());
        }
    }

    if !section.is_center() {
        let toggle = gtk::Switch::new();
        toggle.set_active(enabled);
        toggle.set_valign(gtk::Align::Center);
        toggle.set_halign(gtk::Align::End);
        toggle.set_tooltip_text(Some("Whether this widget is enabled."));
        let state = enabled_state.clone();
        let hook = on_change.clone();
        toggle.connect_active_notify(move |toggle| {
            state.set(toggle.is_active());
            if let Some(cb) = hook.borrow().as_ref() {
                cb();
            }
        });
        body.append(&toggle);
    }

    for field in fields.borrow().iter() {
        field.editor.connect_changed(&hook_notify(on_change));
    }

    let menu = gtk::Popover::new();
    menu.set_parent(&body);
    menu.set_position(gtk::PositionType::Right);
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(8);
    menu_box.set_margin_end(8);
    menu.set_child(Some(&menu_box));
    for (text, target) in [
        ("Move up", -1isize),
        ("Move down", 1isize),
        ("Move to top", isize::MIN),
        ("Move to bottom", isize::MAX),
    ] {
        let button = gtk::Button::with_label(text);
        button.add_css_class("flat");
        let (tiles_move, row_move, list_move, hook) =
            (tiles.clone(), row.clone(), list.clone(), on_change.clone());
        let menu_move = menu.clone();
        button.connect_clicked(move |_| {
            menu_move.popdown();
            let from = tiles_move
                .borrow()
                .iter()
                .position(|tile| tile.row == row_move);
            if let Some(from) = from {
                let last = tiles_move.borrow().len() as isize - 1;
                let to = match target {
                    isize::MIN => 0,
                    isize::MAX => last,
                    delta => from as isize + delta,
                };
                move_tile(&list_move, &tiles_move, from, to.clamp(0, last), &hook);
            }
        });
        menu_box.append(&button);
    }
    if let Some(shared) = shared {
        for target in [
            TileSection::BarLeft,
            TileSection::BarCenter,
            TileSection::BarRight,
        ] {
            if target == section || section == TileSection::Dock {
                continue;
            }
            let label = match target {
                TileSection::BarLeft => "Move to left",
                TileSection::BarCenter => "Move to center",
                TileSection::BarRight => "Move to right",
                TileSection::Dock => "Move to dock",
            };
            let button = gtk::Button::with_label(label);
            button.add_css_class("flat");
            let (tiles_move, row_move, hook, menu_move) =
                (tiles.clone(), row.clone(), on_change.clone(), menu.clone());
            let shared_move = shared.clone();
            button.connect_clicked(move |_| {
                menu_move.popdown();
                let index = tiles_move
                    .borrow()
                    .iter()
                    .position(|tile| tile.row == row_move);
                if let Some(index) = index {
                    move_tile_between(&shared_move, section, index, target, usize::MAX, &hook);
                }
            });
            menu_box.append(&button);
        }
    }

    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    body.add_controller(right_click.clone());
    let menu_click = menu.clone();
    right_click.connect_pressed(move |_, _, _, _| menu_click.popup());

    let drag = gtk::DragSource::new();
    drag.set_actions(gtk::gdk::DragAction::MOVE);
    grip_box.set_cursor_from_name(Some("grab"));
    grip_box.add_controller(drag.clone());
    let (drag_tiles, drag_row) = (tiles.clone(), row.clone());
    {
        let drag_source = drag_source.clone();
        drag.connect_prepare(move |_, _, _| {
            let index = drag_tiles
                .borrow()
                .iter()
                .position(|tile| tile.row == drag_row)? as u32;
            if let Some(state) = drag_source.as_ref() {
                *state.borrow_mut() = Some((section, index as usize));
            }
            let payload = format!("{}:{}", section.key(), index);
            Some(gtk::gdk::ContentProvider::for_value(&payload.to_value()))
        });
    }
    let end_list = list.clone();
    let end_source = drag_source.clone();
    drag.connect_drag_end(move |_, _, _| {
        hide_all_indicators(&end_list);
        if let Some(state) = end_source.as_ref() {
            *state.borrow_mut() = None;
        }
    });

    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove widget"));
    body.append(&remove);
    let (remove_tiles, remove_row, remove_list, remove_hook) =
        (tiles.clone(), row.clone(), list.clone(), on_change.clone());
    remove.connect_clicked(move |_| {
        let position = remove_tiles
            .borrow()
            .iter()
            .position(|tile| tile.row == remove_row);
        if let Some(index) = position {
            remove_tiles.borrow_mut().remove(index);
            remove_list.remove(&remove_row);
            if let Some(cb) = remove_hook.borrow().as_ref() {
                cb();
            }
        }
    });

    WidgetTile {
        id: id.to_string(),
        orig_id: orig_id.to_string(),
        orig_order: orig_order.to_string(),
        fields,
        row,
        section,
        orig_section: section,
        enabled: enabled_state,
        orig_enabled,
    }
}
