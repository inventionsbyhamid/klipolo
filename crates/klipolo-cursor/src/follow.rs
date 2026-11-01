//! The vertical follow-crop: where a Fill crop's centre sits over a take, so a portrait or square version follows the
//! action. It is built once per take and crop size, at 30 Hz, from the **raw** cursor track with its own filter, never
//! the style's smoothing. Its cache key (the caller's) is the crop size, these rules and the activity track's
//! `complete_through`.
//!
//! **Each step** the crop looks at a target (`target`): a click due within the pre-roll, else what is being typed,
//! else a dwell, else the raw cursor. With none (the cursor hidden by an app, outside the picture, before its first
//! sample or burned in) it holds. It then moves (`motion`):
//! - it stays put while the target is inside its central dead zone. Once the target leaves it, the crop eases back
//!   until the target is inside again (a target box larger than the dead zone is kept `safe` from its edges, and one
//!   larger than that room is centred);
//! - it follows critically damped at ω, slowed (never sped) to `speed` crop widths a second, measured on the picture.
//!   While the cursor or the typed text is in the crop's margin (within `safe` of an edge, or past it), the limit rises
//!   toward twice that, across the margin's depth and only along a side the crop can move. Never for a click ahead:
//!   that catch-up fades out over the 0.3 s before a click comes due, so the speed never drops in one step before a
//!   click (it still can when the cursor hides or leaves the picture, or typing starts or ends);
//! - it is kept inside the picture.
//!
//! The picture's shape (for the speed) is the cursor file's area, else the caller's `aspect`, else square: format-1
//! files have no area (0 × 0).
//!
//! **Cost.** Building is O(steps + events): every list is walked once, forward. The one search, whether a click let
//! typing inside playing media count, is done once per pair of a typing span and a media span.
//! The path runs from the take's first cursor sample to its last event inside the take (whose end is its last sample,
//! or its screen files' end if later). Memory is 4 bytes a step (two u16 over 0–1), 0.43 MB an hour, and at most
//! `MAX_STEPS` (16 MB) for any input.
//! Sampling is O(1), linear between steps, with no allocation: a pure function of the take and the time, so preview,
//! scrubbing and export crop the same bits.

use crate::activity::ActivityTrack;
use crate::facts::Dwell;
use crate::{CursorTrack, RegionSpan, Us};

mod aim;
mod motion;
mod target;
mod typed;

use motion::Motion;
use target::Targets;

/// A crop of the source picture: its size as parts of the picture's width and height (≤ 1), the Fill crop divided by
/// `closer`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub w: f64,
    pub h: f64,
}

/// How the crop follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowRules {
    /// The crop holds while its target is inside this central part of it (of its width and of its height), and
    /// brings the target back inside once it leaves.
    pub dead_zone: f64,
    /// Its margins: this part of its width (and height) from each edge. Inside them the crop speeds up toward twice
    /// `speed`, and a box larger than the dead zone is kept out of them.
    pub safe: f64,
    /// The critically damped filter's ω, 1/s.
    pub omega: f64,
    /// The fastest its centre moves while the target is out of its margins: crop widths a second, measured on the
    /// picture.
    pub speed: f64,
    /// A click is looked at from this long before it lands.
    pub pre_roll_us: Us,
}

impl Default for FollowRules {
    fn default() -> Self {
        Self {
            dead_zone: 0.4,
            safe: 0.15,
            omega: 3.0,
            speed: 0.6,
            pre_roll_us: 300_000,
        }
    }
}

/// What a take's follow path is built from, every time on its recording clock.
pub struct FollowInput<'a> {
    /// The raw track.
    pub track: &'a CursorTrack,
    pub dwells: &'a [Dwell],
    /// Typing and playing media: `activity::typing` and `activity::media` of each screen file with its offset added,
    /// each list sorted and disjoint.
    pub typing: &'a [RegionSpan],
    pub media: &'a [RegionSpan],
    /// Each screen file's activity, with where it starts in the take (`SourceRun::offset_us`), in order. Where a typing
    /// span's box is larger than the crop or a quarter of the picture, the crop follows the part typed lately, read
    /// from these; with none there, that span isn't a target.
    pub activity: &'a [(Us, &'a ActivityTrack)],
    /// The take's picture, width over height, for when the cursor file doesn't give its area; 0 if unknown too.
    pub aspect: f64,
    pub crop: Crop,
    pub rules: FollowRules,
}

/// The crop's centre over a take (see the module notes).
#[derive(Debug, Default)]
pub struct FollowPath {
    start: Us,
    /// Half the crop's size: the centre stays this far inside the picture.
    half: (f64, f64),
    xy: Vec<[u16; 2]>,
}

/// The most steps a path stores: 16 MB, about 38 hours. A backstop: events past the take are left out already.
const MAX_STEPS: usize = 4 << 20;
/// The smallest crop side kept (a side that isn't positive is out of range).
const MIN_SIDE: f64 = 1e-3;
const MAX_PRE_ROLL_US: Us = 5_000_000;
const Q: f64 = u16::MAX as f64;

impl Crop {
    /// Each side kept to [`MIN_SIDE`, 1]; one that isn't a number is the whole picture's.
    fn sane(self) -> Self {
        let side = |s: f64| {
            if s.is_nan() {
                1.0
            } else {
                s.clamp(MIN_SIDE, 1.0)
            }
        };
        Self {
            w: side(self.w),
            h: side(self.h),
        }
    }
}

impl FollowRules {
    /// The rules a build follows: a value out of its range, or not a number, is replaced by its default.
    fn sane(self) -> Self {
        let d = Self::default();
        let within =
            |v: f64, lo: f64, hi: f64, or: f64| if (lo..=hi).contains(&v) { v } else { or };
        let pre_roll = (0..=MAX_PRE_ROLL_US).contains(&self.pre_roll_us);
        Self {
            dead_zone: within(self.dead_zone, 0.0, 1.0, d.dead_zone),
            safe: within(self.safe, 0.0, 0.45, d.safe),
            omega: within(self.omega, 0.1, 100.0, d.omega),
            speed: within(self.speed, 0.01, 100.0, d.speed),
            pre_roll_us: if pre_roll {
                self.pre_roll_us
            } else {
                d.pre_roll_us
            },
        }
    }
}

impl FollowPath {
    pub const STEP_US: Us = 33_333;

    /// O(steps + events); 4 bytes a step (≤ 0.5 MB an hour). Deterministic: one pass, f64 in a fixed order.
    pub fn build(input: &FollowInput) -> Self {
        let Some((first, last)) = input.track.span() else {
            return Self::default();
        };
        let (crop, rules) = (input.crop.sane(), input.rules.sane());
        let mut targets = Targets::new(input, crop, rules, (first, last));
        let span = targets.end().max(last).saturating_sub(first);
        let steps = usize::try_from(span / Self::STEP_US)
            .map_or(MAX_STEPS, |n| n.saturating_add(1).min(MAX_STEPS));
        let mut xy = Vec::with_capacity(steps);
        let mut motion = Motion::new(targets.at(first).aim, crop, rules);
        xy.push(quantise(motion.centre()));
        for k in 1..steps {
            let t = first + k as Us * Self::STEP_US;
            let look = targets.at(t);
            let aspect = targets.aspect(t, input.aspect);
            xy.push(quantise(motion.step(look, aspect)));
        }
        Self {
            start: first,
            half: (crop.w / 2.0, crop.h / 2.0),
            xy,
        }
    }

    /// The crop's centre in the source frame (kept so the crop stays inside); None for an empty path. Linear between
    /// steps, the first held before the take's first sample (so a cursor file starting late never makes the crop jump
    /// there) and the last after the end. O(1), no allocation.
    pub fn centre_at(&self, t: Us) -> Option<(f64, f64)> {
        let last = self.xy.len().checked_sub(1)?;
        let since = t.saturating_sub(self.start).max(0);
        let k = usize::try_from(since / Self::STEP_US).unwrap_or(usize::MAX);
        let (a, b, f) = match k {
            k if k < last => {
                let f = (since % Self::STEP_US) as f64 / Self::STEP_US as f64;
                (self.xy[k], self.xy[k + 1], f)
            }
            _ => (self.xy[last], self.xy[last], 0.0),
        };
        let at = |i: usize, half: f64| {
            let (p, q) = (f64::from(a[i]) / Q, f64::from(b[i]) / Q);
            (p + (q - p) * f).clamp(half.min(0.5), (1.0 - half).max(0.5))
        };
        Some((at(0, self.half.0), at(1, self.half.1)))
    }

    /// Its memory, in bytes.
    pub fn bytes(&self) -> usize {
        size_of::<Self>() + self.xy.capacity() * size_of::<[u16; 2]>()
    }
}

/// A centre (inside 0–1) as 16 bits a side, to the nearest.
fn quantise(c: [f64; 2]) -> [u16; 2] {
    c.map(|v| (v.clamp(0.0, 1.0) * Q + 0.5) as u16)
}
