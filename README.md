# nix-settings-gui

## Purpose

nix-settings-gui renders an annotated Nix configuration as a graphical settings window. A `@menu` comment defines a menu and an `@option` comment defines one control, so annotated files stay valid Nix. Saves are
written back as span patches, checked with `nixd`, and formatted with `nixfmt`.

nix-settings-gui contains multiple custom components for ojuny, but is entirely useable on non-ojuny devices, supporting a wide variety of general modules. 


## Requirements

- Nix with flakes enabled, on Linux.

## Install

Add the flake input to `flake.nix`:

```nix
inputs.nix-settings-gui.url = "github:ojuny-project/nix-settings-gui";
```

Enable the NixOS module in `configuration.nix`:

```nix
services.nix-settings-gui.enable = true;
```

Rebuild with `nixos-rebuild switch`. The package installs `nix-settings-gui` in
`environment.systemPackages` and adds a desktop entry. The flake exports
`nixosModules.default`; it does not provide a Home Manager module.

Run it without installing:

```sh
nix run github:ojuny-project/nix-settings-gui
```

The settings window opens. Launch arguments are `--page <regex>` and
`--descriptors <file>`.

## Quickstart

Open a flake or configuration directory in the selector, pick a menu, change a
control, and press Save. Staged values are held in memory until then. Read
[docs/usage.md](docs/usage.md) for the window and save flow, and
[docs/comment-syntax.md](docs/comment-syntax.md) for the markers.

## Configure

Annotate a module so it produces one menu and one control:

```nix
/* @menu displayname = "System"; */
{
  /* @option widget = "text"; label = "Hostname"; */
  networking.hostName = "nixos";
}
```

The marker reference covers widgets, choices, tables, and templates in
[docs/comment-syntax.md](docs/comment-syntax.md). Application settings live in
`~/.config/nix-settings-gui/config.json`.

## Limits

- Options come from the module system, so annotations inside `lib.mkMerge`, a
  list of attribute sets, `map`, or `forEach` are dropped.
- Freeform settings need a descriptor sidecar.
- Changes are staged until Save, and a parse failure aborts the whole save.

## Releases

A `v*` tag, a manual workflow run, or a commit on `main` whose title starts with
a version such as `v0.1.0` builds `nix-settings-gui.bin` and `nix-settings-gui-flake.tar.gz`.

The release flake installs the prebuilt bundle without compiling:

```nix
inputs.nix-settings-gui.url =
  "tarball+https://github.com/ojuny-project/nix-settings-gui/releases/download/vX.Y.Z/nix-settings-gui-flake.tar.gz";
```

Then add `inputs.nix-settings-gui.packages.${pkgs.system}.nix-settings-gui` to `environment.systemPackages`.

The generated flake exposes packages only. Use the source flake when you need
the NixOS module.


## Documentation
- [OPTIONS.md](OPTIONS.md) is the generated options reference.

- [docs/usage.md](docs/usage.md): how to open a configuration and change values.
- [docs/comment-syntax.md](docs/comment-syntax.md): which comments exist and what they accept.
- [docs/gui-design.md](docs/gui-design.md): the window, its regions, and its flows.
- [docs/architecture.md](docs/architecture.md): the scanning, widget, and edit layers.
- [docs/descriptor-schema.md](docs/descriptor-schema.md): JSON descriptors for freeform settings.

## Contributing

Report issues at <https://github.com/ojuny-project/nix-settings-gui/issues>. A patch
must keep the existing test suite passing and include a test for scanner,
parser, or edit changes.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE). The stack reuses `nix-regedit`'s
scanner, `nixd` client, and write path.
