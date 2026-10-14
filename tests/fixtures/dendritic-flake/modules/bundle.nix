{ self, inputs, ... }: {

  /* @menu
     displayname = "Bundle";
     order = 96;
     description = "Pulls PipeWire in through a with-list";
  */

  flake.modules.nixos.bundle = { pkgs, ... }: {

    imports = with self.modules.nixos; [ audio ];

  };
}
