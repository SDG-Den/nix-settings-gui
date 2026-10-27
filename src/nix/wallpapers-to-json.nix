# Emits the wallpaper presets declared by a config flake input as JSON:
# { wallpapers = [ { name, label, path } ]; }.
{flakePath}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  inputs = safe {} (root.inputs or {});
  hasPresets = name: safe false (inputs.${name} ? wallpaperPresets);
  preferred =
    if inputs ? ojuny-desktop && hasPresets "ojuny-desktop"
    then "ojuny-desktop"
    else null;
  candidates =
    if preferred != null
    then []
    else builtins.filter hasPresets (builtins.attrNames inputs);
  source =
    if preferred != null
    then preferred
    else if candidates == []
    then null
    else builtins.head candidates;
  presets =
    if source == null
    then {}
    else safe {} (inputs.${source}.wallpaperPresets or {});
in {
  wallpapers = map (name: let preset = presets.${name}; in {
    inherit name;
    label = preset.label or name;
    path = toString preset.file;
  }) (builtins.attrNames presets);
}
