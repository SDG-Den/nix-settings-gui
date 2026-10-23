use super::fields::attrset_field_nix;
use super::nix::{attr_key, quote};
use super::tile::WidgetTile;
use super::tile_list::WidgetTileList;

#[cfg(test)]
use super::nix::truthy;
#[cfg(test)]
use crate::scan;
/// A tile's serialization inputs, split from the GTK state so order
/// assignment and emission are unit-testable without a display.
pub(crate) struct TileEmit {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) field_lines: Vec<String>,
    pub(crate) orig_order: Option<i64>,
}

/// Derive the `_order` sequence for an emitted list. Original values are
/// collected into an ascending pool and dealt by position, so a no-op save
/// and a pure reorder keep the sparse values; positions past the pool draw
/// 10-step values that never fall behind the pool's tail.
pub(crate) fn assign_tile_orders(orig: &[Option<i64>]) -> Vec<String> {
    let mut pool: Vec<i64> = orig.iter().flatten().copied().collect();
    pool.sort_unstable();
    let mut last = 0i64;
    orig.iter()
        .enumerate()
        .map(|(index, _)| {
            let order = match pool.get(index) {
                Some(value) => *value,
                None => (((index as i64) + 1) * 10).max(last + 10),
            };
            last = order;
            order.to_string()
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn tile_emit_from_scan(row: &scan::TableRowScan) -> Option<TileEmit> {
    let mut enabled = true;
    let mut orig_order = None;
    let mut field_lines = Vec::new();
    for (name, raw) in &row.fields {
        let raw = raw.trim();
        match name.as_str() {
            "enabled" => enabled = truthy(raw),
            "_order" => orig_order = raw.parse::<i64>().ok(),
            _ => field_lines.push(format!("{} = {};", attr_key(name), raw)),
        }
    }
    if !enabled {
        return None;
    }
    Some(TileEmit {
        id: row.key.clone(),
        enabled: true,
        field_lines,
        orig_order,
    })
}

pub(crate) fn tile_emit_from_tile(tile: &WidgetTile) -> TileEmit {
    let field_lines = tile
        .fields
        .borrow()
        .iter()
        .filter_map(attrset_field_nix)
        .collect();
    TileEmit {
        id: tile.id.clone(),
        enabled: tile.enabled.get(),
        field_lines,
        orig_order: tile.orig_order.trim().parse::<i64>().ok(),
    }
}

/// Emit the attrset form of a widget list: `{ id = { enabled = ...; _order = ...; ... }; ... }`.
/// Disabled widgets stay in the configuration so they can be re-enabled later.
pub(crate) fn tiles_nix(entries: &[TileEmit]) -> String {
    let orders = assign_tile_orders(
        &entries
            .iter()
            .map(|entry| entry.orig_order)
            .collect::<Vec<_>>(),
    );
    let parts: Vec<String> = entries
        .iter()
        .zip(orders.iter())
        .map(|(entry, order)| {
            let mut body = format!(
                "enabled = {}; _order = {};",
                if entry.enabled { "true" } else { "false" },
                order
            );
            for line in &entry.field_lines {
                body.push(' ');
                body.push_str(line);
            }
            format!("{} = {{ {} }};", attr_key(&entry.id), body)
        })
        .collect();
    format!("{{ {} }}", parts.join(" "))
}

/// Emit the center list, which stays a plain string list: `[ "music" ... ]`.
pub(crate) fn center_tiles_nix(ids: &[String]) -> String {
    if ids.is_empty() {
        return "[ ]".to_string();
    }
    let items: Vec<String> = ids.iter().map(|id| quote(id)).collect();
    format!("[ {} ]", items.join(" "))
}

/// Read a plain nix string list into its items; anything else yields nothing.
pub(crate) fn parse_string_list(text: &str) -> Vec<String> {
    use rnix::ast::{Expr, InterpolPart};
    let Ok(root) = rnix::Root::parse(text).ok() else {
        return Vec::new();
    };
    let Some(Expr::List(list)) = root.expr() else {
        return Vec::new();
    };
    list.items()
        .filter_map(|item| match item {
            Expr::Str(string) => match string.normalized_parts().as_slice() {
                [InterpolPart::Literal(text)] => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The tile list matches its scanned original: same id sequence and every
/// field editor on its original raw text. `enabled` never participates;
/// entries with a falsy `enabled` are dropped at load and `_order` is derived
/// on save.
pub(crate) fn tiles_unchanged(list: &WidgetTileList) -> bool {
    let tiles = list.tiles.borrow();
    tiles.len() == list.orig_ids.len()
        && tiles
            .iter()
            .zip(list.orig_ids.iter())
            .all(|(tile, orig_id)| {
                tile.orig_id == *orig_id
                    && tile.id == tile.orig_id
                    && tile.orig_section == tile.section
                    && tile.enabled.get() == tile.orig_enabled
                    && tile
                        .fields
                        .borrow()
                        .iter()
                        .all(|field| field.editor.unchanged(&field.orig_value))
            })
}

pub(crate) fn valid_widget_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|next| next.is_ascii_alphanumeric() || next == '_')
}
#[cfg(test)]
mod tests {
    use super::*;

    fn template_file(relative: &str) -> std::path::PathBuf {
        match std::env::var("OJUNY_TEMPLATE") {
            Ok(dir) => std::path::PathBuf::from(dir).join(relative),
            Err(_) => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../ojuny-template")
                .join(relative),
        }
    }

    fn scans(value: &str) -> Vec<TileEmit> {
        scan::scan_table(value, false)
            .iter()
            .filter_map(tile_emit_from_scan)
            .collect()
    }

    #[test]
    fn tiles_nix_round_trips_bar_left_list() {
        let expected = "{ workspaceSwitcher = { enabled = true; _order = 10; showOccupiedWorkspacesOnly = true; showWorkspaceIndex = true; showWorkspacePadding = true; workspacePaddingCount = 5; dwlShowAllTags = false; }; layout = { enabled = true; _order = 20; }; launcherButton = { enabled = true; _order = 30; }; focusedWindow = { enabled = true; _order = 40; }; }";
        let source = "{ workspaceSwitcher = { enabled = true; _order = 10; showOccupiedWorkspacesOnly = true; showWorkspaceIndex = true; showWorkspacePadding = true; workspacePaddingCount = 5; dwlShowAllTags = false; }; layout = { enabled = true; _order = 20; }; launcherButton = { enabled = true; _order = 30; }; focusedWindow = { enabled = true; _order = 40; }; offWidget = { enabled = false; _order = 50; }; }";
        let entries = scans(source);
        assert_eq!(entries.len(), 4, "enabled = false entries never load");
        assert_eq!(entries[0].orig_order, Some(10));
        assert_eq!(tiles_nix(&entries), expected);
        assert_eq!(tiles_nix(&scans(source)), expected);
    }

    #[test]
    fn assigns_orders_from_the_original_pool() {
        assert_eq!(assign_tile_orders(&[Some(20), Some(10)]), ["10", "20"]);
        assert_eq!(
            assign_tile_orders(&[Some(10), None, Some(20)]),
            ["10", "20", "30"]
        );
        assert_eq!(
            assign_tile_orders(&[Some(10), Some(20), None]),
            ["10", "20", "30"]
        );
        assert_eq!(assign_tile_orders(&[None, None]), ["10", "20"]);
        assert_eq!(assign_tile_orders(&[Some(30)]), ["30"]);
        assert_eq!(
            assign_tile_orders(&[Some(10), Some(500), None]),
            ["10", "500", "510"]
        );
    }

    #[test]
    fn tiles_nix_preserves_disabled_entries() {
        let entries = vec![
            TileEmit {
                id: "clock".to_string(),
                enabled: false,
                field_lines: vec!["compact = true;".to_string()],
                orig_order: Some(10),
            },
            TileEmit {
                id: "weather".to_string(),
                enabled: true,
                field_lines: Vec::new(),
                orig_order: Some(20),
            },
        ];
        assert_eq!(
            tiles_nix(&entries),
            "{ clock = { enabled = false; _order = 10; compact = true; }; weather = { enabled = true; _order = 20; }; }"
        );
    }

    #[test]
    fn center_tiles_nix_round_trips_string_list() {
        let source = r#"[ "music" "clock" "weather" ]"#;
        let ids = parse_string_list(source);
        assert_eq!(ids, ["music", "clock", "weather"]);
        assert_eq!(center_tiles_nix(&ids), source);
        assert_eq!(center_tiles_nix(&[]), "[ ]");
        assert!(parse_string_list("{ }").is_empty());
    }

    #[test]
    fn tiles_nix_emits_dock_keys_in_order() {
        let expected = "{ dockLauncher = { enabled = true; _order = 10; }; appsDock = { enabled = true; _order = 20; }; dockTrash = { enabled = true; _order = 30; }; }";
        let source = "{ dockLauncher = { enabled = true; _order = 10; }; appsDock = { enabled = true; _order = 20; }; dockTrash = { enabled = true; _order = 30; }; offDock = { enabled = false; _order = 40; }; }";
        assert_eq!(tiles_nix(&scans(source)), expected);
    }

    #[test]
    fn custom_widget_round_trips() {
        let mut entries =
            scans("{ layout = { enabled = true; _order = 10; }; off = { enabled = false; _order = 5; }; }");
        assert_eq!(entries.len(), 1, "enabled = false entries never load");
        entries.push(TileEmit {
            id: "myThing".to_string(),
            enabled: true,
            field_lines: vec!["foo = \"bar\";".to_string()],
            orig_order: None,
        });
        let out = tiles_nix(&entries);
        assert_eq!(
            out,
            "{ layout = { enabled = true; _order = 10; }; myThing = { enabled = true; _order = 20; foo = \"bar\"; }; }"
        );
        assert_eq!(tiles_nix(&scans(&out)), out);
    }

    #[test]
    fn scans_widget_presets_from_template() {
        use rowan::ast::AstNode;
        let bar_path = template_file("modules/04-gui/01-bar.nix");
        if !bar_path.exists() {
            eprintln!("skipping: sibling ojuny-template checkout not available");
            return;
        }
        let bar = std::fs::read_to_string(bar_path).expect("01-bar.nix is readable");
        let Ok(root) = rnix::Root::parse(&bar).ok() else {
            panic!("01-bar.nix parses");
        };
        let presets = scan::find_widget_presets(&bar, root.syntax());
        assert_eq!(presets.len(), 32, "bar preset count");
        assert_eq!(
            presets
                .iter()
                .filter(|(preset, _)| preset.has_settings)
                .count(),
            20,
            "bar presets with settings"
        );
        let fields: usize = presets.iter().map(|(_, specs)| specs.len()).sum();
        assert!(fields >= 40, "bar preset field specs: {fields}");

        let dock = std::fs::read_to_string(template_file("modules/04-gui/02-dock.nix"))
            .expect("02-dock.nix is readable");
        let Ok(root) = rnix::Root::parse(&dock).ok() else {
            panic!("02-dock.nix parses");
        };
        assert_eq!(scan::find_widget_presets(&dock, root.syntax()).len(), 3);
    }
}
