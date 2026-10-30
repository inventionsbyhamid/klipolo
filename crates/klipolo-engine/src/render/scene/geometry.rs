//! What a node drawn as a path is made of: its commands in its mesh's own units, and how those units fit
//! its box. A `path`'s `d` is in its own units, its `viewBox` (else its own bounds) fitted into the box, `xMidYMid meet`
//! unless `none`, so the same scene draws the same in 16:9 and 9:16. A `polyline`'s points and the outline
//! of a trimmed or dashed rect, ellipse, star, polygon or line are in the box's own pixels (centred on it).

use klipolo_model::scene::path::{PathCmd, bounds};

/// How mesh units become the node's centred box pixels: `x' = sx x + tx`, `y' = sy y + ty`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Fit {
    pub sx: f64,
    pub sy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Fit {
    pub const SAME: Fit = Fit {
        sx: 1.0,
        sy: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// `[x, y, w, h]` of path units filling a `w` × `h` box, whole and centred (`meet`) or stretched to it.
    pub fn of(view_box: [f64; 4], meet: bool, w: f64, h: f64) -> Fit {
        let [vx, vy, vw, vh] = view_box;
        let scale = |side: f64, v: f64| if v > 0.0 { side / v } else { f64::INFINITY };
        let (mut sx, mut sy) = (scale(w, vw), scale(h, vh));
        if meet {
            (sx, sy) = (sx.min(sy), sx.min(sy));
        }
        // A level line has no height and a point no size: the other side's scale, else 1.
        if !sx.is_finite() {
            sx = if sy.is_finite() { sy } else { 1.0 };
        }
        if !sy.is_finite() {
            sy = sx;
        }
        Fit {
            sx,
            sy,
            tx: -(vx + vw / 2.0) * sx,
            ty: -(vy + vh / 2.0) * sy,
        }
    }

    /// Mesh units → centred box pixels.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.sx * x + self.tx, self.sy * y + self.ty)
    }

    /// A length's pixels per mesh unit, whichever way it runs (their geometric mean).
    pub fn scale(&self) -> f64 {
        (self.sx * self.sy).abs().sqrt()
    }
}

/// A path's view of its units: the box they fill (`[x, y, w, h]`, its `viewBox` or its own bounds).
pub(super) fn view_box(cmds: &[PathCmd], written: Option<[f64; 4]>) -> [f64; 4] {
    written
        .or_else(|| bounds(cmds))
        .unwrap_or([0.0, 0.0, 1.0, 1.0])
}

/// FNV-1a over the commands' numbers (and a tag for what made them): the mesh cache's geometry key.
pub(super) fn hash(tag: u8, cmds: &[PathCmd]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ u64::from(tag);
    for c in cmds {
        let numbers: [f64; 7] = match *c {
            PathCmd::Move { x, y } => [0.0, x, y, 0.0, 0.0, 0.0, 0.0],
            PathCmd::Line { x, y } => [1.0, x, y, 0.0, 0.0, 0.0, 0.0],
            PathCmd::Close => [2.0; 7],
            PathCmd::Quad { x1, y1, x, y } => [3.0, x1, y1, x, y, 0.0, 0.0],
            PathCmd::Cubic {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => [4.0, x1, y1, x2, y2, x, y],
        };
        for v in numbers {
            for b in v.to_bits().to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    h
}

/// A polyline through `points` (fractions of a `w` × `h` box) in the box's centred pixels, smoothed into cubics
/// through every point (Catmull–Rom) when asked.
pub(super) fn polyline(points: &[[f64; 2]], w: f64, h: f64, smooth: bool) -> Vec<PathCmd> {
    let p: Vec<(f64, f64)> = points
        .iter()
        .map(|&[x, y]| (x * w - w / 2.0, y * h - h / 2.0))
        .collect();
    let Some(&(x, y)) = p.first() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(p.len());
    out.push(PathCmd::Move { x, y });
    let n = p.len();
    for i in 1..n {
        let (x, y) = p[i];
        if !smooth {
            out.push(PathCmd::Line { x, y });
            continue;
        }
        // Segment p[i−1] → p[i], its tangents from its neighbours (the ends repeat).
        let (a, b) = (p[i.saturating_sub(2)], p[i - 1]);
        let c = p[(i + 1).min(n - 1)];
        out.push(PathCmd::Cubic {
            x1: b.0 + (x - a.0) / 6.0,
            y1: b.1 + (y - a.1) / 6.0,
            x2: x - (c.0 - b.0) / 6.0,
            y2: y - (c.1 - b.1) / 6.0,
            x,
            y,
        });
    }
    out
}

/// A shape's outline for a stroke drawn as a path, in its box's centred pixels, from its top going clockwise (where a
/// trim starts): a rect with its corners (`radius`: top left, top right, bottom right, bottom left), an ellipse, a star
/// of `points` (`inner` deep) or polygon (`inner` None), or a line from the top left to the bottom right.
pub(super) enum Outline {
    Rect([f64; 4]),
    Ellipse,
    Star { points: f64, inner: Option<f64> },
    Line,
}

/// The circle's control-point distance for a quarter arc as one cubic.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A quarter arc from `from` to `to` round `corner` (the square's corner it rounds).
fn quarter(out: &mut Vec<PathCmd>, from: (f64, f64), corner: (f64, f64), to: (f64, f64)) {
    out.push(PathCmd::Cubic {
        x1: from.0 + (corner.0 - from.0) * KAPPA,
        y1: from.1 + (corner.1 - from.1) * KAPPA,
        x2: to.0 + (corner.0 - to.0) * KAPPA,
        y2: to.1 + (corner.1 - to.1) * KAPPA,
        x: to.0,
        y: to.1,
    });
}

pub(super) fn outline(shape: &Outline, w: f64, h: f64) -> Vec<PathCmd> {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let mut out = Vec::with_capacity(16);
    match *shape {
        Outline::Line => {
            out.push(PathCmd::Move { x: -hw, y: -hh });
            out.push(PathCmd::Line { x: hw, y: hh });
            return out;
        }
        Outline::Ellipse => {
            out.push(PathCmd::Move { x: 0.0, y: -hh });
            quarter(&mut out, (0.0, -hh), (hw, -hh), (hw, 0.0));
            quarter(&mut out, (hw, 0.0), (hw, hh), (0.0, hh));
            quarter(&mut out, (0.0, hh), (-hw, hh), (-hw, 0.0));
            quarter(&mut out, (-hw, 0.0), (-hw, -hh), (0.0, -hh));
        }
        Outline::Rect(r) => {
            let r = r.map(|v| v.clamp(0.0, hw.min(hh)));
            let [tl, tr, br, bl] = r;
            out.push(PathCmd::Move { x: 0.0, y: -hh });
            let corners = [
                ((hw - tr, -hh), (hw, -hh), (hw, -hh + tr), tr),
                ((hw, hh - br), (hw, hh), (hw - br, hh), br),
                ((-hw + bl, hh), (-hw, hh), (-hw, hh - bl), bl),
                ((-hw, -hh + tl), (-hw, -hh), (-hw + tl, -hh), tl),
            ];
            for (from, corner, to, radius) in corners {
                out.push(PathCmd::Line {
                    x: from.0,
                    y: from.1,
                });
                if radius > 0.0 {
                    quarter(&mut out, from, corner, to);
                }
            }
        }
        Outline::Star { points, inner } => {
            let n = points.round().max(3.0) as usize;
            let big = hw.min(hh);
            let step = std::f64::consts::PI / n as f64;
            let at = |k: usize, r: f64| {
                let (s, c) = (k as f64 * step).sin_cos();
                (s * r, -c * r)
            };
            for k in 0..2 * n {
                let r = match inner {
                    Some(f) if k % 2 == 1 => big * f.clamp(0.01, 1.0),
                    None if k % 2 == 1 => continue,
                    _ => big,
                };
                let (x, y) = at(k, r);
                out.push(if k == 0 {
                    PathCmd::Move { x, y }
                } else {
                    PathCmd::Line { x, y }
                });
            }
        }
    }
    out.push(PathCmd::Close);
    out
}
