//! A path's own bounds: the tight box around its lines and curves (a curve's extremes, not its control points), as
//! SVG's `getBBox`. A path without a `viewBox` fits this box into its node's, and so does a motion path, so a dot riding
//! a path drawn beside it stays on it.

use super::PathCmd;

/// `[x, y, width, height]` around every point the commands draw through; None for no commands.
pub fn bounds(cmds: &[PathCmd]) -> Option<[f64; 4]> {
    let mut b = Bounds::default();
    let (mut at, mut start) = ((0.0, 0.0), (0.0, 0.0));
    for &cmd in cmds {
        match cmd {
            PathCmd::Move { x, y } => {
                (at, start) = ((x, y), (x, y));
                b.add(at);
            }
            PathCmd::Line { x, y } => {
                at = (x, y);
                b.add(at);
            }
            PathCmd::Quad { x1, y1, x, y } => {
                for t in [quad_extreme(at.0, x1, x), quad_extreme(at.1, y1, y)]
                    .into_iter()
                    .flatten()
                {
                    b.add(quad_at(at, (x1, y1), (x, y), t));
                }
                at = (x, y);
                b.add(at);
            }
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (c1, c2, end) = ((x1, y1), (x2, y2), (x, y));
                let ts = cubic_extremes(at.0, x1, x2, x)
                    .into_iter()
                    .chain(cubic_extremes(at.1, y1, y2, y));
                for t in ts.flatten() {
                    b.add(cubic_at(at, c1, c2, end, t));
                }
                at = end;
                b.add(at);
            }
            PathCmd::Close => at = start,
        }
    }
    b.rect()
}

#[derive(Default)]
struct Bounds(Option<[f64; 4]>);

impl Bounds {
    fn add(&mut self, (x, y): (f64, f64)) {
        let b = self.0.get_or_insert([x, y, x, y]);
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }

    fn rect(&self) -> Option<[f64; 4]> {
        self.0.map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0, y1 - y0])
    }
}

/// Where a quadratic's coordinate `p0 → p1 → p2` turns, inside the curve.
fn quad_extreme(p0: f64, p1: f64, p2: f64) -> Option<f64> {
    let den = p0 - 2.0 * p1 + p2;
    if den.abs() < 1e-12 {
        return None;
    }
    let t = (p0 - p1) / den;
    (t > 0.0 && t < 1.0).then_some(t)
}

/// Where a cubic's coordinate turns (its derivative's roots), inside the curve.
fn cubic_extremes(p0: f64, p1: f64, p2: f64, p3: f64) -> [Option<f64>; 2] {
    // B'(t)/3 = a t² + b t + c.
    let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let b = 2.0 * (p0 - 2.0 * p1 + p2);
    let c = p1 - p0;
    let inside = |t: f64| (t > 0.0 && t < 1.0).then_some(t);
    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return [None, None];
        }
        return [inside(-c / b), None];
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return [None, None];
    }
    let root = disc.sqrt();
    [
        inside((-b + root) / (2.0 * a)),
        inside((-b - root) / (2.0 * a)),
    ]
}

pub(crate) fn quad_at(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let f = |a: f64, b: f64, c: f64| u * u * a + 2.0 * u * t * b + t * t * c;
    (f(p0.0, p1.0, p2.0), f(p0.1, p1.1, p2.1))
}

pub(crate) fn cubic_at(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    t: f64,
) -> (f64, f64) {
    let u = 1.0 - t;
    let f = |a: f64, b: f64, c: f64, d: f64| {
        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
    };
    (f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1))
}
