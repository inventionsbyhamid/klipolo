//! The camera's ease: the quintic 6u⁵ − 15u⁴ + 10u³ (the crate's `smootherstep`) on the centre
//! and on ln(scale) together, so a move starts and ends with no speed and no acceleration; and the peak speeds a move
//! reaches, from which `legal::need` takes the least time a move may take.

use super::View;
use crate::ease::smootherstep;

/// The quintic's peak slope (at u = ½): a move's peak zoom rate is |Δ ln(scale)| × this ÷ its duration.
pub(crate) const PEAK: f64 = 1.875;

/// The view `u` (0–1) of the way through an eased move from `a` to `b`.
pub(crate) fn between(a: View, b: View, u: f64) -> View {
    let e = smootherstep(u);
    if e <= 0.0 {
        return a;
    }
    if e >= 1.0 {
        return b;
    }
    let (la, lb) = (a.scale.ln(), b.scale.ln());
    View {
        cx: a.cx + (b.cx - a.cx) * e,
        cy: a.cy + (b.cy - a.cy) * e,
        scale: (la + (lb - la) * e).exp(),
    }
}

/// The peak of E'(u) · s(u) over a move from scale `s` to s·e^`delta`, where s(u) = s·e^(delta·E(u)): its centre's
/// peak speed in view widths a second, per unit of (distance ÷ duration). The pan limit is in view widths at the scale
/// of the moment, so a pan that also zooms in must be slower near its end than one at its starting scale would be.
///
/// The peak is where 1 − 2u = −15·delta·p³ with p = u(1 − u). Squared: 225·delta²·p⁶ + 4p − 1 = 0, increasing in p,
/// with one root in (0, ¼], so there is one peak; Newton from ¼ walks down onto it (the polynomial is convex). Then
/// E'(u) = 30p², and u is on the side the zoom heads (past ½ zooming in).
pub(crate) fn pan_peak(s: f64, delta: f64) -> f64 {
    let a = 225.0 * delta * delta;
    let mut p: f64 = 0.25;
    for _ in 0..64 {
        let step = (a * p.powi(6) + 4.0 * p - 1.0) / (6.0 * a * p.powi(5) + 4.0);
        p -= step;
        if step.abs() < 1e-15 {
            break;
        }
    }
    let half = (1.0 - 4.0 * p).max(0.0).sqrt() / 2.0;
    let u = if delta >= 0.0 { 0.5 + half } else { 0.5 - half };
    s * 30.0 * p * p * (delta * smootherstep(u)).exp()
}
