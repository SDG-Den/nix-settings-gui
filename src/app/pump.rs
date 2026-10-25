use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::scan;

use super::dialogs::fill_suggestions;
use super::enrich::apply_infos;
use super::git::commit_message_dialog;
use super::menu::rebuild_view;
use super::save::{clear_busy, collect_edits, run_rebuild, set_busy};
use super::state::{App, Msg, Tx};

// ---------------------------------------------------------------------------
// message pump
// ---------------------------------------------------------------------------

fn watch_files(app: &Rc<App>) {
    app.watchers.borrow_mut().clear();
    let mut files: Vec<String> = Vec::new();
    {
        let scan = app.scan.borrow();
        let Some(scan) = scan.as_ref() else {
            return;
        };
        for menu in &scan.menus {
            for record in &menu.records {
                if !record.file.is_empty() && !files.contains(&record.file) {
                    files.push(record.file.clone());
                }
            }
        }
    }
    for file in files {
        let Ok(monitor) = gio::File::for_path(&file)
            .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            continue;
        };
        let handler_app = app.clone();
        monitor.connect_changed(move |_, _, _, _| {
            if collect_edits(&handler_app).is_empty() {
                let base = handler_app.base.borrow().clone();
                spawn(&handler_app.tx, move |tx| {
                    let result = scan::scan_root(&base);
                    let _ = tx.send_blocking(Msg::Scanned(result));
                });
            } else {
                handler_app
                    .status
                    .set_text("File changed on disk; reload to refresh");
            }
        });
        app.watchers.borrow_mut().push(monitor);
    }
}

pub(super) fn handle_msg(app: &Rc<App>, tx: &Tx, msg: Msg) {
    match msg {
        Msg::Scanned(Ok(result)) => {
            *app.targets.borrow_mut() = result.targets.clone();
            let hosts = scan::hosts_from_targets(&result.targets);
            if !app.manual_host.get() {
                let hostname = glib::host_name().to_string();
                if let Some(host) = hosts
                    .iter()
                    .find(|h| **h == hostname || hostname.starts_with(&format!("{h}.")))
                {
                    *app.current_host.borrow_mut() = host.clone();
                    app.system_toggle.set_active(true);
                    app.system_only.set(true);
                }
            }
            let dendritic = result
                .menus
                .iter()
                .any(|menu| menu.records.iter().any(|r| !r.origin.is_empty()));
            *app.hosts.borrow_mut() = hosts.clone();
            app.host_combo.remove_all();
            for host in &hosts {
                app.host_combo.append_text(host);
            }
            if let Some(first) = hosts.first() {
                let current = app.current_host.borrow().clone();
                if current.is_empty() {
                    *app.current_host.borrow_mut() = first.clone();
                }
                let selected = app
                    .host_combo
                    .active_text()
                    .map(|t| t.to_string())
                    .filter(|t| hosts.contains(t))
                    .unwrap_or_else(|| app.current_host.borrow().clone());
                let index = hosts.iter().position(|h| *h == selected).unwrap_or(0);
                app.host_combo.set_active(Some(index as u32));
                *app.current_host.borrow_mut() = hosts[index].clone();
            }
            let target = if dendritic {
                "fp".to_string()
            } else if let Some(host) = hosts.first() {
                format!("nixos:{host}")
            } else {
                String::new()
            };
            *app.target.borrow_mut() = target;

            let path = app.base.borrow().clone();
            let name = Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(path);
            app.title.set_text(&format!("nix-settings-gui - {name}"));
            *app.scan.borrow_mut() = Some(result);
            watch_files(app);
            app.status.set_text("Ready");
            rebuild_view(app);
        }
        Msg::Scanned(Err(e)) => {
            app.status.set_text("Scan failed");
            show_error(&app.window, &e);
        }
        Msg::SaveProgress(fraction, label) => {
            set_busy(app, &label, fraction);
        }
        Msg::Saved(Ok(count)) => {
            clear_busy(app);
            app.status.set_text(&format!("Saved {count} file(s)"));
            let pending = app.pending_rebuild.replace(false);
            let base = app.base.borrow().clone();
            spawn(tx, move |tx| {
                let result = scan::scan_root(&base);
                let _ = tx.send_blocking(Msg::Scanned(result));
            });
            if pending {
                run_rebuild(app);
            }
        }
        Msg::Saved(Err(e)) => {
            clear_busy(app);
            app.pending_rebuild.set(false);
            app.status.set_text("Save failed");
            show_error(&app.window, &e);
        }
        Msg::Invalid(errors) => {
            clear_busy(app);
            app.pending_rebuild.set(false);
            app.status
                .set_text(&format!("Package error: {}", errors.join("; ")));
        }
        Msg::Inserted(Ok(())) => {
            app.status.set_text("Option added");
            let base = app.base.borrow().clone();
            spawn(tx, move |tx| {
                let result = scan::scan_root(&base);
                let _ = tx.send_blocking(Msg::Scanned(result));
            });
        }
        Msg::Inserted(Err(e)) => {
            app.status.set_text("Add option failed");
            show_error(&app.window, &e);
        }
        Msg::SnippetInserted(Ok(())) => {
            app.status.set_text("Snippet added");
            let base = app.base.borrow().clone();
            spawn(tx, move |tx| {
                let result = scan::scan_root(&base);
                let _ = tx.send_blocking(Msg::Scanned(result));
            });
        }
        Msg::SnippetInserted(Err(e)) => {
            app.status.set_text("Add snippet failed");
            show_error(&app.window, &e);
        }
        Msg::Infos(file, infos) => apply_infos(app, &file, &infos),
        Msg::Suggestions(suggestions) => fill_suggestions(app, suggestions),
        Msg::PluginRegistry { epoch, result } => {
            if epoch != app.plugin_registry.epoch.get() {
                return;
            }
            app.plugin_registry.loading.set(false);
            let pending: Vec<_> = app.plugin_registry.pending.borrow_mut().drain(..).collect();
            match result {
                Ok(plugins) => {
                    *app.plugin_registry.cache.borrow_mut() = Some(plugins.clone());
                    for callback in pending {
                        callback(Ok(plugins.clone()));
                    }
                }
                Err(error) => {
                    for callback in pending {
                        callback(Err(error.clone()));
                    }
                }
            }
        }
        Msg::PluginSettings { epoch, id, result } => {
            if epoch != app.plugin_registry.epoch.get() {
                return;
            }
            app.plugin_registry
                .settings_loading
                .borrow_mut()
                .remove(&id);
            app.plugin_registry
                .settings
                .borrow_mut()
                .insert(id.clone(), result.clone());
            let pending = app
                .plugin_registry
                .settings_pending
                .borrow_mut()
                .remove(&id)
                .unwrap_or_default();
            for callback in pending {
                callback(result.clone());
            }
        }
        Msg::GitPulled(Ok(true)) => commit_message_dialog(app, tx),
        Msg::GitPulled(Ok(false)) => app.status.set_text("Git: already up to date"),
        Msg::GitPulled(Err(e)) => {
            app.status.set_text("Git sync failed");
            show_error(&app.window, &e);
        }
        Msg::GitSynced(Ok(_)) => app.status.set_text("Git: synced"),
        Msg::GitSynced(Err(e)) => {
            app.status.set_text("Git sync failed");
            show_error(&app.window, &e);
        }
        Msg::ApplyFinished { path, result } => super::apply::finished(app, &path, result),
    }
}

pub(super) fn show_error(parent: &impl IsA<gtk::Window>, message: &str) {
    let dialog = gtk::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .message_type(gtk::MessageType::Error)
        .buttons(gtk::ButtonsType::Close)
        .text(message)
        .build();
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

pub(super) fn spawn<F: FnOnce(Tx) + Send + 'static>(tx: &Tx, work: F) {
    let tx = tx.clone();
    std::thread::spawn(move || work(tx));
}

fn discover_flakes(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 4 {
            return;
        }
        if dir.join("flake.nix").is_file() {
            out.push(dir.to_path_buf());
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "target" | "result" | "node_modules" | ".git" | ".direnv"
                )
            {
                continue;
            }
            walk(&path, depth + 1, out);
        }
    }
    if root.is_dir() {
        walk(root, 0, &mut found);
    }
    found.sort();
    found.dedup();
    found
}

pub(super) fn populate_flakes(app: &Rc<App>) -> Option<String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut flakes = discover_flakes(&home.join("repos"));
    let config = app.config.borrow().clone();
    let mut recents: Vec<PathBuf> = Vec::new();
    for path in config
        .recent_flake_paths
        .iter()
        .chain(config.flake_path.iter())
    {
        let path = PathBuf::from(path);
        if !recents.contains(&path) && !flakes.contains(&path) {
            recents.push(path);
        }
    }
    for path in recents.into_iter().rev() {
        flakes.insert(0, path);
    }
    app.flake_combo.remove_all();
    for flake in &flakes {
        let label = flake
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| flake.display().to_string());
        app.flake_combo
            .append(Some(&flake.display().to_string()), &label);
    }
    *app.flakes.borrow_mut() = flakes;
    if app.flakes.borrow().is_empty() {
        return None;
    }
    let index = app
        .config
        .borrow()
        .flake_path
        .as_deref()
        .and_then(|path| {
            app.flakes
                .borrow()
                .iter()
                .position(|f| f.display().to_string() == path)
        })
        .or_else(|| {
            app.flakes
                .borrow()
                .iter()
                .position(|f| f.display().to_string().ends_with("dendrite"))
        })
        .unwrap_or(0);
    let path = app.flakes.borrow()[index].display().to_string();
    app.flake_combo.set_active(Some(index as u32));
    Some(path)
}
