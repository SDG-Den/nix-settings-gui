# Usage

nix-settings-gui turns a Nix configuration into a settings window. It reads the
`@menu` and `@option` comments described in `comment-syntax.md`, renders one
control per option, and writes your changes back into the same files. The window
itself is described in `gui-design.md`.

## 1. Opening a configuration

Start the app from a terminal or a desktop launcher. The default configuration
lives in `~/.config/nix-settings-gui/config.json` (see `gui-design.md`, section
7). On first start, pick a flake or a config directory with the flake selector
at the top of the window. The app scans the directory tree, skipping
`flake.nix`, files whose name starts with `_`, and hidden directories.

Recent paths are remembered, so the selector offers them again next time.

## 2. Reading the window

- The left pane lists every menu the scanner found, grouped by `@menu category`
  and sorted by `order`.
- The right pane shows the selected menu: its header, the grouped controls, and
  an Advanced section for controls marked `advanced` or `hidden`.
- A control shows its label, its helper text, and an indicator when its staged
  value differs from the file.
- The search box filters the menu list and the visible controls. The "system
  only" checkbox narrows everything to the selected host.

## 3. Changing a value

Each control type maps to one widget:

| Widget      | Nix value written |
|-------------|-------------------|
| `switch`    | `true` / `false` |
| `dropdown`  | one of the `choices` |
| `spin`      | an integer |
| `slider`    | a number |
| `text`      | a quoted string |
| `multiline` | an indented string |
| `path`      | a path literal |
| `package`   | a package expression |
| `list`      | a list |
| `color`     | a `#rrggbb` string |
| `submodule` | a nested attribute set |
| `raw`       | verbatim text |

Edits are staged in memory and are not written until you save.

## 4. Saving

- **Save** serializes every dirty control, patches the affected files, checks
  each result with `rnix`, formats with `nixfmt`, and writes atomically. If any
  file fails to parse, nothing is written and an error is shown.
- **Save & rebuild** saves first, then runs the configured rebuild command in a
  terminal. If the save fails, the rebuild does not run.
- Files under `/nix/store` are never writable, and their controls refuse edits.

## 5. Adding a file

**Add file** writes a new module with a generated `@menu` header. Choose a
template kind (raw function, NixOS module, or home-manager module), a filename,
and a target directory. For a plain flake the app reminds you to add the module
to the flake's imports; dendritic `import-tree` flakes need no import edit.

## 6. Adding an option

**Add option** on a menu opens a picker of known option paths, with autocomplete.
After choosing a path, pick a widget and an initial value. The app writes the
`@option` block and the assignment into the target file and reformats it.

## 7. Settings

**Configure** edits the config file: the flake path, the terminal command, and
the rebuild command. The rebuild runs `terminal_command` with
`rebuild_command`; a `{cmd}` placeholder is replaced, otherwise the rebuild
command is appended. Privilege escalation is expressed entirely in those two
commands.

## 8. When nixd is missing

`nixd` provides option types, descriptions, defaults, and enum choices. Without
it, the app falls back to `nix eval --json` via `src/nix/options-to-json.nix`,
and options with no type data render as `raw`.
