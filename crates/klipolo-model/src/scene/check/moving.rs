//! A tween's moving units: the split units it blurs while they move, counted once across all its targets as its
//! stagger group draws them, named at the tween, whose stagger and dur are what to change
//!.
//!
//! Units start `each` apart over the tween's span and move for its dur, so as many as overlap move at once, but only
//! in the middle: the count ramps up from the span's start and down to its end. Nested layers count that exactly
//!: level ℓ covers [start + (ℓ − 1)·each, end − (ℓ − 1)·each], which is where at least ℓ units can
//! move, and the levels stop at the units that overlap. So tweens that follow each other closely add up only what
//! actually moves together. This holds for a stagger from the start, the end or at random (every unit's start is
//! one of start + k·each); repeats and centre-out pairs keep one flat count over the whole span.

use std::borrow::Cow;

use super::super::problems::at;
use super::super::{SceneLimits, StaggerArg, StaggerFrom, Tween, DEFAULT_TWEEN_DUR};
use super::cost::Busy;
use super::timeline::Placed;
use super::timing::stagger_each;

/// Layers per tween: one more than the cap refuses on its own, and the innermost carries the rest of the count.
const MAX_LAYERS: usize = SceneLimits::MAX_BLURRING_UNITS + 1;

/// Pushes the moving units of the tween at `index`: `units` split units (instances included) that it blurs while they
/// move and that no held source of their node already counts.
pub(super) fn tween(
    busy: &mut Vec<Busy<'_>>,
    t: &Tween,
    placed: &Placed,
    units: usize,
    index: usize,
) {
    if units == 0 {
        return;
    }
    let each = t
        .stagger
        .as_ref()
        .map_or(0.0, |st| stagger_each(st, placed.members));
    let repeat = t.repeat.unwrap_or(0);
    // At once: the units its stagger lets overlap across every member it selects (an `amount` spread over all of
    // them), and at most the units it moves.
    let most = overlap(
        placed.members,
        t.dur.unwrap_or(DEFAULT_TWEEN_DUR),
        each,
        t.stagger.as_ref(),
        repeat,
    )
    .min(units);
    let path = Cow::Owned(at("/timeline", index));
    let (start, end) = (placed.span.start, placed.span.end);
    if repeat != 0 || pairs(t.stagger.as_ref()) || each <= 0.0 || most <= 1 {
        busy.push(Busy {
            spans: vec![(start, end)],
            weight: most,
            path,
        });
        return;
    }
    // Rounding may leave the innermost level, a single moment, a hair inverted: it still counts there.
    let slack = 1e-9 * (1.0 + end.abs());
    for level in 0..most.min(MAX_LAYERS) {
        let inset = level as f64 * each;
        let (from, to) = (start + inset, end - inset);
        let span = if from <= to {
            (from, to)
        } else if from - to <= slack {
            (to, from)
        } else {
            // No time has this many moving (a shorter dur than the overlap assumed), nor any deeper level.
            break;
        };
        let weight = if level + 1 == MAX_LAYERS {
            most - level
        } else {
            1
        };
        busy.push(Busy {
            spans: vec![span],
            weight,
            path: path.clone(),
        });
    }
}

/// Whether units start in pairs (`from: center` or `edges`).
fn pairs(stagger: Option<&StaggerArg>) -> bool {
    matches!(
        stagger,
        Some(StaggerArg::Spec {
            from: Some(StaggerFrom::Center | StaggerFrom::Edges),
            ..
        })
    )
}

/// Units blurring at once while a source moves: those whose windows (`window` long, `each` apart) overlap, twice as
/// many when they start in pairs, and as many again per repeat; every unit when it repeats forever.
pub(super) fn overlap(
    units: usize,
    window: f64,
    each: f64,
    stagger: Option<&StaggerArg>,
    repeat: i32,
) -> usize {
    let Ok(runs) = usize::try_from(repeat) else {
        return units;
    };
    let mut n = if each > 0.0 && window.is_finite() {
        ((window / each).floor() as usize).saturating_add(1)
    } else {
        units
    };
    if pairs(stagger) {
        n = n.saturating_mul(2);
    }
    n.saturating_mul(runs.saturating_add(1)).min(units)
}
