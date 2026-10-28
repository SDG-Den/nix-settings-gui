use super::context::ChangeHook;
use super::editor::Editor;
use super::nix::{entry_text, package_literal, quote};
use crate::dsl::OptionSpec;
use crate::edit::PendingEdit;
use crate::model::Record;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;

#[derive(Clone)]
pub(crate) struct PackageBinding {
    pub record: Record,
    pub value: Rc<RefCell<String>>,
}

#[derive(Clone)]
pub(crate) struct StyleChoice {
    pub current: Rc<RefCell<String>>,
    pub package: Option<PackageBinding>,
    pub on_change: ChangeHook,
}

impl StyleChoice {
    pub(crate) fn fire(&self) {
        let hook = self.on_change.borrow().clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    pub(crate) fn unchanged(&self, original: &str) -> bool {
        if self.current.borrow().as_str() != entry_text(original) {
            return false;
        }
        self.package.as_ref().is_none_or(|package| {
            package.value.borrow().trim()
                == entry_text(&super::nix::unwrap_modifiers(&package.record.value)).trim()
        })
    }

    pub(crate) fn edits(&self, record: &Record) -> Vec<PendingEdit> {
        let mut edits = Vec::new();
        let original_name = super::nix::unwrap_modifiers(&record.value);
        if self.current.borrow().as_str() != entry_text(&original_name) {
            edits.push(PendingEdit {
                file: record.file.clone(),
                start: record.byte_start,
                end: record.byte_end,
                value: quote(self.current.borrow().as_str()),
            });
        }
        if let Some(package) = &self.package {
            let original = super::nix::unwrap_modifiers(&package.record.value);
            let current = package.value.borrow().clone();
            if current.trim() != entry_text(&original).trim() {
                edits.push(PendingEdit {
                    file: package.record.file.clone(),
                    start: package.record.byte_start,
                    end: package.record.byte_end,
                    value: package_literal(&current),
                });
            }
        }
        edits
    }
}

#[derive(Clone)]
pub(crate) struct ChoiceEntry {
    pub label: String,
    pub value: String,
    pub package: Option<String>,
    pub installed: bool,
    pub search: String,
    pub thumbnail: Rc<dyn Fn() -> gtk::Widget>,
    pub preview: Rc<dyn Fn() -> gtk::Widget>,
}

pub(crate) fn entry(
    label: impl Into<String>,
    value: impl Into<String>,
    package: Option<&str>,
    installed: bool,
    search_extra: &str,
    thumbnail: Rc<dyn Fn() -> gtk::Widget>,
    preview: Rc<dyn Fn() -> gtk::Widget>,
) -> ChoiceEntry {
    let label = label.into();
    let value = value.into();
    let package = package.map(str::to_string);
    let search = format!(
        "{} {} {} {}",
        label.to_lowercase(),
        value.to_lowercase(),
        search_extra.to_lowercase(),
        package.as_deref().unwrap_or_default().to_lowercase()
    );
    ChoiceEntry {
        label,
        value,
        package,
        installed,
        search,
        thumbnail,
        preview,
    }
}

pub(crate) fn custom_entry(value: &str) -> ChoiceEntry {
    let label = format!("{value} (custom)");
    let value_owned = value.to_string();
    let thumbnail_label = label.clone();
    let preview_label = label.clone();
    let preview_value = value_owned.clone();
    entry(
        label,
        value,
        None,
        false,
        "custom",
        Rc::new(move || placeholder(&thumbnail_label, 150, 40)),
        Rc::new(move || {
            let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
            root.append(
                &gtk::Label::builder()
                    .label(&preview_label)
                    .xalign(0.0)
                    .css_classes(["title-4"])
                    .build(),
            );
            root.append(
                &gtk::Label::builder()
                    .label(&preview_value)
                    .xalign(0.0)
                    .selectable(true)
                    .css_classes(["dim-label"])
                    .build(),
            );
            root.append(
                &gtk::Label::builder()
                    .label("This value is not part of the shipped catalog, so its package is left unchanged.")
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label"])
                    .build(),
            );
            root.upcast()
        }),
    )
}

pub(crate) fn placeholder(text: &str, width: i32, height: i32) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(text)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(24)
        .css_classes(["dim-label", "caption"])
        .build();
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root.set_size_request(width, height);
    root.set_halign(gtk::Align::Center);
    root.set_valign(gtk::Align::Center);
    root.append(&label);
    root.upcast()
}

pub(crate) fn build(
    value_text: &str,
    package: Option<PackageBinding>,
    mut items: Vec<ChoiceEntry>,
    title: &str,
    search_placeholder: &str,
) -> Editor {
    if items.is_empty() {
        return Editor::Text(super::basic::build_entry(
            value_text,
            &OptionSpec::default(),
            false,
        ));
    }
    install_css();
    let current = entry_text(value_text);
    if !items.iter().any(|item| item.value == current) {
        items.push(custom_entry(&current));
    }
    let title = title.to_string();
    let search_placeholder = search_placeholder.to_string();
    let choice = Rc::new(StyleChoice {
        current: Rc::new(RefCell::new(current)),
        package,
        on_change: Rc::new(RefCell::new(None)),
    });

    let preview_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_host.set_size_request(150, 40);
    preview_host.set_valign(gtk::Align::Center);
    preview_host.add_css_class("style-choice-inline-preview");
    let name = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(28)
        .build();
    let badge = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    texts.append(&name);
    texts.append(&badge);
    let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
    chevron.set_valign(gtk::Align::Center);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&preview_host);
    content.append(&texts);
    content.append(&chevron);
    let button = gtk::Button::new();
    button.add_css_class("style-choice-button");
    button.set_hexpand(true);
    button.set_child(Some(&content));

    refresh_inline(
        &preview_host,
        &name,
        &badge,
        &items,
        choice.current.borrow().as_str(),
    );

    {
        let choice = choice.clone();
        let items = items.clone();
        let title = title.clone();
        let search_placeholder = search_placeholder.clone();
        let preview_host = preview_host.clone();
        let name = name.clone();
        let badge = badge.clone();
        button.connect_clicked(move |button| {
            let parent = button.root().and_downcast::<gtk::Window>();
            let choice = choice.clone();
            let items = items.clone();
            let preview_host = preview_host.clone();
            let name = name.clone();
            let badge = badge.clone();
            let accept_choice = choice.clone();
            let accept_items = items.clone();
            let accept: Rc<dyn Fn(ChoiceEntry)> = Rc::new(move |item| {
                *accept_choice.current.borrow_mut() = item.value.clone();
                if let (Some(binding), Some(package)) = (&accept_choice.package, &item.package) {
                    *binding.value.borrow_mut() = package.clone();
                }
                refresh_inline(
                    &preview_host,
                    &name,
                    &badge,
                    &accept_items,
                    accept_choice.current.borrow().as_str(),
                );
                accept_choice.fire();
            });
            open_dialog(
                parent.as_ref(),
                &items,
                choice.current.borrow().as_str(),
                &title,
                &search_placeholder,
                accept,
            );
        });
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.append(&button);
    Editor::StyleChoice { root, choice }
}

fn refresh_inline(
    host: &gtk::Box,
    name: &gtk::Label,
    badge: &gtk::Label,
    items: &[ChoiceEntry],
    value: &str,
) {
    while let Some(child) = host.first_child() {
        host.remove(&child);
    }
    let Some(item) = items.iter().find(|item| item.value == value) else {
        return;
    };
    host.append(&(item.thumbnail)());
    name.set_text(&item.label);
    badge.set_text(if item.installed {
        "Installed"
    } else {
        "Not installed"
    });
}

fn open_dialog(
    parent: Option<&gtk::Window>,
    items: &[ChoiceEntry],
    current: &str,
    title: &str,
    search_placeholder: &str,
    accept: Rc<dyn Fn(ChoiceEntry)>,
) {
    let dialog = gtk::Dialog::builder()
        .title(title)
        .modal(true)
        .default_width(760)
        .default_height(520)
        .build();
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let select = dialog.add_button("Select", gtk::ResponseType::Accept);
    select.add_css_class("suggested-action");

    let search = gtk::SearchEntry::builder()
        .placeholder_text(search_placeholder)
        .hexpand(true)
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("style-choice-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .min_content_height(320)
        .child(&list)
        .build();
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left.set_size_request(330, -1);
    left.append(&search);
    left.append(&scrolled);

    let preview_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_host.set_hexpand(true);
    preview_host.set_valign(gtk::Align::Start);
    preview_host.add_css_class("style-choice-preview");
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.set_hexpand(true);
    right.append(&preview_host);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.set_hexpand(true);
    content.append(&left);
    content.append(&right);
    dialog.content_area().append(&content);

    let mut rows: Vec<(gtk::ListBoxRow, ChoiceEntry)> = Vec::new();
    let mut selected: Option<gtk::ListBoxRow> = None;
    for item in items {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&item.value);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        line.set_margin_top(4);
        line.set_margin_bottom(4);
        line.set_margin_start(6);
        line.set_margin_end(6);
        let thumbnail = (item.thumbnail)();
        thumbnail.set_valign(gtk::Align::Center);
        line.append(&thumbnail);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.append(
            &gtk::Label::builder()
                .label(&item.label)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let detail = match (&item.package, item.installed) {
            (Some(package), true) => format!("Installed  {package}"),
            (Some(package), false) => format!("Available  {package}"),
            (None, _) => "Not in catalog".to_string(),
        };
        text.append(
            &gtk::Label::builder()
                .label(detail)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        line.append(&text);
        row.set_child(Some(&line));
        list.append(&row);
        if item.value == current {
            selected = Some(row.clone());
        }
        rows.push((row, item.clone()));
    }

    let rows_for_select = rows.clone();
    let preview_for_select = preview_host.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let name = row.widget_name();
        if let Some((_, item)) = rows_for_select
            .iter()
            .find(|(candidate, _)| candidate.widget_name() == name)
        {
            set_preview(&preview_for_select, &(item.preview)());
        }
    });

    let rows_for_search = rows.clone();
    let list_for_search = list.clone();
    search.connect_search_changed(move |entry| {
        let query = entry.text().trim().to_lowercase();
        for (row, item) in &rows_for_search {
            row.set_visible(query.is_empty() || item.search.contains(&query));
        }
        let selected_visible = list_for_search
            .selected_row()
            .map(|row| row.is_visible())
            .unwrap_or(false);
        if !selected_visible {
            if let Some((row, _)) = rows_for_search.iter().find(|(row, _)| row.is_visible()) {
                list_for_search.select_row(Some(row));
            }
        }
    });

    let dialog_for_activate = dialog.clone();
    list.connect_row_activated(move |_, _| {
        dialog_for_activate.response(gtk::ResponseType::Accept);
    });

    if let Some(row) = selected.or_else(|| rows.first().map(|(row, _)| row.clone())) {
        list.select_row(Some(&row));
    }

    let list_for_response = list.clone();
    let rows_for_response = rows.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            if let Some(row) = list_for_response.selected_row() {
                if let Some((_, item)) = rows_for_response
                    .iter()
                    .find(|(candidate, _)| candidate.widget_name() == row.widget_name())
                {
                    accept(item.clone());
                }
            }
        }
        dialog.close();
    });

    dialog.present();
    search.grab_focus();
}

fn set_preview(host: &gtk::Box, child: &gtk::Widget) {
    while let Some(current) = host.first_child() {
        host.remove(&current);
    }
    host.append(child);
}

fn install_css() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let css = ".style-choice-button { padding: 6px 10px; border-radius: 8px; background: alpha(currentColor, 0.06); }
.style-choice-button:hover { background: alpha(currentColor, 0.10); }
.style-choice-inline-preview { border-radius: 6px; background: alpha(currentColor, 0.08); }
.style-choice-preview { padding: 12px; border-radius: 8px; background: alpha(currentColor, 0.06); }
.style-choice-list row { border-radius: 6px; }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: &str, file: &str, start: usize, end: usize) -> Record {
        Record {
            value: value.to_string(),
            file: file.to_string(),
            byte_start: start,
            byte_end: end,
            ..Record::default()
        }
    }

    fn choice(name: &str, package: Option<(&str, &str)>) -> StyleChoice {
        StyleChoice {
            current: Rc::new(RefCell::new(name.to_string())),
            package: package.map(|(original, current)| PackageBinding {
                record: record(original, "style.nix", 40, 60),
                value: Rc::new(RefCell::new(current.to_string())),
            }),
            on_change: Rc::new(RefCell::new(None)),
        }
    }

    #[test]
    fn emits_name_and_package_edits_together() {
        let choice = choice("Fira Code", Some(("pkgs.dejavu_fonts", "pkgs.fira-code")));
        let edits = choice.edits(&record("\"DejaVu Sans\"", "style.nix", 10, 20));
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].value, "\"Fira Code\"");
        assert_eq!(edits[1].value, "pkgs.fira-code");
        assert!(!choice.unchanged("\"DejaVu Sans\""));
    }

    #[test]
    fn unchanged_compares_name_and_package() {
        let choice = choice(
            "DejaVu Sans",
            Some(("pkgs.dejavu_fonts", "pkgs.dejavu_fonts")),
        );
        assert!(choice.unchanged("\"DejaVu Sans\""));
        assert!(choice
            .edits(&record("\"DejaVu Sans\"", "style.nix", 10, 20))
            .is_empty());
    }

    #[test]
    fn custom_value_keeps_the_existing_package() {
        let choice = choice(
            "Custom Font",
            Some(("pkgs.dejavu_fonts", "pkgs.dejavu_fonts")),
        );
        let edits = choice.edits(&record("\"DejaVu Sans\"", "style.nix", 10, 20));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].value, "\"Custom Font\"");
    }
}
