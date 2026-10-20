use super::context::{BuildContext, ChangeHook};
use super::editor::Editor;
use super::fields::{
    attrset_field_nix, capture_field_values, csv_list, dim_line, dim_line_with_help, field,
    make_attrset_field, popout_button_bar, restore_field_values, AttrsetField,
};
use super::menu::PopoutSnapshot;
use super::names::{bind_display_label, keybind_field_help};
use super::nix::{attr_key, entry_text, parse_string_literal, quote};
use crate::dsl::OptionSpec;
use crate::model::Record;
use crate::scan;
use crate::value;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
/// One bind of the `keybinds` widget: a display label and description on the
/// main line, the bind's full field set in a hidden edit popup.
pub(crate) struct KeybindRow {
    pub(crate) key: gtk::Entry,
    pub(crate) orig_key: String,
    pub(crate) modifier: gtk::Entry,
    pub(crate) keys: gtk::Entry,
    pub(crate) orig_modifier: String,
    pub(crate) orig_keys: String,
    pub(crate) fields: Rc<RefCell<Vec<AttrsetField>>>,
    pub(crate) row_box: gtk::Box,
    pub(crate) remove: gtk::Button,
    pub(crate) popup: gtk::Window,
    pub(crate) snapshot: Rc<RefCell<Option<PopoutSnapshot>>>,
}
/// Install the one-time stylesheet that paints the keybind preview pill.
pub(crate) fn install_keybind_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".keybind-preview {
            background-color: alpha(@window_fg_color, 0.08);
            border-radius: 8px;
            padding: 2px 8px;
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
/// The `modifier+keys` preview shown on a keybind row.
pub(crate) fn bind_preview(modifier: &str, keys: &str) -> String {
    let mut parts: Vec<String> = csv_list(modifier)
        .into_iter()
        .map(|m| m.to_uppercase())
        .collect();
    parts.extend(csv_list(keys).into_iter().map(|k| k.to_uppercase()));
    parts.join("+")
}

/// A comma-separated list field rendered as a Nix list of strings.
pub(crate) fn csv_list_nix(text: &str) -> String {
    let items: Vec<String> = csv_list(text).iter().map(|t| quote(t)).collect();
    value::serialize_list(&items)
}
/// Keep a horizontal separator between consecutive keybind rows: drop any
/// existing separators and re-insert them between the current row widgets.
pub(crate) fn sync_keybind_separators(container: &gtk::Box) {
    let mut children: Vec<gtk::Widget> = Vec::new();
    let mut child = container.first_child();
    while let Some(widget) = child {
        children.push(widget.clone());
        child = widget.next_sibling();
    }
    let mut separators: Vec<gtk::Widget> = Vec::new();
    let mut rows: Vec<gtk::Widget> = Vec::new();
    for widget in children {
        if widget.downcast_ref::<gtk::Separator>().is_some() {
            separators.push(widget);
        } else {
            rows.push(widget);
        }
    }
    for separator in separators {
        container.remove(&separator);
    }
    for pair in rows.windows(2) {
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(6);
        separator.set_margin_bottom(6);
        container.insert_child_after(&separator, Some(&pair[0]));
    }
}

/// One bind of the `keybinds` widget: a display label and description on the
/// main line, the preview pill, and an edit popup holding every field.
pub(crate) fn make_keybind_row(scan_row: &scan::TableRowScan, ctx: &BuildContext) -> KeybindRow {
    install_keybind_css();
    let row_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    row_box.set_hexpand(true);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title_stack = gtk::Box::new(gtk::Orientation::Vertical, 2);
    title_stack.set_hexpand(true);

    let label_override = scan_row.spec.label.clone();
    let title = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(bind_display_label(&scan_row.key, label_override.as_deref()))
        .build();
    title_stack.append(&title);

    let desc_caption = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .label(entry_text(
            field(&scan_row.fields, "description").unwrap_or(""),
        ))
        .css_classes(["dim-label", "caption"])
        .build();
    title_stack.append(&desc_caption);
    main.append(&title_stack);

    let modifier = gtk::Entry::new();
    modifier.set_width_chars(16);
    modifier.set_tooltip_text(Some("Modifiers, comma separated"));
    modifier.set_text(&list_text(
        field(&scan_row.fields, "modifier").unwrap_or(""),
    ));

    let keys = gtk::Entry::new();
    keys.set_width_chars(12);
    keys.set_tooltip_text(Some("Keys, comma separated"));
    keys.set_text(&list_text(field(&scan_row.fields, "keys").unwrap_or("")));

    let preview = gtk::Label::builder()
        .label(bind_preview(&modifier.text(), &keys.text()))
        .css_classes(["keybind-preview", "caption"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .build();
    main.append(&preview);

    let mut field_values = Vec::new();
    for name in ["dispatcher", "args", "description", "group", "subcat"] {
        field_values.push(make_attrset_field(
            name,
            field(&scan_row.fields, name).unwrap_or(""),
            ctx,
        ));
    }
    let fields = Rc::new(RefCell::new(field_values));

    let key = gtk::Entry::new();
    key.set_hexpand(true);
    key.set_text(&scan_row.key);

    let popup = gtk::Window::builder()
        .title(bind_display_label(&scan_row.key, label_override.as_deref()))
        .modal(true)
        .default_width(440)
        .hide_on_close(true)
        .build();
    let popup_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    popup_box.set_margin_top(12);
    popup_box.set_margin_bottom(12);
    popup_box.set_margin_start(12);
    popup_box.set_margin_end(12);

    let popup_desc = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .label(entry_text(
            field(&scan_row.fields, "description").unwrap_or(""),
        ))
        .css_classes(["dim-label", "caption"])
        .build();
    popup_box.append(&popup_desc);
    popup_box.append(&dim_line("name", &key));
    popup_box.append(&dim_line("modifier", &modifier));
    popup_box.append(&dim_line("keys", &keys));
    for f in fields.borrow().iter() {
        popup_box.append(&dim_line_with_help(
            &f.name,
            keybind_field_help(&f.name),
            &f.editor.root(),
        ));
    }
    let (bar, cancel, apply) = popout_button_bar();
    popup_box.append(&bar);
    popup.set_child(Some(&popup_box));

    let edit = gtk::Button::with_label("Edit");
    main.append(&edit);
    let remove = gtk::Button::from_icon_name("user-trash-symbolic");
    remove.add_css_class("flat");
    remove.set_tooltip_text(Some("Remove bind"));
    main.append(&remove);
    row_box.append(&main);

    {
        let modifier_watch = modifier.clone();
        let keys_watch = keys.clone();
        let preview_a = preview.clone();
        modifier.connect_changed(move |_| {
            preview_a.set_label(&bind_preview(&modifier_watch.text(), &keys_watch.text()));
        });
        let modifier_watch = modifier.clone();
        let keys_watch = keys.clone();
        keys.connect_changed(move |_| {
            preview.set_label(&bind_preview(&modifier_watch.text(), &keys_watch.text()));
        });
    }

    let row = KeybindRow {
        key,
        orig_key: scan_row.key.clone(),
        modifier,
        keys,
        orig_modifier: field(&scan_row.fields, "modifier")
            .unwrap_or("")
            .to_string(),
        orig_keys: field(&scan_row.fields, "keys").unwrap_or("").to_string(),
        fields,
        row_box,
        remove,
        popup,
        snapshot: Rc::new(RefCell::new(None)),
    };

    {
        let popup = row.popup.clone();
        row.key.connect_changed(move |entry| {
            let label = bind_display_label(&entry.text(), label_override.as_deref());
            title.set_label(&label);
            popup.set_title(Some(&label));
        });
    }
    {
        if let Some(desc) = row.fields.borrow().iter().find(|f| f.name == "description") {
            if let Editor::Text(entry) = &desc.editor {
                let entry_cb = entry.clone();
                entry.connect_changed(move |_| {
                    desc_caption.set_label(&entry_cb.text());
                    popup_desc.set_label(&entry_cb.text());
                });
            }
        }
    }
    {
        let popup = row.popup.clone();
        let snapshot = row.snapshot.clone();
        let fields = row.fields.clone();
        let key = row.key.clone();
        let modifier = row.modifier.clone();
        let keys = row.keys.clone();
        let edit_probe = edit.clone();
        edit.connect_clicked(move |_| {
            if popup.transient_for().is_none() {
                if let Some(parent) = edit_probe.root().and_downcast::<gtk::Window>() {
                    popup.set_transient_for(Some(&parent));
                }
            }
            let values = capture_field_values(&fields.borrow());
            *snapshot.borrow_mut() = Some(PopoutSnapshot {
                name: key.text().to_string(),
                modifier: modifier.text().to_string(),
                keys: keys.text().to_string(),
                fields: values,
            });
            popup.set_default_size(440, -1);
            popup.present();
        });
    }
    {
        let popup = row.popup.clone();
        let snapshot = row.snapshot.clone();
        apply.connect_clicked(move |_| {
            snapshot.borrow_mut().take();
            popup.hide();
        });
    }
    {
        let popup = row.popup.clone();
        cancel.connect_clicked(move |_| popup.close());
    }
    {
        let snapshot = row.snapshot.clone();
        let fields = row.fields.clone();
        let key = row.key.clone();
        let modifier = row.modifier.clone();
        let keys = row.keys.clone();
        row.popup.connect_close_request(move |_| {
            let taken = snapshot.borrow_mut().take();
            if let Some(popout) = taken {
                key.set_text(&popout.name);
                modifier.set_text(&popout.modifier);
                keys.set_text(&popout.keys);
                restore_field_values(&fields.borrow(), &popout.fields);
            }
            glib::Propagation::Proceed
        });
    }
    {
        let popup = row.popup.clone();
        let controller = gtk::EventControllerKey::new();
        controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                popup.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        row.popup.add_controller(controller);
    }

    row
}

/// The comma-separated text shown in a modifier/keys entry.
pub(crate) fn list_text(source: &str) -> String {
    value::list_items(source)
        .map(|items| {
            items
                .iter()
                .map(|item| parse_string_literal(item))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

pub(crate) fn keybinds_nix(rows: &[KeybindRow]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut fields = vec![
                format!("modifier = {};", csv_list_nix(&row.modifier.text())),
                format!("keys = {};", csv_list_nix(&row.keys.text())),
            ];
            for field in row.fields.borrow().iter() {
                if let Some(line) = attrset_field_nix(field) {
                    fields.push(line);
                }
            }
            format!(
                "{} = {{ {} }};",
                attr_key(&row.key.text()),
                fields.join(" ")
            )
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

pub(crate) fn connect_keybind_row(row: &KeybindRow, update: &Rc<dyn Fn()>) {
    for entry in [&row.key, &row.modifier, &row.keys] {
        let update = update.clone();
        entry.connect_changed(move |_| update());
    }
    for field in row.fields.borrow().iter() {
        field.editor.connect_changed(update);
    }
}

pub(crate) fn build_keybinds(record: &Record) -> Editor {
    let ctx = BuildContext::default();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.set_hexpand(true);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.set_hexpand(true);
    let rows: Rc<RefCell<Vec<KeybindRow>>> = Rc::new(RefCell::new(Vec::new()));
    let on_change: ChangeHook = Rc::new(RefCell::new(None));

    let mut scanned = scan::scan_table(&record.value, false);
    scanned.sort_by(|a, b| {
        let key = |row: &scan::TableRowScan| {
            (
                field(&row.fields, "group").unwrap_or("").to_string(),
                field(&row.fields, "subcat").unwrap_or("").to_string(),
                row.key.clone(),
            )
        };
        key(a).cmp(&key(b))
    });

    for scan_row in scanned {
        let row = make_keybind_row(&scan_row, &ctx);
        {
            let container = container.clone();
            let rows = rows.clone();
            let on_change = on_change.clone();
            let row_box = row.row_box.clone();
            let key = row.key.clone();
            let popup = row.popup.clone();
            row.remove.connect_clicked(move |_| {
                popup.destroy();
                container.remove(&row_box);
                sync_keybind_separators(&container);
                rows.borrow_mut().retain(|r| r.key != key);
                if let Some(cb) = on_change.borrow().as_ref() {
                    cb();
                }
            });
        }
        container.append(&row.row_box);
        sync_keybind_separators(&container);
        rows.borrow_mut().push(row);
    }

    let add = gtk::Button::with_label("Add bind");
    add.set_halign(gtk::Align::Start);
    {
        let container = container.clone();
        let rows = rows.clone();
        let on_change = on_change.clone();
        add.connect_clicked(move |_| {
            let scan_row = scan::TableRowScan {
                key: String::new(),
                fields: vec![
                    ("modifier".to_string(), "[]".to_string()),
                    ("keys".to_string(), "[]".to_string()),
                ],
                spec: OptionSpec::default(),
            };
            let mut row = make_keybind_row(&scan_row, &ctx);
            row.orig_key = "\0".to_string();
            {
                let container = container.clone();
                let rows = rows.clone();
                let on_change = on_change.clone();
                let row_box = row.row_box.clone();
                let key = row.key.clone();
                let popup = row.popup.clone();
                row.remove.connect_clicked(move |_| {
                    popup.destroy();
                    container.remove(&row_box);
                    sync_keybind_separators(&container);
                    rows.borrow_mut().retain(|r| r.key != key);
                    if let Some(cb) = on_change.borrow().as_ref() {
                        cb();
                    }
                });
            }
            container.append(&row.row_box);
            sync_keybind_separators(&container);
            rows.borrow_mut().push(row);
            if let Some(cb) = on_change.borrow().as_ref() {
                cb();
            }
        });
    }

    root.append(&container);
    root.append(&add);
    Editor::Keybinds {
        root,
        rows,
        on_change,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_comma_separated_lists() {
        assert_eq!(csv_list("SUPER, SHIFT"), ["SUPER", "SHIFT"]);
        assert_eq!(csv_list(" , , "), [] as [&str; 0]);
        assert_eq!(csv_list(""), [] as [&str; 0]);
    }

    #[test]
    fn previews_modifier_plus_keys() {
        assert_eq!(
            bind_preview("SUPER, SHIFT", "Return, a"),
            "SUPER+SHIFT+RETURN+A"
        );
        assert_eq!(bind_preview("", ""), "");
    }

    #[test]
    fn reads_list_text_from_nix() {
        assert_eq!(list_text("[ \"SUPER\" \"SHIFT\" ]"), "SUPER, SHIFT");
        assert_eq!(list_text("[]"), "");
        assert_eq!(list_text("null"), "");
    }

    #[test]
    fn serialises_csv_lists_as_nix_lists() {
        assert_eq!(csv_list_nix("SUPER, SHIFT"), "[ \"SUPER\" \"SHIFT\" ]");
        assert_eq!(csv_list_nix(""), "[]");
    }
    #[test]
    fn keybind_labels_derive_from_names_and_overrides() {
        assert_eq!(bind_display_label("", None), "New bind:");
        assert_eq!(bind_display_label("focusDown", None), "Focus Down:");
        assert_eq!(
            bind_display_label("mute", Some("Mute Audio")),
            "Mute Audio:"
        );
        assert_eq!(
            bind_display_label("mute", Some("Mute Audio:")),
            "Mute Audio:"
        );
        assert_eq!(bind_display_label("mute", Some("")), "Mute:");
    }
}
