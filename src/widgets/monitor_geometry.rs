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

pub(crate) const SNAP_TOLERANCE: f64 = 12.0;

/// The one shared geometry function: x/y stay physical, the size becomes the
/// virtual footprint (`effective_size / row.scale`). Every consumer calls this
/// instead of pairing `monitor_geometry` + `effective_size` by hand.
pub(crate) fn row_footprint(row: &MonitorRow) -> (f64, f64, f64, f64, String, bool) {
    let (x, y, w, h, label, sized) = monitor_geometry(row);
    let (ew, eh) = effective_size(w, h, row.rr.current_choice());
    let scale = row.scale.value().max(0.01);
    (x, y, ew / scale, eh / scale, label, sized)
}

pub(crate) const VIEW_PAD: f64 = 16.0;
const MIN_SCREEN_W: f64 = 24.0;
const MIN_SCREEN_H: f64 = 16.0;

#[derive(Clone, Copy)]
pub(crate) struct Viewport {
    pub(crate) scale: f64,
    pub(crate) origin_x: f64,
    pub(crate) origin_y: f64,
}

impl Viewport {
    pub(crate) fn fit(
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
        canvas_w: f64,
        canvas_h: f64,
    ) -> Self {
        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let scale = f64::min(
            (canvas_w - 2.0 * VIEW_PAD) / span_x,
            (canvas_h - 2.0 * VIEW_PAD) / span_y,
        )
        .max(0.001);
        Viewport {
            scale,
            origin_x: min_x,
            origin_y: min_y,
        }
    }
    pub(crate) fn to_screen_x(&self, x: f64) -> f64 {
        VIEW_PAD + (x - self.origin_x) * self.scale
    }
    pub(crate) fn to_screen_y(&self, y: f64) -> f64 {
        VIEW_PAD + (y - self.origin_y) * self.scale
    }
    pub(crate) fn to_config_x(&self, px: f64) -> f64 {
        self.origin_x + (px - VIEW_PAD) / self.scale
    }
    pub(crate) fn to_config_y(&self, py: f64) -> f64 {
        self.origin_y + (py - VIEW_PAD) / self.scale
    }
    pub(crate) fn screen_size(&self, w: f64, h: f64) -> (f64, f64) {
        (
            (w * self.scale).max(MIN_SCREEN_W),
            (h * self.scale).max(MIN_SCREEN_H),
        )
    }
    pub(crate) fn rect_on_screen(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        let (sw, sh) = self.screen_size(w, h);
        (self.to_screen_x(x), self.to_screen_y(y), sw, sh)
    }
}

pub(crate) fn scene_bounds(rows: &[MonitorRow]) -> Option<(f64, f64, f64, f64)> {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for row in rows {
        let (x, y, w, h, _, _) = row_footprint(row);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }
    if min_x == f64::MAX {
        None
    } else {
        Some((min_x, min_y, max_x, max_y))
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
/// still overlaps along the axis of smaller penetration, up to eight rounds.
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
    (x, y)
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

    #[test]
    fn viewport_round_trips_screen_and_config() {
        let v = Viewport {
            scale: 0.5,
            origin_x: -100.0,
            origin_y: 50.0,
        };
        let px = v.to_screen_x(300.0);
        assert_eq!(v.to_config_x(px), 300.0);
        let py = v.to_screen_y(-40.0);
        assert_eq!(v.to_config_y(py), -40.0);
    }

    #[test]
    fn viewport_rect_applies_min_screen_size() {
        let v = Viewport {
            scale: 0.01,
            origin_x: 0.0,
            origin_y: 0.0,
        };
        let (_, _, w, h) = v.rect_on_screen(0.0, 0.0, 100.0, 100.0);
        assert_eq!(w, 24.0);
        assert_eq!(h, 16.0);
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
    }

    #[test]
    fn release_allows_negative_coordinates() {
        let others = vec![(0.0, 0.0, 1920.0, 1080.0)];
        let (x, y) = resolve_release(-20.0, -30.0, 1920.0, 1080.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1920.0, 1080.0, &others[0]));
    }

    #[test]
    fn release_resolves_chained_overlaps() {
        let others = vec![(0.0, 0.0, 1000.0, 1000.0), (1000.0, 0.0, 1000.0, 1000.0)];
        let (x, y) = resolve_release(500.0, 500.0, 1000.0, 1000.0, &others, 12.0);
        assert!(!rect_overlaps(x, y, 1000.0, 1000.0, &others[0]));
        assert!(!rect_overlaps(x, y, 1000.0, 1000.0, &others[1]));
    }
}
