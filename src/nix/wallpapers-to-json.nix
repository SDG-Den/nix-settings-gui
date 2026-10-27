# Emits the generated wallpaper set for a config target as JSON:
# { directory, selected, default, storePath, drvPath, presets, user }.
# Falls back to the raw presets declared by a flake input when the target has
# no generated wallpaper options.
{flakePath, target ? ""}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  parts = builtins.filter builtins.isString (builtins.split ":" target);
  part = i: builtins.elemAt parts i;
  targetConfig =
    if builtins.length parts < 2
    then null
    else if part 0 == "nixos"
    then safe null (root.nixosConfigurations.${part 1}.config or null)
    else if part 0 == "home"
    then safe null (root.homeConfigurations.${part 1}.config or null)
    else if part 0 == "hm" && builtins.length parts >= 3
    then safe null (root.nixosConfigurations.${part 1}.config.home-manager.users.${part 2} or null)
    else null;

  wallpaper = safe null (
    if targetConfig != null
    && targetConfig ? desktop
    && targetConfig.desktop ? ojuny
    && targetConfig.desktop.ojuny ? wallpaper
    then targetConfig.desktop.ojuny.wallpaper
    else null
  );

  set = safe null (
    if wallpaper != null && wallpaper ? set && wallpaper.set != null
    then wallpaper.set
    else null
  );

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
  rawPresets =
    if source == null
    then {}
    else safe {} (inputs.${source}.wallpaperPresets or {});

  fallbackPresets =
    map (name: let preset = rawPresets.${name}; in {
      inherit name;
      label = preset.label or name;
      path = toString preset.file;
    }) (builtins.attrNames rawPresets);

  generatedPresets = safe null (
    if wallpaper != null && wallpaper ? presets && wallpaper.presets != {}
    then
      map (name: let entry = wallpaper.presets.${name}; in {
        inherit name;
        label = entry.label;
        path = entry.path;
      }) (builtins.attrNames wallpaper.presets)
    else null
  );

  presets = if generatedPresets == null || generatedPresets == [] then fallbackPresets else generatedPresets;

  user = safe [] (
    if wallpaper != null && wallpaper ? userWallpapers
    then
      map (name: {
        inherit name;
        label = name;
        path = wallpaper.userWallpapers.${name};
      }) (builtins.attrNames wallpaper.userWallpapers)
    else []
  );
in {
  directory = if wallpaper == null then "" else wallpaper.directory or "";
  selected = if wallpaper == null then "" else wallpaper.selected or "";
  default =
    if wallpaper == null
    then (if presets == [] then "" else (builtins.head presets).path)
    else wallpaper.default or "";
  storePath = if set == null then "" else toString set;
  drvPath = if set == null then "" else set.drvPath;
  inherit presets user;
}
