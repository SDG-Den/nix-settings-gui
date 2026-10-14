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
      self.modules.nixos.audio
      {
        home-manager.users.den.imports = [
          self.modules.homeManager.desktop
        ];
      }
    ];
  };
}
