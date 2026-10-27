# Comment syntax

nix-settings-gui renders a Nix configuration directory as a settings UI. Each
`.nix` file becomes one menu. The GUI is configured entirely by ordinary Nix
block comments, so every annotated file stays valid Nix and keeps working in any
other Nix tooling.

There are six markers:

- `/* @menu ... */` once at the top of a file, describing the menu.
- `/* @option ... */` directly above an assignment, describing how that option
  is rendered.
- `/* @table-item ... */` directly above a row assignment inside a table.
- `/* @template ... */` at the top of a file under `templates/`, describing a
  scaffold offered by the "Add file" dialog.
- `/* @snippet ... */` at the top of a file, defining a reusable fragment.
- `/* @insert ... */` inside a module body, marking where a snippet's button
  appears.

Each marker body is a sequence of Nix assignments. The parser wraps the body in
`{ ... }` and parses it with the same lossless Nix parser used for the rest of
the file, so bodies use real Nix literals (`"strings"`, `[ lists ]`, numbers,
`true`/`false`).

## 1. Menu header (`@menu`)

The first `@menu` comment in a file is the menu header. It must appear before
the module body. If a file has no header, the menu is named after the file and
sorted alphabetically.

```nix
/* @menu
   displayname = "Audio";
   icon = "audio-card-symbolic";
   order = 40;
   description = "Sound server and controls";
   category = "Hardware";
   systems = [ "x570" "webhost" ];
   hidden = false;
*/
```

| Key           | Type            | Default            | Meaning |
|---------------|-----------------|--------------------|---------|
| `displayname` | string          | file name          | Title shown in the left menu list. |
| `icon`        | string          | generic icon       | Icon name from the current theme. |
| `order`       | integer         | 1000               | Sort key in the left list; lower comes first. |
| `description` | string          | nixd description   | Subtitle under the title. |
| `category`    | string          | none               | Groups menus under a heading in the left list. |
| `systems`     | list of strings | shared (all)       | Restricts the menu to these hosts. Empty or absent means shared. |
| `hidden`      | boolean         | false              | Hides the menu until "advanced" is enabled. |

## 2. Option directive (`@option`)

An `@option` comment binds to the attribute path of the next assignment below
it. This works at any depth, so placing it above a leaf configures that leaf,
and placing it above an attribute set configures the whole subtree.

```nix
/* @option
   label = "Enable PipeWire";
   widget = "switch";
   group = "Sound Server";
*/
services.pipewire.enable = true;
```

| Key           | Type            | Meaning |
|---------------|-----------------|---------|
| `widget`      | string          | Forces a control instead of the type default. |
| `label`       | string          | Display label; defaults to the option's short name or nixd description. |
| `description` | string          | Helper text; defaults to the nixd description. |
| `choices`     | list of strings | Values for `dropdown` and `radio`. |
| `min`, `max`  | number          | Bounds for `spin` and `slider`; setting both turns `spin` into `slider`. |
| `step`        | number          | Increment for `spin` and `slider`. |
| `digits`      | integer         | Decimal places for a float `spin`. |
| `unit`        | string          | Suffix shown after a number (for example `"MB"`). |
| `rows`        | integer         | Visible height of a `multiline` or `raw` editor. |
| `placeholder` | string          | Hint text for `text` and `multiline`. |
| `group`       | string          | Sub-heading within the menu. |
| `groupOrder`  | integer         | Rank of the control's group. Groups with `groupOrder` sort before groups without, ordered by the value; ties and unset groups fall back to first source appearance. |
| `order`       | integer         | Sort key within the menu or group. Controls with an explicit `order` come first, ordered by value; the rest keep declaration order. |
| `maxColumns`  | integer         | Maximum children per line for `horizontal-submodule` (default 4). |
| `columns`     | list of strings | Field names (columns) for `table-attrset` and `table-list`. |
| `columnTypes` | list of strings | Per-column editor hints for tables: `bool`, `int`, `float`, `str`, or `icon`. |
| `keyEditable` | boolean         | Table: whether cell keys are editable (default `false`). |
| `firstValueEditable` | boolean   | Table: whether the first cell's value is editable (default `false`). |
| `advanced`    | boolean         | Collapses the control under "Advanced". |
| `hidden`      | boolean         | Renders the control only while the Advanced toggle is on. |
| `readonly`    | boolean         | Shows the control disabled. |
| `nullable`    | boolean         | Adds an "unset" toggle for a `nullOr` option. |
| `pattern`     | string          | Regular expression a `text` value must match. |
| `itemLabel`   | string          | Label for one row of a `list`. |
| `addLabel`    | string          | Label of the list "add" button. |
| `visibleWhen.path` | string     | Another option in the same menu; the control shows only when its value satisfies the conditions below. |
| `visibleWhen.equals` | literal or list | Value the governing option must equal. A list matches any listed value. |
| `visibleWhen.notEquals` | literal | Value the governing option must not equal. |
| `applyCommand` | string     | Shell command that applies the option immediately, without saving. |
| `wallpaperDir` | string     | Directory holding wallpaper images, relative to the file containing the option. Defaults to `./wallpapers`. |
| `presetSource` | string | Flake input that declares wallpaper presets, e.g. `ojuny-desktop`. When set, the wallpaper picker loads the generated themed wallpaper set for the current target and lists those presets before the images in `wallpaperDir`. |
| `iconDir`     | string          | Directory holding icons added by `icon` table columns, relative to the file containing the option. Defaults to `./icons`. |

### Visibility dependencies

`visibleWhen.path` names another option in the same menu. The control renders
only while that option's current value satisfies the conditions. This makes an
enable toggle reveal its dependent settings:

```nix
/* @option widget = "switch"; label = "Enable JACK"; */
services.pipewire.jack.enable = true;

/* @option widget = "text"; label = "JACK server name";
   visibleWhen.path = "services.pipewire.jack.enable"; */
services.pipewire.jack.serverName = "default";
```

`visibleWhen.equals` and `visibleWhen.notEquals` take any literal (`true`,
`"auto"`, `3`). `visibleWhen.equals` also accepts a list, in which case the
governing value may equal any listed item:

```nix
/* @option widget = "text"; label = "NVIDIA bus ID";
   visibleWhen.path = "ojuny.gpu";
   visibleWhen.equals = [ "hybrid-intel-nvidia" "hybrid-amd-nvidia" ]; */
```

With neither set, the control shows while the governing value is truthy (not
`false`, `null`, or zero). When both are set, both must hold.

The dependency is evaluated live as you edit, and is scoped to the menu. If the
named option is not found in the menu, the control stays visible, so a stale
path never hides a setting unexpectedly. A control hidden by its dependency is
not written on save, so a disabled parent cannot commit its children.

### Applying a change immediately

`applyCommand` names a shell command that runs as soon as the option changes,
so a setting takes effect without waiting for "Save & rebuild":

```nix
/* @option widget = "switch"; label = "Dark mode";
   applyCommand = "dms ipc call theme set %%val%%"; */
services.dms.theme.dark = false;
```

`%%val%%` stands for the option's new value. The value is the plain text of
the setting: a string loses its quotes (`"dark"` becomes `dark`), a number
stays a number, and it reaches the command as a single argument. A command
without `%%val%%` runs as it is.

The command runs in the flake's root directory. Changes are collected for a
moment before the command runs, so dragging a slider sends one command with
the value it ended on. Only one command runs at a time: a change made while a
command is still running waits for it and then runs with the newest value. The
bottom bar shows a spinner and the status line names the option while the
command runs.

What the command does is up to the command. Applying a setting this way only
changes the running session: the file is untouched until it is saved, and the
change is gone after a reboot. Use it for settings that a service exposes over
its own interface, such as a DMS or mango IPC call.

## 3. Widgets

| Widget      | Renders | Serialized Nix |
|-------------|---------|----------------|
| `switch`    | Toggle  | `true` / `false` |
| `dropdown`  | Selection menu | one of the `choices` literals |
| `radio`     | Inline radio group | one of the `choices` literals |
| `spin`      | Numeric field | integer or float literal |
| `slider`    | Continuous slider | integer or float literal |
| `text`      | Single-line entry | `"..."` |
| `multiline` | Multi-line entry | `"..."` or `''...''` when the value contains newlines |
| `path`      | Entry plus file chooser | `"..."` path string or `./relative` |
| `package`   | Searchable picker | `pkgs.name` or a quoted attrpath |
| `packages`  | List of package pickers | `[ pkgs.a pkgs.b ]` |
| `list`      | Add, remove, reorder rows | `[ ... ]` |
| `nixosModules` | List of NixOS flake modules | `[ flake.modules.nixos.a ... ]` or bare names under `with flake.modules.nixos;` |
| `hmModules` | List of home-manager flake modules | `[ flake.modules.homeManager.a ... ]` or bare names under `with flake.modules.homeManager;` |
| `color`     | Color button | `"#rrggbb"` |
| `submodule` | Nested group of controls | recursive attribute set |
| `horizontal-submodule` | Row of labelled controls, wrapping to an equal grid | recursive attribute set |
| `table-attrset` | Editable table over an attribute set of sets | `{ key = { ... }; ... }` |
| `table-list` | Editable table over a list of sets | `[ { ... } ... ]` |
| `keybinds` | Bind list with a modifier+keys preview and edit popout | mode -> name -> bind set |
| `monitors` | Draggable monitor layout canvas over `monitor_rule` | list of monitor sets |
| `theme` | Scheme picker with colour swatches | short scheme name string |
| `wallpaper` | Horizontally scrolling wallpaper previews | `null`, a preset name, or a path string |
| `menu` | Nestable tree of menu entries | dotted-id keyed attribute set |
| `plugins` | Plugin list with a settings form | name-keyed attribute set |
| `bar` | Bar widget configuration | bar-name-keyed attribute set |
| `dock` | Dock configuration | dock-name-keyed attribute set |
| `raw`       | Monospace Nix editor | verbatim value |

### Submodules

A `submodule` renders an attribute set as a nested group of controls. Each level
shows its own heading above the group and its own "imported from" footer below
it, and indents 12px per level (up to four levels). Submodules nest: a child that
is itself an attribute set annotated with `@option` renders as another
`submodule`. Nested controls obey `hidden` and `advanced` the same way top-level
controls do, re-evaluated live when the Advanced toggle changes.

### Horizontal submodule

`horizontal-submodule` lays its children side by side, each with just its name
above the control. When a row does not fit, the children wrap to the next line
and the grid keeps equal-width cells. It is not nestable and does not sort its
children: declaration order is preserved. `maxColumns` caps how many children
sit on one line (default 4). It is the intended control for grid-style blocks
such as a desktop card's `col`/`row`/`width`/`height`.

### Tables

`table-attrset` and `table-list` edit a set of rows. Each row is an attribute set
of fields. `columns` lists the field names; a field absent from `columns` is not
shown. Column order follows `columns`, and rows keep source order.

```nix
/* @option
   widget = "table-attrset";
   label = "Monitors";
   columns = [ "name" "width" "height" "primary" ];
*/
monitors = {
  /* @table-item label = "Main"; */
  "DP-1" = { name = "DP-1"; width = 2560; height = 1440; primary = true; };
};
```

- `table-attrset`: the row's identity is the parent attribute name. Each row is
  serialized as `key = { ... };`.
- `table-list`: the row's identity is the first column's value. Each row is
  serialized as a `{ ... }` entry in a list.

Cells are typed automatically: a boolean becomes a switch, a number a spin
button, anything else a text field, so a cell is never plain text.
`columnTypes` overrides that per column with `bool`, `int`, `float`, `str`, or
`icon`. An `icon` column shows a preview with an "Add icon" button; the chosen
image is copied into `iconDir` and staged with `git add`, and the cell stores
its path.

`keyEditable` (default `false`) makes each cell's key editable and shows it
inline as `key = value`. `firstValueEditable` (default `false`) makes the first
cell's value editable; with it off, the first cell renders as a `label:` with its
key hidden and is assumed to be the row's name. With both off, the keys collapse
into one header line above the table and each row shows only its values.

An optional `@table-item` comment directly above a row assignment carries
per-row settings and nothing else:

| Key (`@table-item`) | Type | Default | Meaning |
|---------------------|------|---------|---------|
| `label` | string | the row identity | Row heading. |
| `hidden` | boolean | false | Hides the row until the Advanced toggle is on. |
| `visibleWhen.path` | string | none | Another option in the same menu; the row shows only while it satisfies the conditions. |
| `visibleWhen.equals` | literal or list | none | Value the governing option must equal. A list matches any listed value. |
| `visibleWhen.notEquals` | literal | none | Value the governing option must not equal. |

Rows are added and removed with built-in buttons, not `@snippet`: "add" appends a
row scaffolded from `columns`, "remove" deletes the selected row.

### Bespoke widgets

These widgets hard-code the shape of a specific configuration and attach to the
relevant option with `widget = "..."`. They render at the top level of a menu and
provide their own add, remove, and reorder buttons. The template only names the
widget; the field shape lives in the application.

| Widget | Attach to | Shape |
|--------|-----------|-------|
| `keybinds` | a bind mode, e.g. `binds.default` | `name -> { modifier; keys; dispatcher; args; description; group; subcat; }`. Shows a `modifier+keys` preview per row and opens a popout to edit the fields. |
| `monitors` | `mango.rules.monitor_rule` | a list of monitor sets. Renders the configured rules on a draggable canvas; dragging writes `x`/`y` only. Unset sizes default to a 1920x1080 rectangle (marked visually). |
| `theme` | the scheme option | a scheme name string. Shows a swatch per bundled scheme and writes the short name (`base16-ocean` becomes `ocean`). `polarity` and `wallpaper` stay separate options. |
| `wallpaper` | the wallpaper option | `null` for the default themed preset, a preset name, or a path to an image. With `presetSource`, the strip starts with the themed generated presets, then the images in `wallpaperDir`. Added files are copied into `wallpaperDir` and staged with `git add`; preset tiles are not removable. |
| `menu` | `dankMenu.menu` | an attribute set keyed by dotted id. Splits the id on `.` and renders a nestable tree; leaves are editable rows. |
| `plugins` | `dms.plugins` | a name-keyed attribute set of `{ enable; src; settings; dir; config; }`. `settings` renders as `raw` unless annotated as a typed submodule. |
| `bar` | `barConfigs` | a name-keyed attribute set with id-keyed `left`/`center`/`right` widget lists plus layout and appearance. |
| `dock` | `dockConfigs` | the same shape as `bar`, in its own control. |

The `menu` provider `apps` is only valid on a top-level entry. A top-level
entry with `provider = "apps"` cannot receive child options because the
provider generates its own dynamic submenu. The applications provider is not
offered on non-top-level entries, and existing invalid combinations are kept
and flagged until they are fixed.

Bar and dock widget lists keep disabled entries (`enabled = false`) instead of
dropping them. They can be re-enabled or removed from the editor. Each widget id
appears once because the convenience options are id-keyed attrsets.

Grid controls for control center widgets, dashboard cards, and weather widgets
use the same editor. New cards use the DMS default size for their id, and the
editor supports add, remove, move, resize, enable, footer, reset sizes, reset
selected, and clear all. `gridColumns` fixes the column count for the control;
panel column and row counts are not stored in the card collection.

## 4. Default type mapping

When `@option` has no `widget`, the control is chosen from the option type that
nixd reports:

| Nix option type                | Widget |
|--------------------------------|--------|
| `bool`                         | `switch` |
| integer types                  | `spin` |
| floating point types           | `spin` with `digits` |
| string types                   | `text` |
| enum (`one of "..."`)          | `dropdown` |
| path types                     | `path` |
| package types                  | `package` |
| list of package                | `packages` |
| other list types               | `list` |
| submodule, attrsOf submodule   | `submodule` |
| `nullOr X`                     | X's widget plus an unset toggle |
| freeform attributes, functions | `raw` |

## 5. Choice resolution

`dropdown` and `radio` fill their items in this order:

1. An explicit `choices = [ ... ];` in the `@option` comment.
2. The `one of "a", "b", "c"` clause parsed from the nixd type.
3. The sub-option names, when the option is an `attrsOf` over a set of keys.

If none applies, the control falls back to `text` with `pattern` validation.

## 6. `let` and `with` handling

The scanner unwraps a module body through lambdas, `let ... in`, `with ...;`,
and parentheses before collecting assignments. Comments attach to the next
assignment wherever it sits, including inside a `let` block:

```nix
{ pkgs, ... }:
let
  /* @option label = "Extra packages"; widget = "packages"; */
  extraPackages = [ pkgs.git pkgs.vim ];
in
with pkgs; {
  environment.systemPackages = extraPackages;
}
```

A `with` scope does not change attachment. The comment still binds to the
following assignment:

```nix
with pkgs; {
  /* @option widget = "dropdown"; choices = [ "auto" "manual" ]; */
  programs.example.mode = "auto";
}
```

When a value itself is wrapped in `let ... in` or `with ...;`, the wrapper is
peeled for type detection and preserved on write. Only the inner expression span
is replaced:

```nix
/* @option widget = "list"; */
environment.systemPackages = with pkgs; [ git vim ];
```

Here the editor rewrites `[ git vim ]` and leaves `with pkgs;` intact.

Dynamic attributes (`${...}`) and non-literal paths cannot be addressed, so no
control is generated for them and they are left untouched.

## 7. Multi-system flakes

With flake-parts and the dendritic pattern, shared modules live under
`flake.modules.*` and hosts under `flake.nixosConfigurations.*`. A checkbox
switches between:

- "All settings in the flake": every module and every host configuration.
- "Current system only": one selected host plus the modules it imports.

Menus are filtered by the host's module import graph: a menu shows when one of
the modules its file defines is reachable from the selected host's
configuration, following `imports` transitively. `@menu systems` overrides the
graph when present, and the `hosts/<host>/` directory name is the fallback for
files with no recoverable origin. A menu with no restriction is shared and
shows in both modes. The special system name `"all"` in a `systems` list matches
every host, so `systems = [ "all" ]` keeps a menu available to all systems as if
it were system-specific.

A top-level `imports` list in a `flake.modules.nixos.*` body is detected as a
`nixosModules` control; one in a `flake.modules.homeManager.*` body becomes
`hmModules`. Each lists the flake's available modules of that kind.

## 8. Adding settings

Adding a setting writes both the `@option` block and the assignment at the
chosen path, then reformats the file. For example, choosing
`services.pipewire.jack.enable` with a switch produces:

```nix
/* @option
   widget = "switch";
*/
services.pipewire.jack.enable = true;
```

## 9. Safety guarantee

The GUI never writes free-form text as the primary input. Every value is
serialized from a typed control, then parsed with the lossless Nix parser and
formatted with `nixfmt` before an atomic write. If the result would not parse,
the write is rejected and the file is left unchanged. An unparseable
configuration cannot be produced.

## 10. Worked examples

Single-file configuration:

```nix
/* @menu
   displayname = "System";
   icon = "preferences-system-symbolic";
   order = 10;
*/

{ config, pkgs, ... }:

{
  /* @option widget = "text"; label = "Hostname"; */
  networking.hostName = "myhost";

  /* @option widget = "dropdown"; choices = [ "Europe/Amsterdam" "UTC" ]; */
  time.timeZone = "Europe/Amsterdam";

  /* @option widget = "switch"; */
  nixpkgs.config.allowUnfree = true;
}
```

Dendritic module:

```nix
{ self, inputs, ... }: {

  /* @menu
     displayname = "Audio";
     icon = "audio-card-symbolic";
     order = 40;
     systems = [ "x570" ];
  */

  flake.modules.nixos.audio = { config, pkgs, ... }: {

    /* @option widget = "switch"; label = "Disable PulseAudio"; */
    services.pulseaudio.enable = false;

    /* @option widget = "switch"; group = "PipeWire"; */
    services.pipewire.enable = true;
  };
}
```

## 11. File templates (`@template`)

Files under a `templates/` directory at the flake root become scaffolds in the
"Add file" dialog. A template file is `_`-prefixed so the flake's `import-tree`
filter and the scanner both ignore it, and it starts with a `@template`
comment. The comment is stripped when the file is created; the rest of the file
may begin with a normal `@menu` header.

Placeholders are written `%%name%%` and are replaced by the values typed into
the dialog. Unknown tokens are left untouched. The token name is an identifier
(`[A-Za-z_][A-Za-z0-9_]*`).

```nix
/* @template
   label = "Audio module";
   kind = "nixos";
   filename = "audio.nix";
   output = "modules/nixos";
   placeholders.name.label = "Module name";
   placeholders.name.default = "audio";
   placeholders.title.label = "Menu title";
   placeholders.title.default = "Audio";
*/

/* @menu
   displayname = "%%title%%";
   order = 40;
*/

{ self, inputs, ... }:

{

flake.modules.nixos."%%name%%" = { pkgs, ... }: {

};

}
```

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `label` | string | file name | Entry shown in the Kind dropdown. |
| `kind` | string | `nixos` | `nixos`, `home`, or `raw`. |
| `filename` | string | `<stem>.nix` | Default new-file name. |
| `output` | string | `modules/<kind>` | Default directory, relative to the flake root. |
| `placeholders.<name>.label` | string | the name | Field label in the dialog. |
| `placeholders.<name>.default` | string | empty | Prefilled field value. |
| `placeholders.<name>.hint` | string | empty | Placeholder text shown when the field is empty. |

Placeholder order follows the order the fields first appear in the comment.

## 12. Insert snippets (`@snippet` and `@insert`)

A snippet is a reusable fragment offered as an "Add" button in a menu. A
`@snippet` comment at the top of a file defines the fragment; an `@insert`
comment inside the module body marks where its button appears.

```nix
/* @menu displayname = "Keybinds"; */

/* @snippet
   name = "keybind";
   label = "Add keybind";
   snippet = ''
     # @option widget = "text"; label = "Action";
     keybinds."%%keys%%" = "%%action%%";
   '';
   placeholders.keys.label = "Keys";
   placeholders.action.label = "Action";
   placeholders.action.default = "spawn";
*/

{ config, ... }:

{
  /* @insert
     name = "keybind";
     group = "Keybinds";
     order = 10;
  */
}
```

| Key (`@snippet`) | Type | Default | Meaning |
|------------------|------|---------|---------|
| `name` | string | required | Identifier an `@insert` refers to. |
| `label` | string | the name | Text of the "Add" button. |
| `snippet` | string | required | The fragment to insert. `body` is accepted as a fallback. |
| `placeholders.<name>.label` | string | the name | Field label in the fill-in dialog. |
| `placeholders.<name>.default` | string | empty | Prefilled field value. |
| `placeholders.<name>.hint` | string | empty | Placeholder text shown when the field is empty. |

| Key (`@insert`) | Type | Default | Meaning |
|-----------------|------|---------|---------|
| `name` | string | required | The `@snippet` this button inserts. |
| `label` | string | the snippet's label | Overrides the button text. |
| `group` | string | inferred | Places the button at the end of this group. |
| `order` | integer | 1000 | Sort key among the buttons in the same group. |

Placeholders are written `%%name%%` in the snippet body, the same notation as
`@template`, and are replaced with the values typed into the dialog. Adding
always opens the dialog, even when the snippet has no placeholders.

The fragment is inserted at the marker's line start, so the marker shifts one
line down each time and the snippet always lands directly above it. The inserted
text keeps its own `@option` comments, so the new control appears immediately.
Because comment bodies are lexed as ordinary comments, a multi-line `@snippet`
body annotates its lines with `#` line comments rather than a `/* */` block,
which would close the enclosing comment early. `@insert` belongs directly above
the assignments it introduces, and `nixfmt` rewrites both markers to `#` lines
on write; the scanner reads either form.


