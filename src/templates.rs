use std::path::{Path, PathBuf};

use crate::dsl::{self, MarkerKind};

/// A module template discovered under `<flake-root>/templates/_*.nix`.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub label: String,
    pub kind: String,
    pub filename: String,
    pub output: String,
    pub placeholders: Vec<Placeholder>,
    pub body: String,
    pub path: PathBuf,
}

/// One `%%name%%` token declared by a template.
#[derive(Debug, Clone, PartialEq)]
pub struct Placeholder {
    pub name: String,
    pub label: String,
    pub default: String,
    pub hint: String,
}

/// Load every `_*.nix` template under `<root>/templates`, sorted by path.
/// Files without a leading `@template` marker are skipped.
pub fn load_templates(root: &str) -> Vec<Template> {
    let dir = Path::new(root).join("templates");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().map(|e| e == "nix").unwrap_or(false))
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with('_'))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| parse_template(path))
        .collect()
}

/// Parse one template file. Returns `None` when the file has no leading
/// `@template` block comment.
fn parse_template(path: &Path) -> Option<Template> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim_start();
    let inner = trimmed.strip_prefix("/*")?;
    let close = inner.find("*/")?;
    let (kind, directives) = dsl::parse_marker(&inner[..close])?;
    if kind != MarkerKind::Template {
        return None;
    }
    let leading = text.len() - trimmed.len();
    let body = text[leading + 2 + close + 2..]
        .trim_start_matches(['\n', '\r'])
        .to_string();

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = stem.trim_start_matches('_');
    let kind = directives.string("kind").unwrap_or_else(|| "nixos".into());
    let label = directives
        .string("label")
        .unwrap_or_else(|| stem.to_string());
    let filename = directives
        .string("filename")
        .unwrap_or_else(|| format!("{stem}.nix"));
    let output = directives.string("output").unwrap_or_else(|| {
        if kind == "raw" {
            "modules".to_string()
        } else {
            format!("modules/{kind}")
        }
    });

    Some(Template {
        label,
        kind,
        filename,
        output,
        placeholders: collect_placeholders(&directives),
        body,
        path: path.to_path_buf(),
    })
}

/// Read `placeholders.<name>.<field>` entries, preserving first-seen order.
fn collect_placeholders(directives: &dsl::Directives) -> Vec<Placeholder> {
    let sub = directives.sub("placeholders");
    let mut names: Vec<String> = Vec::new();
    for (key, _) in sub.iter() {
        let Some((name, _field)) = key.split_once('.') else {
            continue;
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
        .into_iter()
        .map(|name| Placeholder {
            label: sub
                .string(&format!("{name}.label"))
                .unwrap_or_else(|| name.clone()),
            default: sub.string(&format!("{name}.default")).unwrap_or_default(),
            hint: sub.string(&format!("{name}.hint")).unwrap_or_default(),
            name,
        })
        .collect()
}

/// Substitute `%%name%%` tokens with `values`. Unknown tokens are left as-is.
pub fn render(body: &str, values: &[(String, String)]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("%%") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("%%") else {
            out.push_str("%%");
            rest = after;
            break;
        };
        let name = &after[..end];
        match values.iter().find(|(key, _)| key == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push_str("%%");
                out.push_str(name);
                out.push_str("%%");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> String {
        format!(
            "{}/tests/fixtures/dendritic-flake",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    #[test]
    fn loads_templates_from_fixture() {
        let templates = load_templates(&fixture_root());
        let template = templates
            .iter()
            .find(|t| t.label == "Example module")
            .expect("example template");
        assert_eq!(template.kind, "nixos");
        assert_eq!(template.filename, "example.nix");
        assert_eq!(template.output, "modules");
        let names: Vec<&str> = template
            .placeholders
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["name", "title"]);
        assert!(!template.body.contains("@template"));
        assert!(template.body.contains("/* @menu"));
    }

    #[test]
    fn loads_user_template_with_placeholders() {
        let templates = load_templates(&fixture_root());
        let template = templates
            .iter()
            .find(|t| t.label == "User account")
            .expect("user template");
        assert_eq!(template.kind, "nixos");
        assert_eq!(template.output, "modules/users");
        let names: Vec<&str> = template
            .placeholders
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["name", "description", "groups", "icon"]);
    }

    #[test]
    fn renders_known_placeholders_and_keeps_unknown() {
        let body = "value = %%name%%; keep %%other%%;";
        let values = vec![("name".to_string(), "audio".to_string())];
        assert_eq!(render(body, &values), "value = audio; keep %%other%%;");
    }
}
