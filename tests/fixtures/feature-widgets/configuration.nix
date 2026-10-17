/* @menu
   displayname = "Features";
   icon = "preferences-system-symbolic";
   order = 90;
   description = "Fixture covering the widgets and ordering rules";
   systems = [ "all" ];
*/

{ config, pkgs, ... }:

{
  /* @option label = "Zeta first"; group = "Zeta"; */
  features.zetaFirst = 1;

  /* @option label = "Alpha unordered"; group = "Alpha"; groupOrder = 5; */
  features.alphaUnordered = 2;

  /* @option label = "Alpha second"; group = "Alpha"; groupOrder = 5; order = 2; */
  features.alphaSecond = 3;

  /* @option label = "Alpha first"; group = "Alpha"; groupOrder = 5; order = 1; */
  features.alphaFirst = 4;

  /* @option widget = "submodule"; label = "Nested"; */
  features.nested = {
    /* @option label = "Visible child"; */
    visibleChild = true;
    /* @option label = "Hidden child"; hidden = true; */
    hiddenChild = false;
    /* @option widget = "submodule"; label = "Deeper"; */
    deeper = {
      /* @option label = "Deep leaf"; */
      deepLeaf = 7;
    };
  };

  /* @option widget = "horizontal-submodule"; label = "Cards"; maxColumns = 2; */
  features.cards = {
    clock = { col = 0; row = 0; w = 2; h = 1; enabled = true; };
    weather = { col = 2; row = 0; w = 2; h = 1; enabled = true; };
  };

  /* @option widget = "table-attrset"; label = "Sites"; columns = [ "enable" "url" ]; firstValueEditable = true; */
  features.sites = {
    /* @table-item label = "Docs"; */
    docs = { enable = true; url = "https://example.com/docs"; };
    blog = { enable = false; url = "https://example.com/blog"; };
  };

  /* @option widget = "table-list"; label = "Rules"; columns = [ "name" "width" "height" ]; keyEditable = true; */
  features.rules = [
    { name = "eDP"; width = 1920; height = 1080; }
  ];

  /* @option widget = "keybinds"; label = "Binds"; */
  features.binds.default = {
    focusUp = {
      modifier = [ "SUPER" ];
      keys = [ "k" ];
      dispatcher = "focus";
      args = "up";
      description = "Focus up";
      group = "Focus";
      subcat = "view";
    };
  };

  /* @option widget = "monitors"; label = "Monitors"; */
  features.monitor_rule = [
    { name = "eDP-1"; width = 1920; height = 1080; refresh = 60.0; x = 0; y = 0; scale = 1.0; }
  ];

  /* @option widget = "theme"; label = "Palette"; */
  features.palette = "ocean";

  /* @option widget = "menu"; label = "Menu"; */
  features.menu = {
    "apps" = { icon = "apps"; label = "Apps"; };
    "apps.files" = { icon = "folder"; label = "Files"; };
  };

  /* @option widget = "plugins"; label = "Plugins"; */
  features.plugins = {
    dankMenu = { enable = true; };
  };

  /* @option widget = "bar"; label = "Bar"; */
  features.bar.default = {
    enabled = true;
    centerWidgets = [ "clock" ];
    /* @option label = "Left widgets"; */
    leftWidgets = {
      workspaceSwitcher = { enabled = true; _order = 10; };
    };
    /* @option label = "Right widgets"; */
    rightWidgets = {
      systemTray = { enabled = true; _order = 10; };
    };
  };

  /* @option widget = "dock"; label = "Dock"; */
  features.dockConfig = {
    enabled = false;
    iconSize = 40;
    /* @option label = "Widgets"; */
    widgets = {
      dock_launcher = { enabled = true; _order = 10; };
    };
  };
}
