use std::time::Instant;

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
