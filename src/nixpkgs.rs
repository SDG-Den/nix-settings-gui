use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const INPUT_NAME: &str = "nixpkgs";
pub const NIXPKGS_GIT_URL: &str = "https://github.com/NixOS/nixpkgs";

pub fn is_nixpkgs(name: &str) -> bool {
    name == INPUT_NAME
}

pub fn default_channels() -> Vec<String> {
    let mut channels = vec![
        "nixos-unstable".to_string(),
        "nixos-unstable-small".to_string(),
    ];
    for release in ["26.05", "25.11", "25.05", "24.11"] {
        channels.push(format!("nixos-{release}"));
        channels.push(format!("nixos-{release}-small"));
    }
    sort_channels(&mut channels);
    channels
}

/// Channels that are already cached, falling back to the built-in list.
pub fn channels() -> Vec<String> {
    cached_channels().unwrap_or_else(default_channels)
}

pub fn cached_channels() -> Option<Vec<String>> {
    let path = cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let fetched = value.get("fetched")?.as_u64()?;
    let age = now_seconds().checked_sub(fetched)?;
    if age > 24 * 60 * 60 {
        return None;
    }
    let channels: Vec<String> = value
        .get("channels")?
        .as_array()?
        .iter()
        .filter_map(|channel| channel.as_str().map(str::to_string))
        .collect();
    if channels.is_empty() {
        None
    } else {
        Some(channels)
    }
}

pub fn scan_channels() -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args([
            "ls-remote",
            "--heads",
            NIXPKGS_GIT_URL,
            "refs/heads/nixos-*",
        ])
        .output()
        .map_err(|e| format!("cannot run git ls-remote: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("git ls-remote failed");
        return Err(detail.trim().to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut channels: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .filter_map(|reference| reference.strip_prefix("refs/heads/"))
        .filter(|name| name.starts_with("nixos-"))
        .map(str::to_string)
        .collect();
    sort_channels(&mut channels);
    if channels.is_empty() {
        return Err("no NixOS channels were found".to_string());
    }
    save_cache(&channels);
    Ok(channels)
}

pub fn sort_channels(channels: &mut Vec<String>) {
    channels.sort();
    channels.dedup();
    channels.sort_by(|left, right| {
        let left = channel_parts(left);
        let right = channel_parts(right);
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then(left.2.cmp(&right.2))
    });
}

fn channel_parts(name: &str) -> (u32, u32, u8) {
    let Some(rest) = name.strip_prefix("nixos-") else {
        return (0, 0, 0);
    };
    let (version, small) = match rest.strip_suffix("-small") {
        Some(version) => (version, 1),
        None => (rest, 0),
    };
    if version == "unstable" {
        return (9999, 0, small);
    }
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    (major, minor, small)
}

fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("nix-settings-gui").join("nixpkgs-channels.json"))
}

fn save_cache(channels: &[String]) {
    let Some(path) = cache_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let value = serde_json::json!({
        "fetched": now_seconds(),
        "channels": channels,
    });
    let _ = std::fs::write(path, value.to_string());
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_unstable_first_then_release_descending() {
        let mut channels = vec![
            "nixos-25.05".to_string(),
            "nixos-unstable-small".to_string(),
            "nixos-26.05-small".to_string(),
            "nixos-26.05".to_string(),
            "nixos-unstable".to_string(),
            "master".to_string(),
        ];
        sort_channels(&mut channels);
        assert_eq!(
            channels,
            vec![
                "nixos-unstable",
                "nixos-unstable-small",
                "nixos-26.05",
                "nixos-26.05-small",
                "nixos-25.05",
                "master",
            ]
        );
    }

    #[test]
    fn only_nixpkgs_is_special() {
        assert!(is_nixpkgs("nixpkgs"));
        assert!(!is_nixpkgs("nixpkgs-unstable"));
    }
}
