//! The cursor's place in the captured picture and its time on the host clock, for every OS's source.
//! Pure, built on every OS. Places are mapped with the area's bounds at that moment, so a window that moves keeps its
//! cursor (Beats Cap 6).

/// A global point (top-left origin, points) inside an area (its global top-left and size, points), 0-1. Outside the
/// area is outside 0-1; an area with no size (or a non-finite one) puts every point outside, at (−1, −1).
pub fn to_area(global: (f64, f64), area: (f64, f64, f64, f64)) -> (f32, f32) {
    let (x, y, w, h) = area;
    let sized = |v: f64| v.is_finite() && v > 0.0;
    if !(sized(w) && sized(h) && x.is_finite() && y.is_finite()) {
        return (-1.0, -1.0);
    }
    (((global.0 - x) / w) as f32, ((global.1 - y) / h) as f32)
}

/// `NSEvent.timestamp` (seconds since boot, the uptime clock) as host ns, rounded to the nearest ns. Negative or
/// non-finite: 0; past `u64::MAX` ns: saturated.
pub fn ns_seconds_to_host_ns(seconds: f64) -> u64 {
    let ns = (seconds * 1e9).round();
    if ns.is_nan() || ns <= 0.0 {
        0
    } else if ns >= u64::MAX as f64 {
        u64::MAX
    } else {
        ns as u64
    }
}
