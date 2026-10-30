//! The scene compiler: once per scene, every node in paint order (children after parents) with its
//! tracks, keys a frame is evaluated from with no allocation. The timeline's tweens are placed by GSAP's rules and each
//! becomes keys on its targets' tracks:
//! - `to` runs from the value at its start to its own; `from` from its own to the value at its end (shown before it
//!   starts, as GSAP renders a `from` at once); `fromTo` from `start` to its values; `set` holds from its time;
//! - a number `at` is absolute, `>` (and no `at`) the end so far, `<` the previous tween's start, `+=s`/`-=s` the end
//!   so far moved, a label its time, `out` the exit segment's start;
//! - a property is the track that started last at that moment (GSAP's render order when it seeks): before any has,
//!   the first one's first value;
//! - a tween with `stagger` on a split text moves each of its units (one without moves the node), its members starting
//!   at whole stagger steps as GSAP's `distribute` spreads them (`random` a shuffle of the same starts); repeats and
//!   yoyos run per unit, and an odd yoyo ends on its start values;
//! - tweens ease with GSAP's default, `power1.out`; a node's own keys (`animate`) are linear unless they name one.
//!
//! The tracks here hold the settings' defaults, placed with the check's counts; each layer's [`SceneInstance`]
//! resolves its own settings, repeats and laid-out units (members are the laid-out units, at most
//! what the check counted).
//!
//! [`SceneInstance`]: super::instance::SceneInstance

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Node, NodeType, Scene, SceneFacts, Segments, Tween};
use crate::ease_names;
use crate::motion::{Ease, Key};
use crate::params::ParamSpec;

pub(crate) mod at;
mod prop;
mod spread;

pub use super::modifiers::ModifierFn;
pub use prop::Prop;
pub(crate) use prop::{lower_of, run_reads, Lower};
pub use spread::{Repeat, Stagger};

/// A compiled scene, shared (`Arc`) by every layer that shows it.
#[derive(Clone, Debug)]
pub struct CompiledScene {
    pub duration: f64,
    pub segments: Option<Segments>,
    /// Paint order: each node before its children; an emitter's particle shape after its children.
    pub nodes: Vec<CNode>,
    pub params: Vec<ParamSpec>,
    pub facts: SceneFacts,
    /// Its settings' defaults' hold point: the latest moment before any exit starts, else its end (a still layer of a
    /// scene without segments is drawn here, never the blank end of a fade-out).
    pub hold: f64,
    pub(crate) tweens: Vec<Option<CTween>>,
    pub(crate) labels: usize,
    pub(crate) ids: BTreeMap<String, usize>,
}

/// One node of a compiled scene.
#[derive(Clone, Debug)]
pub struct CNode {
    /// As written, without its children or particle shape (they are their own nodes here).
    pub node: Node,
    pub parent: Option<usize>,
    /// Its children in order, then its particle shape.
    pub children: Vec<usize>,
    /// An emitter's particle shape: drawn once per live particle.
    pub particle: bool,
    /// A split text's units at the most its texts can show (what the check counted); `set_units` is held to it.
    pub max_units: usize,
    /// Its tracks with the settings' defaults (a repeat's first instance and row), placed with the check's counts.
    pub tracks: Vec<Track>,
    /// The tweens that target it, in timeline order.
    pub(crate) tweens: Vec<usize>,
}

impl CNode {
    /// A split text: tweens with a stagger move its units.
    pub fn is_split(&self) -> bool {
        self.node.kind == NodeType::Text && self.node.props.split.is_some()
    }
}

/// One animated number of a node or of each of its units: keys in scene time, with how they spread and repeat.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub prop: Prop,
    pub keys: Vec<Key>,
    pub stagger: Option<Stagger>,
    pub repeat: Option<Repeat>,
    /// Added after the keys (a track with a modifier has no keys).
    pub modifier: Option<ModifierFn>,
    /// Each of a split text's units runs it, offset by its stagger.
    pub unit: bool,
    /// The tween it comes from (None for a node's own keys and modifiers).
    pub(crate) tween: Option<usize>,
    /// The key that takes the value the property had there (a `to`'s or `set`'s start, a `from`'s end).
    pub(crate) chain: Option<usize>,
    /// The tween's start its keys are placed at, in microseconds.
    pub(crate) placed_us: i64,
}

impl Track {
    /// When it starts (its first key; 0 with none).
    pub fn start_us(&self) -> i64 {
        self.keys.first().map_or(0, |k| k.t_us)
    }
}

/// What a tween does with its values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verb {
    To,
    From,
    FromTo,
    Set,
}

/// A placed tween, read once.
#[derive(Clone, Debug)]
pub(crate) struct CTween {
    pub tween: Tween,
    pub verb: Verb,
    pub timing: at::Timing,
    pub ease: Ease,
    pub stagger: Option<Stagger>,
    pub repeat: Option<Repeat>,
    /// The members the check spread it over.
    pub members: usize,
}

/// Compiles a scene: any scene, checked or not, without panicking (a scene the check stops early on compiles empty).
pub fn compile(scene: &Scene) -> CompiledScene {
    let plan = super::check::plan(scene);
    let mut nodes: Vec<CNode> = plan
        .nodes
        .iter()
        .map(|p| CNode {
            node: Node {
                kind: p.node.kind,
                id: p.node.id.clone(),
                class: p.node.class.clone(),
                bounds: p.node.bounds.clone(),
                props: super::Props {
                    child: None,
                    ..p.node.props.clone()
                },
                animate: p.node.animate.clone(),
                children: Vec::new(),
            },
            parent: p.parent,
            children: Vec::new(),
            particle: p.particle,
            max_units: p.units.min(super::SceneLimits::MAX_TEXT_NODE),
            tracks: Vec::new(),
            tweens: Vec::new(),
        })
        .collect();
    for i in 0..nodes.len() {
        if let Some(parent) = nodes[i].parent.filter(|&p| p < i) {
            nodes[parent].children.push(i);
        }
    }
    let has_out = scene.segments.is_some();
    let mut labels = at::Labels::default();
    let mut tweens = Vec::with_capacity(plan.tweens.len());
    for (ti, (t, placed)) in scene.timeline.iter().zip(&plan.tweens).enumerate() {
        let Some(placed) = placed else {
            tweens.push(None);
            continue;
        };
        let pos = at::parse(t.at.as_ref(), &labels, has_out).unwrap_or(at::At::End(0.0));
        let label = t.label.as_deref().and_then(|l| labels.define(l));
        for &n in &placed.targets {
            if let Some(node) = nodes.get_mut(n) {
                node.tweens.push(ti);
            }
        }
        tweens.push(Some(read_tween(t, ti, pos, label, placed)));
    }
    let ids = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| Some((n.node.id.clone()?, i)))
        .fold(BTreeMap::new(), |mut m, (id, i)| {
            m.entry(id).or_insert(i);
            m
        });
    let mut facts = plan.facts;
    facts.generator_fills = generator_fills(&nodes);
    let mut compiled = CompiledScene {
        duration: if scene.duration.is_finite() {
            scene.duration.clamp(0.0, super::SceneLimits::MAX_DURATION)
        } else {
            0.0
        },
        segments: scene
            .segments
            .filter(|s| s.enter.is_finite() && s.out.is_finite()),
        nodes,
        params: scene.params.clone(),
        facts,
        hold: 0.0,
        tweens,
        labels: labels.names.len(),
        ids,
    };
    // The defaults' tracks, through the same lowering every instance runs.
    let defaults = super::instance::SceneInstance::defaults(&Arc::new(compiled.clone()));
    for (i, node) in compiled.nodes.iter_mut().enumerate() {
        node.tracks = defaults.first_tracks(i);
    }
    compiled.hold = defaults.hold_time();
    compiled
}

fn read_tween(
    t: &Tween,
    ti: usize,
    pos: at::At,
    label: Option<usize>,
    placed: &super::check::PlanTween,
) -> CTween {
    let verb = if t.from.is_some() {
        Verb::From
    } else if t.from_to.is_some() {
        Verb::FromTo
    } else if t.set.is_some() {
        Verb::Set
    } else {
        Verb::To
    };
    let timing = at::Timing::of(t, pos, label);
    let ease = match &t.ease {
        Some(name) => ease_names::read(name).map_or(Ease::Power1Out, |(e, _)| e),
        None => Ease::Power1Out,
    };
    CTween {
        tween: t.clone(),
        verb,
        timing,
        ease: if ease.is_valid() {
            ease
        } else {
            Ease::Power1Out
        },
        stagger: t.stagger.as_ref().map(|st| Stagger::of(st, ti as u32)),
        repeat: (timing.repeat != 0).then_some(Repeat {
            count: timing.repeat,
            yoyo: t.yoyo == Some(true),
            delay: timing.delay,
        }),
        members: placed.members,
    }
}

/// The generators drawn inside shapes and spans, each once.
fn generator_fills(nodes: &[CNode]) -> Vec<crate::contrib::ContribId> {
    let mut out: Vec<crate::contrib::ContribId> = Vec::new();
    for n in nodes {
        let p = &n.node.props;
        let spans = p.spans.iter().flatten().filter_map(|s| s.fill.as_ref());
        for paint in [&p.fill, &p.stroke].into_iter().flatten().chain(spans) {
            if let super::Paint::Generator(g) = paint {
                if !out.contains(&g.generator) {
                    out.push(g.generator.clone());
                }
            }
        }
    }
    out
}

impl CompiledScene {
    /// The node with this id.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.ids.get(id).copied()
    }
}
