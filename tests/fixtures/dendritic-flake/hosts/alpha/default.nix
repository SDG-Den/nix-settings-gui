{ self, inputs, ... }: {

  /*
    @menu
    displayname = "Alpha host";
    order = 90;
    hidden = true;
  */

  flake.nixosConfigurations.alpha = inputs.nixpkgs.lib.nixosSystem {
    modules = [
      inputs.home-manager.nixosModules.home-manager
      self.modules.nixos.common
      self.modules.nixos.bundle
      {
        home-manager.users.den.imports = [
          self.modules.homeManager.desktop
        ];
      }
    ];
  };
}
