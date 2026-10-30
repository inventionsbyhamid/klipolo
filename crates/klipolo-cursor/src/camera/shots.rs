//! Focus into shots. The focus in the span, sorted by time, merges greedily: a focus joins
//! the current shot when it starts within `merge_us` of the shot's last event (its end, for a pause or typing) and
//! their box still fits a view at `min_scale`. Each shot zooms the most, up to `max_scale`, that holds its box in the
//! safe zone (a point gets `max_scale`), and lasts from its first event to max(last + `tail_us`, first + `hold_us`).
//! The span's end is the person's, as their own zooms are: a focus lasts at most to it, and the
//! last shot's hold ends where its move out must start to reach the whole frame by then (`for_end`), never before its
//! last event starts, so an Auto zoom ended while the cursor rests or the typing goes on still zooms.
//!
//! The person's own zooms go in as they are. A focus overlapping one is dropped, and so is a box that no view at
//! `min_scale` can hold: a typing span's box covers everything typed while it lasts, so one that
//! outgrew the widest view gets no zoom rather than a crop of part of what was typed.

use super::safe::{self, Box4, Must, NONE, union};
use super::{Fixed, Focus, Input, Rules, View, Weight};
use crate::Us;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Start,
    Auto,
    Fixed,
    End,
}

/// A shot the ladder places: an Auto shot over its focus, the person's own zoom, or an end of the span (the whole
/// frame there). Only Auto shots are ever widened, merged or dropped.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cand {
    pub kind: Kind,
    /// Its first event; where the person's zoom starts; the span's start or end.
    pub from: Us,
    /// Where its view may start moving on: its hold's end, the person's zoom's end, the span's start.
    pub to: Us,
    last: Us,
    /// Its latest event's start: a hold the span's end cuts never ends before it.
    at_last: Us,
    /// When it is reached from the span's start by a late arrival (`moves::late`; `Us::MIN` otherwise): its hold never
    /// ends before it either. The ladder sets it on every link into the shot.
    pub since: Us,
    must: Must,
    pub view: View,
    pub weight: Weight,
    /// Its focus is `[first, end)` of the sorted list; those not dropped are its events.
    pub first: u32,
    pub end: u32,
    /// Zoom out right after its hold: a dropped action between it and the next shot must show.
    pub open: bool,
}

impl Cand {
    fn new(kind: Kind, from: Us, to: Us, view: View) -> Self {
        let weight = Weight::Person;
        Self {
            kind,
            from,
            to,
            last: from,
            at_last: from,
            since: Us::MIN,
            must: Must::of(NONE),
            view,
            weight,
            first: 0,
            end: 0,
            open: false,
        }
    }

    fn fixed(f: &Fixed) -> Self {
        Self::new(Kind::Fixed, f.from, f.to, f.view)
    }

    fn auto(i: usize, f: &Focus, bx: Box4, top: f64, r: &Rules) -> Self {
        let must = Must::of(bx);
        let view = must.view(top, r.safe);
        let (first, end) = (i as u32, i as u32 + 1);
        let to = hold_end(f.at, f.until, r);
        Self {
            kind: Kind::Auto,
            from: f.at,
            to,
            last: f.until,
            at_last: f.at,
            since: Us::MIN,
            must,
            view,
            weight: f.weight,
            first,
            end,
            open: false,
        }
    }

    /// The focus `i` joins this shot if it starts soon enough and their box still fits at `min_scale` (rule 2).
    fn join(&mut self, i: usize, f: &Focus, bx: Box4, r: &Rules) -> bool {
        if f.at > self.last.saturating_add(r.merge_us) {
            return false;
        }
        let u = Must::of(union(self.must.events, bx));
        let Some(top) = u.largest(r.min_scale, r.max_scale, r.safe) else {
            return false;
        };
        (self.must, self.view, self.end) = (u, u.view(top, r.safe), i as u32 + 1);
        self.last = self.last.max(f.until);
        self.at_last = self.at_last.max(f.at);
        self.to = hold_end(self.from, self.last, r);
        self.weight = self.weight.max(f.weight);
        true
    }

    pub(crate) fn droppable(&self) -> bool {
        self.kind == Kind::Auto
    }

    pub(crate) fn is_end(&self) -> bool {
        matches!(self.kind, Kind::Start | Kind::End)
    }

    /// When its view must be settled: its first event less the pre-roll (rule 1); the person's zoom's start; the span's
    /// end.
    pub(crate) fn settle(&self, r: &Rules) -> Us {
        match self.kind {
            Kind::Auto => self.from - r.pre_roll_us,
            _ => self.from,
        }
    }

    /// Its view widened to at most `cap`, never past `min_scale` (an Auto shot; the others keep theirs).
    pub(crate) fn capped(&self, cap: f64, r: &Rules) -> View {
        let s = self.view.scale;
        let to = s.min(cap).max(r.min_scale.min(s));
        if self.kind != Kind::Auto || to == s {
            return self.view;
        }
        self.must.view(to, r.safe)
    }

    /// Framed wider than it would be alone: a conflict, or an action it must show, lowered its scale below the
    /// largest its own events fit.
    pub(crate) fn widened(&self, r: &Rules) -> bool {
        let top = Must::of(self.must.events).largest(r.min_scale, r.max_scale, r.safe);
        self.kind == Kind::Auto && top.is_some_and(|top| self.view.scale < top)
    }

    pub(crate) fn widen_to(&mut self, s: f64, r: &Rules) {
        if self.kind == Kind::Auto && s != self.view.scale {
            self.view = self.must.view(s, r.safe);
        }
    }

    /// From now on its view also shows (x, y), widening it as needed (never above its scale now); false when no
    /// view at `min_scale` can.
    pub(crate) fn see(&mut self, x: f64, y: f64, r: &Rules) -> bool {
        self.must.seen = union(self.must.seen, [x, y, x, y]);
        let top = self
            .must
            .largest(r.min_scale.min(self.view.scale), self.view.scale, r.safe);
        top.map(|s| self.view = self.must.view(s, r.safe)).is_some()
    }

    /// `self` and the later `b` as one shot holding both, if their box fits at `min_scale` (the ladder's merge).
    pub(crate) fn merged(&self, b: &Cand, r: &Rules) -> Option<Cand> {
        if self.kind != Kind::Auto || b.kind != Kind::Auto {
            return None;
        }
        let (e, n) = (
            union(self.must.events, b.must.events),
            union(self.must.seen, b.must.seen),
        );
        let u = Must { events: e, seen: n };
        let top = u.largest(r.min_scale, r.max_scale, r.safe)?;
        let (last, at_last) = (self.last.max(b.last), self.at_last.max(b.at_last));
        let (view, weight) = (u.view(top, r.safe), self.weight.max(b.weight));
        let to = hold_end(self.from, last, r);
        Some(Cand {
            last,
            at_last,
            must: u,
            view,
            weight,
            to,
            end: b.end,
            open: b.open,
            ..*self
        })
    }

    /// Linked to the span's end at `end` by a move of `n`: its hold ends where that move must start, if that is sooner,
    /// but never before its last event starts or its late arrival ends (then the move doesn't fit, and the ladder
    /// widens or drops it). The person's own zooms, and the ends, keep theirs.
    pub(crate) fn for_end(&self, end: Us, n: Us) -> Self {
        let to = match self.kind {
            Kind::Auto => (self.to.min(end.saturating_sub(n)))
                .max(self.at_last)
                .max(self.since),
            _ => self.to,
        };
        Self { to, ..*self }
    }

    pub(crate) fn drop_events(&self, dropped: &mut [bool]) {
        dropped[self.first as usize..self.end as usize].fill(true);
    }
}

fn hold_end(first: Us, last: Us, r: &Rules) -> Us {
    last.saturating_add(r.tail_us)
        .max(first.saturating_add(r.hold_us))
}

/// The focus in the span with what it keeps in view (`plan_keeping`), sorted by time; a flag per focus for those no
/// shot can hold (each kept-in-view action has it from the start: it never gets a shot); and the shots, Auto and the
/// person's own, in time order between the span's two ends. O(n log n).
pub(crate) fn prepare(
    input: &Input,
    keep: &[Focus],
    r: &Rules,
) -> (Vec<Focus>, Vec<bool>, Vec<Cand>) {
    let (start, end) = input.span;
    // An end before the start reads as the start: every focus in the span is kept or dropped, never ignored.
    let in_span = |f: &&Focus| f.at >= start && f.at < end;
    // A focus lasts at most to the span's end.
    let whole = |f: &Focus| Focus {
        until: f.until.max(f.at).min(end),
        ..*f
    };
    let mut all: Vec<(Focus, bool)> = (input.focus.iter().filter(in_span))
        .map(|f| (whole(f), false))
        .chain(keep.iter().filter(in_span).map(|f| (whole(f), true)))
        .collect();
    all.sort_by_key(|(f, _)| (f.at, f.until));
    let (focus, only_seen): (Vec<Focus>, Vec<bool>) = all.into_iter().unzip();
    // A zoom of the person's with no scale of 1 or more, or no number for its centre, is left out (the
    // model keeps 1–5×, and `plan` is public).
    let real =
        |v: View| v.scale >= 1.0 && v.scale.is_finite() && v.cx.is_finite() && v.cy.is_finite();
    let fine = |f: &&Fixed| f.from < f.to && f.to > start && f.from < end && real(f.view);
    let mut fixed: Vec<Fixed> = input.fixed.iter().filter(fine).copied().collect();
    fixed.sort_by_key(|f| f.from);
    // The latest end among the person's zooms starting by each one's start: an overlap is one search.
    let reach: Vec<Us> = fixed
        .iter()
        .scan(Us::MIN, |m, f| {
            *m = (*m).max(f.to);
            Some(*m)
        })
        .collect();
    let mut dropped = only_seen;
    let mut autos: Vec<Cand> = Vec::new();
    let (mut cur, mut fence): (Option<Cand>, Us) = (None, Us::MAX);
    for (i, f) in focus.iter().enumerate() {
        if dropped[i] {
            continue;
        }
        let n = fixed.partition_point(|x| x.from <= f.until);
        let bx = safe::box_of(f.rect);
        let top = Must::of(bx).largest(r.min_scale, r.max_scale, r.safe);
        let (Some(top), false) = (top, n > 0 && reach[n - 1] > f.at) else {
            dropped[i] = true;
            continue;
        };
        // Never across the person's zoom: the next one starting after the shot's first event fences it.
        if let Some(c) = &mut cur {
            if f.at < fence && c.join(i, f, bx, r) {
                continue;
            }
            autos.push(*c);
        }
        cur = Some(Cand::auto(i, f, bx, top, r));
        fence = fixed
            .get(fixed.partition_point(|x| x.from <= f.at))
            .map_or(Us::MAX, |x| x.from);
    }
    autos.extend(cur);
    let mut cands = Vec::with_capacity(autos.len() + fixed.len() + 2);
    cands.push(Cand::new(Kind::Start, start, start, View::FULL));
    let mut theirs = fixed.iter().map(Cand::fixed).peekable();
    for a in autos {
        while let Some(f) = theirs.next_if(|f| f.from <= a.from) {
            cands.push(f);
        }
        cands.push(a);
    }
    cands.extend(theirs);
    cands.push(Cand::new(Kind::End, end, end, View::FULL));
    (focus, dropped, cands)
}
