//! The spring ease: a mass on a spring with a damper, released from rest one unit from its
//! target, in closed form, and stretched so the key's span is the time it takes to settle. The curves are the
//! textbook unit-step responses of a second-order system (under-, critically and overdamped), derived here.
//!
//! In units of the natural frequency (u = ω₀τ, ω₀ = √(k/m)) the response depends only on the damping ratio
//! ζ = c / (2√(km)), and so does the settle time; stiffness and mass shape the curve only through ζ.

/// The smallest damping ratio a spring may have: at ζ = 0.05 the first overshoot is e^(−πζ/√(1−ζ²)) ≈ 0.855, so
/// the normalised curve peaks near 1.86, inside `EASE_Y`.
pub const MIN_SPRING_ZETA: f64 = 0.05;

/// The values a spring's stiffness, damping and mass may take (validation).
pub(crate) const STIFFNESS: (f64, f64) = (1.0, 1000.0);
pub(crate) const DAMPING: (f64, f64) = (0.0, 100.0);
pub(crate) const MASS: (f64, f64) = (0.1, 10.0);

/// The most damped a valid spring is (damping 100, stiffness 1, mass 0.1: ζ ≈ 158); an unchecked one is drawn as if
/// it were this, so its curve stays finite.
const MAX_DRAWN_ZETA: f64 = 160.0;

/// Settled: within 0.1 % of the target, ln 1000 time constants of the slowest decay.
const SETTLE_LN: f64 = 3.0 * std::f64::consts::LN_10;

/// The damping ratio, ζ = c / (2√(km)).
pub(crate) fn ratio(k: f64, c: f64, m: f64) -> f64 {
    c / (2.0 * (k * m).sqrt())
}

/// Whether stiffness, damping and mass are each in range (all finite).
pub(crate) fn in_range(k: f64, c: f64, m: f64) -> bool {
    let within = |(lo, hi): (f64, f64), v: f64| (lo..=hi).contains(&v);
    within(STIFFNESS, k) && within(DAMPING, c) && within(MASS, m)
}

/// Whether Klipolo draws this spring: in range, and ζ at least [`MIN_SPRING_ZETA`].
pub(crate) fn is_valid(k: f64, c: f64, m: f64) -> bool {
    in_range(k, c, m) && ratio(k, c, m) >= MIN_SPRING_ZETA
}

/// The least damping that gives a stiffness and mass the smallest damping ratio.
pub(crate) fn least_damping(k: f64, m: f64) -> f64 {
    2.0 * MIN_SPRING_ZETA * (k * m).sqrt()
}

/// The ζ a curve is drawn with: never below the floor (validation refuses less; this keeps an unchecked ease
/// bounded too), never above the most a valid spring has, and the floor for nonsense.
fn drawn(k: f64, c: f64, m: f64) -> f64 {
    let zeta = ratio(k, c, m);
    if zeta.is_nan() {
        MIN_SPRING_ZETA
    } else {
        zeta.clamp(MIN_SPRING_ZETA, MAX_DRAWN_ZETA)
    }
}

/// x(u) of the unit step response from rest, u = ω₀τ.
fn step(zeta: f64, u: f64) -> f64 {
    if zeta < 1.0 {
        let wd = (1.0 - zeta * zeta).sqrt();
        1.0 - (-zeta * u).exp() * ((wd * u).cos() + zeta / wd * (wd * u).sin())
    } else if zeta == 1.0 {
        1.0 - (-u).exp() * (1.0 + u)
    } else {
        // The roots of s² + 2ζs + 1: a slow one, −1/(ζ + r), written so it doesn't cancel, and a fast one.
        let r = (zeta * zeta - 1.0).sqrt();
        let (a, b) = (-1.0 / (zeta + r), -(zeta + r));
        1.0 - (b * (a * u).exp() - a * (b * u).exp()) / (b - a)
    }
}

/// How long, in u, the response takes to settle: an underdamped or critical spring's envelope decays at ζ, an
/// overdamped one's slow root at 1/(ζ + √(ζ²−1)).
fn settle(zeta: f64) -> f64 {
    if zeta <= 1.0 {
        SETTLE_LN / zeta
    } else {
        SETTLE_LN * (zeta + (zeta * zeta - 1.0).sqrt())
    }
}

/// The spring ease at `t` (0–1): the key's span is the spring's settle time (to 0.1 %), so the motion ends at the
/// key, and the curve is divided by its value there so 1 → 1 exactly.
pub(super) fn spring(k: f64, c: f64, m: f64, t: f64) -> f64 {
    if t >= 1.0 {
        return 1.0;
    }
    let zeta = drawn(k, c, m);
    let end = settle(zeta);
    step(zeta, t * end) / step(zeta, end)
}

/// The largest value the spring ease reaches over 0–1: its first swing, when that comes before it settles, else 1
/// (it rises all the way). Under 1.86 for every valid spring; an effect target is sized for it.
pub fn spring_peak(stiffness: f64, damping: f64, mass: f64) -> f64 {
    let zeta = drawn(stiffness, damping, mass);
    if zeta >= 1.0 {
        return 1.0;
    }
    let end = settle(zeta);
    let first = std::f64::consts::PI / (1.0 - zeta * zeta).sqrt();
    if first >= end {
        return 1.0;
    }
    (step(zeta, first) / step(zeta, end)).max(1.0)
}
