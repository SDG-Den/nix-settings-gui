use std::collections::BTreeMap;
use std::path::Path;

use crate::scan;

/// A single in-place replacement: `file[start..end]` becomes `value`. Spans
/// index the original file text, as produced by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEdit {
    pub file: String,
    pub start: usize,
    pub end: usize,
    pub value: String,
}

/// Apply every edit in one batch. Edits are grouped by file, applied from the
/// end backwards so earlier offsets stay valid, parse-checked with `rnix`,
/// formatted with `nixfmt`, and written atomically. If any file fails to parse
/// nothing is written and an error is returned, so the batch is all-or-nothing.
pub fn apply(edits: &[PendingEdit], root: &str) -> Result<usize, String> {
    let mut by_file: BTreeMap<&str, Vec<&PendingEdit>> = BTreeMap::new();
    for edit in edits {
        if !editable(&edit.file, root) {
            continue;
        }
        by_file.entry(edit.file.as_str()).or_default().push(edit);
    }

    let mut staged: Vec<(&str, String)> = Vec::new();
    for (file, file_edits) in &by_file {
        let mut text =
            std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
        let mut ordered = file_edits.clone();
        ordered.sort_by_key(|a| std::cmp::Reverse(a.start));
        for edit in ordered {
            if edit.start > edit.end || edit.end > text.len() {
                return Err(format!(
                    "span {}..{} out of range for {file}",
                    edit.start, edit.end
                ));
            }
            text.replace_range(edit.start..edit.end, &edit.value);
        }
        if rnix::Root::parse(&text).ok().is_err() {
            return Err(format!(
                "edit would make {file} unparseable; nothing written"
            ));
        }
        staged.push((file, text));
    }

    for (file, text) in &staged {
        let formatted = nixfmt(text).unwrap_or_else(|| text.clone());
        write_atomic(file, &formatted)?;
    }
    Ok(staged.len())
}

/// Insert `block` (a comment plus an assignment) directly after the opening
/// brace of the module body selected by `origin`, then reformat. `origin` is
/// empty for a plain file and `modules.<kind>.<name>` for a dendritic module.
pub fn insert_assignment(file: &str, origin: &str, block: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let offset = scan::body_insert_offset(&text, origin)
        .ok_or_else(|| format!("cannot locate a module body in {file}"))?;
    let insertion = format!("\n{block}\n");
    let mut patched = String::with_capacity(text.len() + insertion.len());
    patched.push_str(&text[..offset]);
    patched.push_str(&insertion);
    patched.push_str(&text[offset..]);
    if rnix::Root::parse(&patched).ok().is_err() {
        return Err(format!("new assignment would make {file} unparseable"));
    }
    let formatted = nixfmt(&patched).unwrap_or(patched);
    write_atomic(file, &formatted)
}

/// Insert `block` directly at `offset`, then reformat. Used to add a snippet
/// instance at an `@insert` marker, whose line start is the offset, so the
/// marker shifts down and the snippet lands above it.
pub fn insert_at(file: &str, offset: usize, block: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(format!("insert offset {offset} out of range for {file}"));
    }
    let insertion = format!("{block}\n");
    let mut patched = String::with_capacity(text.len() + insertion.len());
    patched.push_str(&text[..offset]);
    patched.push_str(&insertion);
    patched.push_str(&text[offset..]);
    if rnix::Root::parse(&patched).ok().is_err() {
        return Err(format!("new snippet would make {file} unparseable"));
    }
    let formatted = nixfmt(&patched).unwrap_or(patched);
    write_atomic(file, &formatted)
}

fn nixfmt(input: &str) -> Option<String> {
    let mut child = std::process::Command::new("nixfmt")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    use std::io::Write;
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(input.as_bytes());
    }
    // Drop stdin to ensure EOF
    drop(child.stdin.take());
    let mut stdout = child.stdout.take()?;
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        use std::io::Read;
        let _ = stdout.read_to_end(&mut out);
        let _ = out_tx.send(out);
    });
    // Wait for process with timeout
    let (status_tx, status_rx) =
        std::sync::mpsc::channel::<std::io::Result<std::process::ExitStatus>>();
    let child_pid = child.id();
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = status_tx.send(status);
    });
    match status_rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(Ok(status)) => {
            if status.success() {
                match out_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                    Ok(out) => String::from_utf8(out).ok(),
                    Err(_) => None,
                }
            } else {
                None
            }
        }
        Ok(Err(_)) => None,
        Err(_) => {
            // Timeout waiting for process - kill it
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(child_pid.to_string())
                .status();
            None
        }
    }
}

fn write_atomic(file: &str, contents: &str) -> Result<(), String> {
    let path = Path::new(file);
    let tmp = path.with_extension("nix-settings-gui.tmp");
    std::fs::write(&tmp, contents).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {file}: {e}"))?;
    Ok(())
}

/// Whether `file` lives inside the config root and is safe to edit. Files under
/// the Nix store are read-only and never written.
pub fn editable(file: &str, root: &str) -> bool {
    !file.starts_with("/nix/store") && Path::new(file).starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_files_outside_root() {
        assert!(editable("/home/u/repos/x/a.nix", "/home/u/repos/x"));
        assert!(!editable("/nix/store/abc/a.nix", "/home/u/repos/x"));
        assert!(!editable("/etc/nixos/a.nix", "/home/u/repos/x"));
    }

    #[test]
    fn applies_and_inserts_edits() {
        let dir = std::env::temp_dir().join(format!("nsg-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root_dir = dir.display().to_string();
        let file = dir.join("cfg.nix");
        let original = "{ ... }: {\n  a = 1;\n}\n";
        std::fs::write(&file, original).unwrap();
        let path = file.display().to_string();

        let start = original.find('1').unwrap();
        let edits = vec![PendingEdit {
            file: path.clone(),
            start,
            end: start + 1,
            value: "2".to_string(),
        }];
        assert_eq!(apply(&edits, &root_dir).unwrap(), 1);
        let updated = std::fs::read_to_string(&file).unwrap();
        assert!(updated.contains("a = 2;"));

        insert_assignment(&path, "", "/* @option widget = \"switch\"; */\nb = true;").unwrap();
        let updated = std::fs::read_to_string(&file).unwrap();
        assert!(updated.contains("b = true;"));
        assert!(rnix::Root::parse(&updated).ok().is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inserts_above_marker_and_shifts_it_down() {
        let dir = std::env::temp_dir().join(format!("nsg-insert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cfg.nix");
        let original = "{ ... }: {\n  a = 1;\n  /* @insert name = \"x\"; */\n}\n";
        std::fs::write(&file, original).unwrap();
        let path = file.display().to_string();
        let marker = original.find("/* @insert").unwrap();
        let line_start = original[..marker].rfind('\n').map(|i| i + 1).unwrap();

        insert_at(
            &path,
            line_start,
            "# @option widget = \"text\";\nb = \"v\";",
        )
        .unwrap();
        let updated = std::fs::read_to_string(&file).unwrap();
        assert!(rnix::Root::parse(&updated).ok().is_ok());
        let b = updated.find("b = \"v\";").unwrap();
        let marker = updated.find("@insert").unwrap();
        assert!(b < marker, "snippet should sit above the marker");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_unparseable_result() {
        let dir = std::env::temp_dir().join(format!("nsg-edit-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root_dir = dir.display().to_string();
        let file = dir.join("cfg.nix");
        let original = "{ ... }: {\n  a = 1;\n}\n";
        std::fs::write(&file, original).unwrap();
        let start = original.find('1').unwrap();
        let edits = vec![PendingEdit {
            file: file.display().to_string(),
            start,
            end: start + 1,
            value: "{".to_string(),
        }];
        assert!(apply(&edits, &root_dir).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
