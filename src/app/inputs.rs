use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::edit;
use crate::flake_inputs::{self, DeclaredInput, FlakeInput};

use super::pump::{show_error, spawn};
use super::state::{App, Msg};

pub(super) const INPUTS_PAGE: &str = "__flake_inputs";

pub(super) fn flake_icon_image() -> Option<gtk::Image> {
    let display = gtk::gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    for name in [
        "nix-snowflake-symbolic",
        "nix-symbolic",
        "nix",
        "application-x-nix",
        "package-x-generic-symbolic",
    ] {
        if theme.has_icon(name) {
            return Some(gtk::Image::from_icon_name(name));
        }
    }
    None
}

fn flake_file(app: &Rc<App>) -> String {
    Path::new(&app.base.borrow().clone())
        .join("flake.nix")
        .display()
        .to_string()
}

fn non_empty(text: String) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn update_warning(app: &Rc<App>) {
    let declared = declared_inputs(app);
    let existing = flake_inputs::load(&flake_file(app))
        .map(|document| document.inputs)
        .unwrap_or_default();
    let missing = flake_inputs::missing_declared_inputs(&declared, &existing);
    app.inputs_warning.set_visible(!missing.is_empty());
    if missing.is_empty() {
        app.inputs_warning.set_tooltip_text(None);
    } else {
        let names: Vec<&str> = missing.iter().map(|input| input.name.as_str()).collect();
        app.inputs_warning.set_tooltip_text(Some(&format!(
            "{} declared input(s) missing: {}",
            missing.len(),
            names.join(", ")
        )));
    }
}

fn declared_inputs(app: &Rc<App>) -> Vec<DeclaredInput> {
    app.scan
        .borrow()
        .as_ref()
        .map(|scan| scan.declared_inputs.clone())
        .unwrap_or_default()
}

pub(super) fn refresh(app: &Rc<App>) {
    update_warning(app);
    while let Some(child) = app.inputs_page.first_child() {
        app.inputs_page.remove(&child);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_hexpand(true);

    let heading = gtk::Label::builder()
        .label("Flake Inputs")
        .xalign(0.0)
        .css_classes(["title-2"])
        .build();
    content.append(&heading);

    let file = flake_file(app);
    let document = match flake_inputs::load(&file) {
        Ok(document) => document,
        Err(error) => {
            content.append(
                &gtk::Label::builder()
                    .label(&error)
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label"])
                    .build(),
            );
            app.inputs_page.append(&scrolled(content));
            return;
        }
    };

    content.append(
        &gtk::Label::builder()
            .label(&file)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build(),
    );

    let missing = flake_inputs::missing_declared_inputs(&declared_inputs(app), &document.inputs);
    if !missing.is_empty() {
        let names: Vec<&str> = missing
            .iter()
            .take(5)
            .map(|input| input.name.as_str())
            .collect();
        let suffix = if missing.len() > names.len() {
            ", ..."
        } else {
            ""
        };
        let banner = adw::Banner::new(&format!(
            "Missing declared input(s): {}{suffix}",
            names.join(", ")
        ));
        banner.set_revealed(true);
        content.append(&banner);
    }

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if document.inputs.is_empty() {
        list.append(
            &gtk::Label::builder()
                .label("No inputs declared in this flake.")
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build(),
        );
    } else {
        for input in &document.inputs {
            list.append(&input_row(app, input));
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
    }
    content.append(&list);

    let add = gtk::Button::with_label("Add input");
    add.add_css_class("pill");
    add.set_halign(gtk::Align::Start);
    {
        let app = app.clone();
        add.connect_clicked(move |_| open_dialog(&app, None));
    }
    content.append(&add);

    if !missing.is_empty() {
        let add_missing =
            gtk::Button::with_label(&format!("Add {} missing input(s)", missing.len()));
        add_missing.add_css_class("suggested-action");
        add_missing.set_halign(gtk::Align::Start);
        let app = app.clone();
        add_missing.connect_clicked(move |button| {
            button.set_sensitive(false);
            auto_add_missing(&app, &missing);
        });
        content.append(&add_missing);
    }

    app.inputs_page.append(&scrolled(content));
}

fn scrolled(content: gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(&content)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build()
}

fn input_row(app: &Rc<App>, input: &FlakeInput) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_margin_top(8);
    row.set_margin_bottom(8);
    row.set_margin_start(6);
    row.set_margin_end(6);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let name = gtk::Label::builder()
        .label(&input.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let subtitle_text = input
        .description
        .clone()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| input.url.clone())
        .unwrap_or_default();
    let subtitle = gtk::Label::builder()
        .label(&subtitle_text)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label"])
        .build();
    text.append(&name);
    text.append(&subtitle);
    row.append(&text);

    let cog = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Edit input")
        .build();
    cog.add_css_class("flat");
    {
        let app = app.clone();
        let input = input.clone();
        cog.connect_clicked(move |_| open_dialog(&app, Some(input.clone())));
    }
    row.append(&cog);

    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete input")
        .build();
    delete.add_css_class("flat");
    {
        let app = app.clone();
        let name = input.name.clone();
        delete.connect_clicked(move |_| confirm_delete(&app, &name));
    }
    row.append(&delete);

    row
}

#[derive(Clone)]
struct FollowsUi {
    container: gtk::Box,
    entries: Rc<RefCell<Vec<(String, gtk::ComboBoxText)>>>,
    existing: Rc<RefCell<BTreeMap<String, String>>>,
    local_names: Rc<Vec<String>>,
}

impl FollowsUi {
    fn current(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (external, combo) in self.entries.borrow().iter() {
            if let Some(local) = combo.active_id().filter(|id| !id.is_empty()) {
                out.insert(external.clone(), local.to_string());
            }
        }
        out
    }

    fn rebuild(&self, external_names: &[String]) {
        let mut known = self.existing.borrow().clone();
        for (external, combo) in self.entries.borrow().iter() {
            match combo.active_id().filter(|id| !id.is_empty()) {
                Some(local) => {
                    known.insert(external.clone(), local.to_string());
                }
                None => {
                    known.remove(external);
                }
            }
        }
        *self.existing.borrow_mut() = known.clone();

        let mut names = external_names.to_vec();
        for name in known.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names.sort();
        names.dedup();

        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        self.entries.borrow_mut().clear();

        if names.is_empty() {
            self.container.append(
                &gtk::Label::builder()
                    .label("No nested inputs loaded yet.")
                    .xalign(0.0)
                    .wrap(true)
                    .css_classes(["dim-label"])
                    .build(),
            );
            return;
        }

        for external in names {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let label = gtk::Label::builder()
                .label(&external)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let combo = gtk::ComboBoxText::new();
            combo.append(Some(""), "Do not follow");
            for local in self.local_names.iter() {
                combo.append(Some(local.as_str()), local);
            }
            if let Some(local) = known.get(&external) {
                combo.set_active_id(Some(local.as_str()));
            } else {
                combo.set_active_id(Some(""));
            }
            row.append(&label);
            row.append(&combo);
            self.container.append(&row);
            self.entries.borrow_mut().push((external, combo));
        }
    }
}

fn open_dialog(app: &Rc<App>, existing: Option<FlakeInput>) {
    let file = flake_file(app);
    let document = match flake_inputs::load(&file) {
        Ok(document) => document,
        Err(error) => {
            show_error(&app.window, &error);
            return;
        }
    };

    let is_new = existing.is_none();
    let input = existing.unwrap_or_else(|| FlakeInput::new(""));
    let original_name = input.name.clone();
    let local_names: Vec<String> = document
        .inputs
        .iter()
        .map(|input| input.name.clone())
        .filter(|name| name != &original_name)
        .collect();

    let dialog = gtk::Dialog::builder()
        .title(if is_new {
            "Add flake input"
        } else {
            "Edit flake input"
        })
        .transient_for(&app.window)
        .modal(true)
        .default_width(640)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let accept = dialog.add_button(
        if is_new { "Add" } else { "Save" },
        gtk::ResponseType::Accept,
    );
    accept.add_css_class("suggested-action");

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(10)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let name_entry = gtk::Entry::new();
    name_entry.set_text(&input.name);
    name_entry.set_hexpand(true);
    name_entry.set_editable(is_new);
    name_entry.set_placeholder_text(Some("input name"));

    let url_entry = gtk::Entry::new();
    url_entry.set_text(input.url.as_deref().unwrap_or(""));
    url_entry.set_hexpand(true);
    url_entry.set_placeholder_text(Some("github:owner/repo"));

    let description_entry = gtk::Entry::new();
    description_entry.set_text(input.description.as_deref().unwrap_or(""));
    description_entry.set_hexpand(true);
    description_entry.set_placeholder_text(Some("Optional description"));

    let submodules = gtk::Switch::new();
    submodules.set_active(input.submodules);
    submodules.set_halign(gtk::Align::Start);
    let lfs = gtk::Switch::new();
    lfs.set_active(input.lfs);
    lfs.set_halign(gtk::Align::Start);

    grid.attach(&gtk::Label::new(Some("Name")), 0, 0, 1, 1);
    grid.attach(&name_entry, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("URL")), 0, 1, 1, 1);
    grid.attach(&url_entry, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Description")), 0, 2, 1, 1);
    grid.attach(&description_entry, 1, 2, 1, 1);
    grid.attach(&gtk::Label::new(Some("Git submodules")), 0, 3, 1, 1);
    grid.attach(&submodules, 1, 3, 1, 1);
    grid.attach(&gtk::Label::new(Some("Git LFS")), 0, 4, 1, 1);
    grid.attach(&lfs, 1, 4, 1, 1);

    let follows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let follows = FollowsUi {
        container: follows_box.clone(),
        entries: Rc::new(RefCell::new(Vec::new())),
        existing: Rc::new(RefCell::new(input.follows.clone())),
        local_names: Rc::new(local_names),
    };
    follows.rebuild(&[]);

    let load_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let load_btn = gtk::Button::with_label("Load inputs");
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    load_row.append(&load_btn);
    load_row.append(&spinner);

    let error_label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();

    let follows_area = gtk::Box::new(gtk::Orientation::Vertical, 6);
    follows_area.append(&follows_box);
    follows_area.append(&load_row);
    follows_area.append(&error_label);
    grid.attach(&gtk::Label::new(Some("Follows")), 0, 5, 1, 1);
    grid.attach(&follows_area, 1, 5, 1, 1);

    dialog.content_area().append(&grid);

    let generation = Rc::new(Cell::new(0u64));
    {
        let app = app.clone();
        let url_entry = url_entry.clone();
        let button = load_btn.clone();
        let spinner = spinner.clone();
        let error_label = error_label.clone();
        let generation = generation.clone();
        let follows = follows.clone();
        load_btn.connect_clicked(move |_| {
            load_nested_inputs(
                &app,
                &url_entry,
                &button,
                &spinner,
                &error_label,
                &generation,
                &follows,
            );
        });
    }
    if !url_entry.text().trim().is_empty() {
        let button = load_btn.clone();
        glib::timeout_add_local_once(Duration::from_millis(0), move || button.emit_clicked());
    }

    {
        let app = app.clone();
        let file = file.clone();
        let follows = follows.clone();
        dialog.connect_response(move |dialog, response| {
            if response != gtk::ResponseType::Accept {
                dialog.close();
                return;
            }
            let name = if is_new {
                name_entry.text().trim().to_string()
            } else {
                original_name.clone()
            };
            let mut updated = FlakeInput::new(name.clone());
            updated.url = non_empty(url_entry.text().to_string());
            updated.description = non_empty(description_entry.text().to_string());
            updated.submodules = submodules.is_active();
            updated.lfs = lfs.is_active();
            updated.follows = follows.current();

            if name.is_empty() {
                show_error(&app.window, "Input name cannot be empty");
                return;
            }
            let document = match flake_inputs::load(&file) {
                Ok(document) => document,
                Err(error) => {
                    show_error(&app.window, &error);
                    return;
                }
            };
            if document
                .inputs
                .iter()
                .any(|input| input.name == name && input.name != original_name)
            {
                show_error(
                    &app.window,
                    &format!("An input named `{name}` already exists"),
                );
                return;
            }
            match document.edits_for_upsert(&updated) {
                Ok(edits) => {
                    save_edits(&app, edits);
                    dialog.close();
                }
                Err(error) => show_error(&app.window, &error),
            }
        });
    }

    dialog.present();
}

fn load_nested_inputs(
    app: &Rc<App>,
    url_entry: &gtk::Entry,
    button: &gtk::Button,
    spinner: &gtk::Spinner,
    error_label: &gtk::Label,
    generation: &Rc<Cell<u64>>,
    follows: &FollowsUi,
) {
    let url = url_entry.text().trim().to_string();
    if url.is_empty() {
        error_label.set_text("Enter a URL first");
        return;
    }
    let current = generation.get() + 1;
    generation.set(current);
    button.set_sensitive(false);
    spinner.set_visible(true);
    spinner.start();
    error_label.set_text("");

    let base = app.base.borrow().clone();
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = flake_inputs::fetch_nested_inputs(&base, &url);
        let _ = sender.send_blocking(result);
    });

    let button = button.clone();
    let spinner = spinner.clone();
    let error_label = error_label.clone();
    let generation = generation.clone();
    let follows = follows.clone();
    glib::MainContext::default().spawn_local(async move {
        let result = receiver
            .recv()
            .await
            .unwrap_or_else(|_| Err("input lookup stopped unexpectedly".to_string()));
        if generation.get() != current {
            return;
        }
        button.set_sensitive(true);
        spinner.stop();
        spinner.set_visible(false);
        match result {
            Ok(names) => {
                follows.rebuild(&names);
                if names.is_empty() {
                    error_label.set_text("This flake declares no inputs");
                }
            }
            Err(error) => error_label.set_text(&error),
        }
    });
}

fn auto_add_missing(app: &Rc<App>, missing: &[DeclaredInput]) {
    let file = flake_file(app);
    let document = match flake_inputs::load(&file) {
        Ok(document) => document,
        Err(error) => {
            show_error(&app.window, &error);
            return;
        }
    };
    let inputs: Vec<FlakeInput> = missing
        .iter()
        .map(|declared| {
            let mut input = FlakeInput::new(declared.name.clone());
            input.url = Some(declared.url.clone());
            input
        })
        .collect();
    match document.edits_for_add_all(&inputs) {
        Ok(edits) => save_edits(app, edits),
        Err(error) => show_error(&app.window, &error),
    }
}

fn save_edits(app: &Rc<App>, edits: Vec<edit::PendingEdit>) {
    let root = app.base.borrow().clone();
    app.status.set_text("Saving flake inputs...");
    spawn(&app.tx, move |tx| {
        let result = edit::apply(&edits, &root).map(|_| ());
        let _ = tx.send_blocking(Msg::InputsSaved(result));
    });
}

fn confirm_delete(app: &Rc<App>, name: &str) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(&app.window)
        .modal(true)
        .heading("Delete input")
        .body(format!(
            "Delete \"{name}\" from flake.nix? This cannot be undone."
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let app = app.clone();
    let name = name.to_string();
    let file = flake_file(&app);
    dialog.connect_response(None, move |dialog: &adw::MessageDialog, response: &str| {
        if response == "delete" {
            match flake_inputs::load(&file) {
                Ok(document) => match document.edits_for_delete(&name) {
                    Ok(edits) => save_edits(&app, edits),
                    Err(error) => show_error(&app.window, &error),
                },
                Err(error) => show_error(&app.window, &error),
            }
        }
        dialog.close();
    });
    dialog.present();
}
