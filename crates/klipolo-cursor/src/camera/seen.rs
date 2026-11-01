//! No action happens off screen. An action whose shot was dropped still lies inside the view: its point (a box's
//! centre), anywhere in the view. A click or a pause must show at its start; a typing span while it lasts, at its
//! start, every `EVERY_US` and its end (a pause only at its start: a parked cursor shown for its whole rest would block
//! every zoom). A zoom that would hide one is widened to show it, or dropped too, so the camera stays wide. The
//! person's own zoom, a move into or out of it, and the link between two of their zooms win where they cover that time
//! (`influence`).
//!
//! After the ladder, each dropped action is checked against the planned path, and what hides it is fixed:
//! - **in a shot's own hold** (its settle time to its hold's end), that shot must show it from then on
//!   (`Must::seen`), which may widen it; if no view at `min_scale` can, the shot goes;
//! - **between two shots**, the earlier one zooms out right after its hold (`open`), so the gap shows the whole frame;
//! - **if that was done already**, each shot around it that doesn't show it must, as above; where each does and only
//!   the move between them hides it, they go.
//!
//! The ladder then places the rest again. Each round opens a shot, shows a point a shot didn't, or drops a shot, and
//! costs a check of every dropped action (O(focus × log keys), each hold read once) and a whole ladder (O(shots ×
//! widening steps)). After `MAX_ROUNDS` the loop stops: every Auto shot goes and the whole frame shows every action
//! (the person's zooms stay, exempt as ever), so a plan costs at most `MAX_ROUNDS` + 1 rounds.
//! Opening first keeps one dropped shot from taking its neighbours with it: its own events land in the gap its
//! neighbours now leave open.

use super::moves::{self, Kept, Link};
use super::safe::{box_of, shows};
use super::shots::{Cand, Kind};
use super::{Dropped, Focus, Rules, Weight, legal};
use crate::Us;

/// The rounds the repair may take before it keeps the whole frame.
pub const MAX_ROUNDS: u32 = 8;
/// How often a dropped typing span is checked while it lasts.
pub(crate) const EVERY_US: Us = 100_000;

/// A dropped action the view shows: from when, until when (its start, for a click or a pause), and where.
pub(crate) type Point = (Us, Us, f32, f32);

enum Fix {
    Show(f64, f64),
    Open,
    Drop,
}

/// The kept shots once no dropped action is off screen, the dropped actions the view shows (those outside the
/// person's zooms), what the ladder did over every round, and how many rounds ran (at most `cap` + 1).
pub(crate) fn repair(
    mut kept: Kept,
    mut merged: u32,
    focus: &[Focus],
    dropped: &mut [bool],
    r: &Rules,
    cap: u32,
) -> (Kept, Vec<Point>, Dropped, u32) {
    let mut rounds = 0;
    loop {
        let (fixes, in_view) = hidden(&kept, focus, dropped, r);
        // Past the cap the path is the whole frame outside the person's zooms, so nothing is left to fix.
        if fixes.is_empty() || rounds > cap {
            let widened = kept.iter().filter(|k| k.0.widened(r)).count() as u32;
            let out_and_in = kept.iter().filter(|k| k.1.out_and_in).count() as u32;
            let gone = dropped.iter().filter(|&&d| d).count() as u32;
            let ladder = Dropped {
                widened,
                out_and_in,
                merged,
                dropped: gone,
            };
            return (kept, in_view, ladder, rounds);
        }
        let mut go: Vec<bool> = kept
            .iter()
            .map(|k| rounds == cap && k.0.droppable())
            .collect();
        for (i, fix) in fixes.into_iter().filter(|_| rounds < cap) {
            let c = &mut kept[i].0;
            match fix {
                Fix::Show(x, y) => go[i] |= !c.see(x, y, r),
                Fix::Open => c.open = true,
                Fix::Drop => go[i] = true,
            }
        }
        rounds += 1;
        let mut shots = Vec::with_capacity(kept.len());
        for ((c, _), go) in kept.into_iter().zip(go) {
            match go {
                true => c.drop_events(dropped),
                false => shots.push(c),
            }
        }
        let (k, m) = legal::ladder(shots, r, dropped, focus);
        (kept, merged) = (k, merged + m);
    }
}

/// What to fix for each dropped action the path hides, and the dropped actions the view must show (each with a time
/// outside the person's zooms).
fn hidden(
    kept: &Kept,
    focus: &[Focus],
    dropped: &[bool],
    r: &Rules,
) -> (Vec<(usize, Fix)>, Vec<Point>) {
    let keys = moves::keys(kept);
    let reach = influence(kept, r);
    // The person's zooms win where they, or a move into or out of them, cover a time.
    let theirs: Vec<(Us, Us)> = reach
        .iter()
        .zip(&kept[1..])
        .filter(|(_, k)| k.0.kind == Kind::Fixed)
        .map(|(s, _)| *s)
        .collect();
    // Where a person's zoom covers `t`, the end of that cover.
    let held = |t: Us| {
        let s = theirs.get(theirs.partition_point(|s| s.0 <= t).wrapping_sub(1));
        s.filter(|s| s.1 >= t).map(|s| s.1)
    };
    let (mut fixes, mut in_view) = (Vec::new(), Vec::new());
    for f in focus
        .iter()
        .zip(dropped)
        .filter(|(_, d)| **d)
        .map(|(f, _)| f)
    {
        let b = box_of(f.rect);
        // Rounded as stored, so the glide's regions and this agree to the bit.
        let (x, y) = (((b[0] + b[2]) / 2.0) as f32, ((b[1] + b[3]) / 2.0) as f32);
        let until = if f.weight == Weight::Typing {
            f.until
        } else {
            f.at
        };
        let (mut first, mut any, mut t) = (None, false, f.at);
        loop {
            let theirs_until = held(t);
            if theirs_until.is_none() {
                any = true;
                if !shows(moves::path(&keys, t), x as f64, y as f64) {
                    first = Some(t);
                    break;
                }
            }
            if t >= until {
                break;
            }
            // The next 100 ms step. Past a person's zoom at once (nothing is checked there); past a hold at once,
            // its view doesn't change, and a hold to the last key never changes again. Saturating, so a span ending
            // near `Us::MAX` (an open end) still stops.
            let i = keys.partition_point(|k| k.at <= t);
            let past = match theirs_until {
                Some(end) => end.saturating_add(1),
                None if i > 0 && keys[i - 1].moving => t.saturating_add(1),
                None => match keys.get(i) {
                    Some(k) => k.at,
                    None => break,
                },
            };
            let steps = past.saturating_sub(f.at).saturating_add(EVERY_US - 1) / EVERY_US;
            let next =
                f.at.saturating_add(steps.saturating_mul(EVERY_US))
                    .min(until);
            if next <= t {
                break;
            }
            t = next;
        }
        if any {
            in_view.push((f.at, until, x, y));
        }
        let Some(t) = first else {
            continue;
        };
        let (x, y) = (x as f64, y as f64);
        let lo = reach.partition_point(|s| s.1 < t);
        let near = (lo..reach.partition_point(|s| s.0 <= t).max(lo)).map(|i| i + 1);
        let autos: Vec<usize> = near.filter(|&i| kept[i].0.droppable()).collect();
        let hides = |&i: &usize| !shows(kept[i].0.view, x, y);
        let holding = |&i: &usize| (kept[i].0.settle(r)..=kept[i].0.to).contains(&t);
        let shut = autos.iter().find(|&&i| kept[i].0.to < t && !kept[i].0.open);
        if let (false, Some(&i)) = (autos.iter().any(holding), shut) {
            fixes.push((i, Fix::Open));
        } else if autos.iter().any(hides) {
            fixes.extend(
                autos
                    .iter()
                    .filter(|i| hides(i))
                    .map(|&i| (i, Fix::Show(x, y))),
            );
        } else {
            fixes.extend(autos.into_iter().map(|i| (i, Fix::Drop)));
        }
    }
    (fixes, in_view)
}

/// When each kept shot (not the span's ends) affects the view: from the start of its move in to the end of its move
/// out, or its settle time where it has none. Starts and ends both rise from shot to shot.
///
/// The person's zoom takes only the moves touching its own start or end (the move in arriving at its start, or any
/// from another of their zooms; the move out leaving at its end, or before it where the next of theirs starts first),
/// so a zoom of theirs at exactly the whole frame is never taken for an out-and-in's whole-frame leg. With no move
/// out, its cover ends at its own end, unless the next is theirs too: the gap between
/// two of their zooms is theirs.
fn influence(kept: &Kept, r: &Rules) -> Vec<(Us, Us)> {
    // Each window is the shot before, a shot with the link into it, and the next with the link out.
    let span = |w: &[(Cand, Link)]| {
        let ((before, _), (c, link_in), (n, link_out)) = (&w[0], &w[1], &w[2]);
        let (theirs, after_theirs) = (c.kind == Kind::Fixed, before.kind == Kind::Fixed);
        let arrive = (link_in.moves.iter().flatten().last())
            .filter(|m| m.b == c.view)
            .filter(|m| !theirs || m.to == c.from || after_theirs);
        let depart = (link_out.moves.iter().flatten().next())
            .filter(|m| m.a == c.view)
            .filter(|m| !theirs || m.from <= c.to);
        let held = if theirs && n.kind != Kind::Fixed {
            c.to
        } else {
            n.settle(r)
        };
        (
            arrive.map_or(c.settle(r), |m| m.from),
            depart.map_or(held, |m| m.to),
        )
    };
    kept.windows(3).map(span).collect()
}
