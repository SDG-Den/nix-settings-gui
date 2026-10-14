/* @template
   label = "Home-manager module";
   kind = "home";
   filename = "home.nix";
   output = "modules";
   placeholders.name.label = "Module name";
   placeholders.name.default = "new-home";
   placeholders.name.hint = "flake.modules.homeManager.<name>";
   placeholders.title.label = "Menu title";
   placeholders.title.default = "New Home";
*/

/* @menu
   displayname = "%%title%%";
   order = 1000;
*/

{ self, inputs, ... }:

{

flake.modules.homeManager."%%name%%" = { pkgs, ... }: {

};

}
