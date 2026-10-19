use super::monitors::MonitorRow;
use gtk::prelude::*;
/// The geometry a monitor rectangle shows on the canvas. Size falls back to
/// 1920x1080 when unset; `sized` marks the fallback.
pub(crate) fn monitor_geometry(row: &MonitorRow) -> (f64, f64, f64, f64, String, bool) {
    let width = row.width.value();
    let height = row.height.value();
    let sized = width > 0.0 && height > 0.0;
    let width = if width > 0.0 { width } else { 1920.0 };
    let height = if height > 0.0 { height } else { 1080.0 };
    (
        row.x.value(),
        row.y.value(),
        width,
        height,
        row.name.text().to_string(),
        sized,
    )
}

/// The footprint a monitor occupies after its `rr` transform is applied: odd
/// transforms (90/270 degrees, flipped or not) swap width and height.
pub(crate) fn effective_size(w: f64, h: f64, rr: f64) -> (f64, f64) {
    match rr as i32 {
        1 | 3 | 5 | 7 => (h, w),
        _ => (w, h),
    }
}

pub(crate) fn monitor_orig<'a>(row: &'a MonitorRow, name: &str) -> &'a str {
    row.orig
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or("")
}

pub(crate) fn snap_axis(value: f64, size: f64, targets: &[f64], tolerance: f64) -> f64 {
    let mut best = value;
    let mut best_distance = tolerance + 1.0;
    for target in targets {
        for candidate in [value, value + size] {
            let distance = (candidate - target).abs();
            if distance <= tolerance && distance < best_distance {
                best_distance = distance;
                best = value + (target - candidate);
            }
        }
    }
    best
}

pub(crate) fn snap_monitor(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    others: &[(f64, f64, f64, f64)],
    tolerance: f64,
) -> (f64, f64) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (ox, oy, ow, oh) in others {
        xs.push(*ox);
        xs.push(ox + ow);
        ys.push(*oy);
        ys.push(oy + oh);
    }
    (
        snap_axis(x, w, &xs, tolerance),
        snap_axis(y, h, &ys, tolerance),
    )
}

/// Snap to the nearest edge, then push the rectangle out of any monitor it
/// still overlaps along the axis of smaller penetration, up to eight rounds,
/// and clamp the result to non-negative coordinates.
pub(crate) fn resolve_release(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    others: &[(f64, f64, f64, f64)],
    tolerance: f64,
) -> (f64, f64) {
    let (mut x, mut y) = snap_monitor(x, y, w, h, others, tolerance);
    for _ in 0..8 {
        let mut pushed = false;
        for &(ox, oy, ow, oh) in others {
            let overlap_x = (x + w).min(ox + ow) - x.max(ox);
            let overlap_y = (y + h).min(oy + oh) - y.max(oy);
            if overlap_x <= 0.0 || overlap_y <= 0.0 {
                continue;
            }
            if overlap_x < overlap_y {
                x = if x + w / 2.0 < ox + ow / 2.0 {
                    ox - w
                } else {
                    ox + ow
                };
            } else {
                y = if y + h / 2.0 < oy + oh / 2.0 {
                    oy - h
                } else {
                    oy + oh
                };
            }
            pushed = true;
        }
        if !pushed {
            break;
        }
    }
    (x.max(0.0), y.max(0.0))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_size_swaps_on_odd_transforms() {
        assert_eq!(effective_size(1920.0, 1080.0, 0.0), (1920.0, 1080.0));
        assert_eq!(effective_size(1920.0, 1080.0, 1.0), (1080.0, 1920.0));
        assert_eq!(effective_size(1920.0, 1080.0, 2.0), (1920.0, 1080.0));
        assert_eq!(effective_size(1920.0, 1080.0, 3.0), (1080.0, 1920.0));
        assert_eq!(effective_size(1920.0, 1080.0, 5.0), (1080.0, 1920.0));
        assert_eq!(effective_size(1920.0, 1080.0, 7.0), (1080.0, 1920.0));
    }

    fn rect_overlaps(x: f64, y: f64, w: f64, h: f64, other: &(f64, f64, f64, f64)) -> bool {
        (x + w).min(other.0 + other.2) - x.max(other.0) > 0.0
            && (y + h).min(other.1 + other.3) - y.max(other.1) > 0.0
    }

    #[test]
    fn release_snaps_to_nearby_edge() {
        let others = vec![(0.0, 0.0, 1000.0, 1000.0)];
        let (x, y) = resolve_release(1005.0, 0.0, 1000.0, 1000.0, &others, 12.0);
        assert_eq!((x, y), (1000.0, 0.0));
    }

    #[test]
    fn release_pushes_out_of_overlap() {
        let others = vec![(0.0, 0.0, 1920.0, 1080.0)];
        let (x, y) = resolve_release(100.0, 100.0, 1920.0, 1080.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1920.0, 1080.0, &others[0]));
        assert!(x >= 0.0 && y >= 0.0);
    }

    #[test]
    fn release_never_returns_negative_coordinates() {
        let others = vec![(0.0, 0.0, 1920.0, 1080.0)];
        let (x, y) = resolve_release(0.0, 0.0, 1920.0, 1080.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1920.0, 1080.0, &others[0]));
        assert!(x >= 0.0 && y >= 0.0);
    }

    #[test]
    fn release_resolves_chained_overlaps() {
        let others = vec![(0.0, 0.0, 1000.0, 1000.0), (1000.0, 0.0, 1000.0, 1000.0)];
        let (x, y) = resolve_release(500.0, 500.0, 1000.0, 1000.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1000.0, 1000.0, &others[0]));
        assert!(!rect_overlaps(x, y, 1000.0, 1000.0, &others[1]));
        assert!(x >= 0.0 && y >= 0.0);
    }
}
