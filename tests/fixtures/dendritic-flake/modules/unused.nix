{ self, inputs, ... }: {

  /* @menu
     displayname = "Unused";
     order = 95;
     description = "Not imported by any host";
  */

  flake.modules.nixos.unused = { pkgs, ... }: {

    # @option widget = "switch"; label = "Unused flag";
    services.unused.enable = true;
  };
}
