//! The timeline: every node's keys (`keyed`), then tweens with exactly one verb, selectors that match, properties
//! every target animates, and an `at` that resolves. Resolving gives each tween its span in scene time, for the
//! offscreen and blur caps (and the compiler).

use super::super::problems::{at, join, quote, Problems};
use super::super::{NodeType, SceneLimits, Selectors, Tween, DEFAULT_TWEEN_DUR};
use super::nodes::NodeCost;
use super::timing::{stagger, stagger_total, start_time, Clock};
use super::{Flat, Scope};
use crate::ease_names;

/// When a tween runs in scene time: from `start` to `end` (repeats included; `end` may pass the scene's end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    pub start: f64,
    pub end: f64,
}

/// Where a tween lands: its span, its targets in document order, and how many members its stagger spreads over
/// (every target's instances, a split text's units each), as the evaluator's stagger group counts them.
#[derive(Clone, Debug)]
pub(super) struct Placed {
    pub span: Span,
    pub targets: Vec<usize>,
    pub members: usize,
}

/// Every node's keys and the whole timeline; where each tween lands, None where it can't be placed.
pub(super) fn check(s: &Scope, out: &mut Problems, costs: &[NodeCost]) -> Vec<Option<Placed>> {
    for f in &s.flat {
        super::keyed::node_keys(s, out, f);
    }
    let tweens = &s.scene.timeline;
    if tweens.len() > SceneLimits::MAX_TWEENS {
        out.push(
            "/timeline",
            "scene.tooManyTweens",
            format!(
                "A scene has at most {} tweens; this one has {}.",
                SceneLimits::MAX_TWEENS,
                tweens.len()
            ),
        );
    }
    // Past the cap the scene is refused already: the rest aren't resolved.
    let mut clock = Clock::default();
    tweens
        .iter()
        .take(SceneLimits::MAX_TWEENS)
        .enumerate()
        .map(|(i, t)| tween(s, out, costs, &mut clock, t, &at("/timeline", i)))
        .collect()
}

fn tween(
    s: &Scope,
    out: &mut Problems,
    costs: &[NodeCost],
    clock: &mut Clock,
    t: &Tween,
    path: &str,
) -> Option<Placed> {
    let verbs: Vec<(&str, &Selectors)> = [
        ("to", &t.to),
        ("from", &t.from),
        ("fromTo", &t.from_to),
        ("set", &t.set),
    ]
    .into_iter()
    .filter_map(|(name, sel)| sel.as_ref().map(|sel| (name, sel)))
    .collect();
    if verbs.len() != 1 {
        out.push(
            path,
            "scene.tween",
            "A tween has exactly one of to, from, fromTo and set: the nodes it animates.",
        );
    }
    // Each node is marked once however many selectors match it, and each selector is read once.
    let mut chosen = vec![false; s.flat.len()];
    for (verb, sel) in verbs.iter().take(1) {
        let verb_path = join(path, verb);
        match sel {
            Selectors::One(one) => select(s, out, one, &verb_path, &mut chosen),
            Selectors::Many(many) if many.len() > SceneLimits::MAX_SELECTORS => out.push(
                verb_path,
                "scene.selector",
                format!(
                    "A tween lists at most {} selectors; select a class instead.",
                    SceneLimits::MAX_SELECTORS
                ),
            ),
            Selectors::Many(many) => {
                if many.is_empty() {
                    out.push(
                        verb_path.clone(),
                        "scene.selector",
                        "A tween selects at least one node.",
                    );
                }
                for (i, one) in many.iter().enumerate() {
                    if !many.get(..i).is_some_and(|earlier| earlier.contains(one)) {
                        select(s, out, one, &at(&verb_path, i), &mut chosen);
                    }
                }
            }
        }
    }
    // In document order (the offscreen sweep searches them).
    let targets: Vec<usize> = (0..chosen.len())
        .filter(|&i| chosen.get(i) == Some(&true))
        .collect();
    let from_to = t.from_to.is_some();
    if t.vars.is_empty() {
        out.push(
            path,
            "scene.tween",
            "A tween animates nothing: give it a property, such as \"opacity\": 1.",
        );
    }
    super::keyed::vars(s, out, &t.vars, &targets, path);
    match &t.start {
        Some(start) if from_to => {
            super::keyed::vars(s, out, start, &targets, &join(path, "start"));
            if let Some(extra) = start.keys().find(|k| !t.vars.contains_key(*k)) {
                out.push(
                    join(&join(path, "start"), extra),
                    "scene.tween",
                    format!(
                        "`{}` has a start value but no end value in this tween.",
                        quote(extra)
                    ),
                );
            }
        }
        Some(_) => out.push(
            join(path, "start"),
            "scene.tween",
            "Only a fromTo tween has start values.",
        ),
        None if from_to => out.push(
            join(path, "start"),
            "scene.required",
            "A fromTo tween needs start values: \"start\": {\"opacity\": 0}.",
        ),
        None => {}
    }
    let dur = t.dur.unwrap_or(if t.set.is_some() {
        0.0
    } else {
        DEFAULT_TWEEN_DUR
    });
    if !(dur.is_finite() && (0.0..=SceneLimits::MAX_DURATION).contains(&dur)) {
        out.push(
            join(path, "dur"),
            "scene.value",
            "A tween's dur is 0 to 600 seconds.",
        );
    }
    if let Some(e) = &t.ease {
        if let Err(why) = ease_names::read(e) {
            out.push(join(path, "ease"), "scene.ease", why);
        }
    }
    let units = members(s, costs, &targets);
    let spread = match &t.stagger {
        Some(st) => {
            stagger(s, out, st, &join(path, "stagger"));
            stagger_total(st, units)
        }
        None => 0.0,
    };
    let repeats = t.repeat.unwrap_or(0);
    if !(-1..=1000).contains(&repeats) {
        out.push(
            join(path, "repeat"),
            "scene.value",
            "A tween repeats 0 to 1000 times, or -1 until the scene ends.",
        );
    }
    let delay = t.repeat_delay.unwrap_or(0.0);
    if !(delay.is_finite() && (0.0..=SceneLimits::MAX_DURATION).contains(&delay)) {
        out.push(
            join(path, "repeatDelay"),
            "scene.value",
            "A tween's repeatDelay is 0 to 600 seconds.",
        );
    }
    let start = start_time(s, out, clock, t.at.as_ref(), &join(path, "at"))?;
    let end = if repeats < 0 {
        s.scene.duration.max(start)
    } else {
        let runs = f64::from(repeats.clamp(0, 1000)) + 1.0;
        start + spread + runs * dur + (runs - 1.0) * delay
    };
    let span = Span { start, end };
    if let Some(label) = &t.label {
        let label_path = join(path, "label");
        if !super::is_name(label) || label == "out" {
            out.push(
                label_path,
                "scene.label",
                "A label is a letter or _, then letters, digits, _ or - (not out).",
            );
        } else if clock.labels.insert(label.clone(), start).is_some() {
            out.push(
                label_path,
                "scene.label",
                format!("The label `{label}` is already defined."),
            );
        }
    }
    clock.prev = Some(span);
    if end.is_finite() {
        clock.end = clock.end.max(end);
    }
    Some(Placed {
        span,
        targets,
        members: units,
    })
}

/// The nodes one selector names: `#id`, `.class` or a node type, each matching at least one node.
fn select(s: &Scope, out: &mut Problems, sel: &str, path: &str, chosen: &mut [bool]) {
    let matches = |f: &Flat| {
        if let Some(class) = sel.strip_prefix('.') {
            f.node
                .class
                .as_deref()
                .is_some_and(|c| c.split_whitespace().any(|n| n == class))
        } else {
            NodeType::from_name(sel) == Some(f.node.kind)
        }
    };
    let mut any = false;
    if let Some(id) = sel.strip_prefix('#') {
        if let Some(slot) = s.ids.get(id).and_then(|&i| chosen.get_mut(i)) {
            *slot = true;
            any = true;
        }
    } else if sel.starts_with('.') || NodeType::from_name(sel).is_some() {
        for (f, slot) in s.flat.iter().zip(chosen.iter_mut()) {
            if matches(f) {
                *slot = true;
                any = true;
            }
        }
    } else {
        out.push(
            path,
            "scene.selector",
            format!(
                "\"{}\" isn't a selector: write \"#id\", \".class\" or a node type.",
                quote(sel)
            ),
        );
        return;
    }
    if !any {
        out.push(
            path,
            "scene.selector",
            format!("\"{}\" matches no node.", quote(sel)),
        );
    }
}

/// The members a tween's stagger spreads over: each target's instances, a split text's units each.
fn members(s: &Scope, costs: &[NodeCost], targets: &[usize]) -> usize {
    targets
        .iter()
        .filter_map(|&i| {
            Some(
                s.flat
                    .get(i)?
                    .mult
                    .saturating_mul(costs.get(i)?.units.max(1)),
            )
        })
        .fold(0usize, usize::saturating_add)
}

/// After tween texts raise their targets' units, each tween's members follow, and its span grows by the stagger
/// spread over the added members. Later tweens' positions keep the spans they were placed
/// with; the evaluator places them on the texts actually laid out.
pub(super) fn refresh(s: &Scope, times: &mut [Option<Placed>], costs: &[NodeCost]) {
    for (t, time) in s.scene.timeline.iter().zip(times.iter_mut()) {
        let Some(placed) = time else {
            continue;
        };
        let now = members(s, costs, &placed.targets);
        if now == placed.members {
            continue;
        }
        if let Some(st) = &t.stagger {
            let grown = stagger_total(st, now) - stagger_total(st, placed.members);
            if t.repeat.unwrap_or(0) >= 0 && grown.is_finite() && grown > 0.0 {
                placed.span.end += grown;
            }
        }
        placed.members = now;
    }
}
