use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "build_support.rs"]
mod build_support;

fn main() {
    println!("cargo:rerun-if-env-changed=TINTED_SCHEMES_DIR");
    let schemes_dir = env::var("TINTED_SCHEMES_DIR").ok();
    let mut schemes: Vec<(String, [String; 16])> = Vec::new();
    if let Some(dir) = &schemes_dir {
        build_support::collect(Path::new(dir), &mut schemes);
    }
    schemes.sort_by(|a, b| a.0.cmp(&b.0));
    schemes.dedup_by(|a, b| a.0 == b.0);
    match (&schemes_dir, schemes.is_empty()) {
        (None, _) => {
            println!("cargo:warning=TINTED_SCHEMES_DIR unset; theme page will be a text field")
        }
        (Some(d), true) => println!("cargo:warning={d} yielded no base16 schemes; check layout"),
        _ => {}
    }

    let mut out = String::from("pub static SCHEMES: &[(&str, [&str; 16])] = &[\n");
    for (name, palette) in &schemes {
        let colors: Vec<String> = palette.iter().map(|c| format!("\"{c}\"")).collect();
        out.push_str(&format!("    (\"{}\", [{}]),\n", name, colors.join(", ")));
    }
    out.push_str("];\n");

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("schemes.rs");
    fs::write(dest, out).expect("write schemes.rs");

    style_catalog_code();
}

#[derive(Debug, Default, Deserialize)]
struct StyleManifest {
    #[serde(default)]
    fonts: Vec<FontEntry>,
    #[serde(default)]
    icons: Vec<ThemeEntry>,
    #[serde(default)]
    cursors: Vec<ThemeEntry>,
}

#[derive(Debug, Deserialize)]
struct FontEntry {
    label: String,
    value: String,
    package: String,
    preview: String,
    #[serde(default)]
    monospace: bool,
}

#[derive(Debug, Deserialize)]
struct ThemeEntry {
    label: String,
    value: String,
    package: String,
    previews: Vec<String>,
    #[serde(default)]
    variant: String,
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

fn embedded(path: &str) -> String {
    format!(
        "include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/style-catalog/{path}\"))"
    )
}

fn style_catalog_code() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/style-catalog");
    println!("cargo:rerun-if-changed={}", dir.display());
    let manifest_path = dir.join("manifest.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest: StyleManifest = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("pub static FONTS: &[super::StyleEntry] = &[\n");
    for entry in &manifest.fonts {
        out.push_str(&format!(
            "    super::StyleEntry {{ label: {}, value: {}, package: {}, preview: Some({}), previews: &[], monospace: {}, variant: \"\" }},\n",
            rust_string(&entry.label),
            rust_string(&entry.value),
            rust_string(&entry.package),
            embedded(&entry.preview),
            entry.monospace,
        ));
    }
    out.push_str("];\n");

    out.push_str("pub static ICON_THEMES: &[super::StyleEntry] = &[\n");
    for entry in &manifest.icons {
        let previews: Vec<String> = entry.previews.iter().map(|path| embedded(path)).collect();
        out.push_str(&format!(
            "    super::StyleEntry {{ label: {}, value: {}, package: {}, preview: None, previews: &[{}], monospace: false, variant: {} }},\n",
            rust_string(&entry.label),
            rust_string(&entry.value),
            rust_string(&entry.package),
            previews.join(", "),
            rust_string(&entry.variant),
        ));
    }
    out.push_str("];\n");

    out.push_str("pub static CURSOR_THEMES: &[super::StyleEntry] = &[\n");
    for entry in &manifest.cursors {
        let previews: Vec<String> = entry.previews.iter().map(|path| embedded(path)).collect();
        out.push_str(&format!(
            "    super::StyleEntry {{ label: {}, value: {}, package: {}, preview: None, previews: &[{}], monospace: false, variant: \"\" }},\n",
            rust_string(&entry.label),
            rust_string(&entry.value),
            rust_string(&entry.package),
            previews.join(", "),
        ));
    }
    out.push_str("];\n");

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("style_catalog.rs");
    fs::write(dest, out).expect("write style_catalog.rs");
}
