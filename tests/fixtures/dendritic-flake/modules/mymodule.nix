/*
  @menu
  displayname = "mymodule";
  order = 1000;
*/

{ config, pkgs, ... }:

{
  /*
    @option
    widget = "switch";
    label = "Docker";
  */
  programs.docker.enable = true;

}
