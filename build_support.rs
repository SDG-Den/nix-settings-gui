#![allow(dead_code)]

use std::fs;
use std::path::Path;

use serde_yaml::Value;

/// Recursively collect every base16 scheme under `dir`.
pub fn collect(dir: &Path, out: &mut Vec<(String, [String; 16])>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if (ext == "yaml" || ext == "yml") && is_base16(&path) {
                if let Some(item) = parse_scheme(&path) {
                    out.push(item);
                }
            }
        }
    }
}

pub fn parse_scheme(path: &Path) -> Option<(String, [String; 16])> {
    let text = fs::read_to_string(path).ok()?;
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    parse_scheme_text(&text, stem)
}

pub fn parse_scheme_text(text: &str, stem: &str) -> Option<(String, [String; 16])> {
    let value: Value = serde_yaml::from_str(text).ok()?;
    let palette = value.get("palette")?;
    let mut colors = std::array::from_fn(|_| String::new());
    for (i, slot) in colors.iter_mut().enumerate() {
        let upper = palette.get(format!("base{i:02X}"));
        let lower = palette.get(format!("base{i:02x}"));
        let raw = upper.or(lower)?.as_str()?;
        *slot = normalize_color(raw)?;
    }
    Some((slug(stem), colors))
}

pub fn is_base16(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|stem| stem.starts_with("base16-"))
        .unwrap_or(false)
}

pub fn slug(stem: &str) -> String {
    stem.strip_prefix("base16-").unwrap_or(stem).to_string()
}

fn normalize_color(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches('#');
    if t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", t.to_lowercase()))
    } else {
        None
    }
}
