# Emits the Dux Shell plugin registry's ids (plus metadata when available) as
# JSON: { plugins = [ { id, name, description, category, repo } ]; }.
# The registry is reached through the config flake's `dux-plugin-registry`
# input, falling back to the legacy `dms-plugin-registry` and `dms-plugins`
# names so existing configurations keep working. The input may sit at the flake
# root or under the desktop shell input. Every step is tryEval-guarded so a
# flake without any of them yields [].
{flakePath, system ? null}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  # The input can be exposed directly or through the desktop shell input.
  parents =
    [root]
    ++ builtins.filter (parent: parent != null) (builtins.map (name:
      safe null (root.inputs.${name} or null)) [
      "dux-shell"
      "ojuny-desktop"
    ]);

  findInput = name: let
    found =
      builtins.filter (input: input != null) (builtins.map (parent:
        safe null (parent.inputs.${name} or null))
      parents);
  in
    if found == [] then null else builtins.head found;

  # Current name first, then the legacy names in order.
  registry = let
    found =
      builtins.filter (input: input != null) (builtins.map findInput [
        "dux-plugin-registry"
        "dms-plugin-registry"
        "dms-plugins"
      ]);
  in
    if found == [] then null else builtins.head found;

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
