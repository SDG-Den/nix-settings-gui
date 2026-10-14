self: {
  config,
  lib,
  pkgs,
  ...
}:
with lib; let
  cfg = config.services.nix-settings-gui;
  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
in {
  options.services.nix-settings-gui = {
    enable = mkEnableOption "nix-settings-gui, a comment-driven settings GUI for Nix";

    package = mkOption {
      type = types.package;
      default = package;
      description = "The nix-settings-gui package to use.";
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [cfg.package];
  };
}
