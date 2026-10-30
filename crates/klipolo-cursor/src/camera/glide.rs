//! The glide: inside each Auto shot, from its first event until its view stops
//! holding, the view follows the RAW cursor at 60 Hz, only when the cursor leaves the safe zone and only as far as
//! needed. The person's own zooms never glide.
//!
//! - **The wanted centre** stays put while the cursor is inside the safe zone, else shifts the least that puts it
//!   back. It is kept where every event still to come in the shot stays in the safe zone, and every dropped action
//!   still to come stays in view: those win over the cursor. The view stays inside the frame.
//! - **The filter:** `smooth::filter` at `glide_omega`, then slowed to `FOLLOW` of the pan limit and kept inside that
//!   same region. Both only slow it: the region is convex, holds the shot's view, and only grows as events pass.
//! - **Still before each later event:** over the 1/ω before its pre-roll the wanted centre holds where the view is,
//!   which brings the filter to rest; through the pre-roll the view holds.
//! - **Home by the end:** a backward pass caps the offset at what `HOME` of the limit brings back before the move out
//!   starts, easing in over the last `HOME_EASE_S`. It scales toward the shot's view, so it stays inside every region
//!   above, and it is flat through the pre-rolls. A step so moves at most `FOLLOW` + `HOME` of the limit.
//!
//! Offsets are stored as 16 bits a 60 Hz step, rounded toward the shot's view (so still inside every region), and only
//! for shots where one isn't 0. Rounding adds at most `ROUNDING` to a step's speed, taken off the limit first.

use super::safe::{box_of, centres, shows};
use super::{Plan, Rules, Shot, Weight};
use crate::smooth::Filter;
use crate::{Rect01, Us};

pub(crate) const STEP_US: Us = 16_667;
const DT: f64 = STEP_US as f64 / 1e6;
const Q: f64 = 32_768.0;
/// The most the offsets' rounding adds to a step's speed, in frames a second.
const ROUNDING: f64 = std::f64::consts::SQRT_2 / Q / DT;
const FOLLOW: f64 = 0.7;
const HOME: f64 = 0.28;
const HOME_EASE_S: f64 = 0.25;

/// Each glided shot's offsets from its view.
#[derive(Debug, Default)]
pub(crate) struct Glide {
    segs: Vec<Seg>,
    offs: Vec<[i16; 2]>,
}

#[derive(Clone, Copy, Debug)]
struct Seg {
    from: Us,
    first: u32,
    n: u32,
}

impl Glide {
    /// The offset at `t` where a glide covers it, linear between steps. O(log shots), no allocation.
    pub(crate) fn at(&self, t: Us) -> Option<(f64, f64)> {
        let s = self.segs[self.segs.partition_point(|s| s.from <= t).checked_sub(1)?];
        let k = ((t - s.from) / STEP_US) as u32;
        if k + 1 >= s.n {
            return None;
        }
        let f = ((t - s.from) % STEP_US) as f64 / STEP_US as f64;
        let (a, b) = (
            self.offs[(s.first + k) as usize],
            self.offs[(s.first + k + 1) as usize],
        );
        let lerp = |a: i16, b: i16| (a as f64 + (b as f64 - a as f64) * f) / Q;
        Some((lerp(a[0], b[0]), lerp(a[1], b[1])))
    }

    pub(crate) fn bytes(&self) -> usize {
        self.segs.capacity() * size_of::<Seg>() + self.offs.capacity() * size_of::<[i16; 2]>()
    }
}

/// The glide of every Auto shot in `p`, from the raw cursor.
pub(crate) fn build(p: &Plan, raw: &dyn Fn(Us) -> Option<(f32, f32)>) -> Glide {
    let (mut g, mut work, mut seen) = (Glide::default(), Work::default(), Vec::new());
    // A dropped typing span can start before a shot and last into it: look back as far as the longest one.
    let longest = p
        .in_view
        .iter()
        .map(|q| q.1.saturating_sub(q.0))
        .max()
        .unwrap_or(0);
    for (shot, hold) in p.shots.iter().zip(&p.holds) {
        let events = &p.boxes[hold.first as usize..(hold.first + hold.n) as usize];
        // The dropped actions over its glide that its view shows (the repair made sure of each one it covers), each
        // kept in view until it ends.
        let from = p
            .in_view
            .partition_point(|q| q.0 < shot.from.saturating_sub(longest));
        let to = p.in_view.partition_point(|q| q.0 <= hold.until);
        let over =
            |q: &&(Us, Us, f32, f32)| q.1 >= shot.from && shows(shot.view, q.2 as f64, q.3 as f64);
        seen.clear();
        seen.extend(
            p.in_view[from..to]
                .iter()
                .filter(over)
                .map(|q| (q.1, q.2, q.3)),
        );
        let mine = (events, seen.as_slice());
        // A shot arriving late (`moves::late`) glides from its arrival.
        let shot = &Shot {
            from: shot.from.max(hold.since),
            ..*shot
        };
        if shot.weight != Weight::Person && work.run(shot, hold.until, mine, &p.rules, raw) {
            let first = g.offs.len() as u32;
            g.offs
                .extend(work.offs.iter().map(|&(x, y)| [quantise(x), quantise(y)]));
            g.segs.push(Seg {
                from: shot.from,
                first,
                n: work.offs.len() as u32,
            });
        }
    }
    g
}

/// Toward 0, so a rounded offset stays between the shot's view and the glided one.
fn quantise(o: f64) -> i16 {
    (o * Q).trunc() as i16
}

type Event = (Us, Us, Rect01);
/// A shot's events (inside the safe zone), and the dropped actions its view must show while it glides (each as the
/// time it ends and its point).
type Mine<'a> = (&'a [Event], &'a [(Us, f32, f32)]);

const FREE: u8 = 0;
const LEAD: u8 = 1;
const HELD: u8 = 2;

/// One shot's working buffers, reused shot to shot.
#[derive(Default)]
struct Work {
    offs: Vec<(f64, f64)>,
    marks: Vec<u8>,
    /// When each of the shot's events and dropped actions ends, with the centres (x0, x1, y0, y1) that hold it,
    /// latest-ending first.
    items: Vec<(Us, [f64; 4])>,
    /// `region[k]`: the centres holding the k + 1 latest-ending items, inside the frame.
    region: Vec<[f64; 4]>,
}

impl Work {
    /// The shot's offsets into `offs`; false when every one rounds to 0.
    fn run(
        &mut self,
        shot: &Shot,
        until: Us,
        (ev, seen): Mine,
        r: &Rules,
        raw: &dyn Fn(Us) -> Option<(f32, f32)>,
    ) -> bool {
        let limit = r.pan_speed / shot.view.scale * 0.99 - ROUNDING;
        let n = ((until - shot.from) / STEP_US + 1).max(0) as usize;
        if n < 3 || limit <= 0.0 {
            return false;
        }
        self.mark(shot.from, n, ev, r);
        self.regions(shot.view.scale, (ev, seen), r);
        self.follow(shot, n, limit * FOLLOW * DT, r, raw);
        self.home(limit * HOME);
        self.offs
            .iter()
            .any(|&(x, y)| quantise(x) != 0 || quantise(y) != 0)
    }

    /// The steps held still (each later event's pre-roll) and those easing to rest before them.
    fn mark(&mut self, g0: Us, n: usize, ev: &[Event], r: &Rules) {
        self.marks.clear();
        self.marks.resize(n, FREE);
        let lead = (1.0 / r.glide_omega / DT).round() as usize;
        let step = |t: Us| ((t - g0) as f64 / STEP_US as f64).clamp(0.0, (n - 1) as f64);
        for &(at, _, _) in ev.iter().filter(|e| e.0 > g0) {
            let (lo, hi) = (
                step(at - r.pre_roll_us).floor() as usize,
                step(at).ceil() as usize,
            );
            let m = &mut self.marks;
            m[lo.saturating_sub(lead)..lo]
                .iter_mut()
                .filter(|m| **m == FREE)
                .for_each(|m| *m = LEAD);
            m[lo..=hi].fill(HELD);
        }
    }

    /// The events hold their box in the safe zone; the dropped actions only need showing, so no margin.
    fn regions(&mut self, s: f64, (ev, seen): Mine, r: &Rules) {
        let hold = |b: [f64; 4], safe: f64| {
            let ((x0, x1), (y0, y1)) = (centres(b[0], b[2], s, safe), centres(b[1], b[3], s, safe));
            [x0, x1, y0, y1]
        };
        self.items.clear();
        self.items
            .extend(ev.iter().map(|e| (e.1, hold(box_of(e.2), r.safe))));
        let point = |q: &(Us, f32, f32)| (q.0, hold([q.1, q.2, q.1, q.2].map(|v| v as f64), 0.0));
        self.items.extend(seen.iter().map(point));
        self.items.sort_by_key(|i| std::cmp::Reverse(i.0));
        let half = 0.5 / s;
        let mut acc = [half, 1.0 - half, half, 1.0 - half];
        self.region.clear();
        for &(_, [x0, x1, y0, y1]) in &self.items {
            acc = [
                acc[0].max(x0),
                acc[1].min(x1),
                acc[2].max(y0),
                acc[3].min(y1),
            ];
            self.region.push(acc);
        }
    }

    /// The forward pass: the filtered, slowed, region-kept centre at each step, as an offset from the shot's view.
    fn follow(
        &mut self,
        shot: &Shot,
        n: usize,
        cap: f64,
        r: &Rules,
        raw: &dyn Fn(Us) -> Option<(f32, f32)>,
    ) {
        let (s, base) = (shot.view.scale, (shot.view.cx, shot.view.cy));
        let half = 0.5 / s;
        let filter = Filter::new(r.glide_omega, DT);
        let (mut c, mut v, mut want) = (base, (0.0, 0.0), base);
        let mut live = self.items.len();
        self.offs.clear();
        self.offs.push((0.0, 0.0));
        for k in 1..n {
            let t = shot.from + k as Us * STEP_US;
            // An item holds the step at or after its end too: the view between two steps is read between them.
            while live > 0 && self.items[live - 1].0 <= t - STEP_US {
                live -= 1;
            }
            let b = live
                .checked_sub(1)
                .map_or([half, 1.0 - half, half, 1.0 - half], |i| self.region[i]);
            match self.marks[k] {
                HELD => {
                    self.offs.push(self.offs[k - 1]);
                    v = (0.0, 0.0);
                    continue;
                }
                LEAD if self.marks[k - 1] != LEAD => want = c,
                LEAD => {}
                _ => {
                    if let Some((x, y)) = raw(t).filter(|p| p.0.is_finite() && p.1.is_finite()) {
                        let (x0, x1) = centres(x as f64, x as f64, s, r.safe);
                        let (y0, y1) = centres(y as f64, y as f64, s, r.safe);
                        want = (want.0.max(x0).min(x1), want.1.max(y0).min(y1));
                    }
                }
            }
            want = (want.0.max(b[0]).min(b[1]), want.1.max(b[2]).min(b[3]));
            let ((x, vx), (y, vy)) = (filter.step(c.0, v.0, want.0), filter.step(c.1, v.1, want.1));
            let (mut d, mut nv) = ((x - c.0, y - c.1), (vx, vy));
            let len = d.0.hypot(d.1);
            if len > cap {
                d = (d.0 * cap / len, d.1 * cap / len);
                nv = (d.0 / DT, d.1 / DT);
            }
            let (x, y) = (c.0 + d.0, c.1 + d.1);
            let kept = (x.max(b[0]).min(b[1]), y.max(b[2]).min(b[3]));
            v = (
                if kept.0 == x { nv.0 } else { 0.0 },
                if kept.1 == y { nv.1 } else { 0.0 },
            );
            c = kept;
            self.offs.push((c.0 - base.0, c.1 - base.1));
        }
    }

    /// The backward pass: each offset capped by what `rate` (frames a second) brings home by the last step.
    fn home(&mut self, rate: f64) {
        let last = self.offs.len() - 1;
        let mut free = 0.0;
        self.offs[last] = (0.0, 0.0);
        for k in (0..last).rev() {
            if self.marks[k + 1] != HELD {
                free += DT;
            }
            let reach = if free <= HOME_EASE_S {
                free * free / (2.0 * HOME_EASE_S)
            } else {
                free - HOME_EASE_S / 2.0
            };
            let o = &mut self.offs[k];
            let len = o.0.hypot(o.1);
            if len > rate * reach {
                let f = rate * reach / len;
                *o = (o.0 * f, o.1 * f);
            }
        }
    }
}
