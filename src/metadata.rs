use std::process::Command;

use serde::Deserialize;

/// An embedded copy of the option-registry extractor. It is evaluated with
/// `nix eval` and returns one level of a flake's option set as JSON.
const OPTIONS_TO_JSON: &str = include_str!("nix/options-to-json.nix");

/// A configuration target inside a flake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    FlakeParts,
    Nixos(String),
    Home(String),
    HomeInNixos { host: String, user: String },
    Darwin(String),
}

impl Target {
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        match parts.first().copied()? {
            "fp" => Some(Target::FlakeParts),
            "nixos" => Some(Target::Nixos(parts.get(1)?.to_string())),
            "home" => Some(Target::Home(parts.get(1)?.to_string())),
            "hm" => Some(Target::HomeInNixos {
                host: parts.get(1)?.to_string(),
                user: parts.get(2)?.to_string(),
            }),
            "darwin" => Some(Target::Darwin(parts.get(1)?.to_string())),
            _ => None,
        }
    }

    /// The `target` argument the Nix extractor and nixd both expect.
    pub fn as_arg(&self) -> String {
        match self {
            Target::FlakeParts => "fp".to_string(),
            Target::Nixos(host) => format!("nixos:{host}"),
            Target::Home(user) => format!("home:{user}"),
            Target::HomeInNixos { host, user } => format!("hm:{host}:{user}"),
            Target::Darwin(host) => format!("darwin:{host}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChildOption {
    pub name: String,
    pub path: String,
    pub segments: Vec<String>,
    #[serde(rename = "type")]
    pub type_str: String,
    pub description: String,
    pub read_only: bool,
    pub visible: bool,
    pub internal: bool,
    pub set: bool,
    pub has_children: bool,
    #[serde(default)]
    pub default: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub file: String,
    pub line: i64,
    pub column: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Definition {
    pub file: String,
    pub line: i64,
    pub column: i64,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OptionDetail {
    pub config_name: String,
    pub path: String,
    pub value: Option<String>,
    pub declarations: Vec<Position>,
    pub definitions: Vec<Definition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigNames {
    pub nixos: Vec<String>,
    pub home: Vec<String>,
    pub darwin: Vec<String>,
    pub flake_parts: bool,
}

#[derive(Debug, Deserialize)]
struct ConfigsPayload {
    configs: ConfigNames,
}

#[derive(Debug, Deserialize)]
struct OptionsPayload {
    options: Vec<ChildOption>,
}

/// Write the embedded extractor to a stable temp path and return it.
fn script_path() -> std::path::PathBuf {
    let path = std::env::temp_dir().join("nix-settings-gui-options-to-json.nix");
    let _ = std::fs::write(&path, OPTIONS_TO_JSON);
    path
}

fn run(
    flake_path: &str,
    target: &str,
    path: &[String],
    detail: bool,
    full: bool,
) -> Result<String, String> {
    let script = script_path();
    let path_json = serde_json::to_string(path).map_err(|e| format!("cannot encode path: {e}"))?;
    let output = Command::new("nix")
        .args([
            "eval",
            "--json",
            "--extra-experimental-features",
            "nix-command flakes",
            "--file",
        ])
        .arg(&script)
        .arg("--argstr")
        .arg("flakePath")
        .arg(flake_path)
        .arg("--argstr")
        .arg("target")
        .arg(target)
        .arg("--arg")
        .arg("path")
        .arg(path_json)
        .arg("--arg")
        .arg("detail")
        .arg(if detail { "true" } else { "false" })
        .arg("--arg")
        .arg("full")
        .arg(if full { "true" } else { "false" })
        .output()
        .map_err(|e| format!("failed to run nix: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("nix eval failed: {}", stderr.trim()));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("nix output was not utf8: {e}"))
}

/// List the configurations a flake exposes.
pub fn configs(flake_path: &str) -> Result<ConfigNames, String> {
    let json = run(flake_path, "", &[], false, false)?;
    let payload: ConfigsPayload =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    Ok(payload.configs)
}

/// Enumerate the options directly under `path` for one target.
pub fn enumerate(
    flake_path: &str,
    target: &Target,
    path: &[String],
) -> Result<Vec<ChildOption>, String> {
    let json = run(flake_path, &target.as_arg(), path, false, false)?;
    let payload: OptionsPayload =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    Ok(payload.options)
}

/// Fetch the value, declarations, and definitions of a single option.
pub fn detail(flake_path: &str, target: &Target, path: &[String]) -> Result<OptionDetail, String> {
    let json = run(flake_path, &target.as_arg(), path, true, false)?;
    serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))
}

#[derive(Debug, Deserialize)]
struct PathsPayload {
    paths: Vec<String>,
}

/// Every option path in one target's registry, via a recursive (full) eval.
/// Used as the add-option autocomplete source when a complete list is wanted.
pub fn all_options(flake_path: &str, target: &Target) -> Result<Vec<String>, String> {
    let json = run(flake_path, &target.as_arg(), &[], false, true)?;
    let payload: PathsPayload =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    Ok(payload.paths)
}

/// The pretty-printed default expression of one option, read from its parent.
pub fn default_expression(
    flake_path: &str,
    target: &Target,
    path: &[String],
) -> Result<String, String> {
    let (name, parent) = path.split_last().ok_or("empty option path")?;
    let options = enumerate(flake_path, target, parent)?;
    find_default(&options, name)
}

fn find_default(options: &[ChildOption], name: &str) -> Result<String, String> {
    options
        .iter()
        .find(|option| option.name == name)
        .map(|option| option.default.clone())
        .ok_or_else(|| format!("no option named {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(Target::parse("fp"), Some(Target::FlakeParts));
        assert_eq!(
            Target::parse("nixos:x570"),
            Some(Target::Nixos("x570".into()))
        );
        assert_eq!(
            Target::parse("hm:x570:den"),
            Some(Target::HomeInNixos {
                host: "x570".into(),
                user: "den".into()
            })
        );
        assert_eq!(Target::parse("home:den").unwrap().as_arg(), "home:den");
        assert!(Target::parse("bogus").is_none());
    }

    #[test]
    fn parses_child_options() {
        let json = r#"{"config_name":"nixos:x","path":"","options":[
            {"name":"services","path":"services","segments":["services"],
             "type":"namespace","description":"","read_only":false,"visible":true,
             "internal":false,"set":false,"has_children":true}
        ]}"#;
        let payload: OptionsPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.options[0].name, "services");
        assert!(payload.options[0].has_children);
        assert_eq!(payload.options[0].default, "");
    }

    #[test]
    fn find_default_reads_pretty_printed_value() {
        let json = r#"{"config_name":"nixos:x","path":"services","options":[
            {"name":"openssh","path":"services.openssh","segments":["services","openssh"],
             "type":"submodule","description":"","read_only":false,"visible":true,
             "internal":false,"set":false,"has_children":true,
             "default":"{\n  enable = false;\n}"}
        ]}"#;
        let payload: OptionsPayload = serde_json::from_str(json).unwrap();
        let default = find_default(&payload.options, "openssh").unwrap();
        assert!(default.contains("enable = false"));
        assert!(find_default(&payload.options, "missing").is_err());
    }

    #[test]
    fn parses_full_option_paths() {
        let json = r#"{"config_name":"nixos:x","paths":["services.pipewire.enable"]}"#;
        let payload: PathsPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.paths, vec!["services.pipewire.enable"]);
    }

    #[test]
    fn parses_option_detail() {
        let json = r#"{"config_name":"nixos:x","path":"services.openssh.enable",
            "value":"true",
            "declarations":[{"file":"/etc/nixos/configuration.nix","line":5,"column":3}],
            "definitions":[{"file":"/etc/nixos/configuration.nix","line":5,"column":3,"value":"true"}]}"#;
        let detail: OptionDetail = serde_json::from_str(json).unwrap();
        assert_eq!(detail.value.as_deref(), Some("true"));
        assert_eq!(detail.declarations[0].line, 5);
    }
}
