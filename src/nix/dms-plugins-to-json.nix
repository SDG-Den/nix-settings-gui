# Emits the DMS plugin registry's ids (plus metadata when available) as
# JSON: { plugins = [ { id, name, description, category, repo } ]; }.
# The registry is reached through the config flake's ojuny-desktop input;
# every step is tryEval-guarded so a flake without that input yields [].
{flakePath, system ? null}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  desktop = safe null (root.inputs.ojuny-desktop or null);
  registry = safe null (desktop.inputs.dms-plugin-registry or null);

  # The registry source ships this manifest, keyed by meta.id. It is the
  # cheapest and most complete source for the picker.
  prefetch =
    if registry == null
    then null
    else safe null (builtins.fromJSON (builtins.readFile (registry.outPath + "/nix/plugins-prefetch.json")));

  packages =
    if registry == null || system == null
    then {}
    else
      registry.packages.${system}
      or (
        let
          values = builtins.attrValues (safe {} registry.packages);
        in
          if values == [] then {} else builtins.head values
      );

  ids =
    if builtins.isAttrs prefetch && prefetch != {}
    then builtins.attrNames prefetch
    else builtins.attrNames packages;

  entryFor = id: let
    raw =
      if builtins.isAttrs prefetch && prefetch ? ${id}
      then prefetch.${id}
      else {};
    meta =
      if builtins.isAttrs (raw.meta or null)
      then raw.meta
      else raw;
  in {
    inherit id;
    name = meta.name or null;
    description = meta.description or null;
    category = meta.category or null;
    repo = meta.repo or null;
    settings = meta.settings or null;
  };
in {
  plugins = builtins.map entryFor ids;
}
