use super::nix::attr_key;
/// One rectangle of the placement grid, in whole cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CellRect {
    pub(crate) col: i64,
    pub(crate) row: i64,
    pub(crate) w: i64,
    pub(crate) h: i64,
}

/// Spacing between grid cells in pixels; also the pixel gap a drag delta is
/// divided by when it is rounded to whole cells.
pub(crate) const GRID_GAP: i64 = 8;
/// Pixel height reserved per grid row on the canvas.
pub(crate) const GRID_ROW_UNIT: i32 = 72;
/// Placement priority used when a tile has no `_order` field.
pub(crate) const GRID_DEFAULT_ORDER: i64 = 1000;
/// One tile fed to `repack`: its desired cell (None = first fit) and size.
pub(crate) struct TileInput {
    pub(crate) cell: Option<CellRect>,
    pub(crate) w: i64,
    pub(crate) h: i64,
}
/// A `width x height` overlap test between two whole-cell rectangles.
pub(crate) fn cells_overlap(a: CellRect, b: CellRect) -> bool {
    a.col < b.col + b.w && b.col < a.col + a.w && a.row < b.row + b.h && b.row < a.row + a.h
}

pub(crate) fn grid_overlaps(taken: &[CellRect], candidate: CellRect) -> bool {
    taken.iter().any(|cell| cells_overlap(*cell, candidate))
}

/// First free slot scanning rows top to bottom; falls back below the stack,
/// matching DMS `firstFit` (GridLayout.js:74-93).
pub(crate) fn grid_first_fit(taken: &[CellRect], columns: i64, w: i64, h: i64) -> CellRect {
    let ceiling = taken
        .iter()
        .map(|cell| cell.row + cell.h)
        .max()
        .unwrap_or(0);
    let mut row = 0;
    while row < ceiling {
        let mut col = 0;
        while col + w <= columns {
            let candidate = CellRect { col, row, w, h };
            if !grid_overlaps(taken, candidate) {
                return candidate;
            }
            col += 1;
        }
        row += 1;
    }
    CellRect {
        col: 0,
        row: ceiling,
        w,
        h,
    }
}

/// Pull every cell upward while the row above it is free, matching DMS
/// `compact` (GridLayout.js:46-53).
pub(crate) fn grid_compact(cells: &mut [CellRect]) {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|&i| (cells[i].row, cells[i].col));
    for &i in &order {
        while cells[i].row > 0 {
            let mut candidate = cells[i];
            candidate.row -= 1;
            let blocked = cells
                .iter()
                .enumerate()
                .any(|(j, cell)| j != i && cells_overlap(*cell, candidate));
            if blocked {
                break;
            }
            cells[i] = candidate;
        }
    }
}

/// Place every tile into a `columns`-wide grid. Tiles with a committed cell
/// keep their column and settle downward on collision; unplaced tiles first
/// fit. `order` is the placement priority (a permutation of `tiles` indices).
/// Returns the final cells, or `None` when the layout would exceed `max_rows`.
pub(crate) fn repack(
    tiles: &[TileInput],
    order: &[usize],
    columns: i64,
    max_rows: i64,
    gravity: bool,
) -> Option<Vec<CellRect>> {
    let mut taken: Vec<CellRect> = Vec::new();
    let mut out: Vec<Option<CellRect>> = vec![None; tiles.len()];
    for &i in order {
        let tile = &tiles[i];
        let w = tile.w.clamp(1, columns.max(1));
        let h = tile.h.max(1);
        let cell = match tile.cell {
            Some(committed) => {
                let col = committed.col.clamp(0, (columns - w).max(0));
                let mut row = committed.row.max(0);
                let mut candidate = CellRect { col, row, w, h };
                while grid_overlaps(&taken, candidate) {
                    row += 1;
                    candidate.row = row;
                }
                candidate
            }
            None => grid_first_fit(&taken, columns, w, h),
        };
        taken.push(cell);
        out[i] = Some(cell);
    }
    let mut cells: Vec<CellRect> = out
        .into_iter()
        .map(|cell| {
            cell.unwrap_or(CellRect {
                col: 0,
                row: 0,
                w: 1,
                h: 1,
            })
        })
        .collect();
    if gravity {
        grid_compact(&mut cells);
    }
    let rows = cells
        .iter()
        .map(|cell| cell.row + cell.h)
        .max()
        .unwrap_or(0);
    if rows > max_rows {
        return None;
    }
    Some(cells)
}
/// Serialize one tile's fields, placement and `_order`, keeping every field the
/// tile had. `enabled` is written only for profiles that carry it.
pub(crate) fn grid_tile_fields_nix(
    id: &str,
    order: i64,
    cell: CellRect,
    enabled: Option<bool>,
    fields: &[(String, String)],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (name, value) in fields {
        parts.push(format!("{} = {};", attr_key(name), value));
    }
    if let Some(active) = enabled {
        parts.push(format!("enabled = {};", active));
    }
    parts.push(format!("_order = {};", order));
    parts.push(format!(
        "col = {}; row = {}; w = {}; h = {};",
        cell.col, cell.row, cell.w, cell.h
    ));
    format!("{} = {{ {} }};", attr_key(id), parts.join(" "))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn rect(col: i64, row: i64, w: i64, h: i64) -> CellRect {
        CellRect { col, row, w, h }
    }

    #[test]
    fn repack_settles_committed_tiles_downward() {
        let tiles = vec![
            TileInput {
                cell: Some(rect(0, 0, 2, 1)),
                w: 2,
                h: 1,
            },
            TileInput {
                cell: Some(rect(0, 0, 1, 1)),
                w: 1,
                h: 1,
            },
        ];
        let cells = repack(&tiles, &[0, 1], 6, 64, false).unwrap();
        assert_eq!(cells[0], rect(0, 0, 2, 1));
        assert_eq!(cells[1], rect(0, 1, 1, 1));
    }

    #[test]
    fn repack_first_fits_unplaced_tiles() {
        let tiles = vec![
            TileInput {
                cell: Some(rect(0, 0, 3, 1)),
                w: 3,
                h: 1,
            },
            TileInput {
                cell: None,
                w: 1,
                h: 1,
            },
        ];
        let cells = repack(&tiles, &[0, 1], 6, 64, false).unwrap();
        assert_eq!(cells[1], rect(3, 0, 1, 1));
    }

    #[test]
    fn repack_clamps_width_to_columns() {
        let tiles = vec![TileInput {
            cell: Some(rect(0, 0, 99, 1)),
            w: 99,
            h: 1,
        }];
        let cells = repack(&tiles, &[0], 4, 64, false).unwrap();
        assert_eq!(cells[0].w, 4);
    }

    #[test]
    fn repack_reports_overflow_as_none() {
        let tiles = vec![
            TileInput {
                cell: Some(rect(0, 0, 1, 3)),
                w: 1,
                h: 3,
            },
            TileInput {
                cell: Some(rect(0, 0, 1, 1)),
                w: 1,
                h: 1,
            },
        ];
        assert!(repack(&tiles, &[0, 1], 1, 3, false).is_none());
    }

    #[test]
    fn compact_pulls_cells_up_to_the_top() {
        let mut cells = vec![rect(0, 5, 1, 1), rect(1, 5, 1, 1)];
        grid_compact(&mut cells);
        assert_eq!(cells[0].row, 0);
        assert_eq!(cells[1].row, 0);
    }

    #[test]
    fn compact_stops_at_a_blocker() {
        let mut cells = vec![rect(0, 0, 2, 1), rect(0, 3, 1, 1)];
        grid_compact(&mut cells);
        assert_eq!(cells[1].row, 1);
    }

    #[test]
    fn first_fit_places_below_the_stack_when_full() {
        let taken = vec![rect(0, 0, 1, 1)];
        assert_eq!(grid_first_fit(&taken, 1, 1, 1), rect(0, 1, 1, 1));
    }
    #[test]
    fn grid_tile_nix_writes_fields_then_placement() {
        let fields = vec![("footer".to_string(), "true".to_string())];
        let out = grid_tile_fields_nix("nightMode", 1000, rect(2, 1, 2, 1), Some(true), &fields);
        assert_eq!(
            out,
            "nightMode = { footer = true; enabled = true; _order = 1000; col = 2; row = 1; w = 2; h = 1; };"
        );
    }

    #[test]
    fn grid_tile_nix_omits_enabled_when_absent() {
        let out = grid_tile_fields_nix("clock", 10, rect(0, 0, 3, 2), None, &[]);
        assert_eq!(
            out,
            "clock = { _order = 10; col = 0; row = 0; w = 3; h = 2; };"
        );
    }

    #[test]
    fn grid_tile_nix_quotes_awkward_field_keys() {
        let fields = vec![("daily ".to_string(), "true".to_string())];
        let out = grid_tile_fields_nix("daily ", 1000, rect(0, 0, 1, 1), None, &fields);
        assert_eq!(
            out,
            "\"daily \" = { \"daily \" = true; _order = 1000; col = 0; row = 0; w = 1; h = 1; };"
        );
    }
}
