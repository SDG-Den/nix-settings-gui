# nix-settings-gui

A comment-driven settings GUI for Nix. Annotate your Nix configuration with
`@menu` and `@option` comments, and edit those options in a GTK4 / libadwaita
window instead of by hand. Changes are patched back into the original files as
text, checked with `rnix`, and formatted with `nixfmt`.

The GUI targets Dux Shell (formerly DMS) configurations. Option paths accept
both the `desktop.ojuny.dux` and legacy `desktop.ojuny.dms` namespaces, and the
plugin registry is read from `dux-plugin-registry` with the legacy input names
as fallbacks.

## Documentation

- `docs/comment-syntax.md` — the `@menu` and `@option` comment DSL.
- `docs/descriptor-schema.md` — JSON descriptors for freeform settings.
- `docs/gui-design.md` — the window, its regions, and its flows.
- `docs/usage.md` — how to open a configuration and change values.
- `docs/architecture.md` — the scanning, widget, and edit layers.

## Build and run

```
nix build
nix run .
```

With a Rust toolchain and the GTK libraries available:

```
cargo run
cargo test
```

`nix flake check` builds the package and runs the test suite, which scans the
sample configurations under `tests/fixtures/`.

## License

GPL-3.0-or-later. See `LICENSE`.
