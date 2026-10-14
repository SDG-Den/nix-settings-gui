/* @menu
   displayname = "System";
   icon = "preferences-system-symbolic";
   order = 10;
   description = "Single-file demo, like the stock /etc/nixos configuration";
*/

{ config, pkgs, ... }:

{
  /* @option widget = "text"; label = "Hostname"; */
  networking.hostName = "demo";

  /* @option widget = "dropdown"; choices = [ "Europe/Amsterdam" "UTC" "America/New_York" ]; */
  time.timeZone = "Europe/Amsterdam";

  /* @option widget = "dropdown"; choices = [ "en_US.UTF-8" "nl_NL.UTF-8" "de_DE.UTF-8" ]; */
  i18n.defaultLocale = "en_US.UTF-8";

  /* @option widget = "switch"; label = "Allow unfree packages"; */
  nixpkgs.config.allowUnfree = true;

  /* @option widget = "list"; label = "Experimental features"; */
  nix.settings.experimental-features = [ "nix-command" "flakes" ];

  /* @option widget = "switch"; label = "Enable SSH"; */
  services.openssh.enable = true;

  /* @option widget = "spin"; min = 1; max = 64; label = "Max build jobs"; */
  nix.settings.max-jobs = 8;

  /* @option widget = "packages"; label = "System packages"; */
  environment.systemPackages = [ pkgs.git pkgs.curl pkgs.vim ];

  /* @option widget = "submodule"; label = "Firewall"; */
  networking.firewall = {
    enable = true;
    allowedTCPPorts = [ 22 80 443 ];
  };

  system.stateVersion = "25.11";
}
