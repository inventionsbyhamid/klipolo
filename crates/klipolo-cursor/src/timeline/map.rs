//! How one recording clip plays its take's facts (`on_timeline`, `cursor_at`): its window on the take's clock, and the
//! facts' pieces on the timeline, joined where a cut falls inside one and capped at `MAX_FACTS` a kind.

use klipolo_model::project::{Project, Recording};
use klipolo_model::time::scaled;
use klipolo_model::timeline::{Clip, ClipSource};

use super::{ClickAt, MAX_FACTS, RegionAt, SpanAt};
use crate::Us;

/// Where one recording clip, starting at `start` on the timeline, plays its take's facts.
pub(super) struct Window {
    start: Us,
    source_in: Us,
    source_out: Us,
    speed: f64,
    pub(super) nudge: Us,
}

/// A recording clip's take and window, when the take kept a cursor and has a screen.
pub(super) fn window<'p>(
    p: &'p Project,
    clip: &Clip,
    start: Us,
) -> Option<(&'p Recording, Window)> {
    let ClipSource::Recording { id } = &clip.source else {
        return None;
    };
    let r = p.recording(id)?;
    r.cursor.as_ref()?;
    let w = Window {
        start,
        source_in: clip.source_in_us,
        source_out: clip.source_out_us,
        speed: clip.speed,
        nudge: r.screen()?.nudge_us,
    };
    Some((r, w))
}

impl Window {
    /// Where source time `s` (inside the clip) plays.
    fn at(&self, s: Us) -> Us {
        self.start + scaled(s - self.source_in, self.speed)
    }

    /// Where recording time `t` plays, if the clip plays it.
    pub(super) fn point(&self, t: Us) -> Option<Us> {
        let s = t.saturating_add(self.nudge);
        (self.source_in..self.source_out)
            .contains(&s)
            .then(|| self.at(s))
    }

    /// The first of `items` (sorted by `at`) that the clip can play.
    pub(super) fn first<T>(&self, items: &[T], at: impl Fn(&T) -> Us) -> usize {
        items.partition_point(|i| at(i).saturating_add(self.nudge) < self.source_in)
    }

    /// The parts of `items`' spans the clip plays, as `piece`s into `out`. Sorted by start, a kind's spans don't
    /// overlap, so only the one before the first starting inside can reach in.
    pub(super) fn spans<T, P: Piece>(
        &self,
        items: &[T],
        span: impl Fn(&T) -> (Us, Us),
        piece: impl Fn((Us, Us), &T) -> P,
        out: &mut Vec<P>,
    ) {
        let from = self.first(items, |i| span(i).0).saturating_sub(1);
        for item in &items[from..] {
            let (a, b) = span(item);
            let (a, b) = (a.saturating_add(self.nudge), b.saturating_add(self.nudge));
            if a >= self.source_out {
                break;
            }
            let (a, b) = (
                self.at(a.max(self.source_in)),
                self.at(b.min(self.source_out)),
            );
            if a < b && !put(out, piece((a, b), item)) {
                break;
            }
        }
    }
}

/// A fact on the timeline. A piece that continues the last (a cut inside it, between clips that play on) joins it.
pub(super) trait Piece {
    fn join(&mut self, next: &Self) -> bool;
}

impl Piece for ClickAt {
    fn join(&mut self, _: &Self) -> bool {
        false
    }
}

impl Piece for SpanAt {
    fn join(&mut self, next: &Self) -> bool {
        let joins = self.to_us == next.from_us && (self.x, self.y) == (next.x, next.y);
        if joins {
            self.to_us = next.to_us;
        }
        joins
    }
}

impl Piece for RegionAt {
    fn join(&mut self, next: &Self) -> bool {
        let joins = self.to_us == next.from_us && self.rect == next.rect;
        if joins {
            self.to_us = next.to_us;
        }
        joins
    }
}

impl Piece for (Us, Us) {
    fn join(&mut self, next: &Self) -> bool {
        let joins = self.1 == next.0;
        if joins {
            self.1 = next.1;
        }
        joins
    }
}

/// Adds `piece` to `list` (or joins it to the last); false once the list holds `MAX_FACTS`.
pub(super) fn put<P: Piece>(list: &mut Vec<P>, piece: P) -> bool {
    if list.last_mut().is_some_and(|last| last.join(&piece)) {
        return true;
    }
    if list.len() >= MAX_FACTS {
        return false;
    }
    list.push(piece);
    true
}
