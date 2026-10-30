//! What the crop looks at, step by step: a click due within the pre-roll, else what is being typed,
//! else a dwell, else the raw cursor. With it comes what the crop may speed up for (`Look::pull`
//! past the dead zone): the text being typed, or the raw cursor under a dwell or itself; never a click still to come,
//! where the cursor isn't yet. That catch-up fades out over the 0.3 s before a click comes due (`Look::boost`, the
//! C7 fix), so the crop slows to its limit before turning to the click instead of in one step. Every list
//! is walked forward once, so a build is O(steps + events).
//!
//! **Typing before a dwell.** Typing needs the cursor still, so a typing span lies inside one still run, and a dwell
//! over it covers all of it. Clicking a field and typing leaves no dwell (no travel since the press), but clicking it
//! and moving the mouse aside first does. Ranked first, that dwell would keep the crop on the parked cursor while the
//! text ran out of a narrow crop. The camera ranks them the same way (`camera::Weight`).
//!
//! **Left out:**
//! - a click or a dwell outside the picture, or not a number;
//! - a typing box centred inside playing media: those changes aren't typing (`activity::typing` already leaves out
//!   every player, before the media spans merge, so the smaller of two players missing from `media` costs nothing).
//!   Playing media is ignored unless clicked: a click inside it from `CLICKED_US` before the typing
//!   on lets the typing count, and its changes count as typed;
//! - the raw cursor and dwells while an app hides the cursor, the raw cursor outside the picture, and after a burn-in
//!   past its last sample (the frames show the cursor there; the file doesn't follow it);
//! - events past the take, its last sample or its screen files' end, whichever is later.

use super::aim::Aim;
use super::typed::Typed;
use super::{Crop, FollowInput, FollowRules};
use crate::facts::Dwell;
use crate::track::Walk;
use crate::{RegionSpan, Us};

/// What the crop looks at (None: hold), and what it may move faster for while that lies in its margin.
#[derive(Clone, Copy, Debug)]
pub(super) struct Look {
    pub(super) aim: Option<Aim>,
    pub(super) pull: Option<Aim>,
    /// How much of that catch-up is left, 0–1: 1 until a click is 0.3 s from coming due, then down to 0 as it does.
    pub(super) boost: f64,
}

/// A forward walk over a list in time order, each item over `[from, to)` as `span` gives it.
struct Ahead<'a, T> {
    items: &'a [T],
    next: usize,
}

impl<'a, T> Ahead<'a, T> {
    fn new(items: &'a [T]) -> Self {
        Self { items, next: 0 }
    }

    /// The item over `t`, and its index; `t` never goes back, so items over by `t` are passed for good. A list out of
    /// order only loses items, and an item whose span is empty or reversed is never over a time.
    fn at(&mut self, t: Us, span: impl Fn(&T) -> (Us, Us)) -> Option<(usize, &'a T)> {
        while self.items.get(self.next).is_some_and(|x| span(x).1 <= t) {
            self.next += 1;
        }
        let x = self.items.get(self.next)?;
        (span(x).0 <= t).then_some((self.next, x))
    }
}

/// The walks over a take's events (see the module notes), and its cursor file's areas.
pub(super) struct Targets<'a> {
    walk: Walk<'a>,
    /// The primary clicks inside the picture, by time.
    clicks: Vec<(Us, Aim)>,
    next_click: usize,
    pre_roll: Us,
    dwells: Ahead<'a, Dwell>,
    typing: Ahead<'a, RegionSpan>,
    media: Ahead<'a, RegionSpan>,
    hidden: Ahead<'a, (Us, Us)>,
    typed: Typed<'a>,
    /// From here the file no longer follows the cursor: a burn-in, past the last sample.
    lost_from: Us,
    /// The take's end: its last sample, or its screen files' end if later. Events past it are left out.
    take_end: Us,
    /// Whether a click let the typing span and media span of this pair count (`clicked`), worked out once.
    clicked: Option<((usize, usize), bool)>,
    areas: &'a [(Us, f32, f32)],
    /// The area changes at or before the last time asked.
    area: usize,
    head_area: (f32, f32),
}

/// The catch-up fades out over this long before a click comes due, whatever the pre-roll.
const FADE_US: Us = 300_000;
/// A click inside playing media this long before typing there (or during it) lets the typing count.
const CLICKED_US: Us = 1_000_000;
/// Picture shapes outside this are taken as this (width over height).
const ASPECTS: (f64, f64) = (0.1, 10.0);

impl<'a> Targets<'a> {
    pub(super) fn new(
        input: &FollowInput<'a>,
        crop: Crop,
        rules: FollowRules,
        (first, last): (Us, Us),
    ) -> Self {
        let track = input.track;
        let clicks = track
            .clicks()
            .filter_map(|p| Some((p.down, Aim::point(p.x, p.y)?)))
            .collect();
        let lost_from = track
            .burned_from()
            .map_or(Us::MAX, |b| b.max(last.saturating_add(1)));
        let ends = input
            .activity
            .iter()
            .map(|&(at, a)| at.saturating_add(a.duration_us()));
        Self {
            walk: Walk::new(track, first),
            clicks,
            next_click: 0,
            pre_roll: rules.pre_roll_us,
            dwells: Ahead::new(input.dwells),
            typing: Ahead::new(input.typing),
            media: Ahead::new(input.media),
            hidden: Ahead::new(track.hidden()),
            typed: Typed::new(input.activity, crop, rules.safe),
            lost_from,
            take_end: ends.fold(last, Us::max),
            clicked: None,
            areas: track.area_changes(),
            area: 0,
            head_area: track.header().map_or((0.0, 0.0), |h| h.area_pts),
        }
    }

    /// The last time any event in the take asks the crop to look somewhere.
    pub(super) fn end(&self) -> Us {
        let clicks = self.clicks.last().map(|c| c.0);
        let dwells = self.dwells.items.iter().map(|d| d.to).max();
        let typing = self.typing.items.iter().map(|s| s.to).max();
        [clicks, dwells, typing]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(Us::MIN)
            .min(self.take_end)
    }

    /// What the crop looks at at `t` (never earlier than the last time asked).
    pub(super) fn at(&mut self, t: Us) -> Look {
        let typing = self.typing_at(t);
        let (click, boost) = self.click_at(t);
        let shown = self.hidden.at(t, |h| *h).is_none();
        let cursor = self.cursor_at(t).filter(|_| shown);
        let dwell = self.dwells.at(t, |d| (d.from, d.to.saturating_add(1)));
        let dwell = dwell.and_then(|(_, d)| Aim::point(d.x, d.y));
        match (click, typing) {
            (Some(c), _) => Look {
                aim: Some(c),
                pull: None,
                boost,
            },
            (None, Some(typed)) => Look {
                aim: Some(typed),
                pull: Some(typed),
                boost,
            },
            (None, None) => {
                let aim = dwell.filter(|_| shown).or(cursor);
                Look {
                    aim,
                    pull: aim.and(cursor),
                    boost,
                }
            }
        }
    }

    /// The picture's width over its height at `t`: the cursor file's area, else `picture`, else square.
    pub(super) fn aspect(&mut self, t: Us, picture: f64) -> f64 {
        while self.areas.get(self.area).is_some_and(|a| a.0 <= t) {
            self.area += 1;
        }
        let (w, h) = self
            .area
            .checked_sub(1)
            .map_or(self.head_area, |i| (self.areas[i].1, self.areas[i].2));
        let known = |a: f64| a.is_finite() && a > 0.0;
        let area = f64::from(w) / f64::from(h);
        let aspect = [area, picture]
            .into_iter()
            .find(|&a| known(a))
            .unwrap_or(1.0);
        aspect.clamp(ASPECTS.0, ASPECTS.1)
    }

    /// A click due at `t` or within the pre-roll after it; and the catch-up left before it (`Look::boost`), falling
    /// linearly to 0 over the `FADE_US` before the click comes due, so the limit never steps down for it.
    fn click_at(&mut self, t: Us) -> (Option<Aim>, f64) {
        while self.clicks.get(self.next_click).is_some_and(|c| c.0 < t) {
            self.next_click += 1;
        }
        let Some(&(at, c)) = self.clicks.get(self.next_click) else {
            return (None, 1.0);
        };
        // How far the click lies beyond the pre-roll.
        let beyond = at.saturating_sub(t).saturating_sub(self.pre_roll);
        if beyond <= 0 {
            return (Some(c), 0.0);
        }
        (None, (beyond as f64 / FADE_US as f64).min(1.0))
    }

    /// What is being typed at `t`: the span's box, or the part typed lately where the box is too large.
    fn typing_at(&mut self, t: Us) -> Option<Aim> {
        let (k, span) = self.typing.at(t, |s| (s.from, s.to))?;
        let typed = Aim::of_rect(span.rect)?;
        let media = self.media.at(t, |m| (m.from, m.to));
        let mut playing = media.and_then(|(j, m)| Some((j, Aim::of_rect(m.rect)?)));
        if let Some((j, m)) = playing
            && self.clicked((k, j), &m, span)
        {
            playing = None;
        }
        let playing = playing.map(|p| p.1);
        if playing.is_some_and(|m| m.holds(typed.mid(0), typed.mid(1))) {
            return None;
        }
        if typed.fits(self.typed.whole()) {
            return Some(typed);
        }
        self.typed.at(t, k, typed, playing)
    }

    /// A click landed inside playing box `m` from `CLICKED_US` before typing span `span` to its end; `pair` is the
    /// (typing span, media span) it is worked out for, once: O(log clicks), plus the clicks there.
    fn clicked(&mut self, pair: (usize, usize), m: &Aim, span: &RegionSpan) -> bool {
        if let Some((seen, hit)) = self.clicked
            && seen == pair
        {
            return hit;
        }
        let first = self
            .clicks
            .partition_point(|c| c.0 < span.from.saturating_sub(CLICKED_US));
        let mut over = self.clicks[first..].iter().take_while(|c| c.0 < span.to);
        let hit = over.any(|c| c.1.meets(m));
        self.clicked = Some((pair, hit));
        hit
    }

    /// The raw cursor at `t`, where the file follows it and it is inside the picture.
    fn cursor_at(&mut self, t: Us) -> Option<Aim> {
        if t >= self.lost_from {
            return None;
        }
        let (x, y) = self.walk.at(t)?;
        Aim::point(x, y)
    }
}
