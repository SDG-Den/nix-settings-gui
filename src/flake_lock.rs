use std::path::Path;

use serde_json::Value;

/// A locked flake input, as stored under `nodes.<node>.locked` in `flake.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LockedInput {
    pub input_type: String,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub ref_: Option<String>,
    pub rev: Option<String>,
    pub last_modified: Option<i64>,
    pub nar_hash: Option<String>,
}

/// The parts of `flake.lock` needed to resolve one direct input.
#[derive(Debug, Clone)]
pub struct LockFile {
    nodes: Value,
    root: String,
}

pub fn read(path: &Path) -> Result<LockFile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    let nodes = value
        .get("nodes")
        .cloned()
        .ok_or_else(|| format!("{} has no lock nodes", path.display()))?;
    let root = value
        .get("root")
        .and_then(Value::as_str)
        .unwrap_or("root")
        .to_string();
    Ok(LockFile { nodes, root })
}

pub fn read_input(path: &Path, name: &str) -> Option<LockedInput> {
    read(path).ok().and_then(|lock| lock.input(name))
}

impl LockFile {
    pub fn input(&self, name: &str) -> Option<LockedInput> {
        let root = self.nodes.get(&self.root)?;
        let node_name = root.get("inputs")?.get(name)?.as_str()?;
        let node = self.nodes.get(node_name)?;
        let locked = node.get("locked")?;
        Some(parse_locked(locked))
    }
}

fn parse_locked(value: &Value) -> LockedInput {
    LockedInput {
        input_type: string(value, "type").unwrap_or_default(),
        owner: string(value, "owner"),
        repo: string(value, "repo"),
        ref_: string(value, "ref"),
        rev: string(value, "rev"),
        last_modified: value.get("lastModified").and_then(Value::as_i64),
        nar_hash: string(value, "narHash"),
    }
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"{
      "nodes": {
        "root": {"inputs": {"nixpkgs": "nixpkgs_1", "foo": "foo_1"}},
        "nixpkgs_1": {
          "locked": {
            "type": "github",
            "owner": "NixOS",
            "repo": "nixpkgs",
            "ref": "nixos-unstable",
            "rev": "7fd4004d0511991fff3a0524856a9be09e28c5f0",
            "lastModified": 1791734076,
            "narHash": "sha256-example"
          }
        },
        "foo_1": {"locked": {"type": "path", "path": "/tmp/foo"}}
      },
      "root": "root",
      "version": 7
    }"#;

    fn lock() -> LockFile {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nsg-lock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flake.lock");
        std::fs::write(&path, LOCK).unwrap();
        read(&path).unwrap()
    }

    #[test]
    fn resolves_a_direct_input() {
        let input = lock().input("nixpkgs").unwrap();
        assert_eq!(input.input_type, "github");
        assert_eq!(input.owner.as_deref(), Some("NixOS"));
        assert_eq!(input.repo.as_deref(), Some("nixpkgs"));
        assert_eq!(input.ref_.as_deref(), Some("nixos-unstable"));
        assert_eq!(
            input.rev.as_deref(),
            Some("7fd4004d0511991fff3a0524856a9be09e28c5f0")
        );
        assert_eq!(input.last_modified, Some(1791734076));
    }

    #[test]
    fn returns_none_for_unknown_input() {
        assert!(lock().input("missing").is_none());
    }
}
