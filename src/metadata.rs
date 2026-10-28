use std::process::Command;

use serde::Deserialize;

/// An embedded copy of the option-registry extractor. It is evaluated with
/// `nix eval` and returns one level of a flake's option set as JSON.
const OPTIONS_TO_JSON: &str = include_str!("nix/options-to-json.nix");

/// An embedded copy of the DMS plugin registry extractor. It is evaluated
/// with `nix eval` and returns the registry's plugin ids as JSON.
const DMS_PLUGINS_TO_JSON: &str = include_str!("nix/dms-plugins-to-json.nix");

/// An embedded copy of the one-plugin settings extractor. It reads only the
/// requested plugin's `plugin.json` and settings QML.
const DMS_PLUGIN_SETTINGS_TO_JSON: &str = include_str!("nix/dms-plugin-settings-to-json.nix");

/// An embedded copy of the wallpaper extractor. It returns the generated
/// wallpaper directory for the selected target, or the raw presets from the
/// config flake input when the target has no generated set.
const WALLPAPERS_TO_JSON: &str = include_str!("nix/wallpapers-to-json.nix");

/// An embedded copy of the Nixpkgs Nerd Font catalog extractor.
const STYLE_FONTS_TO_JSON: &str = include_str!("nix/style-fonts-to-json.nix");

/// Applies the registry extractor to its named arguments. The values arrive
/// through the environment so no user path is interpolated into Nix source.
const DMS_PLUGINS_APPLY: &str = r#"f: f {
  flakePath = builtins.getEnv "NSG_FLAKE_PATH";
  system = builtins.getEnv "NSG_SYSTEM";
}"#;

/// Applies the wallpaper extractor to its named arguments.
const WALLPAPERS_APPLY: &str = r#"f: f {
  flakePath = builtins.getEnv "NSG_FLAKE_PATH";
  target = builtins.getEnv "NSG_TARGET";
}"#;

/// Applies the font catalog extractor to its named arguments.
const STYLE_FONTS_APPLY: &str = r#"f: f {
  flakePath = builtins.getEnv "NSG_FLAKE_PATH";
}"#;

/// Applies the one-plugin settings extractor to its named arguments.
const DMS_PLUGIN_SETTINGS_APPLY: &str = r#"f: f {
  flakePath = builtins.getEnv "NSG_FLAKE_PATH";
  system = builtins.getEnv "NSG_SYSTEM";
  pluginId = builtins.getEnv "NSG_PLUGIN_ID";
}"#;

/// Applies the option extractor to its named arguments.
const OPTIONS_APPLY: &str = r#"f: f {
  flakePath = builtins.getEnv "NSG_FLAKE_PATH";
  target = builtins.getEnv "NSG_TARGET";
  path = builtins.fromJSON (builtins.getEnv "NSG_PATH_JSON");
  detail = builtins.fromJSON (builtins.getEnv "NSG_DETAIL");
  full = builtins.fromJSON (builtins.getEnv "NSG_FULL");
}"#;

/// The Nix system string for the machine the app runs on.
fn host_system() -> String {
    let arch = match std::env::consts::ARCH {
        "x86" => "i686",
        "arm" => "armv7l",
        other => other,
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{arch}-{os}")
}

/// Evaluate an embedded extractor with explicit arguments.
///
/// `--impure` is required because the extractor reads a local flake through
/// `builtins.getFlake`. `--apply` calls the file's top-level function; recent
/// Nix versions do not auto-call it from `--argstr`.
fn eval_command(script: &std::path::Path, apply: &str, envs: &[(&str, &str)]) -> Command {
    let mut command = Command::new("nix");
    command.args([
        "eval",
        "--json",
        "--impure",
        "--extra-experimental-features",
        "nix-command flakes",
        "--file",
    ]);
    command.arg(script).arg("--apply").arg(apply);
    for (name, value) in envs {
        command.env(name, value);
    }
    command
}

fn eval_script(
    script: &std::path::Path,
    apply: &str,
    envs: &[(&str, &str)],
) -> Result<String, String> {
    let output = eval_command(script, apply, envs)
        .output()
        .map_err(|e| format!("failed to run nix: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("nix eval failed: {}", stderr.trim()));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("nix output was not utf8: {e}"))
}

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

/// One plugin of the upstream DMS plugin registry: the id as written in
/// configuration, plus the registry's display metadata when available.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RegistryPlugin {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// Path to the plugin's settings QML, relative to the plugin directory.
    #[serde(default)]
    pub settings: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PluginsPayload {
    plugins: Vec<RegistryPlugin>,
}

/// The ids (and metadata) of the config flake's DMS plugin registry, via
/// `nix eval` of the embedded extractor. Empty/error when the flake has no
/// `ojuny-desktop` input.
pub fn dms_plugins(flake_path: &str) -> Result<Vec<RegistryPlugin>, String> {
    let path = std::env::temp_dir().join("nix-settings-gui-dms-plugins-to-json.nix");
    let _ = std::fs::write(&path, DMS_PLUGINS_TO_JSON);
    let system = host_system();
    let json = eval_script(
        &path,
        DMS_PLUGINS_APPLY,
        &[("NSG_FLAKE_PATH", flake_path), ("NSG_SYSTEM", &system)],
    )?;
    let payload: PluginsPayload =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    Ok(payload.plugins)
}

/// One wallpaper entry, either a bundled preset or a user image.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WallpaperPreset {
    pub name: String,
    pub label: String,
    pub path: String,
}

/// The generated wallpaper set for one configuration target. The raw fallback
/// returned for unsupported targets leaves the generated fields empty.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WallpaperSet {
    #[serde(default)]
    pub directory: String,
    #[serde(default)]
    pub selected: String,
    #[serde(default, rename = "default")]
    pub default_path: String,
    #[serde(default, rename = "storePath")]
    pub store_path: String,
    #[serde(default, rename = "drvPath")]
    pub drv_path: String,
    #[serde(default)]
    pub presets: Vec<WallpaperPreset>,
    #[serde(default)]
    pub user: Vec<WallpaperPreset>,
}

/// Ensure the generated wallpaper directory exists in the store. A failure is
/// non-fatal: the picker falls back to image placeholders.
fn realize_wallpapers(out_path: &str, drv_path: &str) {
    if out_path.is_empty() || std::path::Path::new(out_path).exists() {
        return;
    }
    if drv_path.is_empty() {
        return;
    }
    let _ = Command::new("nix-store")
        .arg("--realise")
        .arg(drv_path)
        .output();
}

/// The generated wallpaper set for a target, or the raw presets declared by
/// the config flake input when the target has no generated set.
pub fn wallpaper_set(flake_path: &str, target: &str) -> Result<WallpaperSet, String> {
    let path = std::env::temp_dir().join("nix-settings-gui-wallpapers-to-json.nix");
    let _ = std::fs::write(&path, WALLPAPERS_TO_JSON);
    let json = eval_script(
        &path,
        WALLPAPERS_APPLY,
        &[("NSG_FLAKE_PATH", flake_path), ("NSG_TARGET", target)],
    )?;
    let payload: WallpaperSet =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    realize_wallpapers(&payload.store_path, &payload.drv_path);
    Ok(payload)
}

#[derive(Debug, Clone, Deserialize)]
pub struct StyleFont {
    pub family: String,
    pub label: String,
    pub package: String,
    #[serde(default)]
    pub monospace: bool,
}

#[derive(Debug, Deserialize)]
struct StyleFontsPayload {
    #[serde(default)]
    fonts: Vec<StyleFont>,
}

pub fn style_fonts(flake_path: &str) -> Result<Vec<StyleFont>, String> {
    let path = std::env::temp_dir().join("nix-settings-gui-style-fonts-to-json.nix");
    let _ = std::fs::write(&path, STYLE_FONTS_TO_JSON);
    let json = eval_script(&path, STYLE_FONTS_APPLY, &[("NSG_FLAKE_PATH", flake_path)])?;
    let payload: StyleFontsPayload =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    Ok(payload.fonts)
}

#[derive(Debug, Deserialize)]
struct PluginSettingsPayload {
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    settings: Option<String>,
}

/// The declarative settings QML of one registry plugin, or `None` when the
/// plugin ships no settings file. Evaluates only that plugin's package.
pub fn dms_plugin_settings(flake_path: &str, plugin_id: &str) -> Result<Option<String>, String> {
    let path = std::env::temp_dir().join("nix-settings-gui-dms-plugin-settings-to-json.nix");
    let _ = std::fs::write(&path, DMS_PLUGIN_SETTINGS_TO_JSON);
    let system = host_system();
    let json = eval_script(
        &path,
        DMS_PLUGIN_SETTINGS_APPLY,
        &[
            ("NSG_FLAKE_PATH", flake_path),
            ("NSG_SYSTEM", &system),
            ("NSG_PLUGIN_ID", plugin_id),
        ],
    )?;
    let payload: PluginSettingsPayload =
        serde_json::from_str(&json).map_err(|e| format!("cannot parse nix output: {e}"))?;
    Ok(payload.settings)
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
    let detail_json = if detail { "true" } else { "false" };
    let full_json = if full { "true" } else { "false" };
    eval_script(
        &script,
        OPTIONS_APPLY,
        &[
            ("NSG_FLAKE_PATH", flake_path),
            ("NSG_TARGET", target),
            ("NSG_PATH_JSON", &path_json),
            ("NSG_DETAIL", detail_json),
            ("NSG_FULL", full_json),
        ],
    )
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
    fn registry_command_is_impure_and_applies_the_extractor() {
        let command = eval_command(
            std::path::Path::new("/tmp/dms.nix"),
            DMS_PLUGINS_APPLY,
            &[("NSG_FLAKE_PATH", "/repo"), ("NSG_SYSTEM", "x86_64-linux")],
        );
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|arg| arg == "--impure"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--apply" && pair[1] == DMS_PLUGINS_APPLY));
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("NSG_FLAKE_PATH".to_string(), Some("/repo".to_string()))));
        assert!(envs.contains(&("NSG_SYSTEM".to_string(), Some("x86_64-linux".to_string()))));
    }

    #[test]
    fn host_system_is_a_nix_triple() {
        let system = host_system();
        assert!(system.contains('-'), "{system}");
    }

    #[test]
    fn plugin_settings_command_passes_the_plugin_id() {
        let command = eval_command(
            std::path::Path::new("/tmp/dms-settings.nix"),
            DMS_PLUGIN_SETTINGS_APPLY,
            &[
                ("NSG_FLAKE_PATH", "/repo"),
                ("NSG_SYSTEM", "x86_64-linux"),
                ("NSG_PLUGIN_ID", "barDropdown"),
            ],
        );
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("NSG_PLUGIN_ID".to_string(), Some("barDropdown".to_string()))));
    }

    #[test]
    #[ignore = "needs a sibling ojuny-template checkout and Nix evaluation"]
    fn loads_registry_from_sibling_template() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ojuny-template");
        if !path.join("flake.nix").exists() {
            return;
        }
        let plugins =
            dms_plugins(&path.to_string_lossy()).expect("the template registry should load");
        assert!(
            plugins.len() > 100,
            "registry had {} plugins",
            plugins.len()
        );
    }

    #[test]
    #[ignore = "needs a sibling ojuny-template checkout and Nix evaluation"]
    fn loads_plugin_settings_from_sibling_template() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ojuny-template");
        if !path.join("flake.nix").exists() {
            return;
        }
        let qml = dms_plugin_settings(&path.to_string_lossy(), "barDropdown")
            .expect("the registry plugin should load")
            .expect("barDropdown ships settings");
        assert!(qml.contains("settingKey"));
    }

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
