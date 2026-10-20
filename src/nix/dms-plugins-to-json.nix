# Emits the DMS plugin registry's ids (plus metadata when available) as
# JSON: { plugins = [ { id, name, description, category, repo } ]; }.
# The registry is reached through the config flake's ojuny-desktop input;
# every step is tryEval-guarded so a flake without that input yields [].
{flakePath}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  desktop = safe null (root.inputs.ojuny-desktop or null);
  registry = safe null (desktop.inputs.dms-plugin-registry or null);
  system = builtins.currentSystem;
  packages =
    if registry == null
    then {}
    else
      registry.packages.${system}
      or (
        let
          values = builtins.attrValues (safe {} registry.packages);
        in
          if values == [] then {} else builtins.head values
      );
  ids = builtins.attrNames packages;

  # Optional metadata: the registry source ships a prefetch manifest whose
  # keys equal meta.id. A layout change upstream degrades to ids only.
  prefetch = safe null (builtins.fromJSON (builtins.readFile (registry.outPath + "/nix/plugins-prefetch.json")));

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
  };
in {
  plugins = builtins.map entryFor ids;
}
