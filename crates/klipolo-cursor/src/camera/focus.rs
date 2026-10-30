//! Timeline facts into the camera's focus: the one function both callers plan from, the engine's
//! Auto zooms at render time and chat's zoom tools at edit time, so chat's zooms play as it planned them.

use klipolo_model::cursor_style::FocusKind;

use super::{Focus, Weight};
use crate::timeline::TimelineFacts;
use crate::{Rect01, Us};

/// Focus from timeline facts starting in `[from, to)`: clicks (`Click`), dwells (`Dwell`) and typing (`Typing`) per
/// `using` (empty: all), in time order. Playing media is never a focus of its own: a click inside it is a click like
/// any other, so the click is the focus. O(log n + kept): the three lists are already sorted, and the stable sort
/// merges the runs in one pass.
pub fn focus_from(f: &TimelineFacts, using: &[FocusKind], from: Us, to: Us) -> Vec<Focus> {
    let uses = |k: FocusKind| using.is_empty() || using.contains(&k);
    let point = |x: f32, y: f32| Rect01 {
        x,
        y,
        w: 0.0,
        h: 0.0,
    };
    let mut out = Vec::new();
    if uses(FocusKind::Clicks) {
        out.extend(
            starting(&f.clicks, |c| c.timeline_us, from, to)
                .iter()
                .map(|c| Focus {
                    at: c.timeline_us,
                    until: c.timeline_us,
                    rect: point(c.x, c.y),
                    weight: Weight::Click,
                }),
        );
    }
    if uses(FocusKind::Pauses) {
        out.extend(
            starting(&f.dwells, |d| d.from_us, from, to)
                .iter()
                .map(|d| Focus {
                    at: d.from_us,
                    until: d.to_us,
                    rect: point(d.x, d.y),
                    weight: Weight::Dwell,
                }),
        );
    }
    if uses(FocusKind::Typing) {
        out.extend(
            starting(&f.typing, |r| r.from_us, from, to)
                .iter()
                .map(|r| Focus {
                    at: r.from_us,
                    until: r.to_us,
                    rect: r.rect,
                    weight: Weight::Typing,
                }),
        );
    }
    out.sort_by_key(|f| f.at);
    out
}

/// The actions of the kinds a narrowed `using` leaves out, starting in `[from, to)`: what `plan_keeping` keeps on
/// screen. None when `using` is empty (every kind is a focus). O(log n + kept).
pub fn keep_from(f: &TimelineFacts, using: &[FocusKind], from: Us, to: Us) -> Vec<Focus> {
    let others: Vec<FocusKind> = [FocusKind::Clicks, FocusKind::Pauses, FocusKind::Typing]
        .into_iter()
        .filter(|k| !using.contains(k))
        .collect();
    if using.is_empty() || others.is_empty() {
        return Vec::new();
    }
    focus_from(f, &others, from, to)
}

/// The items of a time-sorted list that start in `[from, to)`. O(log n).
fn starting<T>(items: &[T], at: impl Fn(&T) -> Us, from: Us, to: Us) -> &[T] {
    let first = items.partition_point(|i| at(i) < from);
    let end = first + items[first..].partition_point(|i| at(i) < to);
    &items[first..end]
}
