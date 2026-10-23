# GUI design

nix-settings-gui is a GTK4 / libadwaita application, written in Rust, that
renders a Nix configuration as a settings UI. The comment DSL is defined in
`comment-syntax.md`; this document covers the window, its regions, its state
model, and the flows that act on files.

The stack reuses `nix-regedit`'s engine: the `rnix` scanner and record model,
the `nixd` JSON-RPC client, and the span-patch plus `nixfmt` write path.

## 1. Window layout

```
+----------------------------------------------------------------------+
| title            [ search .................. ] [system only] [adv] [x]|
+-------------------+--------------------------------------------------+
| Menu list         | Content                                          |
|  Category A       |   menu header (displayname, description)         |
|    Menu 1         |   group heading                                  |
|    Menu 2         |     [widget] label            value              |
|  Category B       |     ...                                          |
|    Menu 3         |   [ Advanced ]                                   |
|                   |   [ + Add option ]                               |
+-------------------+--------------------------------------------------+
| [Configure] [Add file]                        [Save] [Save & rebuild]|
+----------------------------------------------------------------------+
```

### 1.1 Top bar

- Title: the flake name or config path.
- Search entry: searches option paths and labels across every file, not just
  the current menu. Selecting a result switches the left selection to that
  menu and scrolls to and highlights the option.
- "Current system only" checkbox: filters menus and options to the detected
  system (see section 6).
- Advanced toggle: reveals `hidden` menus and `hidden`/`advanced` controls.
- Close button: closes the window, which triggers the unsaved-changes prompt.

### 1.2 Left pane: menu list

- One entry per `.nix` file that the scanner accepts.
- Entries are grouped under `@menu category` headings and sorted by
  `@menu order`, then by name.
- `@menu hidden = true` entries show only when the advanced toggle is on.
- Home-manager modules for a host are all included; which category and menu
  they appear under is decided by their own `@menu category` and
  `displayname` comments, so the DSL, not the GUI, controls placement.
- Selecting an entry drives the content pane. Selection is preserved across
  reloads when the file still exists.

### 1.3 Right pane: content

- Header from the selected file's `@menu` (`displayname`, `description`).
- Controls grouped by `@option group` and sorted by `order`; ungrouped
  controls come first.
- Group order: groups with an explicit `groupOrder` come first ordered by value,
  then the remaining groups in order of first source appearance. Within a group,
  controls with an explicit `order` come first ordered by value, and the rest
  keep declaration order. The same rule applies inside a submodule.
- Control layout is vertical by default: the label sits above the widget,
  left-aligned, and the widget fills the width underneath. A control
  stays inline (label left, widget right) only for `switch`, `spin`, and
  `dropdown` whose longest choice is under 24 characters. `slider` is vertical.
  Width-hungry widgets (`text`, `multiline`, `path`, `package`, `packages`,
  `list`, module lists, `submodule`, `horizontal-submodule`, tables, and `raw`)
  are full-width.
- Helper text is collapsed behind an information icon below the widget body.
  Activating the icon reveals the nixd or comment description and the
  "imported from <origin>" footer. The icon is hidden when both are absent. A
  submodule shows its own label above the group and its own collapsed details
  row below it, with each nested level indented 12px (capped at four levels).
- A `horizontal-submodule` lays its labelled children out in a homogeneous
  `FlowBox` that wraps to an equal grid, capped by `maxColumns`; each child's
  description uses the same collapsed details row.
- `expand_details_by_default` in the config file opens every details row
  initially. The "Expand setting details" preference toggles it at runtime.
- Tables (`table-attrset`, `table-list`) render one row per set, one cell per
  `columns` entry, typed by value (switch/spin/text), with built-in add and
  remove. A `@table-item` comment above a row sets its `label`, `hidden`, and
  `visibleWhen`.
- `advanced` controls collapse into an "Advanced" section; `hidden` controls
  render only while the Advanced toggle is on, including inside submodules.
- A control with `visibleWhen` renders only while another option in the same
  menu satisfies the condition, re-evaluated live as values change. Hidden
  controls are not written on save.
- Each control shows its label, a collapsed details toggle for helper text, and
  an unsaved indicator when its staged value differs from the file.
- Controls that resolve to another module (a merged import) include an
  "imported from <origin>" line in their details, and edits are applied at the
  origin file.
- A per-option action resets the control to the nixd default.
- The "Add option" button sits below the controls.
- Each `@insert` marker renders an "Add <label>" button at the end of its group
  (or at the end of the menu when it has no group). Pressing it opens a dialog
  for the snippet's `%%placeholder%%` values, then inserts the fragment directly
  above the marker.

## 2. Bottom bar

- Configure: opens the settings submenu (section 7).
- Add file: opens the new-file dialog (section 9).
- Save: writes all staged edits (section 5).
- Save & rebuild: saves, then spawns the rebuild (section 8).
- A spinner shows while a save or rebuild runs. A second spinner shows while an
  option's apply command runs, so the two never share one indicator.
- The status line names the option that is being applied and reports whether
  the command succeeded.

## 3. Global search

Search matches option paths (`services.pipewire.enable`), `@option` labels, and
descriptions. Results list the menu and option; activating one navigates to it.
Search respects the "current system only" filter.

## 4. Option metadata

Types, descriptions, defaults, and enum choices come from `nixd`, queried over
LSP for the active target. When nixd returns nothing for an option, the app
falls back to `nix eval --json` against `src/nix/options-to-json.nix`, which
enumerates the option set for NixOS, home-manager, flake-parts `debug.options`,
and home-manager nested in NixOS. nixd is always tried first.

The valid option keys used by add-autocomplete come from the same source.

## 5. State and saving

- Edits are staged in memory. Each control tracks original, current, and dirty
  state.
- Save is batched and all-or-nothing: serialize every dirty control, collect
  per-file span patches, parse-check each file with `rnix`, format with
  `nixfmt`, then write each file atomically. If any file fails to parse, no
  file is written and an error is shown.
- Unsaved-changes protection: closing with staged edits prompts to save,
  discard, or cancel; a manual reload re-reads files from disk; an explicit
  discard drops all staged edits.
- Options under `/nix/store` are never writable; edits are refused for them.
- Dynamic attribute paths and non-literal keys are not addressable and produce
  no control.
- An optional diff preview shows the pending changes before writing.
- An option that carries an `applyCommand` runs that command as soon as its
  value changes, without saving. Changes are collected for a moment first, so a
  slider drag sends one command with the value it ended on, and only one
  command runs at a time. The file is not written, so the change lasts until
  the next rebuild.

## 6. Current system detection

The "current system only" mode selects the flake's host whose name and
configuration match the running machine. Detection compares the live hostname
against the flake's `nixosConfigurations.<host>`, and matches the live
configuration where it can be read. When no host matches, the app falls back to
showing shared modules and asks the user to pick a host. In "all settings"
mode, every module and every host configuration is shown.

In "current system only" mode, menus are filtered by the host's module import
graph: a menu shows when a module its file defines is reachable from the host's
configuration by following `imports` transitively. `@menu systems` overrides
the graph, and the `hosts/<host>/` directory name is the fallback when no origin
can be resolved. The special name `"all"` in a `systems` list matches every host.

Bespoke controls (`keybinds`, `monitors`, `theme`, `wallpaper`, `menu`, `plugins`,
`bar`, `dock`) render a configuration-specific UI for a single option, with
their own add, remove, and reorder actions. Their field shapes are hard-coded in the
application; the configuration only names the widget through `@option`.

## 7. Settings submenu

Stored in `~/.config/nix-settings-gui/config.json`:

| Key                | Meaning |
|--------------------|---------|
| `flake_path`       | Path to the flake or config directory to open. |
| `terminal_command` | Terminal prefix used to spawn the rebuild. |
| `rebuild_command`  | Command run inside the terminal to rebuild. |
| `expand_details_by_default` | Open setting descriptions and import origins initially. |

The app runs `terminal_command` with `rebuild_command`. If `terminal_command`
contains a `{cmd}` placeholder it is replaced; otherwise the rebuild command is
appended. Privilege escalation is entirely the user's choice, expressed in
these two commands.

Example (the maintainer's own setup):

- `terminal_command = "ghostty -e zsh -c '{cmd}'"`
- `rebuild_command = "sudo echo \"rebuilding\" && sudo nixos-rebuild boot --flake ~/repos/nixos-configs/dendrite#$(hostname) |& nom"`

Recent flake paths are also persisted here for the open flow.

## 8. Rebuild flow

Save & rebuild first performs a full batched save. On success it spawns the
configured terminal running the rebuild command, with the working directory set
to the flake root. The app does not wait for the terminal to exit and does not
manage privileges. If the save fails, the rebuild is not run.

## 9. Add-file flow

The Add file dialog picks:

- Kind: raw function, NixOS module, or home-manager module.
- Filename and target directory.

The app writes a template with a generated `@menu` header and the correct
function signature for the kind. For a plain (non-flake-parts) flake it writes
the file and then shows a reminder that the module must be added to the flake's
imports list; it does not edit the imports automatically. Dendritic
`import-tree` flakes need no import edit.

## 10. Add-option flow

The Add option button opens a picker with autocomplete restricted to valid
option paths from section 4. After a path is chosen, the user picks a widget and
an initial value. The app writes the `@option` block and the assignment into the
chosen target file, then reformats. Under "all settings" mode the user chooses
which file or module the option is added to.

## 11. Discovery rules

- Skip `flake.nix`, files whose name starts with `_`, and hidden directories.
- A file without an `@menu` header still becomes a menu, named after the file.
- Files that fail to parse are listed with an error and skipped.

## 12. Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| Ctrl+S   | Save |
| Ctrl+Shift+S | Save & rebuild |
| Ctrl+F   | Focus search |
| Ctrl+R   | Reload from disk |
| Esc      | Close (with unsaved prompt) |
| Ctrl+Q   | Quit |

## 13. Error handling

- Parse, eval, nixd, and write failures appear in a status area at the bottom
  of the content pane.
- A failed rebuild does not affect saved files; its output is in the spawned
  terminal.
- nixd is optional: without it, controls fall back to eval-provided types and
  descriptions, and options without type data render as `raw`.
