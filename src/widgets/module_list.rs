use super::context::ModuleKind;
use super::editor::Editor;
use super::nix::module_items;
use crate::model::Record;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
pub(crate) fn build_module_list(
    value_text: &str,
    kind: ModuleKind,
    choices: &[String],
    _record: &Record,
) -> Editor {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_hexpand(true);
    let entries: Rc<RefCell<Vec<gtk::Entry>>> = Rc::new(RefCell::new(Vec::new()));
    let prefixes = kind.prefixes();

    if !choices.is_empty() {
        let labels: Vec<&str> = choices.iter().map(String::as_str).collect();
        let chooser = gtk::DropDown::from_strings(&labels);
        chooser.set_halign(gtk::Align::Start);
        let add = gtk::Button::with_label("Add module");
        {
            let root = root.clone();
            let entries = entries.clone();
            let chooser = chooser.clone();
            let choices = choices.to_vec();
            add.connect_clicked(move |_| {
                if let Some(name) = choices.get(chooser.selected() as usize) {
                    add_module_row(&root, &entries, name);
                }
            });
        }
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.append(&chooser);
        bar.append(&add);
        root.append(&bar);
    }

    let mut prefix = String::new();
    for item in module_items(value_text).unwrap_or_default() {
        let raw = item.trim();
        let name = prefixes
            .iter()
            .find_map(|p| raw.strip_prefix(p.as_str()))
            .unwrap_or(raw);
        if prefix.is_empty() {
            if let Some(p) = prefixes.iter().find(|p| raw.starts_with(p.as_str())) {
                prefix = p.clone();
            }
        }
        add_module_row(&root, &entries, name);
    }

    Editor::ModuleList {
        root,
        entries,
        prefix,
    }
}

pub(crate) fn add_module_row(root: &gtk::Box, entries: &Rc<RefCell<Vec<gtk::Entry>>>, name: &str) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let label = gtk::Entry::new();
    label.set_text(name);
    label.set_editable(false);
    label.set_hexpand(true);
    let remove = gtk::Button::with_label("Remove");
    {
        let root = root.clone();
        let entries = entries.clone();
        let row = row.clone();
        let label = label.clone();
        remove.connect_clicked(move |_| {
            root.remove(&row);
            entries.borrow_mut().retain(|e| *e != label);
        });
    }
    row.append(&label);
    row.append(&remove);
    root.append(&row);
    entries.borrow_mut().push(label);
}
