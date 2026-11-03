# nix-settings-gui

## Purpose

nix-settings-gui renders an annotated Nix configuration as a GTK4 and
libadwaita settings window. A `@menu` comment defines a menu and an `@option`
comment defines one control, so annotated files stay valid Nix. Saves are
written back as span patches, checked with `nixd`, and formatted with `nixfmt`.
It runs on Linux and targets Dux Shell configurations, accepting the
`desktop.ojuny.dux` and legacy `desktop.ojuny.dms` namespaces.

## Status

Version 0.1.0. The repository has no release tag, so `Cargo.toml` and the flake
are the only version identifiers. Linux is the only declared platform, and we
have not verified it against a matrix of NixOS releases.

## Requirements

- Nix with flakes enabled, on Linux.
- GTK 4.10 and libadwaita 1.4 or newer, which the flake package supplies.
- `nixd` for option types, descriptions, defaults, and enum choices.
- `nixfmt` on `PATH` for formatting after a save.

Without `nixd` the app falls back to `nix eval` and renders untyped options as
`raw`.

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
- Privilege escalation is expressed in the configured rebuild command; the app
  does not prompt for it.
- Files under `/nix/store` are read-only. No Home Manager module exists, and
  the tool is Linux-only.

## Documentation

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
