//! Auto zoom on a take: the smart camera plans the take's span once, and the plan is cut into pieces where the view
//! rests on the whole frame. The engine plans inside each piece from the same facts the same way, so the zooms play
//! as planned here. Plain geometry: no IO, no model.

use klipolo_model::cursor_style::FocusKind;

use crate::Us;
use crate::camera::{self, Input, Key, Plan, Rules, View};
use crate::timeline::TimelineFacts;

/// What a take's auto zoom frames: its clicks.
pub const CLICKS: &[FocusKind] = &[FocusKind::Clicks];

/// The camera's plan over `[span)`: the focus of the kinds in `using` (empty: every kind), the other kinds kept on
/// screen, no fixed shots. O(n log n) in the facts in the span.
pub fn auto_plan(facts: &TimelineFacts, using: &[FocusKind], span: (Us, Us), rules: Rules) -> Plan {
    let (from, to) = span;
    let focus = camera::focus_from(facts, using, from, to);
    let keep = camera::keep_from(facts, using, from, to);
    let input = Input {
        span,
        focus: &focus,
        fixed: &[],
        rules,
    };
    camera::plan_keeping(&input, &keep)
}

/// The plan's keys cut where the view rests on the whole frame for at least `gap_us`, or at their end: each piece from
/// its first move's start to its last move's end, in time order. The keys come in pairs: a move's start (moving), then
/// its end. O(keys).
pub fn pieces(keys: &[Key], gap_us: Us) -> Vec<(Us, Us)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, k) in keys.iter().enumerate() {
        if k.moving {
            start.get_or_insert(k.at);
            continue;
        }
        let rests = (keys.get(i + 1)).is_none_or(|n| n.at.saturating_sub(k.at) >= gap_us);
        if k.view == View::FULL
            && rests
            && let Some(s) = start.take()
        {
            out.push((s, k.at));
        }
    }
    out
}
