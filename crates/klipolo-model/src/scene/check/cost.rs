//! The scene-wide caps: nodes, instances after `repeat`, particles, characters, groups drawn offscreen at the
//! same moment and split units blurring at the same moment, the last two judged over the key and tween times.

use std::borrow::Cow;

use super::super::problems::Problems;
use super::super::{NodeType, SceneFacts, SceneLimits, Tween, Val};
use super::nodes::NodeCost;
use super::timeline::{Placed, Span};
use super::timing::odd_yoyo;
use super::Scope;
use crate::motion::Blend;

/// A node (or a tween's moving units) costly over `spans` (seconds, possibly overlapping), `weight` times over; `path`
/// is what a refusal names.
pub(super) struct Busy<'p> {
    pub spans: Vec<(f64, f64)>,
    pub weight: usize,
    pub path: Cow<'p, str>,
}

pub(super) fn check(
    s: &Scope,
    out: &mut Problems,
    facts: &mut SceneFacts,
    costs: &[NodeCost],
    times: &[Option<Placed>],
) {
    // At most `MAX_NODES`: `check_scene` refuses more before any node is checked.
    facts.nodes = s.flat.len();
    // Each cap names the node that took it over: an emitter, or the repeat that multiplied a node, or a text.
    let (mut particles_over, mut instances_over, mut text_over) = (None, None, None);
    for (f, cost) in s.flat.iter().zip(costs) {
        if f.node.kind != NodeType::Repeat && !f.particle {
            facts.max_instances = facts.max_instances.saturating_add(f.mult);
            if facts.max_instances > SceneLimits::MAX_INSTANCES && instances_over.is_none() {
                instances_over = Some(f.blame(s));
            }
        }
        facts.max_particles = facts
            .max_particles
            .saturating_add(f.mult.saturating_mul(cost.particles));
        if facts.max_particles > SceneLimits::MAX_PARTICLES && particles_over.is_none() {
            particles_over = Some(f.path.as_str());
        }
        facts.text_chars = facts
            .text_chars
            .saturating_add(f.mult.saturating_mul(cost.chars));
        if facts.text_chars > SceneLimits::MAX_TEXT_SCENE && text_over.is_none() {
            text_over = Some(f.blame(s));
        }
        let p = &f.node.props;
        facts.animates |= !f.node.animate.is_empty()
            || p.modifiers.as_ref().is_some_and(|m| !m.is_empty())
            || f.node.kind == NodeType::Emitter
            || p.count.is_some()
            || p.reveal.is_some()
            || p.scramble.is_some()
            || p.holds.as_ref().is_some_and(|h| !h.is_empty());
    }
    facts.animates |= !s.scene.timeline.is_empty();
    if let Some(path) = instances_over {
        out.push(
            path,
            "scene.tooManyInstances",
            format!(
                "A scene draws at most {} nodes once its repeats are expanded (at their settings' largest); this one \
                 draws {}.",
                SceneLimits::MAX_INSTANCES,
                facts.max_instances
            ),
        );
    }
    if let Some(path) = particles_over {
        out.push(
            path,
            "scene.tooManyParticles",
            format!(
                "A scene has at most {} particles alive at once (burst + rate × life, over every emitter); this one \
                 has {}.",
                SceneLimits::MAX_PARTICLES,
                facts.max_particles
            ),
        );
    }
    if let Some(path) = text_over {
        out.push(
            path,
            "scene.textTooLong",
            format!(
                "A scene's text shows at most {} characters in all; this one can show {}.",
                SceneLimits::MAX_TEXT_SCENE,
                facts.text_chars
            ),
        );
    }
    super::tracks::check(s, out, facts, costs, times);
    let groups = offscreen_groups(s, times);
    let (most, at_time, path) = most_at_once(&groups, SceneLimits::MAX_GROUPS_OFFSCREEN);
    facts.groups_offscreen = most;
    if most > SceneLimits::MAX_GROUPS_OFFSCREEN {
        out.push(
            path.unwrap_or("/nodes"),
            "scene.offscreenGroups",
            format!(
                "At {at_time} s, {most} groups draw offscreen at once (for blur, a blend other than normal, a mask, a \
                 3D tilt or opacity between 0 and 1); Klipolo draws at most {}. Move those effects onto the shapes, or \
                 time them apart.",
                SceneLimits::MAX_GROUPS_OFFSCREEN
            ),
        );
    }
    let blurring = super::blur::blurring_units(s, costs, times);
    let (most, at_time, path) = most_at_once(&blurring, SceneLimits::MAX_BLURRING_UNITS);
    if most > SceneLimits::MAX_BLURRING_UNITS {
        out.push(
            path.unwrap_or("/nodes"),
            "scene.blurringUnits",
            format!(
                "At {at_time} s, {most} split text units blur at once; Klipolo blurs at most {}. Blur fewer units at \
                 once: a stagger with a short blur, or letters blurring in with `from` {{blur, opacity: 0}} with \
                 nothing else changing that text's opacity, so its waiting units aren't drawn.",
                SceneLimits::MAX_BLURRING_UNITS
            ),
        );
    }
}

/// Whether a value of `prop` makes a group draw offscreen: blur above 0, a tilt, opacity between 0 and 1 (0 isn't
/// drawn, 1 is drawn directly). A binding or a percentage might.
pub(super) fn costly(prop: &str, v: &Val) -> bool {
    match v {
        Val::Str(_) => true,
        Val::Num(n) => match prop {
            "opacity" => *n > 0.0 && *n < 1.0,
            "blur" => *n > 0.0,
            _ => *n != 0.0,
        },
    }
}

const OFFSCREEN: [&str; 4] = ["blur", "rotateX", "rotateY", "opacity"];

/// When each group draws offscreen: always for a static reason or a modifier on one of those properties (it moves
/// the value off rest all scene); while its keys or tweens animate one, and before or after when the value held there
/// is costly.
fn offscreen_groups<'p>(s: &'p Scope, times: &[Option<Placed>]) -> Vec<Busy<'p>> {
    let d = s.scene.duration;
    let mut busy = Vec::new();
    for (gi, f) in s.flat.iter().enumerate() {
        if f.node.kind != NodeType::Group {
            continue;
        }
        let p = &f.node.props;
        let mut spans: Vec<(f64, f64)> = Vec::new();
        let held = [
            (&p.blur, "blur"),
            (&p.rotate_x, "rotateX"),
            (&p.rotate_y, "rotateY"),
            (&p.opacity, "opacity"),
        ]
        .iter()
        .any(|(v, prop)| v.as_ref().is_some_and(|v| costly(prop, v)));
        let modified = p
            .modifiers
            .as_ref()
            .is_some_and(|m| OFFSCREEN.iter().any(|prop| m.contains_key(*prop)));
        if held || modified || p.blend.is_some_and(|b| b != Blend::Normal) || p.mask.is_some() {
            spans.push((0.0, d));
        }
        for prop in OFFSCREEN {
            let Some(keys) = f.node.animate.get(prop) else {
                continue;
            };
            let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
                continue;
            };
            let start = if costly(prop, &first.v) { 0.0 } else { first.t };
            let end = if costly(prop, &last.v) { d } else { last.t };
            spans.push((start, end));
        }
        for (t, time) in s.scene.timeline.iter().zip(times) {
            let Some(Placed { span, targets, .. }) = time else {
                continue;
            };
            if targets.binary_search(&gi).is_err() {
                continue;
            }
            for prop in OFFSCREEN {
                if let Some(v) = t.vars.get(prop) {
                    spans.push(tween_span(t, span, prop, v, d));
                }
            }
        }
        if !spans.is_empty() {
            busy.push(Busy {
                spans,
                weight: f.mult,
                path: Cow::Borrowed(f.path.as_str()),
            });
        }
    }
    busy
}

/// A tween's span for one property, stretched to the scene's start when its start value is costly (from, fromTo)
/// and to its end when its end value is (to, set, fromTo), or its start value is and an odd yoyo ends on it.
fn tween_span(t: &Tween, span: &Span, prop: &str, v: &Val, d: f64) -> (f64, f64) {
    let (mut start, mut end) = (span.start, span.end);
    // The start value: a from's, or a fromTo's start. Held before the tween, and after it when a yoyo with an odd
    // repeat ends on it.
    let first = if t.from.is_some() {
        Some(v)
    } else {
        t.start
            .as_ref()
            .and_then(|st| st.get(prop))
            .filter(|_| t.from_to.is_some())
    };
    if first.is_some_and(|first| costly(prop, first)) {
        start = 0.0;
        if odd_yoyo(t) {
            end = d;
        }
    }
    if t.from.is_none() && costly(prop, v) {
        end = d;
    }
    (start, end)
}

/// The most weight busy at one moment, that moment, and the node (or tween) whose start took it past `cap` then: a
/// sweep over every entry's merged spans (closed, so spans touching at a moment both count).
fn most_at_once<'a>(busy: &'a [Busy<'_>], cap: usize) -> (usize, f64, Option<&'a str>) {
    // (time, starts before ends at the same time, weight, path)
    let mut events: Vec<(f64, bool, usize, &'a str)> = Vec::new();
    for b in busy {
        for (start, end) in merge(&b.spans) {
            events.push((start, false, b.weight, b.path.as_ref()));
            events.push((end, true, b.weight, b.path.as_ref()));
        }
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let (mut total, mut best) = (0usize, (0usize, 0.0, None));
    for (time, is_end, weight, path) in events {
        if is_end {
            total = total.saturating_sub(weight);
            continue;
        }
        total = total.saturating_add(weight);
        if total > best.0 {
            best = (total, time, (total > cap).then_some(path));
        }
    }
    best
}

/// `spans` as disjoint closed spans in order (overlapping or touching ones joined), without malformed ones.
pub(super) fn merge(spans: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<(f64, f64)> = spans
        .iter()
        .copied()
        .filter(|(start, end)| start.is_finite() && !end.is_nan() && start <= end)
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(sorted.len());
    for (start, end) in sorted {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}
