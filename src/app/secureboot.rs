//! Secure Boot integration: status collection on a worker thread, config
//! writes through the span-patch engine, and privileged setup commands.

use std::path::Path;
use std::process::Command;
use std::rc::Rc;

use rowan::ast::AstNode;

use crate::edit::{self, PendingEdit};
use crate::flake_inputs::{self, FlakeInput};
use crate::model::Record;
use crate::scan;
use crate::secureboot::{self, SetupOptions};
use crate::value;
use crate::widgets;

use super::save;
use super::state::{App, Msg};

pub(super) fn status_hook(app: &Rc<App>) -> widgets::SecureBootStatus {
    let app = app.clone();
    Rc::new(move |force, done| {
        if force {
            *app.secureboot_state.borrow_mut() = None;
        }
        app.secureboot_pending.borrow_mut().push(done);
        if app.secureboot_loading.replace(true) {
            return;
        }
        let tx = app.tx.clone();
        std::thread::spawn(move || {
            let result = Ok(crate::secureboot::collect_state());
            let _ = tx.send_blocking(Msg::SecureBootStatus(result));
        });
    })
}

pub(super) fn write_hook(app: &Rc<App>) -> widgets::SecureBootWrite {
    let app = app.clone();
    Rc::new(move |record, options| {
        let summary = write_config(&app, &record, &options)?;
        let tx = app.tx.clone();
        save::rescan(&app, &tx);
        Ok(summary)
    })
}

pub(super) fn run_hook(app: &Rc<App>) -> widgets::SecureBootRun {
    let app = app.clone();
    Rc::new(move |commands| run_privileged(&app, commands))
}

pub(super) fn rebuild_hook(app: &Rc<App>) -> widgets::SecureBootRebuild {
    let app = app.clone();
    Rc::new(move || save::run_rebuild(&app))
}

fn has_tool(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_privileged(app: &Rc<App>, commands: &[String]) -> Result<(), String> {
    if commands.is_empty() {
        return Ok(());
    }
    if app.config.borrow().terminal_command.is_some() {
        let script = commands
            .iter()
            .map(|command| format!("sudo {command}"))
            .collect::<Vec<_>>()
            .join(" && ");
        return save::spawn_in_terminal(app, &script);
    }
    if has_tool("pkexec") {
        let script = commands.join(" && ");
        return Command::new("pkexec")
            .arg("sh")
            .arg("-c")
            .arg(script)
            .current_dir(app.base.borrow().clone())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("cannot run pkexec: {error}"));
    }
    Err("Configure a terminal command so sudo can ask for a password".to_string())
}

fn has_path(assignment: &scan::SiblingAssignment, path: &[&str]) -> bool {
    assignment.segments.len() == path.len()
        && assignment
            .segments
            .iter()
            .zip(path.iter())
            .all(|(left, right)| left == right)
}

fn starts_with_path(assignment: &scan::SiblingAssignment, path: &[&str]) -> bool {
    assignment.segments.len() >= path.len()
        && assignment
            .segments
            .iter()
            .zip(path.iter())
            .all(|(left, right)| left == right)
}

fn replace(edits: &mut Vec<PendingEdit>, file: &str, start: usize, end: usize, value: String) {
    edits.push(PendingEdit {
        file: file.to_string(),
        start,
        end,
        value,
    });
}

/// The span and items of the list inside a value, even when the value is
/// wrapped in `with pkgs;` or `let ... in`. Editing only the list keeps the
/// wrapper and its scope intact.
fn list_span(text: &str) -> Option<(usize, usize, Vec<String>)> {
    let root = rnix::Root::parse(text).ok().ok()?;
    let expr = root.expr()?;
    let list = expr
        .syntax()
        .descendants()
        .filter_map(rnix::ast::Expr::cast)
        .find(|expr| matches!(expr, rnix::ast::Expr::List(_)))?;
    let range = list.syntax().text_range();
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let items = value::list_items(text.get(start..end)?)?;
    Some((start, end, items))
}

fn write_config(app: &Rc<App>, record: &Record, options: &SetupOptions) -> Result<String, String> {
    if record.path() != "boot.lanzaboote.enable" {
        return Err("Attach the Secure Boot widget to boot.lanzaboote.enable".to_string());
    }
    if !Path::new("/sys/firmware/efi").exists() {
        return Err("Secure Boot requires a UEFI boot".to_string());
    }
    let root = app.base.borrow().clone();
    if !edit::editable(&record.file, &root) {
        return Err(format!("{} is outside the configuration root", record.file));
    }
    let text = std::fs::read_to_string(&record.file)
        .map_err(|error| format!("cannot read {}: {error}", record.file))?;
    let (siblings, insert_at) =
        scan::sibling_assignments(&text, record.byte_start, record.byte_end)
            .ok_or_else(|| format!("cannot locate the module body in {}", record.file))?;

    let mut edits = Vec::new();
    let mut block = Vec::new();
    let mut notes = Vec::new();

    if !crate::widgets::nix::truthy(&record.value) {
        replace(
            &mut edits,
            &record.file,
            record.byte_start,
            record.byte_end,
            "true".to_string(),
        );
        notes.push("enabled the attached toggle".to_string());
    }

    let import_expression = secureboot::LANZABOOTE_IMPORT;
    if let Some(imports) = siblings
        .iter()
        .find(|assignment| has_path(assignment, &["imports"]))
    {
        match list_span(&imports.value) {
            Some((start, end, mut items)) => {
                if !items.iter().any(|item| item.trim() == import_expression) {
                    items.push(import_expression.to_string());
                    replace(
                        &mut edits,
                        &record.file,
                        imports.value_start + start,
                        imports.value_start + end,
                        value::serialize_list(&items),
                    );
                }
            }
            None => notes.push(format!(
                "imports is not a literal list; add {import_expression} manually"
            )),
        }
    } else {
        block.push(format!("imports = [ {import_expression} ];"));
    }

    if let Some(packages) = siblings
        .iter()
        .find(|assignment| has_path(assignment, &["environment", "systemPackages"]))
    {
        match list_span(&packages.value) {
            Some((start, end, mut items)) => {
                if !items.iter().any(|item| item.trim() == "pkgs.sbctl") {
                    items.push("pkgs.sbctl".to_string());
                    replace(
                        &mut edits,
                        &record.file,
                        packages.value_start + start,
                        packages.value_start + end,
                        value::serialize_list(&items),
                    );
                }
            }
            None => notes.push(
                "environment.systemPackages is not a literal list; add pkgs.sbctl manually"
                    .to_string(),
            ),
        }
    } else {
        block.push("environment.systemPackages = [ pkgs.sbctl ];".to_string());
    }

    let systemd_boot = ["boot", "loader", "systemd-boot", "enable"];
    if let Some(assignment) = siblings
        .iter()
        .find(|assignment| has_path(assignment, &systemd_boot))
    {
        if assignment.value.trim() != "lib.mkForce false" {
            replace(
                &mut edits,
                &record.file,
                assignment.value_start,
                assignment.value_end,
                "lib.mkForce false".to_string(),
            );
        }
    } else {
        block.push("boot.loader.systemd-boot.enable = lib.mkForce false;".to_string());
    }

    if options.disable_grub {
        let grub_enable = ["boot", "loader", "grub", "enable"];
        if let Some(assignment) = siblings
            .iter()
            .find(|assignment| has_path(assignment, &grub_enable))
        {
            if !crate::widgets::nix::truthy(&assignment.value) {
                // GRUB is already disabled.
            } else {
                replace(
                    &mut edits,
                    &record.file,
                    assignment.value_start,
                    assignment.value_end,
                    "lib.mkForce false".to_string(),
                );
            }
        } else if siblings
            .iter()
            .any(|assignment| starts_with_path(assignment, &["boot", "loader", "grub"]))
        {
            block.push("boot.loader.grub.enable = lib.mkForce false;".to_string());
        }
    }

    write_lanzaboote(&siblings, &record.file, &mut edits, &mut block);

    if !block.is_empty() {
        replace(
            &mut edits,
            &record.file,
            insert_at,
            insert_at,
            format!("\n{}\n", block.join("\n")),
        );
    }

    let flake = Path::new(&root).join("flake.nix");
    if flake.is_file() {
        let title = flake.display().to_string();
        match flake_inputs::load(&title) {
            Ok(document) => {
                if document.input("lanzaboote").is_none() {
                    let mut input = FlakeInput::new("lanzaboote");
                    input.url = Some("github:nix-community/lanzaboote".to_string());
                    input
                        .follows
                        .insert("nixpkgs".to_string(), "nixpkgs".to_string());
                    match document.edits_for_upsert(&input) {
                        Ok(mut input_edits) => edits.append(&mut input_edits),
                        Err(error) => notes.push(format!("flake input: {error}")),
                    }
                }
            }
            Err(error) => notes.push(format!("flake.nix: {error}")),
        }
    } else {
        notes.push("no flake.nix; add the Lanzaboote input manually".to_string());
    }

    if edits.is_empty() {
        return Ok("Secure Boot configuration is already up to date".to_string());
    }
    let files = edit::apply(&edits, &root)?;
    *app.secureboot_state.borrow_mut() = None;
    let mut summary = format!("Wrote Secure Boot configuration to {files} file(s).");
    if !notes.is_empty() {
        summary.push(' ');
        summary.push_str(&notes.join("; "));
        summary.push('.');
    }
    summary.push_str(" Rebuild to install Lanzaboote.");
    Ok(summary)
}

fn write_lanzaboote(
    siblings: &[scan::SiblingAssignment],
    file: &str,
    edits: &mut Vec<PendingEdit>,
    block: &mut Vec<String>,
) {
    let wanted: Vec<(&str, String)> = vec![(
        "pkiBundle",
        secureboot::nix_string(secureboot::DEFAULT_PKI_BUNDLE),
    )];

    if let Some(attrset) = siblings.iter().find(|assignment| {
        has_path(assignment, &["boot", "lanzaboote"])
            && value::classify(&assignment.value) == value::ValueKind::AttrSet
    }) {
        let inner = scan::scan_value(&attrset.value);
        let mut missing = Vec::new();
        for (key, value) in &wanted {
            match inner.iter().find(|record| record.path() == *key) {
                Some(record) => {
                    if record.value.trim() != value.trim() {
                        replace(
                            edits,
                            file,
                            attrset.value_start + record.byte_start,
                            attrset.value_start + record.byte_end,
                            value.clone(),
                        );
                    }
                }
                None => missing.push(format!("  {key} = {value};")),
            }
        }
        if !missing.is_empty() {
            if let Some(close) = attrset.value.rfind('}') {
                let at = attrset.value_start + close;
                replace(edits, file, at, at, format!("\n{}\n", missing.join("\n")));
            }
        }
        return;
    }

    let dotted = siblings
        .iter()
        .any(|assignment| starts_with_path(assignment, &["boot", "lanzaboote"]));
    if dotted {
        for (key, value) in &wanted {
            let path = ["boot", "lanzaboote", *key];
            match siblings
                .iter()
                .find(|assignment| has_path(assignment, &path))
            {
                Some(assignment) => {
                    if assignment.value.trim() != value.trim() {
                        replace(
                            edits,
                            file,
                            assignment.value_start,
                            assignment.value_end,
                            value.clone(),
                        );
                    }
                }
                None => block.push(format!("boot.lanzaboote.{key} = {value};")),
            }
        }
        return;
    }

    for (key, value) in &wanted {
        block.push(format!("boot.lanzaboote.{key} = {value};"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toggle_span(text: &str) -> (usize, usize) {
        let start = text.find("false").unwrap();
        (start, start + "false".len())
    }

    #[test]
    fn recognises_sibling_paths() {
        let assignment = scan::SiblingAssignment {
            segments: vec![
                "boot".to_string(),
                "loader".to_string(),
                "systemd-boot".to_string(),
                "enable".to_string(),
            ],
            ..scan::SiblingAssignment::default()
        };
        assert!(has_path(
            &assignment,
            &["boot", "loader", "systemd-boot", "enable"]
        ));
        assert!(starts_with_path(&assignment, &["boot", "loader"]));
        assert!(!has_path(&assignment, &["boot", "loader"]));
    }

    #[test]
    fn finds_the_inner_list_of_a_with_expression() {
        let text = "with pkgs; [ git vim ]";
        let (start, end, items) = list_span(text).unwrap();
        assert_eq!(&text[start..end], "[ git vim ]");
        assert_eq!(items, vec!["git".to_string(), "vim".to_string()]);
    }

    #[test]
    fn fills_missing_keys_in_an_existing_attrset() {
        let text = "{ config, lib, ... }:\n{\n  boot.lanzaboote.enable = false;\n  boot.lanzaboote = {\n    enable = true;\n  };\n}\n";
        let (start, end) = toggle_span(text);
        let (siblings, _) = scan::sibling_assignments(text, start, end).unwrap();
        let mut edits = Vec::new();
        let mut block = Vec::new();
        write_lanzaboote(&siblings, "cfg.nix", &mut edits, &mut block);
        assert!(block.is_empty());
        assert!(edits
            .iter()
            .any(|edit| edit.value.contains("pkiBundle = \"/var/lib/sbctl\";")));
    }

    #[test]
    fn inserts_dotted_options_when_no_lanzaboote_block_exists() {
        let text = "{ config, lib, ... }:\n{\n  boot.lanzaboote.enable = false;\n}\n";
        let (start, end) = toggle_span(text);
        let (siblings, _) = scan::sibling_assignments(text, start, end).unwrap();
        let mut edits = Vec::new();
        let mut block = Vec::new();
        write_lanzaboote(&siblings, "cfg.nix", &mut edits, &mut block);
        assert!(edits.is_empty());
        assert!(block
            .iter()
            .any(|line| line == "boot.lanzaboote.pkiBundle = \"/var/lib/sbctl\";"));
    }
}
