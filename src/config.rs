use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// User settings persisted to `~/.config/nix-settings-gui/config.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub flake_path: Option<String>,
    #[serde(default)]
    pub terminal_command: Option<String>,
    #[serde(default)]
    pub rebuild_command: Option<String>,
    #[serde(default)]
    pub recent_flake_paths: Vec<String>,
}

impl AppConfig {
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"));
        base.join("nix-settings-gui").join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create config dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("cannot write config: {e}"))
    }

    /// Record `flake` as the current path and move it to the front of recents.
    pub fn remember(&mut self, flake: &str) {
        self.flake_path = Some(flake.to_string());
        self.recent_flake_paths.retain(|p| p != flake);
        self.recent_flake_paths.insert(0, flake.to_string());
        self.recent_flake_paths.truncate(10);
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_keeps_recent_unique() {
        let mut config = AppConfig::default();
        config.remember("/tmp/a");
        config.remember("/tmp/b");
        config.remember("/tmp/a");
        assert_eq!(config.flake_path.as_deref(), Some("/tmp/a"));
        assert_eq!(config.recent_flake_paths, vec!["/tmp/a", "/tmp/b"]);
    }

    #[test]
    fn round_trips_through_json() {
        let mut config = AppConfig::default();
        config.remember("/tmp/flake");
        config.terminal_command = Some("ghostty -e zsh -c '{cmd}'".into());
        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.flake_path, config.flake_path);
        assert_eq!(back.terminal_command, config.terminal_command);
    }
}
