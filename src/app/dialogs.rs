use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::dsl::{SnippetSpec, WidgetKind};
use crate::edit;
use crate::metadata::{self, Target};
use crate::model::InsertMarker;
use crate::scan;
use crate::widgets;

use super::menu::{open_flake, strip_add_prefix};
use super::pump::{show_error, spawn};
use super::state::{App, Msg, Tx};

// ---------------------------------------------------------------------------
// dialogs
// ---------------------------------------------------------------------------

pub(super) fn open_configure(app: &Rc<App>) {
    let dialog = gtk::Dialog::builder()
        .title("Configure")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Save", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let config = app.config.borrow().clone();
    let flake_entry = gtk::Entry::new();
    flake_entry.set_text(config.flake_path.as_deref().unwrap_or(""));
    flake_entry.set_hexpand(true);
    let flake_browse = gtk::Button::with_label("Browse...");
    {
        let entry = flake_entry.clone();
        let window = app.window.clone();
        flake_browse.connect_clicked(move |_| select_folder(&window, &entry));
    }
    let flake_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    flake_row.append(&flake_entry);
    flake_row.append(&flake_browse);
    let terminal_entry = gtk::Entry::new();
    terminal_entry.set_text(config.terminal_command.as_deref().unwrap_or(""));
    terminal_entry.set_hexpand(true);
    let rebuild_entry = gtk::Entry::new();
    rebuild_entry.set_text(config.rebuild_command.as_deref().unwrap_or(""));
    rebuild_entry.set_hexpand(true);

    grid.attach(&gtk::Label::new(Some("Flake path")), 0, 0, 1, 1);
    grid.attach(&flake_row, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Terminal command")), 0, 1, 1, 1);
    grid.attach(&terminal_entry, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Rebuild command")), 0, 2, 1, 1);
    grid.attach(&rebuild_entry, 1, 2, 1, 1);

    dialog.content_area().append(&grid);

    {
        let app = app.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let flake = flake_entry.text().to_string();
                {
                    let mut config = app.config.borrow_mut();
                    config.terminal_command = non_empty(terminal_entry.text().to_string());
                    config.rebuild_command = non_empty(rebuild_entry.text().to_string());
                    if !flake.trim().is_empty() {
                        config.remember(flake.trim());
                    }
                }
                let _ = app.config.borrow().save();
                if !flake.trim().is_empty() {
                    let tx = app.tx.clone();
                    open_flake(&app, &tx, flake.trim());
                }
            }
            dialog.close();
        });
    }
    dialog.present();
}

fn non_empty(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Open a folder chooser and write the chosen path into `entry`.
fn select_folder(parent: &impl IsA<gtk::Window>, entry: &gtk::Entry) {
    let dialog = gtk::FileDialog::builder()
        .title("Select folder")
        .modal(true)
        .build();
    let entry = entry.clone();
    dialog.select_folder(
        Some(parent),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            if let Ok(folder) = result {
                if let Some(path) = folder.path() {
                    entry.set_text(&path.display().to_string());
                }
            }
        },
    );
}

pub(super) fn open_add_file(app: &Rc<App>, tx: &Tx) {
    let root = app.base.borrow().clone();
    let templates = Rc::new(crate::templates::load_templates(&root));

    let dialog = gtk::Dialog::builder()
        .title("Add file")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Create", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let kind = gtk::ComboBoxText::new();
    kind.append(Some("raw"), "Raw function");
    kind.append(Some("nixos"), "NixOS module");
    kind.append(Some("home"), "Home-manager module");
    for (index, template) in templates.iter().enumerate() {
        kind.append(
            Some(&format!("template:{index}")),
            &format!("Template: {}", template.label),
        );
    }
    kind.set_active_id(Some("nixos"));

    let filename = gtk::Entry::new();
    filename.set_text("module.nix");
    let module_name = gtk::Entry::new();
    module_name.set_placeholder_text(Some("flake.modules.<kind>.<name>"));
    let directory = gtk::Entry::new();
    directory.set_text(&root);
    directory.set_hexpand(true);
    let directory_browse = gtk::Button::with_label("Browse...");
    {
        let entry = directory.clone();
        let window = app.window.clone();
        directory_browse.connect_clicked(move |_| select_folder(&window, &entry));
    }
    let directory_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    directory_row.append(&directory);
    directory_row.append(&directory_browse);

    grid.attach(&gtk::Label::new(Some("Kind")), 0, 0, 1, 1);
    grid.attach(&kind, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Filename")), 0, 1, 1, 1);
    grid.attach(&filename, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Module name")), 0, 2, 1, 1);
    grid.attach(&module_name, 1, 2, 1, 1);
    grid.attach(&gtk::Label::new(Some("Directory")), 0, 3, 1, 1);
    grid.attach(&directory_row, 1, 3, 1, 1);

    let placeholder_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    placeholder_box.set_margin_start(12);
    placeholder_box.set_margin_end(12);
    placeholder_box.set_margin_bottom(12);
    let placeholder_entries: Rc<RefCell<Vec<(String, gtk::Entry)>>> =
        Rc::new(RefCell::new(Vec::new()));

    {
        let filename = filename.clone();
        let directory = directory.clone();
        let placeholder_box = placeholder_box.clone();
        let placeholder_entries = placeholder_entries.clone();
        let root = root.clone();
        let templates = templates.clone();
        kind.connect_changed(move |combo| {
            while let Some(child) = placeholder_box.first_child() {
                placeholder_box.remove(&child);
            }
            placeholder_entries.borrow_mut().clear();
            let id = combo.active_id().map(|s| s.to_string()).unwrap_or_default();
            let Some(index) = id
                .strip_prefix("template:")
                .and_then(|s| s.parse::<usize>().ok())
            else {
                filename.set_text("module.nix");
                directory.set_text(&root);
                return;
            };
            let Some(template) = templates.get(index) else {
                return;
            };
            filename.set_text(&template.filename);
            directory.set_text(&Path::new(&root).join(&template.output).to_string_lossy());
            for placeholder in &template.placeholders {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let label = gtk::Label::builder()
                    .label(&placeholder.label)
                    .xalign(0.0)
                    .build();
                let entry = gtk::Entry::new();
                entry.set_text(&placeholder.default);
                entry.set_hexpand(true);
                if !placeholder.hint.is_empty() {
                    entry.set_placeholder_text(Some(&placeholder.hint));
                }
                row.append(&label);
                row.append(&entry);
                placeholder_box.append(&row);
                placeholder_entries
                    .borrow_mut()
                    .push((placeholder.name.clone(), entry));
            }
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&grid);
    content.append(&placeholder_box);
    dialog.content_area().append(&content);

    {
        let app = app.clone();
        let tx = tx.clone();
        let templates = templates.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let kind_id = kind
                    .active_id()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "nixos".into());
                let module_name_text = module_name.text().to_string();
                let path = Path::new(&directory.text()).join(filename.text());
                let result = match kind_id
                    .strip_prefix("template:")
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|index| templates.get(index))
                {
                    Some(template) => {
                        let values: Vec<(String, String)> = placeholder_entries
                            .borrow()
                            .iter()
                            .map(|(name, entry)| (name.clone(), entry.text().to_string()))
                            .collect();
                        let body = crate::templates::render(&template.body, &values);
                        write_body(&path, &body)
                    }
                    None => write_template(&kind_id, &module_name_text, &path),
                };
                match result {
                    Ok(()) => {
                        app.status.set_text(&format!("Created {}", path.display()));
                        let base = app.base.borrow().clone();
                        let path_clone = base.clone();
                        spawn(&tx, move |tx| {
                            let result = scan::scan_root(&path_clone);
                            let _ = tx.send_blocking(Msg::Scanned(result));
                        });
                        if !Path::new(&base).join("flake.nix").is_file() {
                            app.status.set_text("Created; add it to the flake imports");
                        }
                    }
                    Err(e) => app.status.set_text(&format!("Cannot create file: {e}")),
                }
            }
            dialog.close();
        });
    }
    dialog.present();
}

fn write_body(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, body).map_err(|e| e.to_string())
}

fn write_template(kind: &str, module_name: &str, path: &Path) -> Result<(), String> {
    let displayname = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "New menu".into());
    let template = template_for(&displayname, kind, module_name, path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, template).map_err(|e| e.to_string())
}

/// Build a new module file. Raw files are plain functions for non-flake-parts
/// use; NixOS and home-manager files wrap their body in the flake-parts
/// `flake.modules.<kind>.<name>` container. The module name falls back to the
/// file stem when the user leaves it blank.
fn template_for(displayname: &str, kind: &str, module_name: &str, path: &Path) -> String {
    let header = format!("/* @menu\n   displayname = \"{displayname}\";\n   order = 1000;\n*/\n\n");
    let name = if module_name.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "module".into())
    } else {
        module_name.trim().to_string()
    };
    let body = match kind {
        "raw" => "{ ... }:\n\n{\n}\n".to_string(),
        "home" => format!(
            "{{ self, inputs, ... }}:\n\n{{\n\nflake.modules.homeManager.{name} = {{ pkgs, ... }}: {{\n\n}};\n}}\n"
        ),
        _ => format!(
            "{{ self, inputs, ... }}:\n\n{{\n\nflake.modules.nixos.{name} = {{ pkgs, ... }}: {{\n\n}};\n}}\n"
        ),
    };
    format!("{header}{body}")
}

/// Prompt for a snippet's `%%token%%` values, render it, and insert it at the
/// marker's line so the marker shifts down one line.
pub(super) fn open_snippet_dialog(
    app: &Rc<App>,
    file: &str,
    marker: &InsertMarker,
    snippet: &SnippetSpec,
) {
    let title_label = strip_add_prefix(&snippet.label);
    let title = if title_label.is_empty() {
        "Add".to_string()
    } else {
        format!("Add {title_label}")
    };
    let dialog = gtk::Dialog::builder()
        .title(title)
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Add", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let mut entries: Vec<(String, gtk::Entry)> = Vec::new();
    for (row, placeholder) in snippet.placeholders.iter().enumerate() {
        let label = gtk::Label::builder()
            .label(&placeholder.label)
            .xalign(0.0)
            .build();
        let entry = gtk::Entry::new();
        entry.set_text(&placeholder.default);
        entry.set_hexpand(true);
        entry.set_width_chars(40);
        if !placeholder.hint.is_empty() {
            entry.set_placeholder_text(Some(&placeholder.hint));
        }
        grid.attach(&label, 0, row as i32, 1, 1);
        grid.attach(&entry, 1, row as i32, 1, 1);
        entries.push((placeholder.name.clone(), entry));
    }
    dialog.content_area().append(&grid);

    let file = file.to_string();
    let offset = marker.offset;
    let body = snippet.body.clone();
    let tx = app.tx.clone();
    let master: gtk::Window = app.window.clone().upcast();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            let values: Vec<(String, String)> = entries
                .iter()
                .map(|(name, entry)| (name.clone(), entry.text().to_string()))
                .collect();
            let rendered = crate::templates::render(&body, &values);
            let result = edit::insert_at(&file, offset, &rendered);
            let message = match &result {
                Ok(()) => Msg::SnippetInserted(Ok(())),
                Err(e) => {
                    show_error(&master, e);
                    Msg::SnippetInserted(Err(e.clone()))
                }
            };
            let _ = tx.send_blocking(message);
        }
        dialog.close();
    });
    dialog.present();
}

pub(super) fn open_add_option(app: &Rc<App>, file: &str, origin: &str) {
    let dialog = gtk::Dialog::builder()
        .title("Add option")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Add", gtk::ResponseType::Accept);

    let grid = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let path_entry = gtk::Entry::new();
    path_entry.set_placeholder_text(Some("services.pipewire.enable"));
    path_entry.set_hexpand(true);
    path_entry.set_width_chars(48);
    let completion = gtk::EntryCompletion::new();
    let store = gtk::ListStore::new(&[glib::Type::STRING]);
    completion.set_model(Some(&store));
    completion.set_text_column(0);
    completion.set_minimum_key_length(2);
    path_entry.set_completion(Some(&completion));
    for path in option_suggestions(app) {
        store.set(&store.append(), &[(0, &path)]);
    }
    request_suggestions(app, store.clone(), Vec::new());
    {
        let app = app.clone();
        let store = store.clone();
        let generation: Rc<Cell<u64>> = Rc::new(Cell::new(0));
        path_entry.connect_changed(move |entry| {
            let current = generation.get() + 1;
            generation.set(current);
            let parent = parent_segments_from_text(&entry.text());
            let app = app.clone();
            let store = store.clone();
            let generation = generation.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                if generation.get() != current {
                    return;
                }
                request_suggestions(&app, store, parent)
            });
        });
    }

    let widget = gtk::ComboBoxText::new();
    for kind in [
        "switch",
        "dropdown",
        "radio",
        "spin",
        "slider",
        "text",
        "multiline",
        "path",
        "package",
        "packages",
        "list",
        "color",
        "submodule",
        "raw",
    ] {
        widget.append(Some(kind), kind);
    }
    widget.set_active_id(Some("text"));

    let label_entry = gtk::Entry::new();
    label_entry.set_placeholder_text(Some("Optional label"));

    let value_entry = gtk::Entry::new();
    value_entry.set_placeholder_text(Some("Initial value"));
    value_entry.set_width_chars(48);

    grid.attach(&gtk::Label::new(Some("Option path")), 0, 0, 1, 1);
    grid.attach(&path_entry, 1, 0, 1, 1);
    grid.attach(&gtk::Label::new(Some("Widget")), 0, 1, 1, 1);
    grid.attach(&widget, 1, 1, 1, 1);
    grid.attach(&gtk::Label::new(Some("Label")), 0, 2, 1, 1);
    grid.attach(&label_entry, 1, 2, 1, 1);
    grid.attach(&gtk::Label::new(Some("Value")), 0, 3, 1, 1);
    grid.attach(&value_entry, 1, 3, 1, 1);

    dialog.content_area().append(&grid);

    let file = file.to_string();
    let origin = origin.to_string();
    let tx = app.tx.clone();
    let master: gtk::Window = app.window.clone().upcast();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Accept {
            let option_path = path_entry.text().trim().to_string();
            let widget_id = widget.active_id().unwrap_or_else(|| "text".into());
            let label = label_entry.text().to_string();
            let value_text = value_entry.text().to_string();
            if !option_path.is_empty() {
                let block = build_assignment_block(&option_path, &widget_id, &label, &value_text);
                let result = edit::insert_assignment(&file, &origin, &block);
                let message = match &result {
                    Ok(()) => Msg::Inserted(Ok(())),
                    Err(e) => {
                        show_error(&master, e);
                        Msg::Inserted(Err(e.clone()))
                    }
                };
                let _ = tx.send_blocking(message);
            }
        }
        dialog.close();
    });
    dialog.present();
}

/// Every option path the scan already knows, plus each dotted prefix, offered
/// as autocomplete for the add-option dialog.
fn option_suggestions(app: &Rc<App>) -> Vec<String> {
    let scan = app.scan.borrow();
    let Some(scan) = scan.as_ref() else {
        return Vec::new();
    };
    let mut set = std::collections::BTreeSet::new();
    for menu in &scan.menus {
        for record in &menu.records {
            let segments = &record.segments;
            for depth in 1..=segments.len() {
                set.insert(segments[..depth].join("."));
            }
        }
    }
    set.into_iter().collect()
}

/// Ask a worker to enumerate the children of `parent` and offer those paths to
/// the add-option entry's completion store, reusing a cached result when present.
fn request_suggestions(app: &Rc<App>, store: gtk::ListStore, parent: Vec<String>) {
    *app.suggest_store.borrow_mut() = Some(store);
    let key = parent.join(".");
    *app.suggest_parent.borrow_mut() = Some(key.clone());
    let cached = app.suggest_cache.borrow().get(&key).cloned();
    if let Some(cached) = cached {
        fill_suggestions(app, cached);
        return;
    }
    let base = app.base.borrow().clone();
    let target = app.target.borrow().clone();
    let tx = app.tx.clone();
    spawn(&tx, move |tx| {
        let Some(target) = Target::parse(&target) else {
            let _ = tx.send_blocking(Msg::Suggestions(Vec::new()));
            return;
        };
        let mut paths: Vec<String> = metadata::enumerate(&base, &target, &parent)
            .unwrap_or_default()
            .into_iter()
            .map(|child| child.path)
            .collect();
        paths.sort();
        paths.dedup();
        let _ = tx.send_blocking(Msg::Suggestions(paths));
    });
}

/// The parent path implied by the text typed so far: everything before the last
/// dot, split into segments. `services.pip` yields `["services"]`; a bare
/// `services` yields the root.
fn parent_segments_from_text(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    match trimmed.rfind('.') {
        Some(index) => trimmed[..index]
            .split('.')
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

/// Merge the fetched children into the completion store, remembering them under
/// the parent key so revisiting a prefix is instant.
pub(super) fn fill_suggestions(app: &Rc<App>, suggestions: Vec<String>) {
    let Some(store) = app.suggest_store.borrow_mut().take() else {
        return;
    };
    if let Some(key) = app.suggest_parent.borrow_mut().take() {
        app.suggest_cache
            .borrow_mut()
            .insert(key, suggestions.clone());
    }
    let mut paths: std::collections::BTreeSet<String> =
        option_suggestions(app).into_iter().collect();
    paths.extend(suggestions);
    store.clear();
    for path in paths {
        store.set(&store.append(), &[(0, &path)]);
    }
}

fn build_assignment_block(option_path: &str, widget: &str, label: &str, value: &str) -> String {
    let kind = WidgetKind::parse(widget).unwrap_or(WidgetKind::Raw);
    let mut comment = String::from("/* @option\n");
    comment.push_str(&format!("   widget = \"{widget}\";\n"));
    if !label.trim().is_empty() {
        comment.push_str(&format!("   label = \"{}\";\n", label.trim()));
    }
    comment.push_str("*/\n");
    let literal = widgets::literal_for_widget(kind, value);
    format!("{comment}{option_path} = {literal};")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_kind_specific_templates() {
        let path = Path::new("/tmp/leaf.nix");

        let raw = template_for("leaf", "raw", "", path);
        assert!(raw.contains("{ ... }:"));
        assert!(raw.ends_with("{\n}\n"));
        assert!(!raw.contains("flake.modules"));

        let nixos = template_for("leaf", "nixos", "audio", path);
        assert!(nixos.contains("{ self, inputs, ... }:"));
        assert!(nixos.contains("flake.modules.nixos.audio = { pkgs, ... }:"));

        let home = template_for("leaf", "home", "shell", path);
        assert!(home.contains("flake.modules.homeManager.shell = { pkgs, ... }:"));

        let fallback = template_for("leaf", "nixos", "   ", path);
        assert!(fallback.contains("flake.modules.nixos.leaf = { pkgs, ... }:"));
    }

    #[test]
    fn derives_suggestion_parent_from_text() {
        assert!(parent_segments_from_text("services").is_empty());
        assert_eq!(
            parent_segments_from_text("services.pip"),
            vec!["services".to_string()]
        );
        assert_eq!(
            parent_segments_from_text("environment.systemPackages"),
            vec!["environment".to_string()]
        );
        assert_eq!(
            parent_segments_from_text("  services.pipewire."),
            vec!["services".to_string(), "pipewire".to_string()]
        );
    }
}
