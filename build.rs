use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "build_support.rs"]
mod build_support;

fn main() {
    println!("cargo:rerun-if-env-changed=TINTED_SCHEMES_DIR");
    let mut schemes: Vec<(String, [String; 16])> = Vec::new();
    if let Ok(dir) = env::var("TINTED_SCHEMES_DIR") {
        build_support::collect(Path::new(&dir), &mut schemes);
    }
    schemes.sort_by(|a, b| a.0.cmp(&b.0));
    schemes.dedup_by(|a, b| a.0 == b.0);

    let mut out = String::from("pub static SCHEMES: &[(&str, [&str; 16])] = &[\n");
    for (name, palette) in &schemes {
        let colors: Vec<String> = palette.iter().map(|c| format!("\"{c}\"")).collect();
        out.push_str(&format!("    (\"{}\", [{}]),\n", name, colors.join(", ")));
    }
    out.push_str("];\n");

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("schemes.rs");
    fs::write(dest, out).expect("write schemes.rs");
}
