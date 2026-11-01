//! The part typed lately. A typing span's box is the union of all that was typed while it lasted, so a long line or a
//! coding session frames most of an editor. Where that box is larger than the crop or a quarter of the picture, the
//! crop follows the part typed lately instead, read from the screen's activity ticks:
//! - a tick's regions count as typed by the rule `activity::typing` counts typing with (`typed_region`: typing-sized,
//!   clear of playing media, not a caret blinking), so an animation playing inside the box is never followed;
//! - a typed region inside the span's box joins the part while the part still fits the crop's safe room (the crop less
//!   `safe` each side) and a quarter of the picture;
//! - when none can, a new part starts at a region beside the part (within one of its own sizes, as `activity::typing`
//!   joins regions): the typing ran past it. A region farther off starts one only once the part hasn't grown for
//!   `FRESH_TICKS` (a line wrap, another field). So a second place changing beside the text never takes the part;
//! - a region already inside the part changes nothing, so a counter or a label changing in place never keeps a part
//!   fresh and a jump to a new line is still followed. Only a change touching the region that
//!   last grew or started the part (the caret's cell, between slow keystrokes) keeps it fresh without growing it, so a
//!   slow typist's crop doesn't flap over to a far counter.
//!
//! So a part is always kept whole inside the crop's safe room, and as it grows the crop moves no more than it must.
//!
//! Each tick is read once, as the steps reach it: O(1) a step.

use super::Crop;
use super::aim::Aim;
use crate::Us;
use crate::activity::{ActivityTrack, Region, TICK_US, Tick, typed_region};

/// The largest box framed whole, and the largest part, either way: a quarter of the picture.
const QUARTER: f64 = 0.25;
/// A part that grew this many ticks ago or less (1 s) keeps to its own text.
const FRESH_TICKS: u32 = 10;

/// The walk over the screen files' ticks, and the part followed.
pub(super) struct Typed<'a> {
    runs: &'a [(Us, &'a ActivityTrack)],
    /// The screen file the last time asked was in.
    run: usize,
    /// The last tick read: its file and index.
    read: Option<(usize, Us)>,
    /// The last lone cell seen in this screen file and its tick, for the caret rule.
    caret: Option<(Region, u32)>,
    /// The part, and the typing span it belongs to.
    part: Option<(usize, Aim)>,
    /// The tick (in this screen file) the part last grew or started, or something touched what grew it.
    grew: Option<u32>,
    /// The region that last grew or started the part.
    newest: Option<Aim>,
    /// The largest typing box framed whole (the crop and a quarter of the picture), and the largest part.
    whole: [f64; 2],
    most: [f64; 2],
}

impl<'a> Typed<'a> {
    /// `safe`: the crop's margins (`FollowRules::safe`).
    pub(super) fn new(runs: &'a [(Us, &'a ActivityTrack)], crop: Crop, safe: f64) -> Self {
        let room = 1.0 - 2.0 * safe;
        Self {
            runs,
            run: 0,
            read: None,
            caret: None,
            part: None,
            grew: None,
            newest: None,
            whole: [crop.w.min(QUARTER), crop.h.min(QUARTER)],
            most: [(room * crop.w).min(QUARTER), (room * crop.h).min(QUARTER)],
        }
    }

    /// The largest typing box framed whole.
    pub(super) fn whole(&self) -> [f64; 2] {
        self.whole
    }

    /// The part of typing span `span` (its box `within`) typed lately, as of `t`, leaving out changes inside `playing`;
    /// None before any is known.
    pub(super) fn at(
        &mut self,
        t: Us,
        span: usize,
        within: Aim,
        playing: Option<Aim>,
    ) -> Option<Aim> {
        while self.runs.get(self.run + 1).is_some_and(|r| r.0 <= t) {
            (self.run, self.caret, self.grew, self.newest) = (self.run + 1, None, None, None);
        }
        if let Some(&(offset, track)) = self.runs.get(self.run).filter(|r| r.0 <= t) {
            let local = t.saturating_sub(offset);
            let tick = (self.run, local.div_euclid(TICK_US));
            if self.read != Some(tick) {
                self.read = Some(tick);
                if let Some(k) = track.tick_at(local) {
                    let i = u32::try_from(tick.1).unwrap_or(u32::MAX);
                    let found = self.typed_in(&k, i, &within, playing);
                    self.grow(span, found, i);
                }
            }
        }
        self.part.filter(|p| p.0 == span).map(|p| p.1)
    }

    /// The tick's typed regions inside `within`, by `activity::typing`'s rule, larger first.
    fn typed_in(
        &mut self,
        tick: &Tick,
        i: u32,
        within: &Aim,
        playing: Option<Aim>,
    ) -> [Option<Aim>; 2] {
        let plays = |r: &Region| {
            let a = Aim::of_rect(r.rect());
            playing.is_some_and(|m| a.is_some_and(|a| a.overlaps(&m)))
        };
        tick.top.map(|r| {
            let typed = typed_region(r, i, &mut self.caret, plays);
            Aim::of_rect(r.rect()).filter(|a| typed && a.meets(within))
        })
    }

    /// At tick `i`: the part grows with a region that still fits it; else a new part starts at the region nearest it,
    /// if that is beside it or the part hasn't grown lately (the larger region, with no part yet). Regions already
    /// inside the part are left out; one touching the region that last grew it keeps it fresh.
    fn grow(&mut self, span: usize, found: [Option<Aim>; 2], i: u32) {
        let part = self.part.filter(|p| p.0 == span).map(|p| p.1);
        let touches = |n: Aim| found.iter().flatten().any(|r| r.meets(&n));
        if part.is_some() && self.newest.is_some_and(touches) {
            self.grew = Some(i);
        }
        let outside = |r: &Aim| part.is_none_or(|p| p.union(r) != p);
        let mut regions = found.into_iter().flatten().filter(outside);
        let next = match part {
            None => regions.next().map(|r| (r, r)),
            Some(p) => {
                let joined = regions
                    .clone()
                    .map(|r| (p.union(&r), r))
                    .find(|u| u.0.fits(self.most));
                let fresh = self
                    .grew
                    .is_some_and(|g| i.saturating_sub(g) <= FRESH_TICKS);
                let nearest = regions.min_by(|a, b| p.gap(a).total_cmp(&p.gap(b)));
                joined.or(nearest.filter(|r| !fresh || p.beside(r)).map(|r| (r, r)))
            }
        };
        if let Some((next, by)) = next {
            (self.part, self.grew, self.newest) = (Some((span, next)), Some(i), Some(by));
        }
    }
}
