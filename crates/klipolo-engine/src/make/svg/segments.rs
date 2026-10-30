//! The segments usvg makes of a shape, counted from above before it runs. An arc is
//! not 4 curves: svgtypes (a path's `A`) and usvg (a circle's, an ellipse's and a rounded rect's quarter arcs) hand
//! each to kurbo's `Arc::to_cubic_beziers(0.1)`, which makes ⌈n_err · |sweep| / 2π⌉ cubics, n_err =
//! max((1.1163 · r / 0.1)^⅙, 4) for r the larger radius once scaled up to reach its end point. So each arc is weighed
//! by its radius, kurbo's own way, with the current point tracked as svgtypes tracks it; one that would make more than
//! [`MAX_ARC_SEGMENTS`] cubics (a radius in the billions, far past any drawing) is refused, for svgtypes drains an arc's
//! cubics from the front of a `Vec`: an arc costs the square of its count.

use std::f64::consts::TAU;

use usvg::roxmltree::Node;

/// kurbo's tolerance as usvg and svgtypes call it.
const TOLERANCE: f64 = 0.1;
/// The most cubics one arc may become.
pub const MAX_ARC_SEGMENTS: f64 = 64.0;

/// kurbo's count for an arc of larger radius `r` over `turn` of a full turn, from above (its `ceil` of a product
/// rounding can only take up); None past [`MAX_ARC_SEGMENTS`].
fn kurbo_count(r: f64, turn: f64) -> Option<f64> {
    let n_err = (1.1163 * r / TOLERANCE).powf(1.0 / 6.0).max(3.999_999);
    let n = (n_err * turn).floor() + 1.0;
    (n.is_finite() && n <= MAX_ARC_SEGMENTS).then_some(n)
}

/// The cubics kurbo makes of an SVG arc from `from` to `to` with radii `r`, its x axis turned `degrees`, the large one
/// or not; a line when kurbo makes one. None past [`MAX_ARC_SEGMENTS`].
pub fn arc(
    from: (f64, f64),
    to: (f64, f64),
    r: (f64, f64),
    degrees: f64,
    large: bool,
) -> Option<f64> {
    let (mut rx, mut ry) = (r.0.abs(), r.1.abs());
    if rx <= 1e-5 || ry <= 1e-5 || from == to {
        return Some(1.0);
    }
    let (sin, cos) = (degrees.to_radians() % TAU).sin_cos();
    let (hx, hy) = ((from.0 - to.0) * 0.5, (from.1 - to.1) * 0.5);
    let (px, py) = (cos * hx + sin * hy, -sin * hx + cos * hy);
    let reach = px * px / (rx * rx) + py * py / (ry * ry);
    // Radii too small to reach scale up (SVG's F.6.6): the arc is then a half turn either way.
    let mut turn = if large { 1.0 } else { 0.5 };
    if reach > 1.0 {
        let k = reach.sqrt();
        (rx, ry, turn) = (rx * k, ry * k, 0.5);
    }
    kurbo_count(rx.max(ry), turn)
}

/// A quarter of an ellipse of radii up to `r` (usvg draws circles, ellipses and rounded corners in quarters).
pub fn quarter(r: f64) -> Option<f64> {
    if r <= 1e-5 {
        return Some(1.0);
    }
    kurbo_count(r, 0.25)
}

fn arity(c: u8) -> usize {
    match c {
        b'M' | b'L' | b'T' => 2,
        b'H' | b'V' => 1,
        b'C' => 6,
        b'S' | b'Q' => 4,
        b'A' => 7,
        _ => 0,
    }
}

/// The number at `b[*i..]` (sign, digits, one point, exponent), moving `i` past it; None when there is none.
fn number(b: &[u8], i: &mut usize) -> Option<f64> {
    let start = *i;
    let sign = usize::from(matches!(b.get(*i), Some(b'+' | b'-')));
    *i += sign;
    let mut dot = false;
    while *i < b.len() && (b[*i].is_ascii_digit() || (b[*i] == b'.' && !dot)) {
        dot |= b[*i] == b'.';
        *i += 1;
    }
    let digits = b[start + sign..*i].iter().any(u8::is_ascii_digit);
    if digits && *i < b.len() && matches!(b[*i], b'e' | b'E') {
        let at = *i;
        *i += 1;
        *i += usize::from(*i < b.len() && matches!(b[*i], b'+' | b'-'));
        let exp = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        if *i == exp {
            *i = at;
        }
    }
    if !digits {
        *i = start;
        return None;
    }
    std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()
}

/// The segments usvg makes of path data `d`, from above: a `Z` as two (svgtypes may open a path after it), an arc as
/// kurbo makes it; None when an arc is past [`MAX_ARC_SEGMENTS`]. Where svgtypes stops at an error, this goes on.
pub fn of_path(d: &str) -> Option<f64> {
    let b = d.as_bytes();
    let (mut i, mut count) = (0, 0.0);
    let (mut cmd, mut rel, mut k) = (b'M', false, 0usize);
    let mut args = [0.0f64; 7];
    let (mut x, mut y, mut sx, mut sy) = (0.0, 0.0, 0.0, 0.0);
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() || c == b',' {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            (cmd, rel, k) = (c.to_ascii_uppercase(), c.is_ascii_lowercase(), 0);
            if cmd == b'Z' {
                count += 2.0;
                (x, y) = (sx, sy);
            }
            i += 1;
            continue;
        }
        let v = if cmd == b'A' && matches!(k, 3 | 4) && matches!(c, b'0' | b'1') {
            i += 1;
            f64::from(c - b'0')
        } else if let Some(v) = number(b, &mut i) {
            v
        } else {
            i += 1;
            continue;
        };
        let n = arity(cmd);
        if n == 0 {
            continue;
        }
        args[k] = v;
        k += 1;
        if k < n {
            continue;
        }
        k = 0;
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        let end = match cmd {
            b'M' | b'L' | b'T' => (ox + args[0], oy + args[1]),
            b'H' => (ox + args[0], y),
            b'V' => (x, oy + args[0]),
            b'C' => (ox + args[4], oy + args[5]),
            b'S' | b'Q' => (ox + args[2], oy + args[3]),
            _ => (ox + args[5], oy + args[6]),
        };
        count += if cmd == b'A' {
            arc((x, y), end, (args[0], args[1]), args[2], args[3] != 0.0)?
        } else {
            1.0
        };
        (x, y) = end;
        if cmd == b'M' {
            (sx, sy, cmd) = (x, y, b'L');
        }
    }
    Some(count)
}

/// A length attribute's user units as usvg converts it, from above: a percentage of `view` (the largest viewport side
/// in the file), absolute units at 96 dpi; None for em and ex (a font size can be anything) and past any drawing.
pub fn length(v: &str, view: f64) -> Option<f64> {
    let b = v.trim().as_bytes();
    let mut i = 0;
    let Some(n) = number(b, &mut i) else {
        return Some(0.0);
    };
    let unit = std::str::from_utf8(&b[i..]).ok()?.trim();
    let factor = match unit {
        "" | "px" => 1.0,
        "%" => view / 100.0,
        "em" | "ex" => return None,
        "in" | "cm" | "mm" | "pt" | "pc" => 96.0,
        // svgtypes refuses it and usvg takes 0; weighed as the largest unit all the same.
        _ => 96.0,
    };
    let r = (n * factor).abs();
    r.is_finite().then_some(r)
}

/// Whether `v` holds a percentage: a number (or its point) followed by `%`.
pub fn percent(v: &str) -> bool {
    v.as_bytes()
        .windows(2)
        .any(|w| w[1] == b'%' && (w[0].is_ascii_digit() || w[0] == b'.'))
}

/// The sides `n` gives a viewport, from above: its `viewBox`, and the `width` and `height` of an `<svg>`, a `<symbol>`
/// or a `<use>` (which sizes the symbol or svg it shows). The largest absolute side (a size in em
/// or ex counted as unbounded), and the largest percentage as a factor of the viewport around it (past 1, every
/// percentage length in the file is refused).
pub fn viewport(n: Node) -> (f64, f64) {
    let (mut side, mut grow) = (0.0f64, 0.0f64);
    if let Some(vb) = n.attribute("viewBox") {
        for v in vb.split([' ', ',']).filter_map(|v| v.parse::<f64>().ok()) {
            side = side.max(v.abs());
        }
    }
    if matches!(n.tag_name().name(), "svg" | "symbol" | "use") {
        for v in ["width", "height"]
            .into_iter()
            .filter_map(|a| n.attribute(a))
        {
            if v.trim_end().ends_with('%') {
                grow = grow.max(length(v, 1.0).unwrap_or(f64::MAX));
            } else {
                side = side.max(length(v, 0.0).unwrap_or(f64::MAX));
            }
        }
    }
    let finite = |v: f64| if v.is_finite() { v } else { f64::MAX };
    (finite(side), finite(grow))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counted from above as usvg makes them: lines and curves one each, a `Z` two, arcs by kurbo's count (a small one
    /// as at most 2, a full turn of radius 10⁶ as 15, radii scaled up to reach), implicit repeats and packed flags.
    #[test]
    fn segments_are_counted_as_usvg_will_make_them() {
        assert_eq!(of_path("M0 0 L1 1 2 2 Z"), Some(5.0));
        assert_eq!(of_path("M0,0h1h1h1v-3z"), Some(7.0));
        assert_eq!(of_path("M0 0 C1 1 2 2 3 3 4 4 5 5 6 6"), Some(3.0));
        assert_eq!(of_path("M.5.5l1e2-3"), Some(2.0));
        assert_eq!(of_path("M0 0 a1 1 0 011 1"), Some(3.0));
        assert_eq!(of_path("M0 0 A1e6 1e6 0 1 1 1 0"), Some(16.0));
        // Radii 10⁻⁴ and 10⁹ can't reach 4·10⁶ away: scaled up 2·10¹⁰ times, a half turn of radius 2·10¹⁹.
        assert_eq!(arc((0.0, 0.0), (4e6, 0.0), (1e-4, 1e9), 0.0, false), None);
        assert_eq!(of_path("M0 0 A4e26 4e26 0 1 1 1 0"), None);
        assert_eq!(quarter(50.0), Some(1.0));
        assert_eq!(quarter(4e20), None);
        assert_eq!(length("20%", 10.0), Some(2.0));
        assert_eq!(length(" 2in ", 0.0), Some(192.0));
        assert_eq!(length("2em", 0.0), None);
        assert_eq!(length("x", 0.0), Some(0.0));
        assert!(percent("50%") && percent(" .5% ") && percent("1e2%") && percent("1 2%"));
        assert!(!percent("50") && !percent("%") && !percent("url(#a%b)"));
    }
}
