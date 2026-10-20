use std::cell::RefCell;
use std::rc::Rc;

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

/// Fetches the DMS plugin registry and hands the id list back on the main
/// thread. Supplied by the app so the widget stays free of nix plumbing.
pub type PluginRegistryHook =
    Rc<dyn Fn(Box<dyn FnOnce(Vec<crate::metadata::RegistryPlugin>)>)>;

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
    /// Fetches the DMS plugin registry ids for the plugins picker.
    pub plugin_registry: Option<PluginRegistryHook>,
    /// Whether the Advanced toggle is on, so nested hidden controls show.
    pub advanced: bool,
    /// Nesting depth of the control being built, used for submodule indent.
    pub depth: usize,
    pub presets: PresetMap,
}
