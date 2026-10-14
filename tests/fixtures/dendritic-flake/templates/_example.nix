/* @template
   label = "Example module";
   kind = "nixos";
   filename = "example.nix";
   output = "modules";
   placeholders.name.label = "Module name";
   placeholders.name.default = "example";
   placeholders.name.hint = "flake.modules.nixos.<name>";
   placeholders.title.label = "Menu title";
   placeholders.title.default = "Example";
*/

/* @menu
   displayname = "%%title%%";
   order = 1000;
*/

{ self, inputs, ... }:

{

flake.modules.nixos."%%name%%" = { pkgs, ... }: {

};

}
