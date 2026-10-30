//! A take's smoothed path, built once per smoothing. A segment runs from where the raw path starts
//! moving to just inside the next long still (`stills`). In it a critically damped follower chases the raw path ahead,
//! and its drawn point is stored every `STEP_US`.
//!
//! **Why the drawn point stays in the raw path's range** since its segment began, through 2/ω ahead:
//! - the follower starts at rest on the raw path, so it is a convex mix of its start and its targets (`filter`);
//! - each drawn point is that mix eased onto the raw point, then clamped to the box of the raw points it may draw
//!   from: the start, every target but the newest, and the raw point itself when it is mixed in;
//! - every one of those is a raw position no later than 2/ω past the step before. The newest target, held over a step
//!   from the step's middle, lies half a step later, so it joins the box a step later: it only ever had a sliver of
//!   the weight;
//! - sampling interpolates inside the box of two neighbouring steps, so it keeps the range too.
//!
//! **Cost:** O(rests + steps), each raw position read once by a forward-only walk; 4 bytes a step. The take's rests are
//! found once for all its smoothings (`Rests`).
//!
//! **Memory while building and the cap**: `room`.

use super::filter::Filter;
use super::room::{hand_back, push, steps_expected};
use super::stills::{Rests, rest_us};
use super::{END_RESIDUAL, STEP_US, Seg, SmoothedPath};
use crate::ease::smootherstep;
use crate::track::{Walk, fix};
use crate::{CursorTrack, Us};

/// A point while building, in f64.
type Point = (f64, f64);

/// One smoothing's times and filter.
struct Params {
    /// The target held over a step is the raw path 2/ω past the step's middle: a held target lags half a step, so
    /// this gives a steady movement no lag.
    lead: Us,
    /// 4/ω: how long a long still takes to ease the drawn point onto the raw path.
    rest: Us,
    filter: Filter,
}

impl Params {
    fn new(omega: f64) -> Self {
        Self {
            lead: (2.0 / omega * 1e6) as Us + STEP_US / 2,
            rest: rest_us(omega),
            filter: Filter::new(omega, STEP_US as f64 / 1e6),
        }
    }
}

pub(super) fn build(
    track: &CursorTrack,
    omega: f64,
    rests: &Rests,
    max_steps: usize,
) -> SmoothedPath {
    let p = Params::new(omega);
    let mut path = SmoothedPath::default();
    let Some((first, last)) = track.span() else {
        return path;
    };
    // A still long enough for a step to land after its ease (so a segment can always end inside it).
    let stills = rests.at_least(p.rest + STEP_US);
    // Movement starts at the first position (unless the track rests there) and where each long still ends.
    let starts: Vec<Us> = (stills[0].0 != first)
        .then_some(first)
        .into_iter()
        .chain(stills.iter().map(|s| s.1).filter(|&end| end != Us::MAX))
        .collect();
    path.segments.reserve_exact(starts.len());
    path.xy
        .reserve_exact(steps_expected(p.rest, &stills, &starts).min(max_steps));
    let mut ended = Us::MIN;
    for start in starts {
        if path.xy.len() >= max_steps {
            break;
        }
        // A button held through a still carries a segment past that still's end.
        if start >= ended {
            ended = segment(track, &p, &stills, (start, last), max_steps, &mut path);
        }
    }
    path.segments.shrink_to_fit();
    path.xy.shrink_to_fit();
    path
}

/// The segment from `start` (where the raw path starts moving), its drawn points appended to `path`; its last step's
/// time. `last`: the track's last position's time.
fn segment(
    track: &CursorTrack,
    p: &Params,
    stills: &[(Us, Us)],
    (start, last): (Us, Us),
    max_steps: usize,
    path: &mut SmoothedPath,
) -> Us {
    let (mut now, mut aim) = (Walk::new(track, start), Walk::new(track, start));
    let Some(origin) = now.at(start) else {
        return start;
    };
    let origin: Point = (origin.0.into(), origin.1.into());
    let mut follower = Follower::at(origin);
    // The box of the raw points a drawn point may draw from (see the module notes), and the newest target.
    let mut drawable = Box2::at(origin);
    let mut newest = origin;
    let mut still = stills.partition_point(|s| s.1 <= start);
    let first = path.xy.len();
    let mut t = start;
    loop {
        // The first long still that ends after `t` (the last never ends).
        while stills[still].1 <= t {
            still += 1;
        }
        let rest_from = stills[still].0;
        // The raw point matters only once the raw path rests (the ease onto it); moving, the drawn point is the
        // follower's.
        let mut bounds = drawable;
        let (raw, rest) = if t >= rest_from {
            let raw = now.at(t).map_or(origin, |(x, y)| (x.into(), y.into()));
            bounds.widen(raw);
            (raw, smootherstep((t - rest_from) as f64 / p.rest as f64))
        } else {
            (origin, 0.0)
        };
        let drawn = bounds.clamp(follower.onto(raw, rest));
        push(path, [fix(drawn.0 as f32), fix(drawn.1 as f32)]);
        let settled = t >= rest_from.saturating_add(p.rest) && distance(drawn, raw) <= END_RESIDUAL;
        let ends = settled && (t >= last.saturating_add(p.rest) || !track.any_held_at(t));
        if ends {
            break;
        }
        if path.xy.len() >= max_steps {
            hand_back(track, p.rest, path, (first, start));
            break;
        }
        if rest >= 1.0 {
            // Settled on a still raw path (a button held keeps the segment going): restart there, at rest.
            follower = Follower::at(raw);
            drawable.widen(raw);
        }
        drawable.widen(newest);
        newest = aim
            .at(t.saturating_add(p.lead).min(rest_from))
            .map_or(raw, |(x, y)| (x.into(), y.into()));
        follower.step(&p.filter, newest);
        t = t.saturating_add(STEP_US);
    }
    let len = path.xy.len() - first;
    path.segments.push(Seg {
        start,
        first: first as u32,
        len: len as u32,
    });
    t
}

/// The follower on both axes: position and velocity.
struct Follower {
    x: Point,
    v: Point,
}

impl Follower {
    fn at(x: Point) -> Self {
        Self { x, v: (0.0, 0.0) }
    }

    fn step(&mut self, filter: &Filter, (tx, ty): Point) {
        (self.x.0, self.v.0) = filter.step(self.x.0, self.v.0, tx);
        (self.x.1, self.v.1) = filter.step(self.x.1, self.v.1, ty);
    }

    /// Eased `w` of the way onto `raw` (exactly it at 1).
    fn onto(&self, raw: Point, w: f64) -> Point {
        if w >= 1.0 {
            return raw;
        }
        (
            self.x.0 + (raw.0 - self.x.0) * w,
            self.x.1 + (raw.1 - self.x.1) * w,
        )
    }
}

/// Lowest and highest x and y of some points.
#[derive(Clone, Copy)]
struct Box2 {
    lo: Point,
    hi: Point,
}

impl Box2 {
    fn at(p: Point) -> Self {
        Self { lo: p, hi: p }
    }

    fn widen(&mut self, (x, y): Point) {
        self.lo = (self.lo.0.min(x), self.lo.1.min(y));
        self.hi = (self.hi.0.max(x), self.hi.1.max(y));
    }

    fn clamp(&self, (x, y): Point) -> Point {
        (x.clamp(self.lo.0, self.hi.0), y.clamp(self.lo.1, self.hi.1))
    }
}

fn distance(a: Point, b: Point) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
