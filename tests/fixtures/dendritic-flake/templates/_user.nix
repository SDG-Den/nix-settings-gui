/* @template
   label = "User account";
   kind = "nixos";
   filename = "userdetails.nix";
   output = "modules/users";
   placeholders.name.label = "Username";
   placeholders.name.default = "newuser";
   placeholders.name.hint = "users.users.<name>";
   placeholders.description.label = "Description";
   placeholders.description.default = "New user";
   placeholders.groups.label = "Extra group";
   placeholders.groups.default = "wheel";
   placeholders.groups.hint = "one group name";
   placeholders.icon.label = "Icon";
   placeholders.icon.default = "avatar-default-symbolic";
*/

/* @menu
   displayname = "%%name%% Account";
   icon = "%%icon%%";
   order = 205;
   description = "Account definition for %%name%%";
   category = "Users";
*/

{ self, inputs, ... }:

{

  flake.modules.nixos."users-%%name%%" = { config, pkgs, ... }: {

    users.mutableUsers = true;

    users.users."%%name%%" = {
      isNormalUser = true;
      description = "%%description%%";
      extraGroups = [ "%%groups%%" ];
    };

  };

}
