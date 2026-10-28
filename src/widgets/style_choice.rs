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
                    .label(
                        "This value is not part of the catalog, so its package is left unchanged.",
                    )
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
    let choice = Rc::new(StyleChoice {
        current: Rc::new(RefCell::new(current.clone())),
        package,
        on_change: Rc::new(RefCell::new(None)),
    });

    let list = gtk::ListBox::new();
    list.add_css_class("style-choice-list");
    list.set_selection_mode(gtk::SelectionMode::Single);

    let mut rows: Vec<(gtk::ListBoxRow, ChoiceEntry)> = Vec::new();
    let mut selected = None;
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
        text.append(
            &gtk::Label::builder()
                .label(if item.installed {
                    "Installed"
                } else {
                    "Not installed"
                })
                .xalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        line.append(&text);
        row.set_child(Some(&line));
        list.append(&row);
        if item.value == current {
            selected = Some(row.clone());
        }
        rows.push((row, item));
    }

    let search = gtk::SearchEntry::builder()
        .placeholder_text(search_placeholder)
        .hexpand(true)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(260)
        .vexpand(true)
        .child(&list)
        .build();
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left.set_size_request(360, -1);
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
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.append(&content);

    if let Some(row) = selected {
        list.select_row(Some(&row));
        if let Some((_, item)) = rows
            .iter()
            .find(|(candidate, _)| candidate.widget_name() == row.widget_name())
        {
            set_preview(&preview_host, &(item.preview)());
        }
    }

    {
        let rows = rows.clone();
        let choice = choice.clone();
        let preview_host = preview_host.clone();
        list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let name = row.widget_name();
            let Some((_, item)) = rows
                .iter()
                .find(|(candidate, _)| candidate.widget_name() == name)
            else {
                return;
            };
            *choice.current.borrow_mut() = item.value.clone();
            if let (Some(binding), Some(package)) = (&choice.package, &item.package) {
                *binding.value.borrow_mut() = package.clone();
            }
            set_preview(&preview_host, &(item.preview)());
            choice.fire();
        });
    }

    {
        let rows = rows.clone();
        search.connect_search_changed(move |entry| {
            let query = entry.text().trim().to_lowercase();
            for (row, item) in &rows {
                row.set_visible(query.is_empty() || item.search.contains(&query));
            }
        });
    }

    Editor::StyleChoice { root, choice }
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
        let css = ".style-choice-list row { border-radius: 6px; }
.style-choice-preview { padding: 12px; border-radius: 8px; background: alpha(currentColor, 0.06); }
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
