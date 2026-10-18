#[derive(Debug, Clone)]
pub struct DetectedMonitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
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
        .and_then(|part| part.trim().split_whitespace().next())
        .and_then(|hz| hz.parse::<f64>().ok())
        .unwrap_or(0.0);
    let preferred = mode.contains("preferred");
    Some((width, height, refresh, preferred))
}

fn parse_wlr_randr(text: &str) -> Vec<DetectedMonitor> {
    let mut monitors = Vec::new();
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
                    width: 0,
                    height: 0,
                    refresh: 0.0,
                });
            }
            continue;
        }
        let Some(monitor) = current.as_mut() else {
            continue;
        };
        let Some((width, height, refresh, preferred)) = parse_mode_line(line) else {
            continue;
        };
        if preferred || monitor.width == 0 {
            monitor.width = width;
            monitor.height = height;
            monitor.refresh = refresh;
        }
    }
    if let Some(monitor) = current {
        monitors.push(monitor);
    }
    monitors
}

use std::process::Command;

pub fn detect() -> Vec<DetectedMonitor> {
    let Ok(output) = Command::new("wlr-randr").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_wlr_randr(&text)
}
