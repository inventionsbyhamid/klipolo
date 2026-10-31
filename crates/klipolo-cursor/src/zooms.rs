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

/// What a take's auto zoom adds over `span`, the zooms already there being `taken`: the pieces of `auto_plan`'s plan,
/// the last ending one move out after its last hold when that is sooner and keeps the take's path
/// (`out_after_hold`), less any that overlap `taken` (as `auto_spans`). O(n log n) in the facts in the span, for the
/// take's plan and the last piece's own.
pub fn take_spans(
    facts: &TimelineFacts,
    using: &[FocusKind],
    span: (Us, Us),
    taken: &[(Us, Us)],
    rules: Rules,
) -> Vec<(Us, Us)> {
    let plan = auto_plan(facts, using, span, rules);
    let mut spans = pieces(plan.keys(), rules.out_gap_us);
    if let Some(last) = spans.last_mut()
        && let Some(out) = out_after_hold(&plan, last.1, rules.out_gap_us)
    {
        let own = auto_plan(facts, using, (last.0, out[1].at), rules);
        let before = (plan.keys().iter()).filter(|k| (last.0..out[0].at).contains(&k.at));
        if own.keys().iter().eq(before.chain(&out)) {
            last.1 = out[1].at;
        }
    }
    free_of(spans, taken)
}

/// The plan's pieces (`pieces` at `gap_us`) of some length that overlap none of `taken` (the zooms already there, in
/// any order; one that only touches a piece leaves it), in time order as the keys are. O((pieces + taken) log taken).
pub fn auto_spans(plan: &Plan, taken: &[(Us, Us)], gap_us: Us) -> Vec<(Us, Us)> {
    free_of(pieces(plan.keys(), gap_us), taken)
}

/// `spans` of some length that overlap none of `taken`, in their order. O((spans + taken) log taken).
fn free_of(spans: Vec<(Us, Us)>, taken: &[(Us, Us)]) -> Vec<(Us, Us)> {
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
    spans.into_iter().filter(free).collect()
}

/// The last piece, ending at `end`, ended sooner: its last two keys with the move out starting as the last shot's
/// hold ends. The camera holds its last shot as long as it can, so its move out arrives at the span's end, and an item
/// ending there would keep the zoom on through the rest of the take. None when the move already starts as the hold
/// ends, or when the whole frame would then rest less than `gap_us` before `end` (the camera zooms out between two
/// shots only that far apart).
///
/// Planned on its own, as the engine plans an item, the shorter piece usually takes the same path up to that hold's
/// end and then this move out. Not always: an action kept on screen (a rest, under `CLICKS`) that starts during the
/// sooner move out is checked against a different path, and the item's own camera may widen or drop its shot. So
/// `take_spans` keeps the shorter end only when the piece's own plan has exactly these keys.
fn out_after_hold(plan: &Plan, end: Us, gap_us: Us) -> Option<[Key; 2]> {
    let (keys, Some(shot)) = (plan.keys(), plan.shots().last()) else {
        return None;
    };
    let [.., out, last] = keys else {
        return None;
    };
    if !out.moving || last.at != end || out.at <= shot.to {
        return None;
    }
    let sooner = shot.to.saturating_add(last.at - out.at);
    (end.saturating_sub(sooner) >= gap_us).then_some([
        Key {
            at: shot.to,
            ..*out
        },
        Key {
            at: sooner,
            ..*last
        },
    ])
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
