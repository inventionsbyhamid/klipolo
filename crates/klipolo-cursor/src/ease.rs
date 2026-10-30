//! The easing the drawn cursor shares: its smoothing's eases onto the raw path, its fades, and the
//! engine's press squish and click ripples.

/// 0 at or below 0, 1 at or above 1, with no slope or curvature at either end (6s⁵ − 15s⁴ + 10s³).
pub fn smootherstep(s: f64) -> f64 {
    let s = s.clamp(0.0, 1.0);
    s * s * s * (s * (s * 6.0 - 15.0) + 10.0)
}

/// 0 at or below 0, 1 at or above 1: fast at first, settling at the end (1 − (1 − s)³): a ripple's growth.
pub fn ease_out(s: f64) -> f64 {
    let rest = 1.0 - s.clamp(0.0, 1.0);
    1.0 - rest * rest * rest
}
