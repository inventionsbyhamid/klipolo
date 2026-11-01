//! Split text units blurring at the same moment (at most 12), counted as an upper bound:
//! - a split text's own blur, or a blur modifier on it, blurs every unit all scene;
//! - a blur source (the node's keys, or a tween) counts every unit while a costly blur is held before or after it: a
//!   unit waiting its turn holds the first key's or a `from`'s blur, and one that has finished the end value;
//! - while it moves, the units its stagger lets overlap (an `amount` spread over every member the tween selects),
//!   twice as many when units start in pairs (`from: center` or `edges`), and that many again for each repeat; a
//!   tween repeating forever counts every unit. Repeats are per unit, as the tween's span assumes, and the
//!   evaluator runs them so. Yoyo with an odd repeat ends on the start values, which finished units then hold;
//! - several sources on one node add up, at most every unit; a tween's moving units count once across all its
//!   targets, as its stagger group draws them, in nested layers that ramp up and down
//!   (`moving`), while held units count on each node;
//! - held and moving together count at most every unit of a node: where its held sources count some of them all
//!   through a tween's span, the tween adds only the rest, none when they count all. So
//!   "SUBSCRIBE" blurring in with `to` from a blurred, invisible rest counts 9, not 18.
//!
//! The rule for opacity 0: a unit held at opacity exactly 0 isn't counted, but only while
//! nothing else writes the node's opacity: no opacity keys (unless the blur comes from those keys), no other tween
//! setting its opacity, and no opacity modifier on it or on a group above it. **The renderer must cull units on
//! their evaluated opacity (after keys, tweens and modifiers) before it blurs them.** So
//! "letters blur in" from opacity 0 fits however many letters there are.

use std::borrow::Cow;

use super::super::{NodeType, SceneKey, Tween, Val};
use super::cost::{costly, merge, Busy};
use super::moving::overlap;
use super::nodes::NodeCost;
use super::timeline::Placed;
use super::timing::{odd_yoyo, stagger_each};
use super::{Flat, Scope};

pub(super) fn blurring_units<'p>(
    s: &'p Scope,
    costs: &[NodeCost],
    times: &[Option<Placed>],
) -> Vec<Busy<'p>> {
    let d = s.scene.duration;
    let mut busy = Vec::new();
    // Per tween: the split units its moving-only targets hold.
    let mut moving = vec![0usize; times.len()];
    for (i, (f, cost)) in s.flat.iter().zip(costs).enumerate() {
        let (p, units) = (&f.node.props, cost.units);
        if f.node.kind != NodeType::Text || p.split.is_none() || units == 0 {
            continue;
        }
        let writers = Writers::of(s, i, f, times);
        let rest = Rest {
            blur: p.blur.as_ref(),
            opacity: p.opacity.as_ref(),
        };
        // The node's spans, and the units its sources blur at once: summed, since each may drive other units, and at
        // most all of them.
        let (mut spans, mut most, mut moved) = (Vec::new(), 0usize, Vec::new());
        let own = rest.blur.is_some_and(|b| costly("blur", b))
            && !(writers.only(Source::Rest) && zero(rest.opacity));
        if own || p.modifiers.as_ref().is_some_and(|m| m.contains_key("blur")) {
            spans.push((0.0, d));
            most = units;
        }
        if let Some(keys) = f.node.animate.get("blur") {
            if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
                if keys.iter().any(|k| costly("blur", &k.v)) {
                    let opacity = f.node.animate.get("opacity").map_or(&[][..], Vec::as_slice);
                    let exempt = writers.only(Source::Keys);
                    let hide = |t: f64, before: bool| exempt && hidden(opacity, t, before, rest);
                    let before = costly("blur", &first.v) && !hide(first.t, true);
                    let after = costly("blur", &last.v) && !hide(last.t, false);
                    let each = p.stagger.as_ref().map_or(0.0, |st| stagger_each(st, units));
                    let spread = each * units.saturating_sub(1) as f64;
                    spans.push((
                        if before { 0.0 } else { first.t },
                        if after { d } else { last.t + spread },
                    ));
                    let keyed = overlap(units, last.t - first.t, each, p.stagger.as_ref(), 0);
                    most = most.saturating_add(if before || after { units } else { keyed });
                }
            }
        }
        for (ti, (t, time)) in s.scene.timeline.iter().zip(times).enumerate() {
            let Some(Placed { span, targets, .. }) = time else {
                continue;
            };
            if targets.binary_search(&i).is_err() {
                continue;
            }
            let Some((before, after)) = held(t, rest, writers.only(Source::Tween(ti))) else {
                continue;
            };
            if before || after {
                spans.push((
                    if before { 0.0 } else { span.start },
                    if after { d } else { span.end },
                ));
                most = most.saturating_add(units);
            } else {
                // Moving only: the tween's stagger group counts it once across all its targets.
                moved.push((ti, *span));
            }
        }
        // Held and moving together count at most every unit: where the held sources count `counted` of them all
        // through a tween's span (to the scene's end), that tween adds only the others, none when they count all.
        let counted = most.min(units);
        let held_spans = if counted > 0 && !moved.is_empty() {
            merge(&spans)
        } else {
            Vec::new()
        };
        for (ti, span) in moved {
            let others = if covers(&held_spans, span.start, span.end.min(d)) {
                units - counted
            } else {
                units
            };
            if let Some(sum) = moving.get_mut(ti) {
                *sum = sum.saturating_add(f.mult.saturating_mul(others));
            }
        }
        if !spans.is_empty() {
            busy.push(Busy {
                spans,
                weight: f.mult.saturating_mul(counted),
                path: Cow::Borrowed(f.path.as_str()),
            });
        }
    }
    for (ti, ((t, time), units)) in s.scene.timeline.iter().zip(times).zip(moving).enumerate() {
        if let Some(placed) = time {
            super::moving::tween(&mut busy, t, placed, units, ti);
        }
    }
    busy
}

/// Whether one of `merged` (disjoint, in order) covers every moment from `from` to `to`.
fn covers(merged: &[(f64, f64)], from: f64, to: f64) -> bool {
    let after = merged.partition_point(|(start, _)| *start <= from);
    after
        .checked_sub(1)
        .and_then(|i| merged.get(i))
        .is_some_and(|(_, end)| *end >= to)
}

/// A node's own blur and opacity, which units hold outside its tweens.
#[derive(Clone, Copy)]
struct Rest<'a> {
    blur: Option<&'a Val>,
    opacity: Option<&'a Val>,
}

/// Where a held opacity comes from.
#[derive(Clone, Copy)]
enum Source {
    /// The node's own opacity.
    Rest,
    /// The node's keys.
    Keys,
    /// The timeline's tween at this index.
    Tween(usize),
}

/// What else writes a node's opacity: its keys, the tweens setting it (by index), a modifier on it or a group above.
struct Writers {
    keys: bool,
    tweens: Vec<usize>,
    modified: bool,
}

impl Writers {
    fn of(s: &Scope, i: usize, f: &Flat, times: &[Option<Placed>]) -> Self {
        let tweens = s
            .scene
            .timeline
            .iter()
            .zip(times)
            .enumerate()
            .filter(|(_, (t, time))| {
                let targeted = time
                    .as_ref()
                    .is_some_and(|placed| placed.targets.binary_search(&i).is_ok());
                let sets = t.vars.contains_key("opacity")
                    || t.start
                        .as_ref()
                        .is_some_and(|st| st.contains_key("opacity"));
                targeted && sets
            })
            .map(|(ti, _)| ti)
            .collect();
        // The node and every group above it (depth ≤ 8).
        let mut modified = false;
        let mut at = Some(f);
        while let Some(node) = at {
            modified |= node
                .node
                .props
                .modifiers
                .as_ref()
                .is_some_and(|m| m.contains_key("opacity"));
            at = node.parent.and_then(|p| s.flat.get(p));
        }
        Self {
            keys: f.node.animate.get("opacity").is_some_and(|k| !k.is_empty()),
            tweens,
            modified,
        }
    }

    /// Whether nothing but `source` writes the opacity, so a 0 it holds is the 0 drawn.
    fn only(&self, source: Source) -> bool {
        !self.modified
            && match source {
                Source::Rest => !self.keys && self.tweens.is_empty(),
                Source::Keys => self.tweens.is_empty(),
                Source::Tween(ti) => !self.keys && self.tweens.iter().all(|&t| t == ti),
            }
    }
}

/// Exactly 0: not drawn (the rule above).
fn zero(v: Option<&Val>) -> bool {
    matches!(v, Some(Val::Num(n)) if *n == 0.0)
}

/// Whether the node's opacity keys hold it at 0 all through the time before (`before`) or after the blur key at `t`;
/// without opacity keys, whether its own opacity is 0.
fn hidden(opacity: &[SceneKey], t: f64, before: bool, rest: Rest) -> bool {
    let key = if before {
        opacity.first()
    } else {
        opacity.last()
    };
    match key {
        Some(k) if before => zero(Some(&k.v)) && k.t >= t,
        Some(k) => zero(Some(&k.v)) && k.t <= t,
        None => zero(rest.opacity),
    }
}

/// Whether a tween moves its targets' blur (None if it doesn't touch blur, or never makes it costly), and whether a
/// unit waiting its turn (first) or finished (second) holds a costly blur that is drawn. A held 0 opacity hides it
/// only when `exempt` (nothing else writes the opacity).
fn held(t: &Tween, rest: Rest, exempt: bool) -> Option<(bool, bool)> {
    let start = |key: &str| t.start.as_ref().and_then(|st| st.get(key));
    let (end_blur, start_blur) = (t.vars.get("blur"), start("blur"));
    if end_blur.is_none() && start_blur.is_none() {
        return None;
    }
    let shows = |blur: Option<&Val>, opacity: Option<&Val>| {
        blur.is_some_and(|b| costly("blur", b)) && !(exempt && zero(opacity))
    };
    let end_opacity = t.vars.get("opacity").or(rest.opacity);
    let (before, after) = if t.from.is_some() {
        // Waiting units hold the from values; finished ones the node's own.
        (shows(end_blur, end_opacity), shows(rest.blur, rest.opacity))
    } else if t.from_to.is_some() {
        (
            shows(start_blur.or(rest.blur), start("opacity").or(rest.opacity)),
            shows(end_blur.or(rest.blur), end_opacity),
        )
    } else {
        // to and set: waiting units hold the node's own values; finished ones the tween's.
        (shows(rest.blur, rest.opacity), shows(end_blur, end_opacity))
    };
    // Yoyo with an odd repeat runs its last cycle backwards: finished units hold the start values.
    let after = after || (odd_yoyo(t) && before);
    let moves = [end_blur, start_blur, rest.blur]
        .iter()
        .any(|b| b.is_some_and(|b| costly("blur", b)));
    moves.then_some((before, after))
}
