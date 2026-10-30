//! A stroke's mesh from its flattened centre line. Every vertex sits on the centre line with the unit
//! offset it moves out by (the shader scales it by the draw's half width), its side across the stroke (−1 to 1; 0 at
//! a join's or a cap's centre) and its distance along the path. Segments are quads; joins (miter within SVG's limit
//! of 4, else bevel; round) and caps (butt, round, square) are fans from the centre point, so the coverage across the
//! stroke, the side times the reach, is exact at a miter's edges and close to it round a round end. A round end is cut
//! into chords within the tolerance at the width `round_hw` it is made for.

use klipolo_model::scene::{LineCap, LineJoin};

use super::mesh_cache::{MAX_MESH_VERTICES, MeshData};
use super::path_pipeline::PathVertex;
use super::paths::SubPath;

/// SVG's default miter limit: a miter longer than 4 half widths is a bevel.
const MITER_LIMIT: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StrokeStyle {
    pub cap: LineCap,
    pub join: LineJoin,
    /// The half width (mesh units) round ends are cut for, and the tolerance they are cut to.
    pub round_hw: f64,
    pub tolerance: f64,
}

struct Builder<'a> {
    mesh: &'a mut MeshData,
    style: StrokeStyle,
    /// The current sub-path's start, along the whole path.
    sub: f32,
}

type V = [f64; 2];

impl Builder<'_> {
    fn vertex(&mut self, p: V, dir: V, side: f32, along: f64) -> Option<u16> {
        let i = self.mesh.vertices.len();
        if i >= MAX_MESH_VERTICES {
            return None;
        }
        self.mesh.vertices.push(PathVertex {
            pos: [p[0] as f32, p[1] as f32],
            dir: [dir[0] as f32, dir[1] as f32],
            side,
            along: along as f32,
            sub: self.sub,
        });
        Some(i as u16)
    }

    fn tri(&mut self, a: u16, b: u16, c: u16) {
        self.mesh.indices.extend_from_slice(&[a, b, c]);
    }

    /// A quad from `a` to `b` (unit normal `n`), `sa` and `sb` along.
    fn segment(&mut self, a: V, b: V, n: V, sa: f64, sb: f64) -> Option<()> {
        let neg = [-n[0], -n[1]];
        let (a0, a1) = (self.vertex(a, n, 1.0, sa)?, self.vertex(a, neg, -1.0, sa)?);
        let (b0, b1) = (self.vertex(b, n, 1.0, sb)?, self.vertex(b, neg, -1.0, sb)?);
        self.tri(a0, a1, b0);
        self.tri(a1, b1, b0);
        Some(())
    }

    /// A fan about `p` from unit offset `from` round to `to` (turning by `sweep` radians, its sign the way), each
    /// rim vertex on side `side`.
    fn fan(&mut self, p: V, from: V, sweep: f64, side: f32, along: f64) -> Option<()> {
        let s = self.style;
        let step = if s.round_hw > s.tolerance {
            2.0 * (1.0 - s.tolerance / s.round_hw).acos()
        } else {
            std::f64::consts::FRAC_PI_2
        };
        let n = ((sweep.abs() / step.max(1e-3)).ceil() as usize).clamp(1, 64);
        let centre = self.vertex(p, [0.0; 2], 0.0, along)?;
        let mut last = self.vertex(p, from, side, along)?;
        let a0 = from[1].atan2(from[0]);
        for i in 1..=n {
            let a = a0 + sweep * i as f64 / n as f64;
            let next = self.vertex(p, [a.cos(), a.sin()], side, along)?;
            self.tri(centre, last, next);
            last = next;
        }
        Some(())
    }

    /// The join at `p` between a segment with unit direction `da` (normal `na`) and the next (`db`, `nb`).
    fn join(&mut self, p: V, (da, na): (V, V), (db, nb): (V, V), along: f64) -> Option<()> {
        let turn = da[0] * db[1] - da[1] * db[0];
        let straight = turn.abs() < 1e-9 && da[0] * db[0] + da[1] * db[1] > 0.0;
        if straight {
            return Some(());
        }
        // The outer side is away from the turn.
        let o = if turn > 0.0 { -1.0 } else { 1.0 };
        let (oa, ob) = ([na[0] * o, na[1] * o], [nb[0] * o, nb[1] * o]);
        let side = o as f32;
        match self.style.join {
            LineJoin::Round => {
                let mut sweep =
                    (oa[0] * ob[1] - oa[1] * ob[0]).atan2(oa[0] * ob[0] + oa[1] * ob[1]);
                if turn.abs() < 1e-9 {
                    // Straight back on itself: half a turn through the way it was going, not back over itself
                    //.
                    let ahead = oa[0] * da[1] - oa[1] * da[0];
                    sweep = std::f64::consts::PI.copysign(ahead);
                }
                self.fan(p, oa, sweep, side, along)
            }
            join => {
                let centre = self.vertex(p, [0.0; 2], 0.0, along)?;
                let a = self.vertex(p, oa, side, along)?;
                let b = self.vertex(p, ob, side, along)?;
                let d = 1.0 + oa[0] * ob[0] + oa[1] * ob[1];
                let miter = [(oa[0] + ob[0]) / d, (oa[1] + ob[1]) / d];
                if join == LineJoin::Miter && d > 1e-9 && miter[0].hypot(miter[1]) <= MITER_LIMIT {
                    let tip = self.vertex(p, miter, side, along)?;
                    self.tri(centre, a, tip);
                    self.tri(centre, tip, b);
                } else {
                    self.tri(centre, a, b);
                }
                Some(())
            }
        }
    }

    /// The cap at `p`, the path heading out of it along unit `d` (normal `n`).
    fn cap(&mut self, p: V, d: V, n: V, along: f64) -> Option<()> {
        match self.style.cap {
            LineCap::Butt => Some(()),
            LineCap::Round => {
                // From the left round the end to the right: half a turn, through `d`.
                let sweep = if n[0] * d[1] - n[1] * d[0] > 0.0 {
                    1.0
                } else {
                    -1.0
                } * std::f64::consts::PI;
                self.fan(p, n, sweep, 1.0, along)
            }
            LineCap::Square => {
                let neg = [-n[0], -n[1]];
                let a0 = self.vertex(p, n, 1.0, along)?;
                let a1 = self.vertex(p, neg, -1.0, along)?;
                let b0 = self.vertex(p, [n[0] + d[0], n[1] + d[1]], 1.0, along)?;
                let b1 = self.vertex(p, [neg[0] + d[0], neg[1] + d[1]], -1.0, along)?;
                self.tri(a0, a1, b0);
                self.tri(a1, b1, b0);
                Some(())
            }
        }
    }
}

fn unit(a: V, b: V) -> (V, V, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy);
    let d = [dx / l, dy / l];
    (d, [-d[1], d[0]], l)
}

/// The stroke of `subpaths` into `mesh` in `style`; its length along every sub-path. None past the vertex cap.
pub(super) fn stroke(subpaths: &[SubPath], style: StrokeStyle, mesh: &mut MeshData) -> Option<f32> {
    let mut b = Builder {
        mesh,
        style,
        sub: 0.0,
    };
    let mut along = 0.0f64;
    for sp in subpaths {
        b.sub = along as f32;
        let p = &sp.points;
        if p.len() == 1 {
            // A dot: a round or square cap both ways.
            let (d, n) = ([1.0, 0.0], [0.0, -1.0]);
            b.cap(p[0], d, n, along)?;
            b.cap(p[0], [-1.0, 0.0], [0.0, 1.0], along)?;
            continue;
        }
        let n = p.len();
        let segments = if sp.closed { n } else { n - 1 };
        let mut first: Option<(V, V)> = None;
        let mut prev: Option<(V, V)> = None;
        for i in 0..segments {
            let (a, c) = (p[i], p[(i + 1) % n]);
            let (d, nn, l) = unit(a, c);
            if l.is_nan() || l <= 0.0 {
                continue;
            }
            if let Some(pv) = prev {
                b.join(a, pv, (d, nn), along)?;
            }
            b.segment(a, c, nn, along, along + l)?;
            along += l;
            first.get_or_insert((d, nn));
            prev = Some((d, nn));
        }
        let (Some(first), Some(last)) = (first, prev) else {
            continue;
        };
        if sp.closed {
            b.join(p[0], last, first, along)?;
        } else {
            let (d0, n0) = first;
            b.cap(p[0], [-d0[0], -d0[1]], [-n0[0], -n0[1]], b.sub as f64)?;
            b.cap(p[n - 1], last.0, last.1, along)?;
        }
    }
    Some(along as f32)
}

#[cfg(test)]
mod tests {
    use klipolo_model::scene::path::PathCmd;

    use super::*;
    use crate::render::scene::paths::flatten;

    fn stroked(cmds: &[PathCmd], cap: LineCap, join: LineJoin) -> (MeshData, Option<f32>) {
        let subpaths = flatten(cmds, 0.25, MAX_MESH_VERTICES).unwrap();
        let mut mesh = MeshData::default();
        let style = StrokeStyle {
            cap,
            join,
            round_hw: 4.0,
            tolerance: 0.25,
        };
        let length = stroke(&subpaths, style, &mut mesh);
        (mesh, length)
    }

    fn line(points: &[[f64; 2]]) -> Vec<PathCmd> {
        let mut out = vec![PathCmd::Move {
            x: points[0][0],
            y: points[0][1],
        }];
        out.extend(
            points[1..]
                .iter()
                .map(|p| PathCmd::Line { x: p[0], y: p[1] }),
        );
        out
    }

    /// A line that turns straight back, round-joined, rounds its turn forward (through the way it was going),
    /// not back over itself.
    #[test]
    fn a_reversal_rounds_forward() {
        let (mesh, _) = stroked(
            &line(&[[0.0, 0.0], [10.0, 0.0], [0.0, 0.0]]),
            LineCap::Butt,
            LineJoin::Round,
        );
        let forward = mesh
            .vertices
            .iter()
            .any(|v| v.pos == [10.0, 0.0] && v.dir[0] > 0.9);
        assert!(forward, "no rim vertex ahead of the turn");
    }

    /// Every cap and join makes finite vertices within the cap; a dot, coincident points and huge numbers
    /// are drawn or skipped, never NaN.
    #[test]
    fn every_cap_and_join_is_finite() {
        let shapes = [
            line(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 3.0]]),
            line(&[[5.0, 5.0], [5.0, 5.0], [5.0, 5.0]]),
            line(&[[0.0, 0.0], [1e7, 1e7], [-1e7, 1e7]]),
            vec![PathCmd::Move { x: 1.0, y: 1.0 }, PathCmd::Close],
        ];
        for cmds in &shapes {
            for cap in [LineCap::Butt, LineCap::Round, LineCap::Square] {
                for join in [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel] {
                    let (mesh, length) = stroked(cmds, cap, join);
                    assert!(length.is_some_and(f32::is_finite));
                    assert!(mesh.vertices.len() <= MAX_MESH_VERTICES);
                    for v in &mesh.vertices {
                        let all = [v.pos[0], v.pos[1], v.dir[0], v.dir[1], v.along, v.sub];
                        assert!(all.iter().all(|n| n.is_finite()), "{cap:?} {join:?}: {v:?}");
                    }
                    assert!(
                        mesh.indices
                            .iter()
                            .all(|&i| usize::from(i) < mesh.vertices.len())
                    );
                }
            }
        }
        // A dot with round ends is drawn; with butt ends it is nothing.
        let dot = [PathCmd::Move { x: 1.0, y: 1.0 }, PathCmd::Close];
        assert!(
            !stroked(&dot, LineCap::Round, LineJoin::Miter)
                .0
                .indices
                .is_empty()
        );
        assert!(
            stroked(&dot, LineCap::Butt, LineJoin::Miter)
                .0
                .indices
                .is_empty()
        );
    }
}
