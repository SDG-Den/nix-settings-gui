# Architecture

nix-settings-gui is a small pipeline: scan comment-annotated Nix files into a
menu and record model, enrich the records with option metadata, render them as
widgets, then write edits back with span patches. This document names the
modules, their responsibilities, and the data that flows between them.

## 1. Layers

```
main.rs
  app.rs        GTK4 / libadwaita shell, view state, flows
    widgets.rs  widget factory and value serialization
      wallpaper.rs  horizontal wallpaper picker, previews, file operations
      icon.rs       icon table-cell picker, preview, git-staged copy
    edit.rs     span-patch engine, nixfmt, atomic write
    scan.rs     file discovery and record extraction
    dsl.rs      @menu / @option / @template comment parsing
    model.rs    MenuModel, categories, ordering, filtering
    metadata.rs nixd and nix-eval option metadata
    nixd.rs     LSP JSON-RPC client
    value.rs    Nix literal classification and rendering
    templates.rs  _-prefixed file templates and placeholder rendering
    config.rs   ~/.config/nix-settings-gui/config.json
    apply.rs    apply-command helpers: value substitution and shell quoting
  nix/options-to-json.nix  eval fallback that enumerates options
```

Each layer depends only on the ones below it. `app.rs` is the only module that
touches GTK widgets; everything below it is plain data.

## 2. Scanning

`scan.rs` walks the configuration root and parses each `.nix` file with `rnix`.
It skips `flake.nix`, names starting with `_`, and hidden directories. For every
file it finds the `@menu` header and the `@option` comments with `dsl.rs`, and
records the source span of each value. A file with no `@menu` header still
becomes a menu named after the file. Files that fail to parse are listed with an
error and skipped. For dendritic files it also records which
`flake.modules.<kind>.<name>` origins the file defines and which module origins
it imports, plus the modules each `flake.nixosConfigurations.<host>` pulls in,
so host filtering can follow the import graph.

Two helpers support add-option: `scan_value` reads the inner bindings of an
attribute set so a submodule control can be built from a value, and
`body_insert_offset` finds where to insert a new binding inside a module body.

## 3. Model

`model.rs` groups records into menus, sorts by `order` then name, and answers
`menus_for(system)`, which returns the menus reachable from shared modules and
from one host configuration. It follows the recorded import graph transitively,
falls back to `@menu systems` and the `hosts/<host>/` directory name, and is
what multi-system filtering is built on:
"system only" shows `menus_for(current_host)`, "all settings" shows everything.

## 4. Metadata

`metadata.rs` asks `nixd.rs` (over LSP) for the type, description, default, and
enum choices of an option, using the option path as the query. When nixd is
absent or returns nothing, it falls back to `nix eval --json` against
`nix/options-to-json.nix`. `value.rs` classifies the resulting literal so the
widget factory can pick a control.

### Option discovery and module shape

nix-settings-gui discovers options through the NixOS/Home Manager module system
and nixd/eval, not only through `@option` comments. `@option` comments attach
rendering metadata to an assignment; they do not declare the option. Enumeration
reads the option tree exposed by the chosen target: nixd's option index when
available, and `nix eval` via `nix/options-to-json.nix` as the fallback.

An option declared inside `lib.mkMerge`, a list of attribute sets, or another
config-level expression is not reliably visible to option enumeration. The module
system may still evaluate the expression, but the option does not appear as a
stable declaration in the registry that the GUI walks. It can therefore be
missing from the option picker or render without type metadata.

Options must be declared at the top level of a module's `options` attribute. The
recommended pattern for an optional feature is a thin
`flake.modules.nixos.patch-<id>` module that declares the enable toggle and
imports a permanent backend module. The backend declares the feature's options
at its own top level and gates its `config` with `lib.mkIf`. Home-manager
backends should be added conditionally through `home-manager.sharedModules` and
should read the enable flag from `osConfig` instead of redeclaring it.

## 5. Widgets

`widgets.rs` maps a record and its type string to one of the `Editor` variants.
`widgets/wallpaper.rs` renders the horizontal wallpaper strip, resolves
`wallpaperDir` relative to the module file, reads the generated themed
wallpaper set for the current target through `presetSource`, and asks the app
to copy or remove images and stage them with git. `widgets/icon.rs` does the same for `icon` table
columns, resolving `iconDir` and showing a preview with an "Add icon" button.
`widgets/style_choice.rs` hosts the shared inline style chooser and preview pane.
`widgets/font.rs`, `widgets/icon_theme.rs`, and `widgets/cursor_theme.rs` supply
their catalogs and previews; `widgets/theme_catalog.rs` discovers installed
themes, `widgets/xcursor.rs` decodes cursor files, and `widgets/style_catalog.rs`
loads the Nixpkgs-only manifest embedded by `build.rs`. A style editor can emit
two staged edits, one for the value and one for its package option.
Each editor owns a GTK root widget, reports whether it is `unchanged`, produces
its `PendingEdit`s, and renders its value as a Nix literal. Serialization rules
live here: strings are quoted and escaped, numbers respect `digits`, paths stay
verbatim, and lists, enums, and submodules are rendered structurally.

## 6. Editing

`edit.rs` is the write path. `apply` groups `PendingEdit`s by file, sorts each
group by span, patches from the end of the file backwards, parse-checks the
result with `rnix`, formats with `nixfmt`, and writes each file atomically.
`editable` refuses any path outside the configuration root. `insert_assignment`
adds a whole block at a computed offset. The whole operation is all-or-nothing:
any parse failure aborts every file.

## 7. Shell

`app.rs` builds the window, owns the view state (current flake, host, system
filter, advanced filter, current menu, staged editors), and drives the flows:
open, reload, search, save, save-and-rebuild, add-file, add-option, and
configure. Background work (scan, nixd, eval, rebuild) runs on worker threads
and reports back over an `async-channel`, so the UI thread never blocks.

An option can carry an `applyCommand`, which runs it right away instead of
waiting for a save. The widget that changed hands the command and the new
value to the app through a `BuildContext` hook; `app/apply.rs` debounces the
change, keeps one command running at a time, and runs it through `sh -c`.
`apply.rs` holds the pure part: it substitutes `%%val%%` with the option's
plain value and quotes it for the shell.

## 8. Configuration

`config.rs` reads and writes `~/.config/nix-settings-gui/config.json`: the flake
path, the terminal and rebuild commands, and the recent flake paths. It is the
only persistent state the app keeps.

## 9. Flake inputs

`flake_inputs.rs` parses the flake's `inputs` set, accepting a literal `inputs`
attribute set and dotted declarations such as `inputs.nixpkgs.url`. It groups
each input's bindings, keeps `@input description` comments, and produces span
edits for upsert, add-all, and delete. `render_block` writes dotted entries back
as `inputs.<name> = {...}` so mixed styles round-trip.

`flake_ref.rs` is the pure flake-location model. It parses and renders every
Nix flake URI type (indirect, path, git, mercurial, tarball, file, github,
gitlab, sourcehut) with their common query attributes. It is used both to load
an existing nixpkgs URL into the dialog and to build the URL written back.

`nixpkgs.rs` holds the nixpkgs-specific rules. It recognizes the exact
`nixpkgs` input name, builds the built-in channel list, discovers `nixos-*`
branches with `git ls-remote`, sorts them, and caches the result for 24 hours
under `$XDG_CACHE_HOME/nix-settings-gui`. `flake_lock.rs` reads the direct
`nixpkgs` node from `flake.lock` for the card's pinned-revision line.

`ref_suggest.rs` implements the manual scans. GitHub and GitLab use their REST
APIs through `curl`, with optional `GITHUB_TOKEN` and `GITLAB_TOKEN`; generic
Git and SourceHut fall back to `git ls-remote`, and indirect ids come from
`nix registry list`. Every scan rejects missing prerequisites.

The GTK flow lives in `app/nixpkgs.rs`: a `PreferencesGroup` card above the
input list, and a dialog whose form is rebuilt when the flake-location type
changes. `app/inputs.rs` filters the exact `nixpkgs` entry out of the regular
list and reuses its `save_edits` path.
