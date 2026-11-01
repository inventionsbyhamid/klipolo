//! The speaker's path for one format's crop, from a tracker's boxes (`TrackedFrame`s, each with its time in the clip's
//! source time: from one tracked stretch or several):
//!
//! - **Shots** run between the cuts. Each picks its own subject (`main_subject`): the person with the most area near
//!   the middle over the shot, so the largest, most central one. Someone else who is there for most of the shot and at
//!   least half as big joins them while both fit in the crop (a two-person shot is framed together).
//! - **The aim** on a frame is the subject's middle across and, down, a crop whose top sits just above their head
//!   (`HEADROOM`): their head and upper body. Between two frames it moves straight from one to the next; while the
//!   subject is gone it holds where they were last, and before they first show, where they first do.
//! - **The crop** starts each shot at rest where the subject is in its first two seconds, and holds while the aim stays
//!   inside its central dead zone. Once the aim leaves it, the crop moves (critically damped at `OMEGA`, through the
//!   cursor follow's own filter) until it is back on the aim, never faster than `SPEED` crop widths a second and never
//!   turning back mid-move, so a tracker's wobble can't make it jitter.
//! - **Keys** every `KEY_STEP_US` from each shot's start while the crop moves, and only the first and last of a hold
//!   (the crop's centre goes straight between them, so it draws the same), so a long still talk stays a few keys a
//!   shot. A shot's first key is a cut, so the crop jumps there and never pans from one shot to the next. A shot with
//!   nobody in it holds the middle.

use klipolo_cursor::smooth::step;
use klipolo_model::fill_focus::{FocusKey, FocusPath};
use klipolo_model::time::Us;

use crate::providers::{TrackedBox, TrackedFrame};

/// The crop holds while the aim is within this part of its size around its centre (half of it each side).
pub const DEAD_ZONE: f64 = 0.4;
/// A move ends once the crop is this close to the aim, in crop sizes.
pub const SETTLE: f64 = 0.04;
/// The fastest the crop moves, in crop sizes a second.
pub const SPEED: f64 = 0.6;
/// The follower's ω, 1/s (the cursor follow's).
pub const OMEGA: f64 = 3.0;
/// Room above the head, as a part of the crop's height.
pub const HEADROOM: f64 = 0.1;
/// A key every quarter second.
pub const KEY_STEP_US: Us = 250_000;
/// Boxes the tracker is less sure of than this are left out.
pub const MIN_CONFIDENCE: f32 = 0.4;
/// The crop starts each shot where the subject is over this long.
const START_US: Us = 2_000_000;
/// The motion's step.
const DT_US: Us = 1_000_000 / 30;
/// Two frames further apart than this aren't moved between: the aim holds.
const MAX_GAP_US: Us = 1_000_000;

/// What a path is made from.
#[derive(Clone, Copy, Debug)]
pub struct PathInput<'a> {
    /// The tracker's frames, each at its time in the clip's source time, in time order.
    pub frames: &'a [(Us, &'a TrackedFrame)],
    /// Where the footage the frames cover starts, in the same time.
    pub start_us: Us,
    /// Where the footage the frames cover ends, in the same time.
    pub end_us: Us,
    /// The footage's cuts (each the first frame of a new shot), sorted, in the same time.
    pub cuts: &'a [Us],
    /// The crop's size, as parts of the picture's width and height.
    pub crop: (f64, f64),
}

/// The path, or None when nobody was found on any frame.
pub fn focus_path(input: &PathInput) -> Option<FocusPath> {
    if input.end_us <= input.start_us {
        return None;
    }
    let crop = (sane_side(input.crop.0), sane_side(input.crop.1));
    let timed = input.frames;
    let shots = shots(input.start_us, input.end_us, input.cuts);
    // Fewer keys a second on very long footage, to stay within the path's cap.
    let room = FocusPath::MAX_KEYS.saturating_sub(shots.len()).max(1) as Us;
    let key_step = KEY_STEP_US.max((input.end_us - input.start_us) / room + 1);

    let mut keys = Vec::new();
    let mut found = false;
    for (start, end) in shots {
        let in_shot: Vec<(Us, &TrackedFrame)> = (timed.iter())
            .filter(|(t, _)| (start..end).contains(t))
            .copied()
            .collect();
        let aims = aims(&in_shot, crop);
        found |= !aims.is_empty();
        let first = keys.len();
        if aims.is_empty() {
            keys.push(key(start, (0.5, 0.5)));
        } else {
            follow(&aims, (start, end), crop, key_step, &mut keys);
        }
        if first > 0 {
            keys[first].cut = true;
        }
    }
    found.then_some(FocusPath { keys })
}

/// The id of the shot's main subject in `frames`: the most area near the middle across the frames (a box's area times
/// how near its middle is to the picture's, from 1 there to 0.5 at an edge). None when nobody is there.
pub fn main_subject(frames: &[TrackedFrame]) -> Option<u32> {
    let mut scores: Vec<(u32, f64)> = Vec::new();
    for b in frames
        .iter()
        .flat_map(|f| f.boxes.iter())
        .filter(|b| sure(b))
    {
        let score = b.width() * b.height() * (1.0 - (b.centre_x() - 0.5).abs());
        match scores.iter_mut().find(|(id, _)| *id == b.id) {
            Some((_, s)) => *s += score,
            None => scores.push((b.id, score)),
        }
    }
    scores
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(id, _)| id)
}

fn sure(b: &TrackedBox) -> bool {
    b.confidence.is_none_or(|c| c >= MIN_CONFIDENCE) && b.width() > 0.0 && b.height() > 0.0
}

fn sane_side(side: f64) -> f64 {
    if side.is_finite() {
        side.clamp(1e-3, 1.0)
    } else {
        1.0
    }
}

/// The shots between `start` and `end`, split at the cuts strictly inside.
fn shots(start: Us, end: Us, cuts: &[Us]) -> Vec<(Us, Us)> {
    let mut edges = vec![start];
    edges.extend(cuts.iter().copied().filter(|c| *c > start && *c < end));
    edges.sort_unstable();
    edges.dedup();
    edges.push(end);
    edges.windows(2).map(|w| (w[0], w[1])).collect()
}

/// The aim on each frame of a shot where its subject (and anyone framed with them) is: (time, x, y).
fn aims(frames: &[(Us, &TrackedFrame)], crop: (f64, f64)) -> Vec<(Us, f64, f64)> {
    let plain: Vec<TrackedFrame> = frames.iter().map(|(_, f)| (*f).clone()).collect();
    let Some(main) = main_subject(&plain) else {
        return Vec::new();
    };
    let together = companions(&plain, main);
    let mut out = Vec::new();
    for (t, frame) in frames {
        let Some(subject) = frame.boxes.iter().find(|b| b.id == main && sure(b)) else {
            continue;
        };
        let mut group = *subject;
        for other in frame
            .boxes
            .iter()
            .filter(|b| together.contains(&b.id) && sure(b))
        {
            let joined = TrackedBox {
                left: group.left.min(other.left),
                top: group.top.min(other.top),
                right: group.right.max(other.right),
                bottom: group.bottom.max(other.bottom),
                ..group
            };
            // Only while both fit in the crop, with a little room.
            if joined.width() <= crop.0 * 0.9 {
                group = joined;
            }
        }
        let x = group.centre_x();
        let y = if crop.1 >= 0.999 {
            0.5
        } else {
            group.top - HEADROOM * crop.1 + crop.1 / 2.0
        };
        out.push((*t, x, y));
    }
    out
}

/// Who is framed with `main`: there on at least half the frames, and at least half its size on average.
fn companions(frames: &[TrackedFrame], main: u32) -> Vec<u32> {
    let mut seen: Vec<(u32, usize, f64)> = Vec::new();
    for b in frames
        .iter()
        .flat_map(|f| f.boxes.iter())
        .filter(|b| sure(b))
    {
        let area = b.width() * b.height();
        match seen.iter_mut().find(|(id, _, _)| *id == b.id) {
            Some((_, n, sum)) => {
                *n += 1;
                *sum += area;
            }
            None => seen.push((b.id, 1, area)),
        }
    }
    let mean = |id: u32| {
        (seen.iter().find(|(i, _, _)| *i == id)).map_or(0.0, |(_, n, sum)| sum / *n as f64)
    };
    let main_area = mean(main);
    seen.iter()
        .filter(|(id, n, sum)| {
            *id != main && *n * 2 >= frames.len() && sum / *n as f64 >= main_area * 0.5
        })
        .map(|(id, _, _)| *id)
        .collect()
}

/// The aim at `t`: straight between two frames close enough, else the nearest frame's before (or the first's).
fn aim_at(aims: &[(Us, f64, f64)], t: Us) -> (f64, f64) {
    let n = aims.partition_point(|a| a.0 <= t);
    if n == 0 {
        return (aims[0].1, aims[0].2);
    }
    let a = aims[n - 1];
    match aims.get(n) {
        Some(b) if b.0 - a.0 <= MAX_GAP_US && b.0 > a.0 => {
            let k = (t - a.0) as f64 / (b.0 - a.0) as f64;
            (a.1 + (b.1 - a.1) * k, a.2 + (b.2 - a.2) * k)
        }
        _ => (a.1, a.2),
    }
}

/// One axis of the crop as it follows.
struct Axis {
    c: f64,
    v: f64,
    size: f64,
    /// Moving toward the aim (+1 or -1), or at rest (0).
    moving: f64,
}

impl Axis {
    /// The crop's centre kept where the crop stays inside the picture.
    fn inside(&self, c: f64) -> f64 {
        let half = self.size / 2.0;
        c.clamp(half, 1.0 - half)
    }

    fn step(&mut self, aim: f64, dt: f64) {
        let aim = self.inside(aim);
        let off = aim - self.c;
        if self.moving == 0.0 {
            if off.abs() <= self.size * DEAD_ZONE / 2.0 {
                return;
            }
            self.moving = off.signum();
        }
        // Never back: an aim behind the move ends it where it is.
        let want = if off * self.moving < 0.0 { self.c } else { aim };
        let (mut next, mut v) = step(self.c, self.v, want, OMEGA, dt);
        let most = SPEED * self.size * dt;
        let moved = (next - self.c).clamp(-most, most);
        if moved * self.moving < 0.0 {
            (next, v) = (self.c, 0.0);
        } else {
            next = self.c + moved;
            v = v.clamp(-SPEED * self.size, SPEED * self.size);
        }
        self.c = self.inside(next);
        self.v = v;
        if (aim - self.c).abs() <= self.size * SETTLE || off * self.moving < 0.0 {
            self.moving = 0.0;
            self.v = 0.0;
        }
    }
}

/// The keys of one shot with a subject, from `start` (its first key) every `key_step` until `end`.
fn follow(
    aims: &[(Us, f64, f64)],
    (start, end): (Us, Us),
    crop: (f64, f64),
    key_step: Us,
    keys: &mut Vec<FocusKey>,
) {
    let early: Vec<&(Us, f64, f64)> = aims.iter().filter(|a| a.0 < start + START_US).collect();
    let opening = if early.is_empty() {
        (aims[0].1, aims[0].2)
    } else {
        (
            median(early.iter().map(|a| a.1)),
            median(early.iter().map(|a| a.2)),
        )
    };
    let mut x = Axis {
        c: 0.5,
        v: 0.0,
        size: crop.0,
        moving: 0.0,
    };
    let mut y = Axis {
        c: 0.5,
        v: 0.0,
        size: crop.1,
        moving: 0.0,
    };
    x.c = x.inside(opening.0);
    y.c = y.inside(opening.1);
    let dt = DT_US as f64 / 1e6;
    let (mut t, mut next_key) = (start, start);
    while next_key < end {
        while t < next_key {
            let aim = aim_at(aims, t);
            x.step(aim.0, dt);
            y.step(aim.1, dt);
            t += DT_US;
        }
        keys.push(key(next_key, (x.c, y.c)));
        next_key += key_step;
    }
    let first = keys.len() - shot_keys(keys, start);
    drop_holds(keys, first);
}

/// How many of `keys` are this shot's: from `start` on.
fn shot_keys(keys: &[FocusKey], start: Us) -> usize {
    keys.iter().rev().take_while(|k| k.at_us >= start).count()
}

/// Keeps only the first and last key of each run of equal keys from `first` on: the centre goes straight between
/// them, so the crop draws the same.
fn drop_holds(keys: &mut Vec<FocusKey>, first: usize) {
    let same = |a: &FocusKey, b: &FocusKey| a.x == b.x && a.y == b.y;
    let shot: Vec<FocusKey> = keys.split_off(first);
    let n = shot.len();
    for i in 0..n {
        let inside =
            i > 0 && i + 1 < n && same(&shot[i - 1], &shot[i]) && same(&shot[i], &shot[i + 1]);
        if !inside {
            keys.push(shot[i]);
        }
    }
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn key(at_us: Us, (x, y): (f64, f64)) -> FocusKey {
    FocusKey {
        at_us,
        x: x.clamp(0.0, 1.0),
        y: y.clamp(0.0, 1.0),
        cut: false,
    }
}
