//! Applying an option immediately: a debounced command that runs one at a time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;

use crate::apply::{plain_value, substitute, ApplyRequest};

use super::state::{App, Msg};

/// How long a change waits before its command runs, so a slider drag produces
/// one command with the final value.
const DEBOUNCE_MS: u64 = 300;

/// The waiting and running state of the apply queue.
pub(super) struct ApplyState {
    /// The request waiting for its debounce timer, or for the running command.
    pending: RefCell<Option<ApplyRequest>>,
    /// The timer that fires the pending request.
    timer: RefCell<Option<glib::SourceId>>,
    /// Whether a command is running right now.
    running: Cell<bool>,
}

impl ApplyState {
    pub(super) fn new() -> Rc<Self> {
        Rc::new(Self {
            pending: RefCell::new(None),
            timer: RefCell::new(None),
            running: Cell::new(false),
        })
    }
}

/// Queue an option's apply command. The newest request replaces a waiting one,
/// so only the final value of a burst of changes is applied.
pub(super) fn schedule(app: &Rc<App>, request: ApplyRequest) {
    if !app.config.borrow().live_apply {
        return;
    }
    let state = app.apply_state.clone();
    *state.pending.borrow_mut() = Some(request);
    let previous = state.timer.borrow_mut().take();
    if let Some(id) = previous {
        id.remove();
    }
    let app = app.clone();
    let timer = glib::timeout_add_local_once(Duration::from_millis(DEBOUNCE_MS), move || {
        app.apply_state.timer.borrow_mut().take();
        tick(&app);
    });
    *state.timer.borrow_mut() = Some(timer);
}

/// The debounce elapsed: run the waiting request unless a command is running.
fn tick(app: &Rc<App>) {
    if app.apply_state.running.get() {
        return;
    }
    let request = app.apply_state.pending.borrow_mut().take();
    if let Some(request) = request {
        run(app, request);
    }
}

/// Run one command on a worker thread and report its result back.
fn run(app: &Rc<App>, request: ApplyRequest) {
    app.apply_state.running.set(true);
    app.apply_spinner.start();
    app.status
        .set_text(&format!("Applying {}...", request.path));
    let command = substitute(&request.command, &plain_value(&request.value));
    let base = app.base.borrow().clone();
    let tx = app.tx.clone();
    let path = request.path.clone();
    std::thread::spawn(move || {
        let result = run_command(&command, &base);
        let _ = tx.send_blocking(Msg::ApplyFinished { path, result });
    });
}

/// Run the command in a shell and report its error output when it fails.
fn run_command(command: &str, base: &str) -> Result<(), String> {
    let mut process = std::process::Command::new("sh");
    process.arg("-c").arg(command);
    if !base.is_empty() {
        process.current_dir(base);
    }
    let output = process.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first_line = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if first_line.trim().is_empty() {
        Err(format!("command failed: {command}"))
    } else {
        Err(first_line.trim().to_string())
    }
}

/// Report a finished command and run the request that waited for it.
pub(super) fn finished(app: &Rc<App>, path: &str, result: Result<(), String>) {
    app.apply_state.running.set(false);
    app.apply_spinner.stop();
    match result {
        Ok(()) => app.status.set_text(&format!("Applied {path}")),
        Err(error) => app.status.set_text(&format!("Apply failed: {error}")),
    }
    let waiting = app.apply_state.pending.borrow().is_some();
    let timer = app.apply_state.timer.borrow().is_some();
    if waiting && !timer {
        let request = app.apply_state.pending.borrow_mut().take();
        if let Some(request) = request {
            run(app, request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_a_command_in_a_shell() {
        assert!(run_command("true", "").is_ok());
        assert!(run_command("exit 0", "").is_ok());
    }

    #[test]
    fn reports_the_first_error_line_of_a_failed_command() {
        let error = run_command("echo first >&2; echo second >&2; exit 1", "").unwrap_err();
        assert_eq!(error, "first");
    }

    #[test]
    fn reports_the_command_when_a_failure_has_no_output() {
        let error = run_command("exit 3", "").unwrap_err();
        assert_eq!(error, "command failed: exit 3");
    }
}
