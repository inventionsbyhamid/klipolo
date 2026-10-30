//! Legal moves and the conflict ladder.
//!
//! **A legal move** takes at least `need`: the longest of its pan at `pan_speed` (view widths a second at the scale of
//! each moment, so a pan that zooms in slows as it narrows), its zoom at `zoom_rate` (both through the quintic), and
//! `move_min_us`. It fits between the earlier shot's hold end and the later one's settle time (its first event less
//! the pre-roll), within `move_max_us`, except a move into or out of the person's own zoom (their scale is theirs: 3×
//! from the whole frame needs 1.7 s). No move is ever given less than it needs.
//!
//! **The ladder.** Each next shot B is placed after the last kept A, trying in order: a glide; the glide with both
//! views widened in steps of 0.05 toward `min_scale`; an out-and-in, widened the same way; a merge into one wider
//! shot; dropping the lower-priority shot (weight, then the later). A gap of `out_gap_us`, or centres over `out_dist`
//! apart, zoom out and in first (rule 5). Widening A places A again after its own predecessor first. Scales only fall
//! and shots only go, so the stack work is linear amortised.

use super::ease::{PEAK, between, pan_peak};
use super::moves::{self, Kept, Link, Moves};
use super::safe::{box_of, shows};
use super::shots::{Cand, Kind};
use super::{Focus, Rules, View};
use crate::Us;

const WIDEN_STEP: f64 = 0.05;

/// The least time the limits let a move from `a` to `b` take (µs); 0 when there is nothing to move.
pub(crate) fn need(a: View, b: View, r: &Rules) -> Us {
    if a == b {
        return 0;
    }
    let delta = b.scale.ln() - a.scale.ln();
    let pan = (b.cx - a.cx).hypot(b.cy - a.cy) * pan_peak(a.scale, delta) / r.pan_speed;
    let zoom = delta.abs() * PEAK / r.zoom_rate;
    ((pan.max(zoom) * 1e6).ceil() as Us).max(r.move_min_us)
}

fn too_long(n: Us, c: &Cand, r: &Rules) -> bool {
    n > r.move_max_us && c.kind != Kind::Fixed
}

/// The moves of a legal glide (`out` false) or out-and-in from `a` at `va` to `b` at `vb`, or None.
fn legal(a: &Cand, b: &Cand, (va, vb): (View, View), out: bool, r: &Rules) -> Option<Moves> {
    // Saturating: a span may end at `Us::MAX` (the crate's open end).
    let window = b.settle(r).saturating_sub(a.to);
    if !out {
        let n = need(va, vb, r);
        let fits = n <= window && !(too_long(n, a, r) && too_long(n, b, r));
        return fits.then(|| moves::glide(a, b, (va, vb), n, r));
    }
    let (n, m) = (need(va, View::FULL, r), need(View::FULL, vb, r));
    let fits = n.saturating_add(m) <= window && !too_long(n, a, r) && !too_long(m, b, r);
    fits.then(|| moves::out_and_in(a, b, (va, vb), (n, m), r))
}

enum Step {
    Link(Link, f64, f64),
    DropA,
    DropB,
    Merge(Cand),
}

/// The ladder's inputs beyond the two shots: the focus the shots index, and which of it is dropped.
type Events<'a> = (&'a [Focus], &'a [bool]);

fn resolve(a: &Cand, b: &Cand, r: &Rules, events: Events) -> Step {
    if let Some(wider) = (b.kind == Kind::End)
        .then(|| late_into_end(a, b, r))
        .flatten()
    {
        return wider;
    }
    // Into the span's end, the end cuts the hold (`Cand::for_end`), at the move its view needs as it is.
    let cut = (b.kind == Kind::End).then(|| a.for_end(b.settle(r), need(a.view, View::FULL, r)));
    let a = cut.as_ref().unwrap_or(a);
    let window = b.settle(r).saturating_sub(a.to);
    let shots = !a.is_end() && !b.is_end();
    let (va, vb) = (a.view, b.view);
    let far = (vb.cx - va.cx).hypot(vb.cy - va.cy) > r.out_dist;
    let out_first =
        shots && (a.open || far || window.saturating_sub(need(va, vb, r)) >= r.out_gap_us);
    let autos = [a, b].into_iter().filter(|c| c.droppable());
    let top = autos.map(|c| c.view.scale).fold(1.0, f64::max);
    let steps = ((top - r.min_scale) / WIDEN_STEP).ceil().max(0.0) as u32;
    for out in [out_first, !out_first] {
        for j in (0..=steps).filter(|_| window >= 0 && (shots || !out)) {
            let cap = top - WIDEN_STEP * j as f64;
            let views = (a.capped(cap, r), b.capped(cap, r));
            if let Some(moves) = legal(a, b, views, out, r) {
                let link = Link {
                    moves,
                    out_and_in: out && !out_first,
                };
                return Step::Link(link, views.0.scale, views.1.scale);
            }
        }
    }
    if let Some(m) = a.merged(b, r) {
        return Step::Merge(m);
    }
    // From the span's start, no move fits before the shot's pre-roll: arrive late, as soon as the move allows, at the
    // largest view that still arrives before its hold ends and keeps the shot's own earlier events in view on the way
    // (`moves::late`).
    if a.kind == Kind::Start && b.droppable() {
        for j in 0..=steps {
            let vb = b.capped(top - WIDEN_STEP * j as f64, r);
            let n = need(va, vb, r);
            let arrive = a.to.saturating_add(n);
            if !too_long(n, b, r) && arrive < b.to && on_the_way(a.to, (va, vb), n, b, events) {
                return Step::Link(moves::late(a, (va, vb), n), va.scale, vb.scale);
            }
        }
    }
    match (a.droppable(), b.droppable()) {
        // Two of the person's zooms (or an end) closer than a move between them needs: theirs, as they placed them.
        (false, false) => {
            let n = need(va, vb, r).min(window.max(0));
            Step::Link(moves::squeezed(b, (va, vb), n, r), va.scale, vb.scale)
        }
        (false, true) => Step::DropB,
        (true, false) => Step::DropA,
        (true, true) if a.weight < b.weight => Step::DropA,
        _ => Step::DropB,
    }
}

/// A shot reached late from the span's start (`Cand::since`) and linked to the span's end: a wider view arrives sooner,
/// so each is tried with its own arrival (the late move's start plus that view's need), its hold cut as `for_end` cuts
/// it. The first whose move out then fits gives the scale the ladder places the shot again
/// at, through the late branch, which checks `on_the_way` again; only that scale is read, since the shot is placed
/// again first. None when its view fits as it is, or no view does: the usual path links it or drops it.
fn late_into_end(a: &Cand, b: &Cand, r: &Rules) -> Option<Step> {
    if a.kind != Kind::Auto || a.since == Us::MIN {
        return None;
    }
    let start = a.since.saturating_sub(need(View::FULL, a.view, r));
    let end = b.settle(r);
    let top = a.view.scale;
    let steps = ((top - r.min_scale) / WIDEN_STEP).ceil().max(0.0) as u32;
    for j in 0..=steps {
        let v = a.capped(top - WIDEN_STEP * j as f64, r);
        let n = need(v, View::FULL, r);
        let mut there = *a;
        there.since = start.saturating_add(need(View::FULL, v, r));
        let cut = there.for_end(end, n).to;
        if !too_long(n, a, r) && end.saturating_sub(cut) >= n {
            return (j > 0).then(|| Step::Link(Link::default(), v.scale, View::FULL.scale));
        }
    }
    None
}

/// Each of `b`'s events before a move of `n` from `va` at `from` to `vb` ends lies inside the view moving there, at its
/// time (a typing span while it lasts), every 10 ms: a late arrival's zoom in never leaves its own click outside.
fn on_the_way(from: Us, (va, vb): (View, View), n: Us, b: &Cand, (focus, dropped): Events) -> bool {
    let arrive = from.saturating_add(n);
    let mine = (b.first as usize..b.end as usize).filter(|&i| !dropped[i]);
    mine.map(|i| &focus[i]).filter(|f| f.at < arrive).all(|f| {
        let bx = box_of(f.rect);
        let last = f.until.min(arrive - 1).max(f.at);
        (f.at..=last).step_by(10_000).chain([last]).all(|t| {
            let v = between(
                va,
                vb,
                ((t - from) as f64 / n.max(1) as f64).clamp(0.0, 1.0),
            );
            shows(v, bx[0], bx[1]) && shows(v, bx[2], bx[3])
        })
    })
}

/// `shots` (the span's ends first and last) placed, each kept one with its link in, and how many merges it made.
/// `dropped` gains the focus of every dropped shot; `focus` is what the shots index.
pub(crate) fn ladder(
    shots: Vec<Cand>,
    r: &Rules,
    dropped: &mut [bool],
    focus: &[Focus],
) -> (Kept, u32) {
    let mut next = shots.into_iter();
    let mut kept: Kept = next
        .next()
        .map(|start| (start, Link::default()))
        .into_iter()
        .collect();
    let (mut pending, mut merged): (Vec<Cand>, u32) = (Vec::new(), 0);
    while let Some(mut b) = pending.pop().or_else(|| next.next()) {
        loop {
            let a = kept[kept.len() - 1].0;
            match resolve(&a, &b, r, (focus, dropped)) {
                Step::Link(_, sa, _) if sa < a.view.scale => {
                    // A was widened: its own way in must hold at its new scale, so it is placed again first.
                    kept.pop();
                    pending.push(b);
                    b = a;
                    b.widen_to(sa, r);
                    // Placed again: its way in is found afresh.
                    b.since = Us::MIN;
                }
                Step::Link(link, _, sb) => {
                    // A late arrival from the span's start: its hold never ends before it.
                    b.since = match (a.kind, link.moves[0]) {
                        (Kind::Start, Some(m)) if m.to > b.settle(r) => m.to,
                        _ => Us::MIN,
                    };
                    // Linked to the span's end: the hold lasts until the move out starts (`Cand::for_end`).
                    if let (Kind::End, Some(m)) = (b.kind, link.moves[0]) {
                        let a = &mut kept.last_mut().expect("the span's start").0;
                        a.to = a.for_end(m.to, m.to - m.from).to;
                    }
                    b.widen_to(sb, r);
                    kept.push((b, link));
                    break;
                }
                Step::DropA => {
                    kept.pop();
                    a.drop_events(dropped);
                }
                Step::DropB => {
                    b.drop_events(dropped);
                    break;
                }
                Step::Merge(m) => {
                    kept.pop();
                    (b, merged) = (m, merged + 1);
                }
            }
        }
    }
    (kept, merged)
}
