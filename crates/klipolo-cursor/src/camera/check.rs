//! The camera's checks as one function (feature `test-util`): this crate's camera tests, and the engine's and
//! chat's checks of what they plan. The safe zone and what a view shows are worked out afresh from the
//! view's rectangle, not with the planner's own arithmetic.

use super::safe::box_of;
use super::seen::EVERY_US;
use super::{Input, Plan, Rules, View, Weight, moves};
use crate::{Rect01, Us};

const EPS: f64 = 1e-9;

/// Every rule a plan keeps, over `[0, span)`:
/// - each kept event: the view is settled from `pre_roll_us` before it, and holds its box in the safe zone (a side on
///   the frame's own edge reaches that edge) until it ends, or its shot's hold does (the span's end cuts the last one);
///   a first shot arriving late from the span's start is settled, and holds the box, from its arrival;
/// - each action in the span that no kept shot frames (or that it keeps in view, `plan_keeping`), found from the input
///   itself, lies inside the view: a click or a pause at its start, a typing span while it lasts; outside the person's
///   zooms, the moves into and out of them, and the link between two of them;
/// - the path holds each Auto shot's view from its first event to its hold's end, and the person's zoom as they made
///   it (a glide moves only the centre, inside the rules above);
/// - each move between Auto shots or the span's ends lasts `move_min_us..=move_max_us`;
/// - the view stays inside the frame, and between samples 10 ms apart the pan stays within `pan_speed` view widths a
///   second and the zoom within `zoom_rate`, except inside a move filling the whole gap between two of the person's
///   zooms, or one and the span's start or end (`squeezed`): their placement, which the planner can't better.
pub fn check_plan(p: &Plan, r: Rules, span: Us) -> Result<(), String> {
    // Each kept event with when its shot's view is reached and when its hold ends: the span's start may make the first
    // shot arrive late (`moves::late`), the end may cut the last hold.
    let held = (p.holds.iter()).flat_map(|h| {
        p.boxes[h.first as usize..(h.first + h.n) as usize]
            .iter()
            .map(|b| (*b, h.since, h.until))
    });
    for ((at, until, rect), since, hold) in held {
        let until = until.min(hold.max(at));
        // Arrived late: inside the view while it zooms in, then settled from the arrival with its
        // box in the safe zone from then on.
        let before = until.min(since.saturating_sub(1));
        for t in (at..=before)
            .step_by(10_000)
            .chain((at <= before).then_some(before))
        {
            if !inside(p.view_at(t), rect, 0.0) {
                let v = p.view_at(t);
                return Err(format!(
                    "the event at {at} ({rect:?}) outside {v:?} at {t}, zooming in"
                ));
            }
        }
        let (from, at, until) = (
            (at - r.pre_roll_us).max(since),
            at.max(since),
            until.max(since),
        );
        let settled = p.view_at(from);
        for t in (from..=at).step_by(10_000).chain([at]) {
            if p.view_at(t) != settled {
                let v = p.view_at(t);
                return Err(format!(
                    "moving within the pre-roll of the event at {at}: {settled:?} then {v:?} at {t}"
                ));
            }
        }
        for t in (at..=until.max(at)).step_by(100_000).chain([until.max(at)]) {
            let v = p.view_at(t);
            if !inside(v, rect, r.safe) {
                return Err(format!(
                    "the event at {at} ({rect:?}) outside the safe zone of {v:?} at {t}"
                ));
            }
        }
    }
    dropped_in_view(p, span)?;
    for (s, h) in p.shots.iter().zip(&p.holds) {
        let end = if s.weight == Weight::Person {
            s.to - 1
        } else {
            s.to
        };
        let from = s.from.max(h.since);
        for t in (from..=end).step_by(100_000).chain([end]) {
            if moves::path(&p.keys, t) != s.view {
                return Err(format!(
                    "{s:?} not held at {t}: {:?}",
                    moves::path(&p.keys, t)
                ));
            }
        }
    }
    let theirs = |v: View| {
        p.shots
            .iter()
            .any(|s| s.weight == Weight::Person && s.view == v)
    };
    for k in p
        .keys
        .windows(2)
        .filter(|k| k[0].moving && !theirs(k[0].view) && !theirs(k[1].view))
    {
        let d = k[1].at - k[0].at;
        if d < r.move_min_us || d > r.move_max_us {
            return Err(format!("a move of {d} µs at {}", k[0].at));
        }
    }
    let (squeezed, mut next) = (squeezed(p), 0);
    let mut prev = p.view_at(0);
    for t in (10_000..span).step_by(10_000) {
        let v = p.view_at(t);
        while squeezed.get(next).is_some_and(|m| m.1 < t - 10_000) {
            next += 1;
        }
        let theirs = squeezed.get(next).is_some_and(|m| m.0 <= t);
        let half = 0.5 / v.scale;
        if v.scale < 1.0
            || [v.cx, v.cy]
                .iter()
                .any(|&c| c < half - EPS || c > 1.0 - half + EPS)
        {
            return Err(format!("t {t}: {v:?} shows past the frame"));
        }
        let width = 1.0 / v.scale.min(prev.scale);
        let pan = (v.cx - prev.cx).hypot(v.cy - prev.cy) / 0.01 / width;
        let zoom = (v.scale.ln() - prev.scale.ln()).abs() / 0.01;
        if !theirs && (pan > r.pan_speed * 1.001 || zoom > r.zoom_rate * 1.001) {
            return Err(format!(
                "t {t}: pan {pan} zoom {zoom}, from {prev:?} to {v:?}"
            ));
        }
        prev = v;
    }
    Ok(())
}

/// The moves filling the whole gap between two fixed points, as `(from, to)` in time order: from the end of one of the
/// person's zooms (or the span's start, at the whole frame) to the start of the next (or the span's end, at the whole
/// frame), at their views; where two of theirs overlap, the cut at the later one's start. Only two of their zooms (or
/// one and an end) closer than their move needs give one that breaks a limit (`legal`'s squeezed link); a legal glide
/// fills a gap only when it is legal anyway, and moves into and out of an Auto shot never have both ends fixed.
fn squeezed(p: &Plan) -> Vec<(Us, Us)> {
    let theirs: Vec<&super::Shot> = p
        .shots
        .iter()
        .filter(|s| s.weight == Weight::Person)
        .collect();
    let (start, end) = p.span;
    let leaves = |a: &moves::Key, cut: bool| {
        (a.at == start && a.view == View::FULL)
            || (theirs.iter()).any(|s| {
                s.view == a.view && (a.at == s.to || (cut && (s.from..s.to).contains(&a.at)))
            })
    };
    let arrives = |b: &moves::Key| {
        (b.at == end && b.view == View::FULL)
            || theirs.iter().any(|s| s.view == b.view && b.at == s.from)
    };
    (p.keys.windows(2))
        .filter(|k| k[0].moving && leaves(&k[0], k[0].at == k[1].at) && arrives(&k[1]))
        .map(|k| (k[0].at, k[1].at))
        .collect()
}

/// `plan` with its repair capped at `max_rounds` rounds (`plan` allows `MAX_ROUNDS`), and how many rounds it ran: for
/// tests that force the whole-frame fallback, and to measure the rounds.
pub fn plan_capped(input: &Input, max_rounds: u32) -> (Plan, u32) {
    super::build(input, &[], max_rounds)
}

/// Each action in the span the plan was given that no kept shot frames shows in the view, at the times above that
/// fall before `span`.
fn dropped_in_view(p: &Plan, span: Us) -> Result<(), String> {
    // Kept events carry their `until` cut to the span's end (`shots::prepare`): the input is matched the same way.
    let end = p.span.1;
    let key = |at: Us, until: Us, r: Rect01| {
        let until = until.max(at).min(end.max(at));
        (at, until, [r.x, r.y, r.w, r.h].map(f32::to_bits))
    };
    let mut kept: Vec<_> = p
        .boxes
        .iter()
        .map(|&(at, until, r)| key(at, until, r))
        .collect();
    kept.sort_unstable();
    let theirs = person_spans(p);
    let free = |t: Us| theirs.iter().all(|&(a, b)| t < a || t > b);
    for f in p
        .all
        .iter()
        .filter(|f| kept.binary_search(&key(f.at, f.until, f.rect)).is_err())
    {
        let b = box_of(f.rect);
        let (x, y) = (((b[0] + b[2]) / 2.0) as f32, ((b[1] + b[3]) / 2.0) as f32);
        let point = Rect01 {
            x,
            y,
            w: 0.0,
            h: 0.0,
        };
        let until = if f.weight == Weight::Typing {
            f.until.max(f.at)
        } else {
            f.at
        };
        let last = (until <= span).then_some(until);
        for t in (f.at..=until.min(span))
            .step_by(EVERY_US as usize)
            .chain(last)
            .filter(|&t| free(t))
        {
            if !inside(p.view_at(t), point, 0.0) {
                return Err(format!(
                    "the dropped action {f:?} off screen at {t} in {:?}",
                    p.view_at(t)
                ));
            }
        }
    }
    Ok(())
}

/// When each of the person's zooms holds the view, read from the keys: from the start of the move into it to the end of
/// the move out. The move in arrives at its start, or early only on the link from another of their zooms, which is
/// theirs too.
fn person_spans(p: &Plan) -> Vec<(Us, Us)> {
    let (k, mine) = (&p.keys, |s: &&super::Shot| s.weight == Weight::Person);
    let ended = |v: View, at: Us| {
        p.shots
            .iter()
            .filter(mine)
            .any(|s| s.view == v && s.to == at)
    };
    let next = |s: &super::Shot| p.shots.get(p.shots.partition_point(|n| n.from <= s.from));
    let span = |s: &super::Shot| {
        // The last two keys at or before its start: the move in, and the key arriving at its view.
        let j = k.partition_point(|k| k.at <= s.from);
        let start = match j.checked_sub(2).map(|h| (k[h], k[h + 1])) {
            Some((a, b))
                if a.moving && b.view == s.view && (b.at == s.from || ended(a.view, a.at)) =>
            {
                a.at
            }
            _ => s.from,
        };
        let j = k.partition_point(|k| k.at < s.to);
        let end = match (k.get(j), k.get(j + 1)) {
            (Some(a), Some(b)) if a.moving && a.at == s.to && a.view == s.view => b.at,
            // The next shot is theirs at this same view with no key between: the gap is theirs.
            _ => match next(s) {
                Some(n)
                    if n.weight == Weight::Person
                        && n.view == s.view
                        && k.get(j).is_none_or(|a| a.at >= n.from) =>
                {
                    n.from
                }
                _ => s.to,
            },
        };
        (start, end)
    };
    p.shots.iter().filter(mine).map(span).collect()
}

/// `rect` inside `v`'s safe zone: its rectangle less `safe` of its width on each side not on the frame's edge.
fn inside(v: View, rect: Rect01, safe: f64) -> bool {
    let w = 1.0 / v.scale.max(1.0);
    let corner = |c: f64| (c - w / 2.0).clamp(0.0, 1.0 - w);
    let (x, y, m) = (corner(v.cx), corner(v.cy), safe * w);
    let near = |p: f64| if p <= EPS { 0.0 } else { p + m };
    let far = |p: f64| if 1.0 - (p + w) <= EPS { 1.0 } else { p + w - m };
    let b = box_of(rect);
    b[0] >= near(x) - EPS && b[2] <= far(x) + EPS && b[1] >= near(y) - EPS && b[3] <= far(y) + EPS
}

#[cfg(test)]
mod tests;
