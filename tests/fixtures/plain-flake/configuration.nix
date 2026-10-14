/* @menu
   displayname = "System";
   icon = "preferences-system-symbolic";
   order = 10;
   description = "Plain flake demo module";
*/

{ config, pkgs, ... }:

{
  /* @option widget = "text"; label = "Hostname"; */
  networking.hostName = "demo";

  /* @option widget = "dropdown"; choices = [ "Europe/Amsterdam" "UTC" ]; */
  time.timeZone = "Europe/Amsterdam";

  /* @option widget = "switch"; label = "Enable SSH"; */
  services.openssh.enable = true;

  /* @option widget = "packages"; label = "System packages"; */
  environment.systemPackages = [ pkgs.git pkgs.curl ];

  system.stateVersion = "25.11";
}
