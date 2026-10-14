{ self, inputs, ... }: {

  /*
    @menu
    displayname = "Desktop";
    icon = "user-desktop-symbolic";
    order = 20;
    category = "Desktop";
    description = "Home-manager desktop settings, every user on every host";
  */

  flake.modules.homeManager.desktop = { pkgs, ... }: {

    # @option widget = "switch"; label = "Enable Firefox";
    programs.firefox.enable = true;

    # @option widget = "text"; label = "GTK theme";
    gtk.theme.name = "Adwaita";
  };
}
