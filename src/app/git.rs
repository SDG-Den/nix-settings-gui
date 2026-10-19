use std::rc::Rc;

use gtk::prelude::*;

use super::pump::spawn;
use super::state::{App, Msg, Tx};

// ---------------------------------------------------------------------------
// git sync
// ---------------------------------------------------------------------------

/// Pull the flake directory, then report whether the tree is dirty or has
/// untracked files. Runs off the UI thread.
pub(super) fn start_git_sync(app: &Rc<App>, tx: &Tx) {
    let dir = app.base.borrow().clone();
    if dir.is_empty() {
        app.status.set_text("No flake directory open");
        return;
    }
    app.status.set_text("Git: pulling...");
    spawn(tx, move |tx| {
        let result = run_git(&dir, &["pull"]).and_then(|_| {
            let status = run_git(&dir, &["status", "--porcelain"])?;
            Ok(!status.trim().is_empty())
        });
        let _ = tx.send_blocking(Msg::GitPulled(result));
    });
}

/// Stage every change in the flake directory, commit it, and push.
fn git_commit_and_push(dir: &str, message: &str) -> Result<String, String> {
    run_git(dir, &["add", "./*"])?;
    run_git(dir, &["commit", "-m", message])?;
    run_git(dir, &["push"])?;
    Ok(message.to_string())
}

/// Run a git subcommand in `dir`, returning stdout on success and a message on
/// failure.
fn run_git(dir: &str, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run git {}: {e}", args.join(" ")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(if detail.is_empty() {
        format!("git {} failed", args.join(" "))
    } else {
        format!("git {}: {detail}", args.join(" "))
    })
}

/// Ask for a commit message, then commit and push the staged changes.
pub(super) fn commit_message_dialog(app: &Rc<App>, tx: &Tx) {
    let dialog = gtk::Dialog::builder()
        .title("Commit changes")
        .transient_for(&app.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", gtk::ResponseType::Cancel);
    let commit = dialog.add_button("Commit & push", gtk::ResponseType::Accept);
    commit.add_css_class("suggested-action");

    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Commit message"));
    entry.set_hexpand(true);
    entry.set_activates_default(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(
        &gtk::Label::builder()
            .label("Commit message")
            .xalign(0.0)
            .build(),
    );
    content.append(&entry);
    dialog.content_area().append(&content);

    {
        let app = app.clone();
        let tx = tx.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let message = entry.text().to_string();
                let dir = app.base.borrow().clone();
                app.status.set_text("Git: committing and pushing...");
                spawn(&tx, move |tx| {
                    let result = git_commit_and_push(&dir, &message);
                    let _ = tx.send_blocking(Msg::GitSynced(result));
                });
            }
            dialog.close();
        });
    }
    dialog.present();
}
