use std::time::{Duration, Instant};

/// Reload timings are printed only when `NSG_TIMING` is set.
fn enabled() -> bool {
    std::env::var_os("NSG_TIMING").is_some()
}

pub fn start() -> Instant {
    Instant::now()
}

pub fn report(label: &str, start: Instant) {
    if enabled() {
        eprintln!("[nsg-timing] {label}: {:?}", start.elapsed());
    }
}

/// Report only work that takes at least `threshold`, to find hot spots without
/// printing a line for every control.
pub fn report_slow(label: &str, start: Instant, threshold: Duration) {
    if enabled() {
        let elapsed = start.elapsed();
        if elapsed >= threshold {
            eprintln!("[nsg-timing] {label}: {elapsed:?}");
        }
    }
}
