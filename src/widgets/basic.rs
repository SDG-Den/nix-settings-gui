use super::context::PackageSearch;
use super::editor::Editor;
use super::nix::{choice_literal, entry_text, parse_string_literal, truthy};
use crate::dsl::OptionSpec;
use crate::value;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
pub(crate) fn build_switch(value_text: &str) -> gtk::Switch {
    let switch = gtk::Switch::new();
    switch.set_active(truthy(value_text));
    switch.set_halign(gtk::Align::Start);
    switch
}

pub(crate) fn build_dropdown(choices: &[String], value_text: &str) -> Editor {
    let current = value_text.trim();
    let mut labels: Vec<String> = choices.to_vec();
    let mut literals: Vec<String> = choices.iter().map(|c| choice_literal(c)).collect();
    if !literals.iter().any(|l| l == current) {
        for existing in literals.iter_mut() {
            if existing.is_empty() {
                *existing = current.to_string();
                break;
            }
        }
        if !literals.iter().any(|l| l == current) {
            labels.push(value_text.trim().to_string());
            literals.push(current.to_string());
        }
    }
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let combo = gtk::DropDown::from_strings(&refs);
    let selected = literals.iter().position(|l| l == current).unwrap_or(0);
    combo.set_selected(selected as u32);
    combo.set_halign(gtk::Align::Start);
    Editor::Dropdown {
        combo,
        choices: literals,
    }
}
pub(crate) fn build_spin(spec: &OptionSpec, value_text: &str) -> gtk::SpinButton {
    let min = spec.min.unwrap_or(-1.0e15);
    let max = spec.max.unwrap_or(1.0e15);
    let step = spec.step.unwrap_or(1.0);
    let default_digits = if value_text.contains('.') { 2 } else { 0 };
    let spin = gtk::SpinButton::with_range(min, max, step);
    spin.set_digits(spec.digits.unwrap_or(default_digits));
    spin.set_value(value_text.trim().parse::<f64>().unwrap_or(0.0));
    spin.set_halign(gtk::Align::Start);
    spin
}

pub(crate) fn build_slider(spec: &OptionSpec, value_text: &str) -> gtk::Scale {
    let min = spec.min.unwrap_or(0.0);
    let max = spec.max.unwrap_or(100.0);
    let step = spec.step.unwrap_or(1.0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_value(value_text.trim().parse::<f64>().unwrap_or(min));
    scale.set_draw_value(true);
    scale.set_hexpand(true);
    scale
}

pub(crate) fn build_entry(value_text: &str, spec: &OptionSpec, monospace: bool) -> gtk::Entry {
    let unquoted = entry_text(value_text);
    let entry = gtk::Entry::new();
    entry.set_text(&unquoted);
    entry.set_hexpand(true);
    if let Some(width) = spec.width_chars {
        entry.set_width_chars(width as i32);
        entry.set_hexpand(false);
        entry.set_halign(gtk::Align::Start);
    }
    if monospace {
        entry.add_css_class("monospace");
    }
    if let Some(placeholder) = &spec.placeholder {
        entry.set_placeholder_text(Some(placeholder));
    }
    entry
}
pub(crate) fn build_multiline(
    value_text: &str,
    spec: &OptionSpec,
) -> (gtk::TextView, gtk::ScrolledWindow) {
    let view = gtk::TextView::new();
    view.buffer().set_text(&parse_string_literal(value_text));
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.set_left_margin(4);
    view.set_right_margin(4);
    let rows = spec.rows.unwrap_or(4).max(2);
    let root = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .min_content_width(440)
        .min_content_height((rows as i32) * 24)
        .has_frame(true)
        .build();
    (view, root)
}

pub(crate) fn build_raw(
    value_text: &str,
    spec: &OptionSpec,
) -> (gtk::TextView, gtk::ScrolledWindow) {
    let view = gtk::TextView::new();
    view.buffer().set_text(value_text);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_top_margin(4);
    view.set_bottom_margin(4);
    view.set_left_margin(4);
    view.set_right_margin(4);
    let rows = spec.rows.unwrap_or(5).max(2);
    let root = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .min_content_width(440)
        .min_content_height((rows as i32) * 22)
        .has_frame(true)
        .build();
    (view, root)
}

pub(crate) fn build_color(value_text: &str) -> gtk::ColorDialogButton {
    let dialog = gtk::ColorDialog::new();
    let button = gtk::ColorDialogButton::new(Some(dialog));
    let raw = value_text.trim().trim_matches('"');
    if let Ok(rgba) = gtk::gdk::RGBA::parse(raw) {
        button.set_rgba(&rgba);
    }
    button.set_halign(gtk::Align::Start);
    button
}

pub(crate) fn build_list(
    value_text: &str,
    packages: bool,
    search: Option<&PackageSearch>,
) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    let entries: Rc<RefCell<Vec<gtk::Entry>>> = Rc::new(RefCell::new(Vec::new()));

    let add = gtk::Button::with_label("Add item");
    add.set_halign(gtk::Align::Start);
    {
        let root = root.clone();
        let entries = entries.clone();
        let search = search.cloned();
        add.connect_clicked(move |_| add_list_row(&root, &entries, "", packages, search.as_ref()));
    }
    root.append(&add);

    for item in value::list_items(value_text).unwrap_or_default() {
        add_list_row(&root, &entries, &item, packages, search);
    }

    Editor::List {
        root,
        entries,
        packages,
    }
}

pub(crate) fn add_list_row(
    root: &gtk::Box,
    entries: &Rc<RefCell<Vec<gtk::Entry>>>,
    text: &str,
    packages: bool,
    search: Option<&PackageSearch>,
) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let entry = gtk::Entry::new();
    entry.set_text(text);
    entry.set_hexpand(true);
    if packages {
        entry.add_css_class("monospace");
        entry.set_editable(false);
        entry.set_tooltip_text(Some("Choose a package with the search button"));
    }
    if let Some(search) = search {
        row.append(&attach_package_search(&entry, search));
    }
    let remove = gtk::Button::with_label("Remove");
    {
        let root = root.clone();
        let entries = entries.clone();
        let row = row.clone();
        let entry = entry.clone();
        remove.connect_clicked(move |_| {
            root.remove(&row);
            entries.borrow_mut().retain(|e| *e != entry);
        });
    }
    row.append(&entry);
    row.append(&remove);
    root.append(&row);
    entries.borrow_mut().push(entry);
}

/// A single package value: an entry plus a search button that opens a popover
/// of nixd-suggested package names.
pub(crate) fn build_package_editor(
    value_text: &str,
    spec: &OptionSpec,
    search: Option<&PackageSearch>,
) -> Editor {
    let entry = build_entry(value_text, spec, true);
    entry.set_editable(false);
    entry.set_tooltip_text(Some("Choose a package with the search button"));
    let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    root.set_hexpand(true);
    if let Some(search) = search {
        root.append(&attach_package_search(&entry, search));
    }
    root.append(&entry);
    Editor::Package { root, entry }
}

/// A search button backed by the app's package search. Picking a result writes
/// it into `entry`.
pub(crate) fn attach_package_search(entry: &gtk::Entry, search: &PackageSearch) -> gtk::MenuButton {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&list)
        .min_content_height(160)
        .max_content_height(280)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Search packages")
        .build();
    let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    pop_box.set_size_request(280, -1);
    pop_box.append(&search_entry);
    pop_box.append(&scrolled);
    let popover = gtk::Popover::builder().child(&pop_box).build();
    let button = gtk::MenuButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Search packages")
        .popover(&popover)
        .build();
    button.add_css_class("flat");

    let entry = entry.clone();
    let search: PackageSearch = Rc::clone(search);
    let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    search_entry.connect_search_changed(move |field| {
        let current = generation.get() + 1;
        generation.set(current);
        let term = field.text().to_string();
        if term.trim().len() < 2 {
            clear_rows(&list);
            return;
        }
        let entry = entry.clone();
        let list = list.clone();
        let popover = popover.clone();
        let search = PackageSearch::clone(&search);
        let generation = generation.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            if generation.get() != current {
                return;
            }
            let entry = entry.clone();
            let list = list.clone();
            let popover = popover.clone();
            search(
                term,
                Box::new(move |names| {
                    clear_rows(&list);
                    for name in names.into_iter().take(50) {
                        let row = gtk::Button::with_label(&name);
                        row.add_css_class("flat");
                        row.set_halign(gtk::Align::Start);
                        let entry = entry.clone();
                        let popover = popover.clone();
                        row.connect_clicked(move |_| {
                            entry.set_text(&name);
                            popover.popdown();
                        });
                        list.append(&row);
                    }
                }),
            );
        });
    });
    button
}

pub(crate) fn clear_rows(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
