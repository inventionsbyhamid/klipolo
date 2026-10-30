//! An elliptical arc as up to four cubic curves (SVG 1.1 implementation notes F.6.5–F.6.6): the centre form from the
//! endpoint form, radii scaled up when they can't reach, and one cubic per quarter turn at most.

use super::{Parser, PathCmd};

impl Parser<'_> {
    /// An elliptical arc from `p0` to `p1` as up to four cubic curves (SVG 1.1 F.6.5–F.6.6).
    pub(super) fn arc(
        &mut self,
        (x0, y0): (f64, f64),
        (rx, ry): (f64, f64),
        turn: f64,
        large: bool,
        sweep: bool,
        (x1, y1): (f64, f64),
    ) -> Result<(), String> {
        if x0 == x1 && y0 == y1 {
            return Ok(());
        }
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 {
            return self.push(PathCmd::Line { x: x1, y: y1 });
        }
        let (sin, cos) = turn.to_radians().sin_cos();
        let (dx, dy) = ((x0 - x1) / 2.0, (y0 - y1) / 2.0);
        let (px, py) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        let lambda = (px * px) / (rx * rx) + (py * py) / (ry * ry);
        if lambda > 1.0 {
            let s = lambda.sqrt();
            rx *= s;
            ry *= s;
        }
        let (rx2, ry2) = (rx * rx, ry * ry);
        let den = rx2 * py * py + ry2 * px * px;
        let num = rx2 * ry2 - den;
        let mut k = if den > 0.0 {
            (num / den).max(0.0).sqrt()
        } else {
            0.0
        };
        if large == sweep {
            k = -k;
        }
        let (qx, qy) = (k * rx * py / ry, -k * ry * px / rx);
        let (cx, cy) = (
            cos * qx - sin * qy + (x0 + x1) / 2.0,
            sin * qx + cos * qy + (y0 + y1) / 2.0,
        );
        let angle =
            |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        let (ux, uy) = ((px - qx) / rx, (py - qy) / ry);
        let (vx, vy) = ((-px - qx) / rx, (-py - qy) / ry);
        let start = angle(1.0, 0.0, ux, uy);
        let mut delta = angle(ux, uy, vx, vy);
        let tau = std::f64::consts::TAU;
        if !sweep && delta > 0.0 {
            delta -= tau;
        } else if sweep && delta < 0.0 {
            delta += tau;
        }
        if !(start.is_finite() && delta.is_finite()) {
            return Err("An arc in the path data can't be drawn.".into());
        }
        let pieces = (delta.abs() / std::f64::consts::FRAC_PI_2)
            .ceil()
            .clamp(1.0, 4.0);
        let step = delta / pieces;
        let handle = 4.0 / 3.0 * (step / 4.0).tan();
        let point = |t: f64| {
            let (s, c) = t.sin_cos();
            (
                cx + rx * c * cos - ry * s * sin,
                cy + rx * c * sin + ry * s * cos,
            )
        };
        let tangent = |t: f64| {
            let (s, c) = t.sin_cos();
            (-rx * s * cos - ry * c * sin, -rx * s * sin + ry * c * cos)
        };
        let n = pieces as usize;
        for i in 0..n {
            let (t0, t1) = (start + step * i as f64, start + step * (i + 1) as f64);
            let (a, da) = (point(t0), tangent(t0));
            let (b, db) = if i + 1 == n {
                ((x1, y1), tangent(t1))
            } else {
                (point(t1), tangent(t1))
            };
            let cmd = PathCmd::Cubic {
                x1: a.0 + handle * da.0,
                y1: a.1 + handle * da.1,
                x2: b.0 - handle * db.0,
                y2: b.1 - handle * db.1,
                x: b.0,
                y: b.1,
            };
            let finite = [a.0, a.1, b.0, b.1, da.0, da.1, db.0, db.1, handle]
                .iter()
                .all(|v| v.is_finite());
            if !finite {
                return Err("An arc in the path data can't be drawn.".into());
            }
            self.push(cmd)?;
        }
        Ok(())
    }
}
