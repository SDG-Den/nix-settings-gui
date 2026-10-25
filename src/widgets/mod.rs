mod bar;
mod basic;
mod context;
mod details;
mod dispatch;
mod dms_catalog;
mod editor;
mod fields;
mod grid;
mod grid_details;
mod grid_gestures;
mod grid_layout;
mod icon;
mod keybinds;
mod menu;
mod module_list;
mod monitor_geometry;
mod monitor_picker;
mod monitors;
mod names;
pub(crate) mod nix;
mod submodule;
mod table;
mod theme;
mod tile;
mod tile_list;
mod tile_nix;
mod wallpaper;

pub(crate) use basic::{build_entry, build_spin, build_switch};
pub(crate) use context::{
    ApplyHook, BuildContext, ChangeHook, IconActions, PackageSearch, PluginRegistryHook,
    PluginSettingsHook, WallpaperActions,
};
pub(crate) use details::InfoDetails;
pub(crate) use dispatch::{build, is_package};
pub(crate) use editor::Editor;
pub(crate) use nix::{attr_key, literal_for_widget};
pub(crate) use submodule::{build_bespoke_child, hook_notify, scan_children};
