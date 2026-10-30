//! One range per number property, shared by its static value (`common`), its keys and the tweens that animate it
//! (`keyed`), so an animated blur, size or perspective is held to what a static one is.
//! Nested properties are named as they animate (`trim.end`, `shadow.blur`). Positions without a unit are fractions
//! of the box; with `"unit": "px"` or written `"22px"` they are pixels, within [`MAX_PX`](super::super::units::MAX_PX).

/// The values `name` may take, if it is a ranged number.
pub(in crate::scene) fn range_of(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "x" | "y" => (-10.0, 10.0),
        "scale" | "scaleX" | "scaleY" => (-100.0, 100.0),
        "rotate" | "fill.angle" => (-36_000.0, 36_000.0),
        "rotateX" | "rotateY" => (-360.0, 360.0),
        "perspective" => (1.0, 100_000.0),
        "opacity" | "innerRadius" | "trim.start" | "trim.end" | "motionPath.progress" => (0.0, 1.0),
        "blur" | "shadow.blur" => (0.0, 256.0),
        "strokeWidth" => (0.0, 1000.0),
        "dashOffset" => (-100_000.0, 100_000.0),
        "trim.offset" | "fill.offset" => (-100.0, 100.0),
        "radius" => (0.0, 10_000.0),
        "shadow.x" | "shadow.y" => (-10_000.0, 10_000.0),
        "shadow.spread" | "letterSpacing" => (-1000.0, 1000.0),
        "size" => (0.5, 2000.0),
        "weight" => (1.0, 1000.0),
        "lineHeight" => (0.1, 10.0),
        "wrap" => (0.0, 10.0),
        _ => return None,
    })
}

/// [`range_of`], or every finite number for a property without a range.
pub(super) fn limits(name: &str) -> (f64, f64) {
    range_of(name).unwrap_or((-f64::MAX, f64::MAX))
}
