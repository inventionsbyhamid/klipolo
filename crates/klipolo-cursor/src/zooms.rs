//! Auto zoom on a take: the smart camera plans the take's span once, the plan is cut into pieces where the view rests
//! on the whole frame, and each piece no zoom covers yet becomes one Auto zoom item attached to the clip under it. The
//! engine plans inside each item from the same facts the same way, so the zooms play as planned here. Plain geometry:
//! no IO, no model.
//!
//! A click within about 350 ms of a take's end gets no zoom: the camera holds a shot through its click, then needs a
//! whole move to reach the whole frame by the end.

use klipolo_model::cursor_style::FocusKind;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::ops::{Op, TrackTarget};
use klipolo_model::timeline::{Item, ItemContent, ZoomTarget};

use crate::Us;
use crate::camera::{self, Input, Key, Plan, Rules, View};
use crate::timeline::TimelineFacts;

/// How far an auto zoom goes: the camera's own most (`Rules::default().max_scale`).
pub const AUTO_SCALE: f64 = 2.0;

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

/// The plan's pieces (`pieces` at `gap_us`) of some length that overlap none of `taken` (the zooms already there, in
/// any order; one that only touches a piece leaves it), in time order as the keys are. The last ends one move out
/// after its last hold when the take goes on for at least `gap_us` after that (`out_after_hold`). O((pieces + taken)
/// log taken).
pub fn auto_spans(plan: &Plan, taken: &[(Us, Us)], gap_us: Us) -> Vec<(Us, Us)> {
    let mut taken: Vec<(Us, Us)> = taken.iter().copied().filter(|t| t.0 < t.1).collect();
    taken.sort_unstable();
    // The furthest end among the spans starting up to each, so an overlap is one binary search.
    let reach: Vec<Us> = (taken.iter())
        .scan(Us::MIN, |m, t| {
            *m = (*m).max(t.1);
            Some(*m)
        })
        .collect();
    let free = |&(a, b): &(Us, Us)| {
        let n = taken.partition_point(|t| t.0 < b);
        a < b && (n == 0 || reach[n - 1] <= a)
    };
    let mut out = pieces(plan.keys(), gap_us);
    if let Some(last) = out.last_mut() {
        last.1 = out_after_hold(plan, last.1, gap_us);
    }
    out.into_iter().filter(free).collect()
}

/// Where the last piece, ending at `end`, ends instead. The camera holds its last shot as long as it can, so its move
/// out arrives at the span's end: an item ending there would keep the zoom on through the rest of the take. Moved to
/// start as that shot's hold ends, the move arrives one move later; that is the end when the whole frame then rests at
/// least `gap_us` before `end`, as the camera zooms out between two shots that far apart. Planned on its own, as the
/// engine plans an item, the shorter piece takes the same path up to that hold's end and zooms out from there.
fn out_after_hold(plan: &Plan, end: Us, gap_us: Us) -> Us {
    let (keys, Some(shot)) = (plan.keys(), plan.shots().last()) else {
        return end;
    };
    let [.., out, last] = keys else {
        return end;
    };
    if !out.moving || last.at != end || out.at <= shot.to {
        return end;
    }
    let sooner = shot.to.saturating_add(last.at - out.at);
    if end.saturating_sub(sooner) >= gap_us {
        sooner
    } else {
        end
    }
}

/// One Auto zoom item per span, each attached to the clip under it on the first zoom track with room: the whole
/// frame, no effects, zooming at most `scale`, framing the kinds in `using` (sorted, each once, none when all three:
/// every kind). Ids come from `mint`, in span order.
pub fn auto_zoom_ops(
    spans: &[(Us, Us)],
    scale: f64,
    using: &[FocusKind],
    mint: &mut dyn FnMut() -> String,
) -> Vec<Op> {
    let using = canonical(using);
    (spans.iter())
        .map(|&(from, to)| Op::AddItem {
            track: TrackTarget::Auto,
            attach: true,
            item: Item {
                id: mint(),
                start_us: from,
                duration_us: to - from,
                attach: None,
                layout: Layout::new(Anchor::Center, BoxN::FULL),
                content: ItemContent::Zoom {
                    target: ZoomTarget::Auto {
                        using: using.clone(),
                    },
                    scale,
                },
                effects: Vec::new(),
            },
        })
        .collect()
}

/// `using` as an Auto zoom keeps it: sorted, each kind once, and every kind as none.
fn canonical(using: &[FocusKind]) -> Vec<FocusKind> {
    let mut u = using.to_vec();
    u.sort_unstable();
    u.dedup();
    if u.len() == 3 {
        u.clear();
    }
    u
}
