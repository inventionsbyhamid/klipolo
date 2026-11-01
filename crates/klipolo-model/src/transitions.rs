//! Transitions over media handles. A clip's `transition_out` blends it into the next clip
//! across their cut: `split(d).0` before the cut and `split(d).1` after it. Before the cut the next clip shows the
//! footage before its in point; after it, this clip shows the footage after its out point. Clip positions never
//! change, so trims and ripples work as before; after every batch of ops `normalize` shortens a transition that no
//! longer fits, or drops it under `MIN_TRANSITION_US`.

use crate::project::Project;
use crate::query::{clip_starts, source_duration_us};
use crate::time::{scaled, Us};
use crate::timeline::Clip;

pub const MIN_TRANSITION_US: Us = 100_000;
pub const MAX_TRANSITION_US: Us = 5_000_000;
pub const DEFAULT_TRANSITION_US: Us = 500_000;
/// `Project::requires` while any clip has a transition (an older build opens the file read-only).
pub const FEATURE: &str = "transitions@1";

/// A transition of `d` µs around its cut: (before, after). The odd microsecond goes after.
pub fn split(d: Us) -> (Us, Us) {
    let before = d / 2;
    (before, d - before)
}

/// Spare footage around clip `i`, in timeline µs at the clip's speed: before its in point and after its out point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handles {
    pub before: Us,
    pub after: Us,
}

pub fn handles(p: &Project, i: usize) -> Handles {
    let c = &p.timeline.main[i];
    let length = source_duration_us(p, &c.source).unwrap_or(c.source_out_us);
    Handles {
        before: scaled(c.source_in_us.max(0), c.speed),
        after: scaled(length.saturating_sub(c.source_out_us).max(0), c.speed),
    }
}

/// The longest transition from `main[i]` into `main[i + 1]` when the last `incoming_after` µs of clip `i`'s start
/// belong to its incoming transition and the first `next_before` µs of clip `i + 1`'s end to that clip's outgoing
/// one. The part before the cut needs clip `i + 1`'s spare footage and room at the end of clip `i`; the part after,
/// clip `i`'s spare footage and room at the start of clip `i + 1`. 0 for the last clip. A batch that sets several cuts
/// fits each one beside the transitions it gave the cuts before it, as `apply_ops` checks them (the chat compiler).
pub fn fit_us(p: &Project, i: usize, incoming_after: Us, next_before: Us) -> Us {
    let main = &p.timeline.main;
    if i + 1 >= main.len() {
        return 0;
    }
    let room_before = handles(p, i + 1)
        .before
        .min(main[i].duration_us().saturating_sub(incoming_after));
    let room_after = handles(p, i)
        .after
        .min(main[i + 1].duration_us().saturating_sub(next_before));
    if room_before <= 0 || room_after <= 0 {
        return 0;
    }
    room_before
        .saturating_mul(2)
        .saturating_add(1)
        .min(room_after.saturating_mul(2))
        .min(MAX_TRANSITION_US)
}

/// The longest transition `main[i]` can have now, leaving its neighbours' transitions as they are (what
/// `SetTransition` accepts and what the UI offers).
pub fn max_transition_us(p: &Project, i: usize) -> Us {
    let main = &p.timeline.main;
    if i + 1 >= main.len() {
        return 0;
    }
    let incoming_after = i
        .checked_sub(1)
        .and_then(|k| main[k].transition_out.as_ref())
        .map_or(0, |t| split(t.duration_us).1);
    let next_before = main[i + 1]
        .transition_out
        .as_ref()
        .map_or(0, |t| split(t.duration_us).0);
    fit_us(p, i, incoming_after, next_before)
}

/// Where a transition runs on the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionWindow {
    /// The outgoing clip.
    pub clip: usize,
    pub start_us: Us,
    pub cut_us: Us,
    pub end_us: Us,
}

/// Every transition's window, in timeline order (valid projects only: each has a clip after it). It allocates on
/// every call (the clip starts and the result), so it isn't for per-frame use: the engine builds it once per plan.
pub fn windows(main: &[Clip]) -> Vec<TransitionWindow> {
    let starts = clip_starts(main);
    main.iter()
        .enumerate()
        .filter(|(i, _)| i + 1 < main.len())
        .filter_map(|(i, c)| {
            let t = c.transition_out.as_ref()?;
            let cut = starts[i].saturating_add(c.duration_us());
            let (before, after) = split(t.duration_us);
            Some(TransitionWindow {
                clip: i,
                start_us: cut - before,
                cut_us: cut,
                end_us: cut + after,
            })
        })
        .collect()
}

/// The length each clip's transition has once [`normalize`] refits it, or None (it has none, or the refit drops it):
/// transitions only between two clips, within their footage and clips (earlier cuts first), at least
/// `MIN_TRANSITION_US` long. The chat compiler reads it so a later call never sees a transition an earlier call's edit
/// is about to drop (a batch's ops see transitions unrefitted until its end). O(clips), and `p` is unchanged.
pub fn refitted(p: &Project) -> Vec<Option<Us>> {
    let main = &p.timeline.main;
    let mut out = Vec::with_capacity(main.len());
    let mut incoming_after: Us = 0;
    for (i, clip) in main.iter().enumerate() {
        let fitted = clip
            .transition_out
            .as_ref()
            .map(|t| t.duration_us.min(fit_us(p, i, incoming_after, 0)))
            .filter(|d| *d >= MIN_TRANSITION_US);
        incoming_after = fitted.map_or(0, |d| split(d).1);
        out.push(fitted);
    }
    out
}

/// After every batch of ops (`apply_ops`): every transition as [`refitted`] leaves it, and no fades at a
/// transition's cut.
pub(crate) fn normalize(p: &mut Project) {
    let fitted = refitted(p);
    let main = &mut p.timeline.main;
    for (i, d) in fitted.into_iter().enumerate() {
        match d {
            None => main[i].transition_out = None,
            Some(d) => {
                if let Some(t) = main[i].transition_out.as_mut() {
                    t.duration_us = d;
                }
                main[i].fade_out_us = 0;
                main[i + 1].fade_in_us = 0;
            }
        }
    }
    // `requires` names the feature only while a transition exists, so a file whose last transition was
    // removed opens normally in older builds again.
    if p.timeline.main.iter().any(|c| c.transition_out.is_some()) {
        p.requires.insert(FEATURE.to_string());
    } else {
        p.requires.remove(FEATURE);
    }
}
