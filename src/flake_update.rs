use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::flake_inputs::FlakeInput;
use crate::flake_lock;
use crate::flake_ref::{self, FileTransport, FlakeRef, GitTransport, MercurialTransport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputState {
    UpToDate,
    UpdateAvailable,
    NotLocked,
    Skipped(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputStatus {
    pub current_rev: Option<String>,
    pub current_date: Option<i64>,
    pub remote_rev: Option<String>,
    pub remote_date: Option<i64>,
    pub state: InputState,
}

impl InputStatus {
    fn skipped(reason: impl Into<String>) -> Self {
        Self {
            current_rev: None,
            current_date: None,
            remote_rev: None,
            remote_date: None,
            state: InputState::Skipped(reason.into()),
        }
    }
}

/// Compare the locked revision of one direct input with the revision its URL
/// currently resolves to.
pub fn check(base: &str, input: &FlakeInput) -> Result<InputStatus, String> {
    let Some(url) = input
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Ok(InputStatus::skipped("no URL declared"));
    };
    if let Ok(reference) = flake_ref::parse(url) {
        if is_local(&reference) {
            return Ok(InputStatus::skipped("local input"));
        }
    }

    let current = flake_lock::read_input(&Path::new(base).join("flake.lock"), &input.name);
    let (remote_rev, remote_date) = remote_locked(base, url)?;
    let Some(remote_rev) = remote_rev else {
        return Ok(InputStatus::skipped("latest revision unavailable"));
    };
    let current_rev = current.as_ref().and_then(|locked| locked.rev.clone());
    let current_date = current.as_ref().and_then(|locked| locked.last_modified);
    let state = match &current_rev {
        Some(current_rev) if current_rev == &remote_rev => InputState::UpToDate,
        Some(_) => InputState::UpdateAvailable,
        None => InputState::NotLocked,
    };

    Ok(InputStatus {
        current_rev,
        current_date,
        remote_rev: Some(remote_rev),
        remote_date,
        state,
    })
}

/// Update exactly one named input.
pub fn update(base: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("flake input name is empty".to_string());
    }
    run_update(base, &[name])
}

/// Update a list of named inputs. Nixpkgs is deliberately rejected here so the
/// bulk action can never move it.
pub fn update_all(base: &str, names: &[String]) -> Result<usize, String> {
    let names: Vec<&str> = names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return Err("no flake inputs selected".to_string());
    }
    if names.iter().any(|name| crate::nixpkgs::is_nixpkgs(name)) {
        return Err("update all must not include nixpkgs".to_string());
    }
    run_update(base, &names)?;
    Ok(names.len())
}

fn run_update(base: &str, names: &[&str]) -> Result<(), String> {
    if base.trim().is_empty() {
        return Err("no flake directory is open".to_string());
    }
    if names.is_empty() {
        return Err("no flake inputs selected".to_string());
    }
    let output = Command::new("nix")
        .args(["flake", "update"])
        .args(names)
        .current_dir(base)
        .output()
        .map_err(|e| format!("cannot run nix flake update: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = stderr
        .lines()
        .find(|line| !line.trim().is_empty())
        .or_else(|| stdout.lines().find(|line| !line.trim().is_empty()))
        .unwrap_or("nix flake update failed");
    Err(detail.trim().to_string())
}

fn remote_locked(base: &str, url: &str) -> Result<(Option<String>, Option<i64>), String> {
    let mut command = Command::new("nix");
    command.args(["flake", "metadata", "--json", "--no-write-lock-file", url]);
    if !base.trim().is_empty() {
        command.current_dir(base);
    }
    let output = command
        .output()
        .map_err(|e| format!("cannot run nix flake metadata: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("nix flake metadata failed");
        return Err(detail.trim().to_string());
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("cannot parse nix flake metadata output: {e}"))?;
    Ok(metadata_locked(&value))
}

fn metadata_locked(value: &Value) -> (Option<String>, Option<i64>) {
    let locked = value.get("locked");
    let rev = locked
        .and_then(|locked| locked.get("rev"))
        .and_then(Value::as_str)
        .or_else(|| value.get("revision").and_then(Value::as_str))
        .map(str::to_string);
    let date = locked
        .and_then(|locked| locked.get("lastModified"))
        .and_then(Value::as_i64)
        .or_else(|| value.get("lastModified").and_then(Value::as_i64));
    (rev, date)
}

fn is_local(reference: &FlakeRef) -> bool {
    matches!(
        reference,
        FlakeRef::Path { .. }
            | FlakeRef::Git {
                transport: GitTransport::File,
                ..
            }
            | FlakeRef::Mercurial {
                transport: MercurialTransport::File,
                ..
            }
            | FlakeRef::Tarball {
                transport: FileTransport::File,
                ..
            }
            | FlakeRef::File {
                transport: FileTransport::File,
                ..
            }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_locked_metadata_with_top_level_fallback() {
        let value: Value = serde_json::from_str(
            r#"{
              "locked": {"rev": "abc", "lastModified": 100},
              "revision": "ignored",
              "lastModified": 200
            }"#,
        )
        .unwrap();
        assert_eq!(
            metadata_locked(&value),
            (Some("abc".to_string()), Some(100))
        );

        let fallback: Value =
            serde_json::from_str(r#"{"revision": "def", "lastModified": 300}"#).unwrap();
        assert_eq!(
            metadata_locked(&fallback),
            (Some("def".to_string()), Some(300))
        );
    }

    #[test]
    fn detects_local_references() {
        assert!(is_local(&flake_ref::parse("./flake").unwrap()));
        assert!(is_local(&flake_ref::parse("path:/tmp/flake").unwrap()));
        assert!(!is_local(
            &flake_ref::parse("github:NixOS/nixpkgs").unwrap()
        ));
        assert!(!is_local(
            &flake_ref::parse("git+https://example.org/repo").unwrap()
        ));
    }

    #[test]
    fn skips_inputs_without_a_url() {
        let status = check("/tmp", &FlakeInput::new("foo")).unwrap();
        assert_eq!(status.state, InputState::Skipped("no URL declared".into()));
    }

    #[test]
    fn rejects_an_empty_update_name() {
        assert!(update("/tmp", "  ").is_err());
    }

    #[test]
    fn update_all_rejects_empty_and_nixpkgs() {
        assert!(update_all("/tmp", &[]).is_err());
        assert!(update_all("/tmp", &["nixpkgs".to_string()]).is_err());
    }
}
