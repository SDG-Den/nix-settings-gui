use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::apply::ApplyRequest;
use crate::model::Record;

/// A late-bound change callback, set when the app wires a table editor's rows
/// so dynamically added rows can notify the same handler.
pub(crate) type ChangeHook = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
/// Which flake module scope a module list belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Nixos,
    Home,
}

impl ModuleKind {
    pub(crate) fn scope(self) -> &'static str {
        match self {
            ModuleKind::Nixos => "nixos",
            ModuleKind::Home => "homeManager",
        }
    }

    /// The qualifiers a value may use, longest first, e.g.
    /// `self.modules.nixos.` and `flake.modules.nixos.`.
    pub(crate) fn prefixes(self) -> [String; 2] {
        [
            format!("self.modules.{}.", self.scope()),
            format!("flake.modules.{}.", self.scope()),
        ]
    }
}

/// Runs a package-name search and calls back with results on the main thread.
/// Supplied by the app so the widgets stay free of nixd plumbing.
pub type PackageSearch = Rc<dyn Fn(String, Box<dyn FnOnce(Vec<String>)>)>;

/// Fetches the Dux Shell plugin registry and hands the result back on the main
/// thread. Supplied by the app so the widget stays free of nix plumbing.
pub type PluginRegistryHook =
    Rc<dyn Fn(Box<dyn FnOnce(Result<Vec<crate::metadata::RegistryPlugin>, String>)>)>;

/// Fetches one Dux Shell plugin's declarative settings QML and hands it back on the
/// main thread. `None` means the plugin ships no settings file.
pub type PluginSettingsHook = Rc<dyn Fn(String, Box<dyn FnOnce(Result<Option<String>, String>)>)>;

/// Fetches the generated wallpaper set for the current configuration target.
pub type WallpaperSetHook =
    Rc<dyn Fn(Box<dyn FnOnce(Result<crate::metadata::WallpaperSet, String>)>)>;

/// Runs the apply command of an option the user changed. Supplied by the app
/// so the widgets stay free of process handling.
pub type ApplyHook = Rc<dyn Fn(ApplyRequest)>;

/// Prompts for a password and hands back a generated hash, or `None` when the
/// prompt was cancelled.
pub type PasswordPromptHook = Rc<dyn Fn(Box<dyn FnOnce(Option<String>)>)>;

/// Fetches live Secure Boot state. `true` bypasses the cached result.
pub type SecureBootStatus =
    Rc<dyn Fn(bool, Box<dyn FnOnce(Result<crate::secureboot::SecureBootState, String>)>)>;

/// Writes the Lanzaboote configuration for the attached option.
pub type SecureBootWrite =
    Rc<dyn Fn(crate::model::Record, crate::secureboot::SetupOptions) -> Result<String, String>>;

/// Runs privileged setup commands in the configured terminal.
pub type SecureBootRun = Rc<dyn Fn(&[String]) -> Result<(), String>>;

/// Saves the configuration and starts the configured rebuild.
pub type SecureBootRebuild = Rc<dyn Fn()>;

/// Copies a wallpaper into the repository and stages it with git.
pub type WallpaperAdd = Rc<dyn Fn(&Path, &Path) -> Result<PathBuf, String>>;

/// Removes a wallpaper from the repository and stages the deletion.
pub type WallpaperRemove = Rc<dyn Fn(&Path) -> Result<(), String>>;

/// File operations for the wallpaper picker, supplied by the app.
#[derive(Clone)]
pub struct WallpaperActions {
    pub add: WallpaperAdd,
    pub remove: WallpaperRemove,
}

/// Copies a general file into the repository and stages it with git.
pub type FileAdd = Rc<dyn Fn(&Path, &Path) -> Result<PathBuf, String>>;

/// Removes a general file from the repository and stages the deletion.
pub type FileRemove = Rc<dyn Fn(&Path) -> Result<(), String>>;

/// File operations for the general file picker, supplied by the app.
#[derive(Clone)]
pub struct FileActions {
    pub add: FileAdd,
    pub remove: FileRemove,
}

/// Copies an icon into the repository and stages it with git.
pub type IconAdd = Rc<dyn Fn(&Path, &Path) -> Result<PathBuf, String>>;

/// File operations for the icon picker, supplied by the app.
#[derive(Clone)]
pub struct IconActions {
    pub add: IconAdd,
}

/// File path -> presets declared by @widget-preset markers in that file.
pub type PresetMap = std::rc::Rc<
    std::collections::HashMap<
        String,
        Vec<(
            crate::dsl::WidgetPresetSpec,
            Vec<crate::dsl::PresetFieldSpec>,
        )>,
    >,
>;

/// Extra context a control needs beyond its own record: the flake's available
/// module names, split by kind, and the package search hook.
#[derive(Default, Clone)]
pub struct BuildContext {
    pub nixos_modules: Vec<String>,
    pub hm_modules: Vec<String>,
    pub search: Option<PackageSearch>,
    /// Fetches the Dux Shell plugin registry ids for the plugins picker.
    pub plugin_registry: Option<PluginRegistryHook>,
    /// Fetches one plugin's declarative settings QML.
    pub plugin_settings: Option<PluginSettingsHook>,
    /// Fetches the generated wallpaper set.
    pub wallpaper_set: Option<WallpaperSetHook>,
    /// Runs the apply command of an option that carries one.
    pub apply: Option<ApplyHook>,
    /// Prompts for a password and returns a hash for a user account.
    pub password_prompt: Option<PasswordPromptHook>,
    /// Reads live Secure Boot state.
    pub secureboot_status: Option<SecureBootStatus>,
    /// Writes the Lanzaboote configuration for the attached option.
    pub secureboot_write: Option<SecureBootWrite>,
    /// Runs privileged Secure Boot commands.
    pub secureboot_run: Option<SecureBootRun>,
    /// Starts the configured rebuild.
    pub secureboot_rebuild: Option<SecureBootRebuild>,
    /// The flake root, used by widgets that resolve repository paths.
    pub base: String,
    /// Wallpaper copy and remove operations.
    pub wallpaper: Option<WallpaperActions>,
    /// General file copy and remove operations.
    pub file: Option<FileActions>,
    /// Icon copy operation for `icon` table columns.
    pub icon: Option<IconActions>,
    /// Whether the Advanced toggle is on, so nested hidden controls show.
    pub advanced: bool,
    /// Nesting depth of the control being built, used for submodule indent.
    pub depth: usize,
    /// The path of the enclosing option, so a nested option reports its full
    /// path instead of one relative to its parent.
    pub path_prefix: String,
    pub presets: PresetMap,
    /// Records in the current menu, keyed by full option path.
    pub records: Rc<std::collections::HashMap<String, Record>>,
    /// Whether helper text rows start expanded.
    pub expand_details: bool,
}
