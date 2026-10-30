//! An SVG's paint as the scene's: a colour with its opacity as `#RRGGBBAA`, a gradient mapped from its
//! own space through the path's transform into fractions of the view (a linear one exactly, a radial one's circle by
//! the view's mean side, its focus and spread dropped), at most 16 stops. Patterns are refused.

use klipolo_model::scene::{Gradient, Paint, SceneProblem};
use usvg::tiny_skia_path::Transform;

use super::scan::unsupported;

/// `#RRGGBBAA`.
fn hex(c: usvg::Color, alpha: f32) -> String {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}{a:02X}", c.red, c.green, c.blue)
}

/// A paint at `opacity`, its gradient's space mapped by the path's transform `t` into fractions of `view`.
pub(super) fn paint(
    p: &usvg::Paint,
    opacity: f32,
    t: &Transform,
    view: [f64; 4],
) -> Result<Option<Paint>, Vec<SceneProblem>> {
    let (w, h) = (view[2].max(1e-9), view[3].max(1e-9));
    let (space, stops, linear, radial) = match p {
        usvg::Paint::Color(c) => return Ok(Some(Paint::Color(hex(*c, opacity)))),
        usvg::Paint::Pattern(_) => {
            return Err(unsupported("a pattern fill", "expand it into shapes"));
        }
        usvg::Paint::LinearGradient(g) => (
            g.transform(),
            g.stops(),
            Some((g.x1(), g.y1(), g.x2(), g.y2())),
            None,
        ),
        usvg::Paint::RadialGradient(g) => (
            g.transform(),
            g.stops(),
            None,
            Some((g.cx(), g.cy(), g.r().get())),
        ),
    };
    let m = t.pre_concat(space);
    let stops = self::stops(stops, opacity);
    if stops.len() == 1 {
        return Ok(Some(Paint::Color(stops[0].1.clone())));
    }
    let (a, b, c, d) = (
        f64::from(m.sx),
        f64::from(m.ky),
        f64::from(m.kx),
        f64::from(m.sy),
    );
    let (tx, ty) = (f64::from(m.tx), f64::from(m.ty));
    let det = a * d - b * c;
    if stops.is_empty() || !det.is_finite() || det.abs() < 1e-12 {
        // A gradient that flattens to nothing paints nothing (not the scene's default black).
        return Ok(Some(Paint::Color("none".into())));
    }
    let mut g = Gradient {
        linear: None,
        radial: None,
        stops,
        angle: None,
        offset: None,
    };
    if let Some((x1, y1, x2, y2)) = linear {
        // The gradient's value at a point is affine in it: s(q) = q · gv + k on the canvas, then in the view's
        // fractions; the scene's gradient runs from where s is 0 to where it is 1 along that direction.
        let (dx, dy) = (f64::from(x2 - x1), f64::from(y2 - y1));
        let dd = (dx * dx + dy * dy).max(1e-12);
        // gv = A⁻ᵀ D / |D|², with A = [a c; b d].
        let gv = ((d * dx - b * dy) / det / dd, (-c * dx + a * dy) / det / dd);
        let k = -(tx * gv.0 + ty * gv.1) - (f64::from(x1) * dx + f64::from(y1) * dy) / dd;
        // In the view's fractions: s = uv · u + K.
        let (ux, uy) = (gv.0 * w, gv.1 * h);
        let uu = (ux * ux + uy * uy).max(1e-12);
        let big_k = k + view[0] * gv.0 + view[1] * gv.1;
        let start = (-big_k * ux / uu, -big_k * uy / uu);
        g.linear = Some([start.0, start.1, start.0 + ux / uu, start.1 + uy / uu]);
    } else if let Some((cx, cy, r)) = radial {
        let (px, py) = (
            a * f64::from(cx) + c * f64::from(cy) + tx,
            b * f64::from(cx) + d * f64::from(cy) + ty,
        );
        let radius = f64::from(r) * det.abs().sqrt();
        g.radial = Some([
            (px - view[0]) / w,
            (py - view[1]) / h,
            radius / (w * h).sqrt(),
        ]);
    }
    Ok(Some(Paint::Gradient(g)))
}

/// A gradient's stops as the scene's: at most 16 (spread evenly over the SVG's), each colour at its opacity.
fn stops(s: &[usvg::Stop], opacity: f32) -> Vec<(f64, String)> {
    let n = s.len();
    let pick: Vec<usize> = if n <= 16 {
        (0..n).collect()
    } else {
        (0..16).map(|i| i * (n - 1) / 15).collect()
    };
    pick.into_iter()
        .map(|i| {
            let st = &s[i];
            (
                f64::from(st.offset().get()),
                hex(st.color(), st.opacity().get() * opacity),
            )
        })
        .collect()
}
