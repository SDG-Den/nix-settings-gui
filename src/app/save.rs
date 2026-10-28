use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::edit;
use crate::scan;
use crate::widgets;

use super::pump::{show_error, spawn};
use super::state::{App, Msg, Tx};

// ---------------------------------------------------------------------------
// save and rebuild
// ---------------------------------------------------------------------------

/// Every staged edit across every page, in page order. Controls hidden by a
/// `visibleWhen` dependency are skipped, so a disabled parent cannot commit
/// its children's staged edits.
pub(super) fn collect_edits(app: &Rc<App>) -> Vec<edit::PendingEdit> {
    let mut edits = Vec::new();
    for page in app.pages.borrow().iter() {
        for control in &page.controls {
            if !control.dep_visible.get() {
                continue;
            }
            let editor = control.editor.borrow();
            edits.extend(editor.edits(&control.record));
        }
    }
    edits
}

/// A save-blocking error reported by a control that enforces an invariant,
/// such as the user list keeping one sudo account with a password.
fn validation_error(app: &Rc<App>) -> Option<String> {
    for page in app.pages.borrow().iter() {
        for control in &page.controls {
            if !control.dep_visible.get() {
                continue;
            }
            if let Some(error) = control.editor.borrow().validation_error() {
                return Some(error);
            }
        }
    }
    None
}

/// Package assignments that changed, as `(path, literal)` pairs to validate
/// with nixd before writing. Hidden-by-dependency controls are skipped.
fn collect_package_checks(app: &Rc<App>) -> Vec<(String, String)> {
    let mut checks = Vec::new();
    for page in app.pages.borrow().iter() {
        for control in &page.controls {
            if !control.dep_visible.get() {
                continue;
            }
            if !widgets::is_package(&control.record) {
                continue;
            }
            let editor = control.editor.borrow();
            if editor.unchanged(&control.record.value) {
                continue;
            }
            checks.push((
                control.record.path(),
                format!("with pkgs; {}", editor.to_nix()),
            ));
        }
    }
    checks
}

pub(super) fn save_all(app: &Rc<App>, tx: &Tx, rebuild: bool) {
    let edits = collect_edits(app);
    if edits.is_empty() {
        clear_busy(app);
        app.status.set_text("Nothing to save");
        if rebuild {
            run_rebuild(app);
        }
        return;
    }
    if let Some(error) = validation_error(app) {
        app.status.set_text("Cannot save");
        show_error(&app.window, &error);
        return;
    }
    let root = app.base.borrow().clone();
    let checks = collect_package_checks(app);
    let flake = app.config.borrow().flake_path.clone();
    set_busy(app, "Saving...", 0.0);
    spawn(tx, move |tx| {
        const VALIDATE_END: f64 = 0.7;
        let report_tx = tx.clone();
        let report = move |done: usize, total: usize| {
            let fraction = if total == 0 {
                VALIDATE_END
            } else {
                VALIDATE_END * done as f64 / total as f64
            };
            let label = if done == 0 {
                "Validating package changes...".to_string()
            } else {
                format!("Validated {done} of {total} package change(s)...")
            };
            let _ = report_tx.send_blocking(Msg::SaveProgress(fraction, label));
        };
        if let Ok(errors) = crate::nixd::query_errors(flake.as_deref(), checks, report) {
            if !errors.is_empty() {
                let _ = tx.send_blocking(Msg::Invalid(errors));
                return;
            }
        }
        let _ = tx.send_blocking(Msg::SaveProgress(
            VALIDATE_END,
            "Writing changes...".to_string(),
        ));
        let result = edit::apply(&edits, &root);
        let _ = tx.send_blocking(Msg::SaveProgress(1.0, "Saved".to_string()));
        let _ = tx.send_blocking(Msg::Saved(result));
    });
    app.pending_rebuild.set(rebuild);
}

/// Show the field overlay while any save or scan is in flight. The spinner
/// starts and stops on the scrim's map and unmap signals.
fn refresh_field_overlay(app: &Rc<App>) {
    let visible = app.saving.get() || app.scans_in_flight.get() > 0;
    app.field_scrim.set_visible(visible);
}

/// Record whether a save is in flight and refresh the field overlay.
pub(super) fn set_saving(app: &Rc<App>, saving: bool) {
    app.saving.set(saving);
    refresh_field_overlay(app);
}

/// Record that a scan started and refresh the field overlay.
fn scan_started(app: &Rc<App>) {
    app.scans_in_flight.set(app.scans_in_flight.get() + 1);
    refresh_field_overlay(app);
}

/// Record that a scan finished and refresh the field overlay.
pub(super) fn scan_finished(app: &Rc<App>) {
    app.scans_in_flight
        .set(app.scans_in_flight.get().saturating_sub(1));
    refresh_field_overlay(app);
}

/// Show the busy indicators (spinner and progress bar) with a status message.
pub(super) fn set_busy(app: &Rc<App>, message: &str, fraction: f64) {
    set_saving(app, true);
    app.spinner.set_visible(true);
    app.spinner.start();
    app.progress.set_visible(true);
    app.progress.set_fraction(fraction);
    app.status.set_text(message);
}

/// Hide the busy indicators once a long-running task finishes.
pub(super) fn clear_busy(app: &Rc<App>) {
    set_saving(app, false);
    app.spinner.stop();
    app.spinner.set_visible(false);
    app.progress.set_visible(false);
    app.progress.set_fraction(0.0);
}

/// What the unsaved-changes dialog should do for a response.
#[derive(Debug, PartialEq, Eq)]
enum ChangesAction {
    Save,
    Discard,
    Cancel,
}

fn changes_action(response: gtk::ResponseType) -> ChangesAction {
    match response {
        gtk::ResponseType::Yes => ChangesAction::Save,
        gtk::ResponseType::No => ChangesAction::Discard,
        _ => ChangesAction::Cancel,
    }
}

/// Prompt to save, discard, or cancel staged edits. `on_save` runs after a
/// successful save or an explicit discard, never after cancel.
pub(super) fn run_changes_prompt(app: &Rc<App>, on_save: Box<dyn Fn()>) {
    let dialog = gtk::Dialog::builder()
        .title("Unsaved changes")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    dialog.add_button("Discard", gtk::ResponseType::No);
    let save = dialog.add_button("Save", gtk::ResponseType::Yes);
    save.add_css_class("suggested-action");
    dialog.set_default_response(gtk::ResponseType::Yes);

    let message = gtk::Label::builder()
        .label("You have unsaved changes. Save them before closing?")
        .xalign(0.0)
        .wrap(true)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    spinner.set_halign(gtk::Align::Center);
    spinner.set_margin_bottom(12);
    {
        let spinner = spinner.clone();
        spinner.connect_map(move |spinner| spinner.start());
    }
    {
        let spinner = spinner.clone();
        spinner.connect_unmap(move |spinner| spinner.stop());
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&message);
    content.append(&spinner);
    dialog.content_area().append(&content);

    let on_save: Rc<dyn Fn()> = Rc::from(on_save);
    let state = app.clone();
    dialog.connect_response(move |dialog, response| match changes_action(response) {
        ChangesAction::Save => {
            if let Some(error) = validation_error(&state) {
                show_error(&state.window, &error);
                return;
            }
            dialog.set_response_sensitive(gtk::ResponseType::Yes, false);
            dialog.set_response_sensitive(gtk::ResponseType::No, false);
            dialog.set_response_sensitive(gtk::ResponseType::Cancel, false);
            dialog.set_deletable(false);
            spinner.set_visible(true);
            set_saving(&state, true);

            let edits = collect_edits(&state);
            let root = state.base.borrow().clone();
            let (done_tx, done_rx) = async_channel::bounded(1);
            spawn(&state.tx, move |_| {
                let _ = done_tx.send_blocking(edit::apply(&edits, &root));
            });

            let dialog = dialog.clone();
            let spinner = spinner.clone();
            let state = state.clone();
            let on_save = on_save.clone();
            glib::MainContext::default().spawn_local(async move {
                let result = done_rx.recv().await;
                spinner.set_visible(false);
                set_saving(&state, false);
                match result {
                    Ok(Ok(count)) => {
                        state.status.set_text(&format!("Saved {count} file(s)"));
                        dialog.close();
                        on_save();
                    }
                    Ok(Err(error)) => {
                        state.status.set_text("Save failed");
                        show_error(&state.window, &error);
                        dialog.close();
                    }
                    Err(_) => {
                        state.status.set_text("Save failed");
                        show_error(&state.window, "Save worker stopped unexpectedly");
                        dialog.close();
                    }
                }
            });
        }
        ChangesAction::Discard => {
            discard_changes(&state);
            on_save();
            dialog.close();
        }
        ChangesAction::Cancel => dialog.close(),
    });
    dialog.present();
}

/// Drop every staged edit by rebuilding the editors from the scanned records.
pub(super) fn discard_changes(app: &Rc<App>) {
    super::menu::rebuild_view(app);
    app.status.set_text("Changes discarded");
}

pub(super) fn reload(app: &Rc<App>, tx: &Tx) {
    if collect_edits(app).is_empty() {
        rescan(app, tx);
        return;
    }
    let app_for_save = app.clone();
    let tx_for_save = tx.clone();
    run_changes_prompt(app, Box::new(move || rescan(&app_for_save, &tx_for_save)));
}

/// Re-read every file under the current root, exactly like `open_flake`.
pub(super) fn rescan(app: &Rc<App>, tx: &Tx) {
    let base = app.base.borrow().clone();
    *app.wallpaper_set.borrow_mut() = None;
    app.status.set_text("reloading from disk...");
    scan_started(app);
    spawn(tx, move |tx| {
        let result = scan::scan_root(&base);
        let _ = tx.send_blocking(Msg::Scanned(result));
    });
}

/// Show the pending span patches as a unified diff, old text read from disk.
pub(super) fn preview_changes(app: &Rc<App>, _tx: &Tx) {
    let edits = collect_edits(app);
    let dialog = gtk::Dialog::builder()
        .title("Pending changes")
        .transient_for(&app.window)
        .modal(true)
        .default_width(720)
        .default_height(480)
        .build();
    dialog.add_button("Close", gtk::ResponseType::Close);

    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .build();
    let buffer = view.buffer();
    if edits.is_empty() {
        buffer.set_text("No pending changes.");
    } else {
        let mut cache: HashMap<String, String> = HashMap::new();
        let mut diff = String::new();
        for pending in &edits {
            let text = cache
                .entry(pending.file.clone())
                .or_insert_with(|| std::fs::read_to_string(&pending.file).unwrap_or_default());
            let old = text.get(pending.start..pending.end).unwrap_or("");
            diff.push_str(&format!(
                "--- {}\n-{}\n+{}\n\n",
                pending.file, old, pending.value
            ));
        }
        buffer.set_text(&diff);
    }

    let scrolled = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(360)
        .build();
    dialog.content_area().append(&scrolled);
    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

pub(super) fn run_rebuild(app: &Rc<App>) {
    let config = app.config.borrow();
    let base = app.base.borrow().clone();
    let Some(command) = config.rebuild_command.clone() else {
        app.status.set_text("No rebuild command configured");
        return;
    };
    let command = with_reloads(
        command,
        config.reload_mango_on_rebuild,
        config.reload_dms_on_rebuild,
    );
    let terminal = config.terminal_command.clone();
    let mut process = match terminal {
        Some(prefix) if prefix.contains("{cmd}") => {
            let full = prefix.replace("{cmd}", &command);
            let mut parts = shell_words(&full);
            if parts.is_empty() {
                app.status.set_text("Empty terminal command");
                return;
            }
            let program = parts.remove(0);
            let mut cmd = std::process::Command::new(program);
            cmd.args(parts);
            cmd
        }
        Some(prefix) => {
            let parts = shell_words(&prefix);
            if parts.is_empty() {
                app.status.set_text("Empty terminal command");
                return;
            }
            let program = parts[0].clone();
            let mut cmd = std::process::Command::new(program);
            cmd.args(&parts[1..]);
            cmd.arg(&command);
            cmd
        }
        None => {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c").arg(&command);
            cmd
        }
    };
    process.current_dir(base);
    match process.spawn() {
        Ok(_) => app.status.set_text("Rebuild started in terminal"),
        Err(e) => app.status.set_text(&format!("Cannot start rebuild: {e}")),
    }
}

fn with_reloads(mut command: String, reload_mango: bool, reload_dms: bool) -> String {
    if reload_mango {
        command.push_str(" && mmsg dispatch reload_config");
    }
    if reload_dms {
        command.push_str(" && dms kill && mmsg dispatch spawn_shell,dms run");
    }
    command
}

fn shell_words(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|word| word.trim_matches('\'').trim_matches('"').to_string())
        .filter(|word| !word.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reloads_are_appended_to_the_rebuild_command() {
        assert_eq!(with_reloads("rebuild".into(), false, false), "rebuild");
        assert_eq!(
            with_reloads("rebuild".into(), true, false),
            "rebuild && mmsg dispatch reload_config"
        );
        assert_eq!(
            with_reloads("rebuild".into(), false, true),
            "rebuild && dms kill && mmsg dispatch spawn_shell,dms run"
        );
        assert_eq!(
            with_reloads("rebuild".into(), true, true),
            "rebuild && mmsg dispatch reload_config && dms kill && mmsg dispatch spawn_shell,dms run"
        );
    }

    #[test]
    fn maps_prompt_responses() {
        assert_eq!(changes_action(gtk::ResponseType::Yes), ChangesAction::Save);
        assert_eq!(
            changes_action(gtk::ResponseType::No),
            ChangesAction::Discard
        );
        assert_eq!(
            changes_action(gtk::ResponseType::Cancel),
            ChangesAction::Cancel
        );
        assert_eq!(
            changes_action(gtk::ResponseType::DeleteEvent),
            ChangesAction::Cancel
        );
    }
}
