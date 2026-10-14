{ self, inputs, ... }: {

  /*
    @menu
    displayname = "Common";
    icon = "preferences-system-symbolic";
    order = 10;
    description = "Settings shared by every host";
  */

  flake.modules.nixos.common = { pkgs, lib, ... }: {

    # @option widget = "text"; label = "Time zone";
    time.timeZone = "Europe/Amsterdam";

    # @option widget = "dropdown"; choices = [ "en_US.UTF-8" "nl_NL.UTF-8" ];
    i18n.defaultLocale = "en_US.UTF-8";

    # @option widget = "switch"; label = "Allow unfree packages";
    nixpkgs.config.allowUnfree = true;

    # @option widget = "list"; label = "Experimental features";
    nix.settings.experimental-features = [
      "nix-command"
      "flakes"
    ];

    # @option widget = "packages"; label = "Base packages";
    environment.systemPackages = with pkgs; [
      git
      vim
    ];
  };
}
