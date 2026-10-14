{ self, inputs, ... }: {

  /* @menu
     displayname = "Beta host";
     order = 91;
     hidden = true;
  */

  flake.nixosConfigurations.beta = inputs.nixpkgs.lib.nixosSystem {
    modules = [
      inputs.home-manager.nixosModules.home-manager
      self.modules.nixos.common
      {
        home-manager.users.den.imports = [
          self.modules.homeManager.desktop
        ];
      }
    ];
  };
}
