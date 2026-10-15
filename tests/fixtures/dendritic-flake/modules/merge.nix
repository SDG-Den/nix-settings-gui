{ self, inputs, ... }: {

  /* @menu
     displayname = "Merge";
     order = 80;
     description = "Merge and conditional module body";
  */

  flake.modules.nixos.merge = { config, pkgs, ... }:
    let
      /* @option widget = "text"; label = "Extra"; */
      extra = "value";
      enabled = true;
    in
    {
      /* @option widget = "switch"; label = "Base"; */
      services.base.enable = true;
    } // (if enabled then {
      /* @option widget = "text"; label = "Theme"; */
      theme.name = "dark";
    } else { });
}
