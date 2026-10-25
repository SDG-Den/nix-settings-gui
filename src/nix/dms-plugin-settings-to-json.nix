# Emits one DMS plugin's declarative settings QML as JSON:
# { id, settings = <text or null> }.
# Only the requested plugin's package is read, so adding one plugin does not
# instantiate the whole registry.
{flakePath, system ? null, pluginId}: let
  root = builtins.getFlake flakePath;

  safe = default: e: let
    r = builtins.tryEval e;
  in
    if r.success then r.value else default;

  desktop = safe null (root.inputs.ojuny-desktop or null);
  registry = safe null (desktop.inputs.dms-plugin-registry or null);

  packages =
    if registry == null
    then {}
    else if system != null
    then
      registry.packages.${system}
      or (
        let
          values = builtins.attrValues (safe {} registry.packages);
        in
          if values == [] then {} else builtins.head values
      )
    else
      let
        values = builtins.attrValues (safe {} registry.packages);
      in
        if values == [] then {} else builtins.head values;

  plugin =
    if builtins.isAttrs packages && packages ? ${pluginId}
    then packages.${pluginId}
    else null;

  manifest =
    if plugin == null
    then {}
    else safe {} (builtins.fromJSON (safe "{}" (builtins.readFile (toString plugin + "/plugin.json"))));

  stripDot = s:
    if builtins.substring 0 2 s == "./"
    then builtins.substring 2 (builtins.stringLength s) s
    else s;

  settingsPath = manifest.settings or null;

  settings =
    if plugin == null || settingsPath == null
    then null
    else safe null (builtins.readFile (toString plugin + "/" + stripDot settingsPath));
in {
  id = pluginId;
  inherit settings;
}
