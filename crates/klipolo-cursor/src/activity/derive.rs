//! What the ticks mean: typing, a focus for the camera and the follow-crop, and playing media, their
//! exclusion. Each is one pass over the ticks: a few `Slot`s follow a region from tick to tick, each counting the
//! ticks it changed in over a sliding window held as a bit ring. O(ticks), and memory only for the spans found.
//!
//! Both lists come out sorted and disjoint in time, as the timeline mapping needs: where two spans overlap they
//! become one, with the larger region.

mod slot;

use slot::{Found, Slot};

use super::{ActivityTrack, Region, TICK_US};
use crate::facts::StillRuns;
use crate::{RegionSpan, Us};

/// Typing: a region at most this many cells across and down…
const TYPING_MAX: (u8, u8) = (6, 4);
/// …changing in at least 3 of every 10 ticks (3 a second)…
const TYPING_RATE: (u32, u32) = (3, 10);
/// …for at least 1.5 s, while the cursor is still.
const TYPING_MIN_TICKS: u32 = 15;
/// Typing places followed at once.
const TYPING_SLOTS: usize = 2;
/// A caret: a lone cell (1 across, at most 2 down) changing again in the same place at least this many ticks after it
/// last did: 2 Hz or slower, with a tick of sampling jitter.
const CARET_GAP: u32 = 4;
/// Media: one box changing in at least 80 of every 100 ticks…
const MEDIA_RATE: (u32, u32) = (80, 100);
/// …for at least 10 s.
const MEDIA_MIN_TICKS: u32 = 100;
/// A region this small never starts a media box (a caret, a glyph).
const MEDIA_MIN_CELLS: u16 = 4;
/// A region changed inside a media box when the boxes' intersection over union is at least 1/4. It grows the box only
/// at 1/2 or more, so the box stays steady: a region that keeps moving soon matches nothing.
const SAME_BOX: (u32, u32) = (1, 4);
const GROWS_BOX: (u32, u32) = (1, 2);
const MEDIA_SLOTS: usize = 4;

/// Small regions (≤ 6 × 4 cells) changing ≥ 3 ticks a second for ≥ 1.5 s while the cursor is still, merged while they
/// stay within one region of each other; a lone cell toggling at a steady ≤ 2 Hz (a caret blink) is left out. Changes
/// inside playing media aren't typing. The file starts `offset_us` into the take (its `SourceRun::offset_us`): tick
/// `i` is read against `still` (the recording clock) at `offset_us + i × TICK_US`. The spans are on the file's clock.
/// O(ticks + still runs).
pub fn typing(a: &ActivityTrack, still: &StillRuns, offset_us: Us) -> Vec<RegionSpan> {
    let playing = boxes_playing(a);
    let (mut next, mut active) = (0, Vec::new());
    let (runs, mut run) = (still.runs(), 0);
    let mut slots = [Slot::FREE; TYPING_SLOTS];
    let mut caret = None;
    let mut found = Vec::new();
    for (i, tick) in a.ticks.iter().enumerate() {
        let (i, t) = (i as u32, offset_us.saturating_add(i as Us * TICK_US));
        while runs.get(run).is_some_and(|r| r.1 < t) {
            run += 1;
        }
        while playing.get(next).is_some_and(|m| m.from <= i) {
            active.push(next);
            next += 1;
        }
        active.retain(|&k| playing[k].to > i);
        if !(tick.is_known() && runs.get(run).is_some_and(|r| r.0 <= t)) {
            // The cursor moves (or the tick isn't known): whatever was typed ends here.
            for s in &mut slots {
                s.end(&mut found, TYPING_MIN_TICKS);
            }
            continue;
        }
        for s in &mut slots {
            s.shift(TYPING_RATE.1);
        }
        for r in tick.top {
            let plays = |r: &Region| active.iter().any(|&k| playing[k].rect.overlap(r) > 0);
            if !typed_region(r, i, &mut caret, plays) {
                continue;
            }
            let near = |s: &Slot| {
                s.rect.is_some_and(|b| {
                    let (across, down) = b.gap(&r);
                    across <= r.w() && down <= r.h()
                })
            };
            match slots.iter().position(near) {
                Some(k) => slots[k].hit(r, true),
                None => Slot::seed(&mut slots, r),
            }
        }
        for s in &mut slots {
            s.advance(i, TYPING_RATE, TYPING_MIN_TICKS, &mut found);
        }
    }
    for s in &mut slots {
        s.end(&mut found, TYPING_MIN_TICKS);
    }
    disjoint(a, found)
}

/// One region changing in ≥ 80 % of ticks for ≥ 10 s at a steady box: playing media, never a focus by itself. The
/// spans are on the file's clock. O(ticks).
pub fn media(a: &ActivityTrack) -> Vec<RegionSpan> {
    disjoint(a, boxes_playing(a))
}

/// Every box that played, each on its own (they can overlap in time), sorted by start, on the file's clock: what
/// `media` merges. The follow-crop's part typed lately reads them one by one. O(ticks).
pub fn players(a: &ActivityTrack) -> Vec<RegionSpan> {
    boxes_playing(a)
        .into_iter()
        .map(|f| span_of(a, f))
        .collect()
}

/// Every box that played, each slot's own (so they can overlap), sorted by start.
fn boxes_playing(a: &ActivityTrack) -> Vec<Found> {
    let mut slots = [Slot::FREE; MEDIA_SLOTS];
    let mut found = Vec::new();
    for (i, tick) in a.ticks.iter().enumerate() {
        for s in &mut slots {
            s.shift(MEDIA_RATE.1);
        }
        let regions = tick.top.iter().filter(|r| tick.is_known() && r.cells > 0);
        for r in regions {
            let mut inside = false;
            let mut grown = false;
            for s in &mut slots {
                let Some(b) = s.rect else { continue };
                if r.iou_at_least(&b, SAME_BOX.0, SAME_BOX.1) {
                    inside = true;
                    // Only the first steady match grows its box.
                    let grows = !grown && r.iou_at_least(&b, GROWS_BOX.0, GROWS_BOX.1);
                    grown |= grows;
                    s.hit(*r, grows);
                }
            }
            if !inside && r.cells >= MEDIA_MIN_CELLS {
                Slot::seed(&mut slots, *r);
            }
        }
        for s in &mut slots {
            s.advance(i as u32, MEDIA_RATE, MEDIA_MIN_TICKS, &mut found);
        }
    }
    for s in &mut slots {
        s.end(&mut found, MEDIA_MIN_TICKS);
    }
    found.sort_by_key(|f| (f.from, f.to));
    found
}

/// Whether `r`, changed at tick `i`, counts as typing: typing-sized, clear of playing media (`plays`), and not a caret
/// blinking (`caret` remembers lone cells as it goes). The one rule `typing` and the follow-crop's part typed lately
/// both read, so they never disagree.
pub(crate) fn typed_region(
    r: Region,
    i: u32,
    caret: &mut Option<(Region, u32)>,
    plays: impl Fn(&Region) -> bool,
) -> bool {
    let small = r.cells > 0 && r.w() <= TYPING_MAX.0 && r.h() <= TYPING_MAX.1;
    small && !plays(&r) && !caret_blink(caret, r, i)
}

/// A lone cell changing again where it last did, at 2 Hz or slower: a caret blinking. Remembers lone cells as it goes.
fn caret_blink(last: &mut Option<(Region, u32)>, r: Region, i: u32) -> bool {
    if r.w() != 1 || r.h() > 2 {
        return false;
    }
    let same = |b: &Region| (b.x0, b.y0, b.x1, b.y1) == (r.x0, r.y0, r.x1, r.y1);
    let blink = last.is_some_and(|(b, at)| same(&b) && i.saturating_sub(at) >= CARET_GAP);
    *last = Some((r, i));
    blink
}

/// `found` sorted and made disjoint in time (overlapping spans become one, with the larger region), in µs.
fn disjoint(a: &ActivityTrack, mut found: Vec<Found>) -> Vec<RegionSpan> {
    found.sort_by_key(|f| (f.from, f.to));
    let mut out: Vec<Found> = Vec::with_capacity(found.len());
    for f in found {
        match out.last_mut() {
            Some(last) if f.from < last.to => {
                last.to = last.to.max(f.to);
                if f.rect.area() > last.rect.area() {
                    last.rect = f.rect;
                }
            }
            _ => out.push(f),
        }
    }
    out.into_iter().map(|f| span_of(a, f)).collect()
}

/// A found span in µs on the file's clock.
fn span_of(a: &ActivityTrack, f: Found) -> RegionSpan {
    let at = |i: u32| (Us::from(i) * TICK_US).min(a.duration_us);
    RegionSpan {
        from: at(f.from),
        to: at(f.to),
        rect: f.rect.rect(),
    }
}
