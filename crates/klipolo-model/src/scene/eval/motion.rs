//! Motion paths: each node's `motionPath` flattened once, when the instance is built, into a table of at
//! most [`MOTION_SAMPLES`] points with the length run to each, so a frame finds where `progress` puts the node (and
//! which way the path heads there) by one binary search, allocating nothing. Points are in the path's own units; the
//! renderer fits them into the box (the path's `viewBox`, else its own bounds, as a `path` node is fitted).

use super::super::instance::SceneInstance;
use super::super::path::{bounds, cubic_at, parse_d, quad_at, PathCmd};
use super::super::timeline::CompiledScene;
use super::super::NodeType;

/// The most points a motion path's table keeps.
pub const MOTION_SAMPLES: usize = 256;

/// A motion path flattened: its points, the length from the start to each, and its own bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionTable {
    points: Vec<[f64; 2]>,
    lengths: Vec<f64>,
    /// `[x, y, width, height]`: the path's bounds (`path::bounds`), its fit without a `viewBox`.
    pub bounds: [f64; 4],
}

impl MotionTable {
    /// The path data `cmds` as a table; None when it draws nothing.
    pub fn new(cmds: &[PathCmd]) -> Option<Self> {
        let bounds = bounds(cmds)?;
        let curves = cmds
            .iter()
            .filter(|c| matches!(c, PathCmd::Quad { .. } | PathCmd::Cubic { .. }))
            .count();
        let others = cmds.len() - curves;
        let room = MOTION_SAMPLES.saturating_sub(1 + others);
        // Each curve gets an even share of the room (at most 64 chords); past the room, 4 chords each and the whole
        // path resampled evenly along its length.
        let per = (room / curves.max(1)).clamp(1, 64);
        // A point per command, `per` per curve.
        let fits = cmds.len() + curves * (per - 1) <= MOTION_SAMPLES;
        let mut t = Self {
            points: Vec::new(),
            lengths: Vec::new(),
            bounds,
        };
        t.flatten(cmds, if fits { per } else { 4 });
        if t.points.len() > MOTION_SAMPLES {
            t.resample();
        }
        (t.points.len() >= 2).then_some(t)
    }

    fn push(&mut self, p: (f64, f64), jump: bool) {
        let along = match (self.points.last(), self.lengths.last()) {
            (Some(q), Some(l)) if !jump => l + (p.0 - q[0]).hypot(p.1 - q[1]),
            (_, Some(l)) => *l,
            _ => 0.0,
        };
        self.points.push([p.0, p.1]);
        self.lengths.push(along);
    }

    fn flatten(&mut self, cmds: &[PathCmd], per: usize) {
        let (mut at, mut start) = ((0.0, 0.0), (0.0, 0.0));
        for &cmd in cmds {
            match cmd {
                PathCmd::Move { x, y } => {
                    (at, start) = ((x, y), (x, y));
                    self.push(at, true);
                }
                PathCmd::Line { x, y } => {
                    at = (x, y);
                    self.push(at, false);
                }
                PathCmd::Quad { x1, y1, x, y } => {
                    for i in 1..=per {
                        let t = i as f64 / per as f64;
                        self.push(quad_at(at, (x1, y1), (x, y), t), false);
                    }
                    at = (x, y);
                }
                PathCmd::Cubic {
                    x1,
                    y1,
                    x2,
                    y2,
                    x,
                    y,
                } => {
                    for i in 1..=per {
                        let t = i as f64 / per as f64;
                        self.push(cubic_at(at, (x1, y1), (x2, y2), (x, y), t), false);
                    }
                    at = (x, y);
                }
                PathCmd::Close => {
                    at = start;
                    self.push(at, false);
                }
            }
        }
    }

    /// [`MOTION_SAMPLES`] points spread evenly along the length.
    fn resample(&mut self) {
        let total = self.length();
        let fine = std::mem::take(&mut self.points);
        let fine_lengths = std::mem::take(&mut self.lengths);
        let mut k = 0;
        for i in 0..MOTION_SAMPLES {
            let s = total * i as f64 / (MOTION_SAMPLES - 1) as f64;
            while k + 1 < fine.len() && fine_lengths[k + 1] < s {
                k += 1;
            }
            let (a, b) = (k, (k + 1).min(fine.len() - 1));
            let span = fine_lengths[b] - fine_lengths[a];
            let f = if span > 0.0 {
                ((s - fine_lengths[a]) / span).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let p = [0, 1].map(|c| fine[a][c] + (fine[b][c] - fine[a][c]) * f);
            self.points.push(p);
            self.lengths.push(s);
        }
    }

    /// The path's length.
    pub fn length(&self) -> f64 {
        self.lengths.last().copied().unwrap_or(0.0)
    }

    /// The points kept.
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Where `progress` (0–1 of the length) is on the path, and the way it heads there in degrees (clockwise from
    /// the x axis, y down, as `rotate`).
    pub fn at(&self, progress: f64) -> (f64, f64, f64) {
        let p = if progress.is_finite() {
            progress.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let s = p * self.length();
        let n = self.points.len();
        // The first point at or past `s`: the segment before it holds `s` (a jump, of no length, is skipped).
        let i = self.lengths.partition_point(|&l| l < s).clamp(1, n - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let span = self.lengths[i] - self.lengths[i - 1];
        let f = if span > 0.0 {
            ((s - self.lengths[i - 1]) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let x = a[0] + (b[0] - a[0]) * f;
        let y = a[1] + (b[1] - a[1]) * f;
        (x, y, self.turning(i, f))
    }

    /// The way the path heads at fraction `f` of the segment ending at point `i`: turning smoothly from the heading at
    /// its start to the one at its end where it meets its neighbours at a gentle bend (a curve's chords), and its own
    /// where they meet at a corner.
    fn turning(&self, i: usize, f: f64) -> f64 {
        let n = self.points.len();
        let here = self.heading(i);
        // Halfway round to `other` from `here` when they differ by under 15°, else `here`.
        let blend = |other: f64| {
            let d = (other - here + 540.0).rem_euclid(360.0) - 180.0;
            if d.abs() < 15.0 {
                here + d / 2.0
            } else {
                here
            }
        };
        let start = if i >= 2 {
            blend(self.heading(i - 1))
        } else {
            here
        };
        let end = if i + 1 < n {
            blend(self.heading(i + 1))
        } else {
            here
        };
        let d = (end - start + 540.0).rem_euclid(360.0) - 180.0;
        let h = start + d * f;
        (h + 540.0).rem_euclid(360.0) - 180.0
    }

    /// The direction of the segment ending at point `i`, or the nearest one with a length.
    fn heading(&self, i: usize) -> f64 {
        let n = self.points.len();
        let seg = |j: usize| (self.lengths[j] > self.lengths[j - 1]).then_some(j);
        let j = seg(i)
            .or_else(|| (i + 1..n).find_map(seg))
            .or_else(|| (1..i).rev().find_map(seg));
        j.map_or(0.0, |j| {
            let (a, b) = (self.points[j - 1], self.points[j]);
            (b[1] - a[1]).atan2(b[0] - a[0]).to_degrees()
        })
    }
}

/// Each compiled node's motion path table (None without one, or when its path draws nothing): its own `d`, or that
/// of the `path` node its `"#id"` names.
pub(crate) fn tables(cs: &CompiledScene) -> Vec<Option<MotionTable>> {
    cs.nodes
        .iter()
        .map(|c| {
            let mp = c.node.props.motion_path.as_ref()?;
            let d = match mp.path.strip_prefix('#') {
                Some(id) => {
                    let target = &cs.nodes.get(cs.find(id)?)?.node;
                    (target.kind == NodeType::Path)
                        .then_some(target.props.d.as_deref())
                        .flatten()?
                }
                None => mp.path.as_str(),
            };
            MotionTable::new(&parse_d(d).ok()?)
        })
        .collect()
}

/// Where instance node `node` is on its motion path at `progress`, in the path's own units, and the way the path
/// heads there (degrees, clockwise): None without a motion path.
pub fn motion_path(inst: &SceneInstance, node: usize, progress: f64) -> Option<(f64, f64, f64)> {
    Some(inst.motion_table(node)?.at(progress))
}
