use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DetectedMode {
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
    #[serde(default)]
    pub preferred: bool,
    #[serde(default)]
    pub current: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetectedMonitor {
    pub name: String,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    pub modes: Vec<DetectedMode>,
}

impl DetectedMonitor {
    pub fn default_mode(&self) -> Option<&DetectedMode> {
        self.modes
            .iter()
            .find(|m| m.current)
            .or_else(|| self.modes.iter().find(|m| m.preferred))
            .or_else(|| self.modes.first())
    }
}

fn parse_mode_line(line: &str) -> Option<(u32, u32, f64, bool)> {
    let mode = line.trim();
    let size = mode.split(" px").next()?;
    let mut parts = size.split('x');
    let width = parts.next()?.trim().parse().ok()?;
    let height = parts.next()?.trim().parse().ok()?;
    let refresh = mode
        .split(',')
        .nth(1)
        .and_then(|part| part.split_whitespace().next())
        .and_then(|hz| hz.parse::<f64>().ok())
        .unwrap_or(0.0);
    let preferred = mode.contains("preferred");
    Some((width, height, refresh, preferred))
}

fn parse_wlr_randr_json(text: &str) -> Result<Vec<DetectedMonitor>, serde_json::Error> {
    let heads: Vec<serde_json::Value> = serde_json::from_str(text)?;
    Ok(heads
        .into_iter()
        .filter_map(|head| {
            let name = head.get("name")?.as_str()?.to_string();
            let mut monitor: DetectedMonitor = serde_json::from_value(head).ok()?;
            if monitor.name.is_empty() {
                monitor.name = name;
            }
            Some(monitor)
        })
        .collect())
}

fn parse_wlr_randr(text: &str) -> Vec<DetectedMonitor> {
    let mut monitors: Vec<DetectedMonitor> = Vec::new();
    let mut current: Option<DetectedMonitor> = None;
    for line in text.lines() {
        if !line.starts_with(' ') {
            if let Some(previous) = current.take() {
                monitors.push(previous);
            }
            let name = line.split_whitespace().next().unwrap_or("");
            if !name.is_empty() {
                current = Some(DetectedMonitor {
                    name: name.to_string(),
                    make: None,
                    model: None,
                    enabled: line.find("(disabled)").is_none(),
                    modes: Vec::new(),
                });
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed == "Enabled: no" {
            if let Some(monitor) = current.as_mut() {
                monitor.enabled = false;
            }
            continue;
        }
        let Some(monitor) = current.as_mut() else {
            continue;
        };
        if let Some((width, height, refresh, preferred)) = parse_mode_line(line) {
            monitor.modes.push(DetectedMode {
                width,
                height,
                refresh,
                preferred,
                current: trimmed.contains("current"),
            });
        }
    }
    if let Some(monitor) = current {
        monitors.push(monitor);
    }
    monitors
}

pub fn detect() -> Option<Vec<DetectedMonitor>> {
    if let Ok(output) = Command::new("wlr-randr").arg("--json").output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(monitors) = parse_wlr_randr_json(&text) {
                return Some(monitors);
            }
        }
    }
    let Ok(output) = Command::new("wlr-randr").output() else {
        return None;
    };
    if !output.status.success() {
        return None;
    }
    Some(parse_wlr_randr(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wlr-randr/json.json"
    ));
    const TEXT_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wlr-randr/text.txt"
    ));

    #[test]
    fn parses_json_heads_and_modes() {
        let monitors = parse_wlr_randr_json(JSON_FIXTURE).unwrap();
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].name, "eDP-1");
        assert_eq!(monitors[0].modes.len(), 3);
        assert!(monitors[0].enabled);
        assert!(monitors[0].modes[0].preferred);
        assert!(monitors[0].modes[0].current);
        assert_eq!(monitors[0].default_mode().unwrap().refresh, 60.01);
        assert_eq!(monitors[1].name, "HDMI-A-1");
        assert!(monitors[1].modes.is_empty());
        assert!(!monitors[1].enabled);
        assert!(monitors[1].default_mode().is_none());
    }

    #[test]
    fn parses_plain_text_heads_and_modes() {
        let monitors = parse_wlr_randr(TEXT_FIXTURE);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].name, "eDP-1");
        assert_eq!(monitors[0].modes.len(), 3);
        assert!(monitors[0].modes[0].preferred);
        assert!(monitors[0].modes[0].current);
        assert_eq!(monitors[0].default_mode().unwrap().width, 1920);
        assert!(!monitors[1].enabled);
        assert!(monitors[1].modes.is_empty());
    }

    static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn shell_path() -> Option<std::path::PathBuf> {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("sh");
            if candidate.is_file() {
                return std::fs::canonicalize(candidate).ok();
            }
        }
        None
    }

    fn with_path<R>(dir: &std::path::Path, run: impl FnOnce() -> R) -> R {
        let old = std::env::var_os("PATH");
        std::env::set_var("PATH", dir);
        let result = run();
        match old {
            Some(old) => std::env::set_var("PATH", old),
            None => std::env::remove_var("PATH"),
        }
        result
    }

    #[test]
    fn detect_falls_back_when_json_rejected() {
        let _guard = PATH_LOCK.lock().unwrap();
        let Some(shell) = shell_path() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("nsg-wlr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("wlr-randr");
        let script = format!(
            "#!{}\nif [ \"$1\" = \"--json\" ]; then exit 1; fi\nprintf '%s\\n' 'eDP-1 \"Some Panel\"' '    1920x1080 px, 60.010000 Hz (preferred, current)' '    1280x720 px, 59.940000 Hz'\n",
            shell.display()
        );
        std::fs::write(&stub, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let monitors = with_path(&dir, detect).unwrap();
        assert_eq!(monitors.len(), 1);
        assert_eq!(monitors[0].modes.len(), 2);
    }

    #[test]
    fn detect_reports_unavailable_when_binary_missing() {
        let _guard = PATH_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("nsg-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let monitors = with_path(&dir, detect);
        assert!(monitors.is_none());
    }
}
