mod bar;
mod basic;
mod context;
mod cursor_theme;
mod details;
mod dispatch;
mod dms_catalog;
mod editor;
mod fields;
mod font;
mod grid;
mod grid_details;
mod grid_gestures;
mod grid_layout;
mod icon;
mod icon_theme;
mod keybinds;
mod menu;
mod module_list;
mod monitor_geometry;
mod monitor_picker;
mod monitors;
mod names;
pub(crate) mod nix;
mod style_catalog;
mod style_choice;
mod submodule;
mod table;
mod theme;
mod theme_catalog;
mod tile;
mod tile_list;
mod tile_nix;
mod users;
mod wallpaper;
mod xcursor;

pub(crate) use basic::{build_entry, build_spin, build_switch};
pub(crate) use context::{
    ApplyHook, BuildContext, ChangeHook, IconActions, PackageSearch, PasswordPromptHook,
    PluginRegistryHook, PluginSettingsHook, WallpaperActions, WallpaperSetHook,
};
pub(crate) use details::InfoDetails;
pub(crate) use dispatch::{build, is_package};
pub(crate) use editor::Editor;
pub(crate) use nix::{attr_key, literal_for_widget};
pub(crate) use submodule::{build_bespoke_child, hook_notify, scan_children};
