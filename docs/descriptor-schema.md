# Descriptor sidecar

A descriptor sidecar is a JSON array that describes options the scanner cannot
annotate in place. It exists for values that are not options in the Nix module
system: freeform settings such as `dux.settings`, `dux.barConfigs`,
`dux.dashOptions`, and plugin `settings`. An `@option` comment needs a literal
assignment below it; a freeform attribute has no option declaration, so the
sidecar supplies the missing rendering metadata.

The feature is inert unless a file is supplied, either with
`--descriptors <file>` or with the `NSG_DESCRIPTORS` environment variable. A
path in the flag wins over the environment variable.

## Shape

```json
[
  {
    "path": "desktop.ojuny.dux.settings.clockDateFormat",
    "widget": "text",
    "label": "Clock date format",
    "description": "Format string for the date on the bar clock.",
    "group": "Clock",
    "order": 10,
    "choices": ["%H:%M", "%I:%M %p"],
    "min": null,
    "max": null,
    "step": null,
    "tier": "normal",
    "applyCommand": "dms ipc call settings set clockDateFormat %%val%%"
  }
]
```

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `path` | string | yes | Full option path, dot-separated. `desktop.ojuny.dms` and `desktop.ojuny.dux` are treated as the same namespace. A leading `config.` is ignored. |
| `widget` | string | yes | One of the `@option widget` names: `switch`, `dropdown`, `radio`, `spin`, `slider`, `text`, `multiline`, `path`, `package`, `packages`, `list`, `color`, `submodule`, `horizontal-submodule`, `grid-configuration`, `table-attrset`, `table-list`, `keybinds`, `monitors`, `theme`, `wallpaper`, `menu`, `plugins`, `bar`, `dock`, `font`, `icon-theme`, `cursor-theme`, `users`, `raw`. |
| `label` | string | no | Display label. Defaults to the last segment of the path. |
| `description` | string | no | Helper text. |
| `group` | string | no | Sub-heading within the menu. |
| `order` | integer | no | Sort key within the menu or group. |
| `choices` | list of strings | no | Values for `dropdown` and `radio`. Must not be empty when present. |
| `min`, `max` | number | no | Bounds for `spin` and `slider`. `min` must not exceed `max`. |
| `step` | number | no | Increment for `spin` and `slider`. Must be positive. |
| `tier` | string | no | `normal` (default), `advanced` (collapsed under "Advanced"), or `hidden` (only with the Advanced toggle). |
| `applyCommand` | string | no | Shell command run on change. `%%val%%` stands for the new value. `apply_command` is accepted as an alias. |

Unknown keys are rejected so a misspelled directive fails at load instead of
being silently ignored. Duplicate paths after namespace normalization are also
rejected.

## Precedence

1. A real `@option` annotation wins over a descriptor for the same path.
2. A descriptor wins over an assignment that has no annotation.
3. A descriptor for a path with no scanned assignment is added as a new record.
   The option registry (`metadata::detail`) is asked for the definition file,
   walking up parent paths until one is found, because freeform leaves are not
   registered options themselves. If the registry yields nothing, the scanned
   files are searched by attribute path.
4. A descriptor whose assignment cannot be located, or whose file no longer
   exists, is skipped and does not fail the scan.

## When to prefer a descriptor

Use an `@option` comment when the annotated assignment is a real option path
and you control the file. Use a descriptor when:

- the value lives in a freeform attribute set or a `types.toml`-style schema,
- the consuming file is generated or not under your control,
- one sidecar should describe many settings across several files,
- the same setting needs to be described for both the legacy and current
  namespace during the rename window.

A descriptor record is editable exactly like an annotated one: it carries a
file and byte span, participates in widget dispatch, and appears in the normal
save, diff, and apply flows.
