use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::fields::{
    attrset_field_nix, capture_field_values, dim_line_with_help, make_attrset_field,
    make_dropdown_field, popout_button_bar, restore_field_values, AttrsetField,
};
use super::names::{
    menu_field_help, menu_provider_fields, menu_provider_initial, menu_provider_labels,
    menu_provider_value, title_case_id, widget_display_name, MENU_PROVIDERS,
};
use super::nix::{attr_key, expr_literal, string_literal};
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) const POPOUT_MAIN_FIELDS: &[&str] =
    &["provider", "target", "action", "title", "iconFont"];
pub(crate) const POPOUT_CONDITION_FIELDS: &[&str] = &["checked", "when", "disabled"];
/// The values a pop-out held when it opened, so Cancel can write them back
/// into the live row widgets.
pub(crate) struct PopoutSnapshot {
    pub(crate) name: String,
    pub(crate) modifier: String,
    pub(crate) keys: String,
    pub(crate) fields: Vec<(String, String)>,
}

/// One menu row's pop-out state, cloned into the shared edit/cancel wiring of
/// the menu pop-out window.
#[derive(Clone)]
pub(crate) struct MenuPopout {
    pub(crate) popup_box: gtk::Box,
    pub(crate) fields: Rc<RefCell<Vec<AttrsetField>>>,
    pub(crate) blocks: Vec<(String, gtk::Box)>,
    pub(crate) snapshot: Rc<RefCell<Option<PopoutSnapshot>>>,
}

impl MenuPopout {
    /// Capture the row's current field values as the restore point.
    pub(crate) fn capture(&self) {
        let values = capture_field_values(&self.fields.borrow());
        *self.snapshot.borrow_mut() = Some(PopoutSnapshot {
            name: String::new(),
            modifier: String::new(),
            keys: String::new(),
            fields: values,
        });
    }

    /// Put the captured values back into the row's live editors.
    pub(crate) fn cancel(&self) {
        let taken = self.snapshot.borrow_mut().take();
        if let Some(snapshot) = taken {
            restore_field_values(&self.fields.borrow(), &snapshot.fields);
            apply_provider_visibility(&self.fields.borrow(), &self.blocks);
        }
    }

    /// Drop the restore point without touching the editors, as Apply does.
    pub(crate) fn discard(&self) {
        self.snapshot.borrow_mut().take();
    }
}

#[derive(Clone)]
pub(crate) struct MenuRow {
    pub(crate) section: String,
    pub(crate) key: gtk::Entry,
    pub(crate) orig_key: String,
    pub(crate) icon: String,
    pub(crate) label: gtk::Entry,
    pub(crate) orig_label: String,
    pub(crate) aliases: gtk::Entry,
    pub(crate) orig_aliases: String,
    pub(crate) popup_fields: Rc<RefCell<Vec<AttrsetField>>>,
    pub(crate) popup_box: gtk::Box,
    pub(crate) row_box: gtk::Box,
    pub(crate) remove: gtk::Button,
    pub(crate) snapshot: Rc<RefCell<Option<PopoutSnapshot>>>,
}
pub(crate) fn menu_aliases_text(raw: &str) -> String {
    raw.split('"')
        .skip(1)
        .step_by(2)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn menu_aliases_nix(text: &str) -> Option<String> {
    let items: Vec<String> = text
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| expr_literal(&s))
        .collect();
    (!items.is_empty()).then(|| format!("aliases = {};", value::serialize_list(&items)))
}
/// The pop-out layout: the root box plus one named block per field, so
/// provider changes can show or hide individual blocks later.
pub(crate) struct PopoutLayout {
    pub(crate) root: gtk::Box,
    pub(crate) blocks: Vec<(String, gtk::Box)>,
}

pub(crate) fn menu_popup_fields(fields: &[AttrsetField], on_resize: Rc<dyn Fn()>) -> PopoutLayout {
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let mut blocks: Vec<(String, gtk::Box)> = Vec::new();
    for field in fields
        .iter()
        .filter(|field| POPOUT_MAIN_FIELDS.contains(&field.name.as_str()))
    {
        let block = dim_line_with_help(
            &field.name,
            menu_field_help(&field.name),
            &field.editor.root(),
        );
        blocks.push((field.name.clone(), block.clone()));
        vbox.append(&block);
    }
    let advanced = gtk::Expander::new(Some("Advanced conditions"));
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for field in fields
        .iter()
        .filter(|field| POPOUT_CONDITION_FIELDS.contains(&field.name.as_str()))
    {
        let block = dim_line_with_help(
            &field.name,
            menu_field_help(&field.name),
            &field.editor.root(),
        );
        blocks.push((field.name.clone(), block.clone()));
        inner.append(&block);
    }
    advanced.set_child(Some(&inner));
    {
        let on_resize = on_resize.clone();
        advanced.connect_expanded_notify(move |_| on_resize());
    }
    vbox.append(&advanced);
    PopoutLayout {
        root: vbox,
        blocks,
    }
}

/// The provider dropdown's currently selected label, or None when the field
/// is missing or is not a dropdown.
fn provider_selected_label(fields: &[AttrsetField]) -> Option<String> {
    fields
        .iter()
        .find(|field| field.name == "provider")
        .and_then(|field| match &field.editor {
            Editor::Dropdown { combo, choices } => {
                choices.get(combo.selected() as usize).cloned()
            }
            _ => None,
        })
}

/// Show or hide the provider-dependent main-box blocks. `target` and `action`
/// are the only conditional fields; provider, title, iconFont and the
/// expander's three conditions stay shown for every row kind.
fn apply_provider_visibility(fields: &[AttrsetField], blocks: &[(String, gtk::Box)]) {
    let label = provider_selected_label(fields).unwrap_or_default();
    let visible = menu_provider_fields(&label);
    for (name, block) in blocks {
        if name == "target" || name == "action" {
            block.set_visible(visible.contains(&name.as_str()));
        }
    }
}

pub(crate) fn connect_menu_edit(
    edit: &gtk::Button,
    row_box: &gtk::Box,
    popup: &gtk::Window,
    popup_content: &gtk::Box,
    popout: &MenuPopout,
    active: &Rc<RefCell<Option<MenuPopout>>>,
) {
    let row_box = row_box.clone();
    let popup = popup.clone();
    let popup_content = popup_content.clone();
    let popout = popout.clone();
    let active = active.clone();
    edit.connect_clicked(move |_| {
        let already_open = active
            .borrow()
            .as_ref()
            .is_some_and(|open| open.popup_box == popout.popup_box);
        if !already_open {
            if let Some(previous) = active.replace(Some(popout.clone())) {
                previous.cancel();
            }
            popout.capture();
        }
        while let Some(child) = popup_content.first_child() {
            popup_content.remove(&child);
        }
        if let Some(win) = row_box
            .root()
            .and_then(|w| w.downcast::<gtk::Window>().ok())
        {
            popup.set_transient_for(Some(&win));
        }
        popup_content.append(&popout.popup_box);
        popup.set_default_size(440, -1);
        popup.present();
    });
}

pub(crate) fn make_menu_row(
    section: &str,
    rel_key: &str,
    fields: &[(String, String)],
    ctx: &BuildContext,
    popup: &gtk::Window,
    popup_content: &gtk::Box,
    active: &Rc<RefCell<Option<MenuPopout>>>,
) -> MenuRow {
    let raw_of = |name: &str| {
        fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r.trim().to_string())
            .unwrap_or_default()
    };
    let unquote = |raw: &str| raw.trim().trim_matches('"').to_string();

    let key = gtk::Entry::new();
    key.set_width_chars(16);
    key.set_text(rel_key);
    let orig_key = if rel_key.is_empty() {
        section.to_string()
    } else {
        format!("{section}.{rel_key}")
    };

    let icon = unquote(&raw_of("icon"));
    let label = gtk::Entry::new();
    label.set_text(&unquote(&raw_of("label")));
    let orig_label = label.text().to_string();
    let aliases = gtk::Entry::new();
    aliases.set_text(&menu_aliases_text(&raw_of("aliases")));
    let orig_aliases = aliases.text().to_string();

    let provider_raw = raw_of("provider");
    let provider_unquoted = unquote(&provider_raw);
    let in_table = MENU_PROVIDERS
        .iter()
        .any(|c| c.label == provider_unquoted || c.provider == provider_unquoted);
    let extra = (!provider_unquoted.is_empty() && !in_table).then(|| provider_unquoted.clone());
    let labels = menu_provider_labels(extra);
    let initial = menu_provider_initial(&provider_unquoted, &raw_of("target"), &raw_of("action"));
    let selected = labels.iter().position(|l| *l == initial).unwrap_or(0);
    let popup_fields: Rc<RefCell<Vec<AttrsetField>>> = Rc::new(RefCell::new(
        POPOUT_MAIN_FIELDS
            .iter()
            .chain(POPOUT_CONDITION_FIELDS.iter())
            .map(|name| match *name {
                "provider" => make_dropdown_field(name, &provider_raw, &labels, selected),
                _ => make_attrset_field(name, &raw_of(name), ctx),
            })
            .collect(),
    ));
    let on_resize: Rc<dyn Fn()> = {
        let popup = popup.clone();
        Rc::new(move || popup.set_default_size(440, -1))
    };
    let layout = menu_popup_fields(&popup_fields.borrow(), on_resize.clone());
    let popup_box = layout.root.clone();
    let (bar, cancel, apply) = popout_button_bar();
    popup_box.append(&bar);
    apply_provider_visibility(&popup_fields.borrow(), &layout.blocks);
    if let Some(AttrsetField {
        editor: Editor::Dropdown { combo, .. },
        ..
    }) = popup_fields.borrow().iter().find(|f| f.name == "provider")
    {
        let fields = popup_fields.clone();
        let blocks = layout.blocks.clone();
        let on_resize = on_resize.clone();
        combo.connect_selected_notify(move |_| {
            apply_provider_visibility(&fields.borrow(), &blocks);
            on_resize();
        });
    }

    let icon_widget: gtk::Widget = if icon.is_empty() {
        gtk::Label::new(Some(&orig_label.chars().next().unwrap_or(' ').to_string())).upcast()
    } else {
        gtk::Image::from_icon_name(&icon).upcast()
    };
    let action = raw_of("action");
    let target = raw_of("target");
    let kind = if !action.is_empty() && action != "null" {
        "shell"
    } else if !target.is_empty() && target != "null" {
        "link"
    } else if !provider_unquoted.is_empty() && provider_unquoted != "null" {
        "provider"
    } else {
        "group"
    };
    let caption = gtk::Label::new(Some(kind));
    caption.add_css_class("dim-label");
    caption.add_css_class("caption");

    let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    main.set_hexpand(true);
    main.append(&icon_widget);
    if !rel_key.is_empty() {
        main.append(&key);
    }
    label.set_hexpand(true);
    main.append(&label);
    main.append(&aliases);
    main.append(&caption);

    let edit = gtk::Button::with_label("Edit");
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    main.append(&edit);
    main.append(&remove);

    let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row_box.set_hexpand(true);
    row_box.append(&main);

    let row = MenuRow {
        section: section.to_string(),
        key,
        orig_key,
        icon,
        label,
        orig_label,
        aliases,
        orig_aliases,
        popup_fields,
        popup_box,
        row_box,
        remove,
        snapshot: Rc::new(RefCell::new(None)),
    };

    let popout = MenuPopout {
        popup_box: row.popup_box.clone(),
        fields: row.popup_fields.clone(),
        blocks: layout.blocks.clone(),
        snapshot: row.snapshot.clone(),
    };
    connect_menu_edit(&edit, &row.row_box, popup, popup_content, &popout, active);
    {
        let popup = popup.clone();
        cancel.connect_clicked(move |_| popup.close());
    }
    {
        let popup = popup.clone();
        let active = active.clone();
        apply.connect_clicked(move |_| {
            popout.discard();
            let mut open = active.borrow_mut();
            if open
                .as_ref()
                .is_some_and(|current| current.popup_box == popout.popup_box)
            {
                *open = None;
            }
            popup.hide();
        });
    }
    row
}

/// Serialize one row's provider dropdown label back to a `provider` line.
/// Blank-provider labels map to None so no line is written even when the
/// row originally had one.
pub(crate) fn provider_nix_for_label(label: &str) -> Option<String> {
    let value = menu_provider_value(label);
    (!value.is_empty()).then(|| format!("provider = {};", string_literal(value)))
}

fn provider_field_nix(field: &AttrsetField) -> Option<String> {
    match &field.editor {
        Editor::Dropdown { combo, choices } => {
            let label = choices.get(combo.selected() as usize)?;
            provider_nix_for_label(label)
        }
        _ => attrset_field_nix(field),
    }
}

fn provider_field_unchanged(field: &AttrsetField) -> bool {
    match &field.editor {
        Editor::Dropdown { combo, choices } => {
            let selected = choices
                .get(combo.selected() as usize)
                .map(String::as_str)
                .unwrap_or("");
            let value = menu_provider_value(selected);
            let orig = field.orig_value.trim();
            let orig = if orig == "null" {
                ""
            } else {
                orig.trim_matches('"')
            };
            value == orig
        }
        _ => field.editor.unchanged(&field.orig_value),
    }
}

pub(crate) fn menu_rows_nix(rows: &[MenuRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let full = if row.key.text().is_empty() {
                row.section.clone()
            } else {
                format!("{}.{}", row.section, row.key.text())
            };
            let mut fields: Vec<String> = Vec::new();
            fields.push(format!("icon = {};", string_literal(&row.icon)));
            fields.push(format!("label = {};", string_literal(&row.label.text())));
            if let Some(line) = menu_aliases_nix(&row.aliases.text()) {
                fields.push(line);
            }
            for field in row.popup_fields.borrow().iter() {
                let line = if field.name == "provider" {
                    provider_field_nix(field)
                } else {
                    attrset_field_nix(field)
                };
                if let Some(line) = line {
                    fields.push(line);
                }
            }
            format!("{} = {{ {} }};", attr_key(&full), fields.join(" "))
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

pub(crate) fn menu_rows_unchanged(rows: &Rc<RefCell<Vec<MenuRow>>>) -> bool {
    rows.borrow().iter().all(|row| {
        let full = if row.key.text().is_empty() {
            row.section.clone()
        } else {
            format!("{}.{}", row.section, row.key.text())
        };
        full == row.orig_key
            && row.label.text() == row.orig_label
            && row.aliases.text() == row.orig_aliases
            && row
                .popup_fields
                .borrow()
                .iter()
                .all(|f| {
                    if f.name == "provider" {
                        provider_field_unchanged(f)
                    } else {
                        f.editor.unchanged(&f.orig_value)
                    }
                })
    })
}

pub(crate) fn connect_menu_row(row: &MenuRow, update: &Rc<dyn Fn()>) {
    let update_key = update.clone();
    row.key.connect_changed(move |_| update_key());
    let update_label = update.clone();
    row.label.connect_changed(move |_| update_label());
    let update_aliases = update.clone();
    row.aliases.connect_changed(move |_| update_aliases());
    for field in row.popup_fields.borrow().iter() {
        field.editor.connect_changed(update);
    }
}

pub(crate) fn attach_menu_row(
    container: &gtk::Box,
    rows: &Rc<RefCell<Vec<MenuRow>>>,
    row: MenuRow,
    on_change: &ChangeHook,
    popup_content: &gtk::Box,
) {
    {
        let container = container.clone();
        let rows = rows.clone();
        let row_box = row.row_box.clone();
        let key = row.key.clone();
        let popup_box = row.popup_box.clone();
        let popup_content = popup_content.clone();
        let snapshot = row.snapshot.clone();
        let on_change = on_change.clone();
        row.remove.connect_clicked(move |_| {
            container.remove(&row_box);
            if popup_box.parent() == Some(popup_content.clone().upcast::<gtk::Widget>()) {
                popup_box.unparent();
            }
            snapshot.borrow_mut().take();
            rows.borrow_mut().retain(|r| r.key != key);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    container.append(&row.row_box);
    rows.borrow_mut().push(row);
}
pub(crate) fn build_menu(record: &Record, ctx: &BuildContext) -> Editor {
    let on_change: ChangeHook = Rc::new(RefCell::new(None));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_hexpand(true);
    let rows = Rc::new(RefCell::new(Vec::new()));
    let active: Rc<RefCell<Option<MenuPopout>>> = Rc::new(RefCell::new(None));

    let popup = gtk::Window::builder()
        .hide_on_close(true)
        .default_width(440)
        .title("Entry details")
        .build();
    let popup_content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    popup_content.set_margin_top(12);
    popup_content.set_margin_bottom(12);
    popup_content.set_margin_start(12);
    popup_content.set_margin_end(12);
    popup.set_child(Some(&popup_content));
    {
        let active = active.clone();
        popup.connect_close_request(move |_| {
            let taken = active.borrow_mut().take();
            if let Some(popout) = taken {
                popout.cancel();
            }
            glib::Propagation::Proceed
        });
    }
    {
        let target = popup.clone();
        let controller = gtk::EventControllerKey::new();
        controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                target.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        popup.add_controller(controller);
    }

    let sections: Rc<RefCell<Vec<(String, gtk::Box)>>> = Rc::new(RefCell::new(Vec::new()));
    for scan_row in scan::scan_table(&record.value, false) {
        let (section, rel) = match scan_row.key.split_once('.') {
            Some((p, c)) => (p.to_string(), c.to_string()),
            None => (scan_row.key.clone(), String::new()),
        };
        let existing = sections
            .borrow()
            .iter()
            .find(|(name, _)| *name == section)
            .map(|(_, c)| c.clone());
        let container = match existing {
            Some(c) => c,
            None => {
                let header = gtk::Label::builder()
                    .xalign(0.0)
                    .label(widget_display_name(&section))
                    .css_classes(["heading"])
                    .build();
                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
                vbox.set_hexpand(true);
                vbox.set_margin_start(12);
                root.append(&header);
                root.append(&vbox);
                sections.borrow_mut().push((section.clone(), vbox.clone()));
                vbox
            }
        };
        let row = make_menu_row(
            &section,
            &rel,
            &scan_row.fields,
            ctx,
            &popup,
            &popup_content,
            &active,
        );
        attach_menu_row(&container, &rows, row, &on_change, &popup_content);
    }

    let add_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let names: Vec<String> = sections
        .borrow()
        .iter()
        .map(|(name, _)| widget_display_name(name))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let parent_menu = gtk::DropDown::from_strings(&refs);
    let child = gtk::Entry::new();
    child.set_placeholder_text(Some("child-id"));
    child.set_width_chars(14);
    let new_label = gtk::Entry::new();
    new_label.set_placeholder_text(Some("Label"));
    new_label.set_hexpand(true);
    let add = gtk::Button::with_label("Add");
    add_box.append(&parent_menu);
    add_box.append(&child);
    add_box.append(&new_label);
    add_box.append(&add);

    let section_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let section_id = gtk::Entry::new();
    section_id.set_placeholder_text(Some("section-id"));
    section_id.set_width_chars(14);
    let section_label = gtk::Entry::new();
    section_label.set_placeholder_text(Some("Label"));
    section_label.set_hexpand(true);
    let add_section = gtk::Button::with_label("Add section");
    section_box.append(&section_id);
    section_box.append(&section_label);
    section_box.append(&add_section);

    root.append(&add_box);
    root.append(&section_box);
    {
        let child = child.clone();
        let new_label = new_label.clone();
        let parent_menu = parent_menu.clone();
        let sections = sections.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let popup = popup.clone();
        let popup_content = popup_content.clone();
        let ctx = ctx.clone();
        let active = active.clone();
        add.connect_clicked(move |_| {
            let idx = parent_menu.selected() as usize;
            let Some((section, container)) = sections.borrow().get(idx).cloned() else {
                return;
            };
            let fields = vec![
                ("label".to_string(), string_literal(&new_label.text())),
                ("icon".to_string(), String::new()),
            ];
            let row = make_menu_row(
                &section,
                &child.text(),
                &fields,
                &ctx,
                &popup,
                &popup_content,
                &active,
            );
            let hook: Rc<dyn Fn()> = {
                let on_change = on_change.clone();
                Rc::new(move || {
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                })
            };
            connect_menu_row(&row, &hook);
            attach_menu_row(&container, &rows, row, &on_change, &popup_content);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    {
        let root = root.clone();
        let add_box = add_box.clone();
        let section_box = section_box.clone();
        let section_id = section_id.clone();
        let section_label = section_label.clone();
        let parent_menu = parent_menu.clone();
        let sections = sections.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        let popup = popup.clone();
        let popup_content = popup_content.clone();
        let ctx = ctx.clone();
        let active = active.clone();
        add_section.connect_clicked(move |_| {
            let id = section_id.text().trim().to_string();
            if id.is_empty() || id.contains('.') || id.contains('"') {
                return;
            }
            if rows.borrow().iter().any(|r| r.orig_key == id && r.key.text().is_empty()) {
                return;
            }
            let existing = sections
                .borrow()
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, c)| c.clone());
            let container = match existing {
                Some(c) => c,
                None => {
                    let header = gtk::Label::builder()
                        .xalign(0.0)
                        .label(widget_display_name(&id))
                        .css_classes(["heading"])
                        .build();
                    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
                    vbox.set_hexpand(true);
                    vbox.set_margin_start(12);
                    root.remove(&add_box);
                    root.remove(&section_box);
                    root.append(&header);
                    root.append(&vbox);
                    root.append(&add_box);
                    root.append(&section_box);
                    sections.borrow_mut().push((id.clone(), vbox.clone()));
                    vbox
                }
            };
            let label_text = {
                let text = section_label.text();
                if text.trim().is_empty() {
                    title_case_id(&id)
                } else {
                    text.to_string()
                }
            };
            let fields = vec![
                ("label".to_string(), string_literal(&label_text)),
                ("icon".to_string(), String::new()),
            ];
            let row = make_menu_row(
                &id,
                "",
                &fields,
                &ctx,
                &popup,
                &popup_content,
                &active,
            );
            let hook: Rc<dyn Fn()> = {
                let on_change = on_change.clone();
                Rc::new(move || {
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                })
            };
            connect_menu_row(&row, &hook);
            attach_menu_row(&container, &rows, row, &on_change, &popup_content);
            let names: Vec<String> = sections
                .borrow()
                .iter()
                .map(|(name, _)| widget_display_name(name))
                .collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            parent_menu.set_model(Some(&gtk::StringList::new(&refs)));
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }
    Editor::Menu {
        root,
        rows,
        on_change,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popout_field_order_matches_help() {
        for name in POPOUT_MAIN_FIELDS
            .iter()
            .chain(POPOUT_CONDITION_FIELDS.iter())
        {
            assert!(
                menu_field_help(name).is_some(),
                "missing menu help for {name}"
            );
        }
        for name in POPOUT_MAIN_FIELDS {
            assert!(
                !POPOUT_CONDITION_FIELDS.contains(name),
                "{name} appears in both popout lists"
            );
        }
    }

    #[test]
    fn provider_nix_omits_custom_and_open() {
        assert_eq!(provider_nix_for_label("Run Command"), None);
        assert_eq!(provider_nix_for_label("Open Application"), None);
        assert_eq!(
            provider_nix_for_label("Apps List"),
            Some("provider = \"apps\";".to_string())
        );
    }

    #[test]
    fn provider_visibility_hides_target_for_custom() {
        let custom = menu_provider_fields("Run Command");
        assert!(custom.contains(&"action"));
        assert!(!custom.contains(&"target"));
        assert!(menu_provider_fields("Apps List").is_empty());
        assert!(menu_provider_fields("nope").is_empty());
    }
}
