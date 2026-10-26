# Emits the wallpaper presets declared by the config flake's ojuny-desktop
# input as JSON: { wallpapers = [ { name, label, path } ]; }.
{flakePath}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  desktop = safe null (root.inputs.ojuny-desktop or null);
  presets =
    if desktop == null
    then {}
    else safe {} (desktop.wallpaperPresets or {});
in {
  wallpapers = builtins.mapAttrsToList (name: preset: {
    inherit name;
    label = preset.label or name;
    path = toString preset.file;
  }) presets;
}
