{flakePath}: let
  root = builtins.getFlake flakePath;
  nixpkgs = root.inputs.nixpkgs or null;
  source =
    if nixpkgs == null
    then null
    else (nixpkgs.outPath or (toString nixpkgs));
  manifestPath =
    if source == null
    then null
    else source + "/pkgs/data/fonts/nerd-fonts/manifests/fonts.json";
  manifest =
    if manifestPath != null && builtins.pathExists manifestPath
    then builtins.fromJSON (builtins.readFile manifestPath)
    else [];
  lib = nixpkgs.lib;
  convertAttrName = name: let
    lowerName = lib.strings.toLower name;
  in
    if builtins.match "^[[:digit:]].*" lowerName != null
    then "_" + lowerName
    else lowerName;
  entry = family: monospace: font: {
    family = family font.patchedName;
    label = family font.patchedName;
    package = "pkgs.nerd-fonts.${convertAttrName font.caskName}";
    inherit monospace;
  };
  fonts = builtins.concatMap (font: [
    (entry (name: "${name} Nerd Font") false font)
    (entry (name: "${name} Nerd Font Mono") true font)
    (entry (name: "${name} Nerd Font Propo") false font)
  ]) manifest;
in {
  inherit fonts;
}
