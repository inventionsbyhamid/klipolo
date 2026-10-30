//! What a frame can read: an upper bound on the tracks the evaluator reads, at most
//! `MAX_TRACKS`. Each property a tween or a node's own keys animate is one track per number it writes (`x` and `y`
//! two, a colour and `origin` four), on every instance of every target, and on every unit of a split text for the
//! properties units have. One property's run of `g` tracks, `c` of them chained (they start or end from the value
//! before), reads at most `run_reads(g, c)`: the run, then a shorter run for each chained read, at most 8 deep. The
//! evaluator decides each instance against the same bound, and draws one past it as its settled still. Clips are
//! counted too, each tween's and key's once: at most 65,535.

use std::collections::BTreeMap;

use super::super::problems::Problems;
use super::super::timeline::{lower_of, run_reads, Lower, Prop};
use super::super::{NodeType, SceneFacts, SceneLimits};
use super::nodes::NodeCost;
use super::timeline::Placed;
use super::Scope;

/// The clips an instance can hold: one stands for a node's own, then one per clip value.
const MAX_CLIPS: usize = u16::MAX as usize;

pub(super) fn check(
    s: &Scope,
    out: &mut Problems,
    facts: &mut SceneFacts,
    costs: &[NodeCost],
    times: &[Option<Placed>],
) {
    // Per (node, unit run, property): its tracks and how many are chained.
    let mut runs: BTreeMap<(usize, bool, Prop), (usize, usize)> = BTreeMap::new();
    let mut clips = 1usize;
    let split = |i: usize| {
        s.flat
            .get(i)
            .is_some_and(|f| f.node.kind == NodeType::Text && f.node.props.split.is_some())
    };
    for (t, time) in s.scene.timeline.iter().zip(times) {
        let Some(placed) = time else {
            continue;
        };
        for name in t.vars.keys() {
            let Some(lower) = lower_of(name) else {
                continue;
            };
            let started =
                t.from_to.is_some() && t.start.as_ref().is_some_and(|st| st.contains_key(name));
            if lower == Lower::Clip {
                clips = clips.saturating_add(1 + usize::from(started));
            }
            for &n in &placed.targets {
                for p in lower.props() {
                    let unit = t.stagger.is_some() && split(n) && p.per_unit();
                    let run = runs.entry((n, unit, p)).or_default();
                    run.0 += 1;
                    run.1 += usize::from(!started);
                }
            }
        }
    }
    for (n, f) in s.flat.iter().enumerate() {
        for (name, keys) in &f.node.animate {
            let Some(lower) = lower_of(name) else {
                continue;
            };
            if lower == Lower::Clip {
                clips = clips.saturating_add(keys.len().min(SceneLimits::MAX_KEYS));
            }
            for p in lower.props() {
                let run = runs.entry((n, split(n) && p.per_unit(), p)).or_default();
                run.0 += 1;
                run.1 += usize::from(p.switches());
            }
        }
    }
    // Per node: its own runs once, its units' runs once per unit; then every instance.
    let mut per_node = vec![(0usize, 0usize); s.flat.len()];
    for (&(n, unit, _), &(g, c)) in &runs {
        if let Some(slot) = per_node.get_mut(n) {
            let reads = run_reads(g, c);
            if unit {
                slot.1 = slot.1.saturating_add(reads);
            } else {
                slot.0 = slot.0.saturating_add(reads);
            }
        }
    }
    let reads = per_node
        .iter()
        .zip(&s.flat)
        .zip(costs)
        .map(|((&(own, units_each), f), cost)| {
            let node = own.saturating_add(cost.units.saturating_mul(units_each));
            f.mult.saturating_mul(node)
        })
        .fold(0usize, usize::saturating_add);
    facts.track_reads = reads;
    if reads > SceneLimits::MAX_TRACKS {
        out.push(
            "/timeline",
            "scene.tooManyTracks",
            format!(
                "A frame of this scene can read {reads} animated values; Klipolo reads at most {}. Each instance a \
                 tween reaches counts once per number it animates (x and y two, a colour four), every split unit \
                 once more, and tweens that start from the value before another count it again. Tween a group \
                 around a repeat instead of every instance, or animate fewer properties.",
                SceneLimits::MAX_TRACKS
            ),
        );
    }
    if clips > MAX_CLIPS {
        out.push(
            "/timeline",
            "scene.tooManyClips",
            format!("A scene has at most {MAX_CLIPS} clip values in its keys and tweens; this one has {clips}."),
        );
    }
}
