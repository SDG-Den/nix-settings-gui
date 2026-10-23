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
    "apps" = { icon = "apps"; label = "Apps"; provider = "apps"; };
    "tools" = { icon = "build"; label = "Tools"; };
    "tools.files" = { icon = "folder"; label = "Files"; };
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
      clock = { enabled = false; _order = 20; clockCompactMode = true; };
    };
    /* @option label = "Right widgets"; */
    rightWidgets = {
      systemTray = { enabled = true; _order = 10; };
      customThing = { enabled = true; _order = 20; };
    };
  };

  /* @option widget = "grid-configuration"; label = "Control center"; gridColumns = 8; */
  features.controlCenterWidgets = {
    user = { enabled = true; _order = 10; col = 0; row = 0; w = 5; h = 1; };
    settings = { enabled = true; _order = 20; col = 5; row = 0; w = 1; h = 1; small = true; };
    disabledCard = { enabled = false; _order = 30; col = 6; row = 0; w = 1; h = 1; };
  };

  /* @option widget = "grid-configuration"; label = "Weather widgets"; gridColumns = 4; */
  features.weatherWidgets = {
    conditions = { col = 0; row = 0; w = 2; h = 2; tone = "primary"; };
    hourly = { col = 2; row = 0; w = 2; h = 2; forecast = "chart"; };
  };

  /* @option widget = "dock"; label = "Dock"; */
  features.dockConfig = {
    enabled = false;
    iconSize = 40;
    /* @option label = "Widgets"; */
    widgets = {
      dock_launcher = { enabled = true; _order = 10; };
      appsDock = { enabled = false; _order = 20; };
      customDockThing = { enabled = true; _order = 30; };
    };
  };
}
