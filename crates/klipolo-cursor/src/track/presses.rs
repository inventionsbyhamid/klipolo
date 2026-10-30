//! Button events paired into presses, and the held spans the track's lookups search.

use super::CursorTrack;
use crate::{Press, Us};

/// Held spans `[down, up)`, sorted and disjoint.
pub(super) type Spans = Vec<(Us, Us)>;

/// A press's cost in the track: itself and its two held spans.
pub(super) const PRESS_BYTES: usize = size_of::<Press>() + 2 * size_of::<(Us, Us)>();

/// The buttons down, waiting for their ups: (button, since, where).
#[derive(Default)]
pub(super) struct Pairing {
    down: Vec<(u8, Us, f32, f32)>,
}

impl Pairing {
    /// A button event, and the press it ends: an up ends its button's press, and a second down without an up ends it
    /// where the new press begins. An up with no press is ignored.
    pub(super) fn event(&mut self, t: Us, btn: u8, down: bool, x: f32, y: f32) -> Option<Press> {
        let ended = self.down.iter().position(|d| d.0 == btn).map(|i| {
            let (_, from, px, py) = self.down.swap_remove(i);
            Press {
                down: from,
                up: t,
                btn,
                x: px,
                y: py,
            }
        });
        if down {
            self.down.push((btn, t, x, y));
        }
        ended
    }

    /// The presses still held at the file's end, open (`Us::MAX`).
    pub(super) fn still_held(self) -> impl Iterator<Item = Press> {
        self.down.into_iter().map(|(btn, down, x, y)| Press {
            down,
            up: Us::MAX,
            btn,
            x,
            y,
        })
    }
}

/// `presses` sorted by down time, and their held spans `[down, up)`: the primary button's, and every button's merged.
/// Each list is sorted and disjoint, so a lookup is one binary search. O(p log p).
pub(super) fn order_and_spans(presses: &mut [Press]) -> (Spans, Spans) {
    presses.sort_by_key(|p| (p.down, p.btn));
    let primary = presses
        .iter()
        .filter(|p| p.btn == 0)
        .map(|p| (p.down, p.up))
        .collect();
    let mut held = Spans::new();
    for p in presses.iter() {
        match held.last_mut() {
            Some(last) if p.down <= last.1 => last.1 = last.1.max(p.up),
            _ => held.push((p.down, p.up)),
        }
    }
    (primary, held)
}

impl CursorTrack {
    /// The last span any button was held (every button's merged) that began at or before `t`: `[down, up)`, `up`
    /// being `Us::MAX` while still held at the file's end. The drag ramp reads it either side of a release.
    /// O(log n).
    pub fn held_span(&self, t: Us) -> Option<(Us, Us)> {
        let i = self.held.partition_point(|s| s.0 <= t);
        i.checked_sub(1).map(|i| self.held[i])
    }
}
