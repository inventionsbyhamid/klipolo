//! The schedule and the keys. A glide ends exactly at the next shot's settle time (its
//! first event less the pre-roll); an out-and-in goes out at the earlier shot's hold end and comes in to arrive then;
//! out of the person's own zoom, or out of a shot that must zoom out after its hold when the next isn't the
//! person's zoom (never reached early from an Auto shot), the move starts as that hold ends. Each move eases from one view to the next and the view holds between moves, so the plan is sampled by one
//! binary search over its keys, with no allocation.

use super::safe::box_of;
use super::shots::{Cand, Kind};
use super::{Dropped, Focus, Plan, Rules, Shot, View, ease, glide};
use crate::{Rect01, Us};

/// One eased move from view `a` at `from` to view `b` at `to` (a cut when `from == to`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Move {
    pub from: Us,
    pub to: Us,
    pub a: View,
    pub b: View,
}

impl Move {
    /// None when there is nothing to move.
    fn new(from: Us, to: Us, a: View, b: View) -> Option<Self> {
        (a != b).then_some(Self { from, to, a, b })
    }
}

/// How the camera gets from one kept shot to the next (a glide is one move, an out-and-in two), and whether it took
/// an out-and-in to make it legal.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Link {
    pub moves: Moves,
    pub out_and_in: bool,
}

/// The moves of a link: a glide's one, an out-and-in's two.
pub(crate) type Moves = [Option<Move>; 2];

/// The kept shots in time order, each with the link into it (the span's start first, with none).
pub(crate) type Kept = Vec<(Cand, Link)>;

/// A glide taking `n` from `a` (at `va`) to `b` (at `vb`): as late as it can be, or as soon as `a`'s hold ends when
/// it is the person's zoom, or `open` and not heading into the person's zoom (which shows exactly over its own span,
/// so the camera never reaches it early).
pub(crate) fn glide(a: &Cand, b: &Cand, (va, vb): (View, View), n: Us, r: &Rules) -> Moves {
    let to = if a.kind == Kind::Fixed || (a.open && b.kind != Kind::Fixed) {
        a.to.saturating_add(n)
    } else {
        b.settle(r)
    };
    [Move::new(to - n, to, va, vb), None]
}

/// Out to the whole frame over `n` from `a`'s hold end, then in over `m`, arriving at `b`'s settle time.
pub(crate) fn out_and_in(
    a: &Cand,
    b: &Cand,
    (va, vb): (View, View),
    (n, m): (Us, Us),
    r: &Rules,
) -> Moves {
    let settle = b.settle(r);
    [
        Move::new(a.to, a.to.saturating_add(n), va, View::FULL),
        Move::new(settle - m, settle, View::FULL, vb),
    ]
}

/// From the span's start into its first shot when no move fits before the shot's pre-roll: as soon as the move allows,
/// arriving after the shot's first event. The span's start is the person's, as its end is:
/// a zoom whose click comes right at its start still zooms on it, late.
pub(crate) fn late(a: &Cand, (va, vb): (View, View), n: Us) -> Link {
    Link {
        moves: [Move::new(a.to, a.to.saturating_add(n), va, vb), None],
        ..Link::default()
    }
}

/// Between two of the person's zooms (or an end) closer than their move needs: over the `n` there is, as placed.
pub(crate) fn squeezed(b: &Cand, (va, vb): (View, View), n: Us, r: &Rules) -> Link {
    let to = b.settle(r);
    Link {
        moves: [Move::new(to - n, to, va, vb), None],
        ..Link::default()
    }
}

/// A key of the path: from `at` the view is `view`, eased (a quintic on the centre and on ln(scale)) to the next key's
/// view when `moving`, else held until the next key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub at: Us,
    pub view: View,
    pub moving: bool,
}

/// A kept shot's events (`[first, first + n)` of the plan's), when its view is reached (the end of the move into it;
/// after its first event only for a late arrival from the span's start, `late`), and when its view stops holding: the
/// start of the move out of it, where its glide must be home.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hold {
    pub first: u32,
    pub n: u32,
    pub since: Us,
    pub until: Us,
}

pub(crate) fn keys(kept: &Kept) -> Vec<Key> {
    let ends = |m: &Move| {
        [
            Key {
                at: m.from,
                view: m.a,
                moving: true,
            },
            Key {
                at: m.to,
                view: m.b,
                moving: false,
            },
        ]
    };
    kept.iter()
        .flat_map(|k| k.1.moves.iter().flatten())
        .flat_map(ends)
        .collect()
}

/// The plan's keys, shots, holds and events from the kept shots (the span's ends first and last).
pub(crate) fn fill(p: &mut Plan, kept: &Kept, focus: &[Focus], dropped: &[bool]) {
    p.keys = keys(kept);
    for (i, (c, link_in)) in kept.iter().enumerate().filter(|(_, k)| !k.0.is_end()) {
        let since = (link_in.moves.iter().flatten().last()).map_or(Us::MIN, |m| m.to);
        let first = p.events.len() as u32;
        let range = c.first as usize..c.end as usize;
        for (f, _) in focus[range.clone()]
            .iter()
            .zip(&dropped[range])
            .filter(|(_, d)| !**d)
        {
            let b = box_of(f.rect);
            p.events.push((
                f.at,
                ((b[0] + b[2]) / 2.0) as f32,
                ((b[1] + b[3]) / 2.0) as f32,
            ));
            p.boxes.push((f.at, f.until, f.rect));
        }
        let n = p.events.len() as u32 - first;
        let (next, out) = &kept[i + 1];
        let until = out.moves[0].map_or(next.settle(&p.rules), |m| m.from);
        let (from, to, view, weight) = (c.from, c.to, c.view, c.weight);
        p.shots.push(Shot {
            from,
            to,
            view,
            weight,
            events: n.min(u16::MAX as u32) as u16,
        });
        p.holds.push(Hold {
            first,
            n,
            since,
            until,
        });
    }
}

/// The view the keys give at `t`, before any glide. A moving key is read only before the next key's time (a move of no
/// length is a cut), so the division is never by 0.
pub(crate) fn path(keys: &[Key], t: Us) -> View {
    let i = keys.partition_point(|k| k.at <= t);
    let Some(k) = i.checked_sub(1).map(|i| keys[i]) else {
        return View::FULL;
    };
    match keys.get(i) {
        Some(n) if k.moving => {
            ease::between(k.view, n.view, (t - k.at) as f64 / (n.at - k.at) as f64)
        }
        _ => k.view,
    }
}

impl Plan {
    /// The view at `t` (FULL outside every shot and move). O(log n), no allocation.
    pub fn view_at(&self, t: Us) -> View {
        let v = path(&self.keys, t);
        match self.glide.at(t) {
            Some((dx, dy)) => View {
                cx: v.cx + dx,
                cy: v.cy + dy,
                ..v
            },
            None => v,
        }
    }

    pub fn shots(&self) -> &[Shot] {
        &self.shots
    }

    /// The events each kept shot holds (their times and points: a box's centre), in time order.
    pub fn events(&self) -> &[(Us, f32, f32)] {
        &self.events
    }

    /// The path's keys, in time order (chat cuts its layers where the whole frame holds).
    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    pub fn ladder(&self) -> Dropped {
        self.ladder
    }

    /// Adds the safe-zone glide inside each shot from the RAW cursor, precomputed at 60 Hz with its own filter.
    pub fn with_glide(mut self, raw_cursor: &dyn Fn(Us) -> Option<(f32, f32)>) -> Self {
        self.glide = glide::build(&self, raw_cursor);
        self
    }

    /// What the plan holds in memory.
    pub fn bytes(&self) -> usize {
        // The copy of the input `check_plan` reads, in test builds only.
        #[cfg(feature = "test-util")]
        let all = self.all.capacity() * size_of::<Focus>();
        #[cfg(not(feature = "test-util"))]
        let all = 0;
        size_of_val(self)
            + all
            + self.keys.capacity() * size_of::<Key>()
            + self.shots.capacity() * size_of::<Shot>()
            + self.holds.capacity() * size_of::<Hold>()
            + self.events.capacity() * size_of::<(Us, f32, f32)>()
            + self.in_view.capacity() * size_of::<(Us, Us, f32, f32)>()
            + self.boxes.capacity() * size_of::<(Us, Us, Rect01)>()
            + self.glide.bytes()
    }
}
