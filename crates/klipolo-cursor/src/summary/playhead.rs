//! The cursor at the exact playhead, for segment C. Segment B's line lists the facts nearest the
//! middle of the playhead's minute, so that it stays the same all minute (the pre-fill's rule); in a busy take it can
//! then list none at the playhead itself, which "zoom on what I clicked here" is about. This note says those, in a few
//! words, when B's line leaves any out.
//!
//! **Cost:** a binary search per kind for the facts near the playhead, `summary_line` once more to know what B lists,
//! then the nearest facts one at a time (a handful fit): O(kinds × log n + k²) for k listed.

use klipolo_model::time::clock;

use super::{KINDS, Listed, by_kind, count, first_not, listed_near, nearest, times};
use crate::Us;
use crate::timeline::TimelineFacts;

/// A fact this near the playhead is at it.
pub const NEAR_US: Us = 5_000_000;

/// "Cursor at the playhead (1:30:02): click 1:30:01 (top right); paused 1:29:58–1:30:03 (centre)." The facts within
/// `NEAR_US` of `playhead_us`, nearest first, as many as fit `max_bytes`, when segment B's line doesn't list every one
/// of them. `b`: the time B's line centres on and its byte budget (as `summary_line` takes them), or None when B left
/// the line out. None when no fact is that near, B lists them all, or not even one fits.
pub fn playhead_note(
    f: &TimelineFacts,
    playhead_us: Us,
    b: Option<(Us, usize)>,
    max_bytes: usize,
) -> Option<String> {
    let (from, to) = (
        playhead_us.saturating_sub(NEAR_US),
        playhead_us.saturating_add(NEAR_US),
    );
    // Each kind's facts reaching into [from, to]: their ends and starts are sorted (a kind's spans never overlap).
    let near: Listed = std::array::from_fn(|k| {
        let n = count(f, k);
        let lo = first_not(n, |i| times(f, k, i).1 < from);
        (lo, first_not(n, |i| times(f, k, i).0 <= to).max(lo))
    });
    if near.iter().all(|&(lo, hi)| lo == hi) {
        return None;
    }
    let in_b = b
        .and_then(|(at, max)| listed_near(f, at, max))
        .map(|(l, _)| l);
    let covered = |l: &Listed| {
        (0..KINDS).all(|k| near[k].0 == near[k].1 || (l[k].0 <= near[k].0 && near[k].1 <= l[k].1))
    };
    if in_b.as_ref().is_some_and(covered) {
        return None;
    }
    let head = format!("Cursor at the playhead ({}): ", clock(playhead_us));
    // Grown outward from the playhead, one nearest fact at a time, as segment B's line is.
    let mut listed: Listed = std::array::from_fn(|k| {
        let (lo, hi) = near[k];
        let split = lo + first_not(hi - lo, |j| times(f, k, lo + j).0 <= playhead_us);
        (split, split)
    });
    let mut line = None;
    while let Some((k, earlier)) = nearest(f, &listed, &near, playhead_us) {
        let mut more = listed;
        if earlier {
            more[k].0 -= 1;
        } else {
            more[k].1 += 1;
        }
        let mut longer = head.clone();
        by_kind(&mut longer, f, &more);
        longer.push('.');
        if longer.len() > max_bytes {
            break;
        }
        (listed, line) = (more, Some(longer));
    }
    line
}
