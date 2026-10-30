//! One layer's scene: its settings resolved, every `repeat` and table row expanded into instance nodes (at most
//! `MAX_INSTANCES`, the rest counted as dropped), and its tweens placed on the units its texts actually lay out. Tracks
//! are shared: made once per compiled node and once per tween and property, read by each instance node with its own
//! stagger offsets; an instance node has tracks of its own only for values bound to its repeat's
//! index or row, and for texts. Built when a layer's settings change and re-placed when the renderer reports laid-out
//! units (`set_units`), never per frame.
//!
//! Text unit counts start as an estimate from the resolved texts (characters without spaces, whitespace-separated
//! words, lines) and are held to what the check counted, at most 500 per node and 2,000 per scene (the runtime caps).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::eval::{ClipState, MotionTable, NodeState, SceneFrame};
use super::timeline::{CompiledScene, Track};
use super::SceneLimits;
use crate::params::{ParamValue, TableRow};

mod emitter;
mod expand;
mod fields;
mod hold;
mod index;
mod layout;
mod lower;
mod own;
mod renderer;
mod resolve;
mod statics;
mod text_anim;
mod text_info;

pub(crate) use emitter::Emitter;
pub use fields::Field;
pub(crate) use layout::Group;
pub(crate) use resolve::Ctx;
use resolve::Values;
pub(crate) use text_anim::TextAnim;
pub use text_info::CounterText;

/// A compiled scene with one layer's settings.
pub struct SceneInstance {
    compiled: Arc<CompiledScene>,
    values: Values,
    pub(crate) nodes: Vec<INode>,
    /// Per compiled node, its first instance node (`u32::MAX` for none).
    pub(crate) first: Arc<[u32]>,
    /// Per tween, its target instance nodes in paint order.
    pub(crate) targets: Vec<Vec<usize>>,
    /// Per tween, each target's first member in its stagger, aligned with `targets`.
    pub(crate) bases: Vec<Vec<u32>>,
    /// One per tween, then one per split text with its own stagger.
    pub(crate) groups: Vec<Group>,
    /// Per tween, its start and end on the laid-out texts.
    pub(crate) placed: Vec<(f64, f64)>,
    /// Every track once.
    pub(crate) pool: Vec<Track>,
    /// Track lists into `pool`, each ordered by property and start: one per compiled node (shared by its instances),
    /// then one per instance node with tracks of its own; with where its unit tracks start and its compiled node.
    pub(crate) lists: Vec<Vec<u32>>,
    pub(crate) list_unit_from: Vec<usize>,
    pub(crate) list_cnode: Vec<usize>,
    /// Per compiled node, its modifiers, each with whether every instance reseeds it (a wiggle left unseeded).
    pub(crate) mods: Vec<Vec<(Track, bool)>>,
    /// The clips tracks name; 0 stands for a node's own.
    pub(crate) clips: Vec<ClipState>,
    /// Per compiled node, its motion path flattened.
    pub(crate) motions: Vec<Option<MotionTable>>,
    pub(crate) fills_animate: bool,
    pub(crate) has_backdrop: bool,
    /// Live particles at once at most, over every emitter (at most `MAX_PARTICLES`).
    pub(crate) particle_cap: usize,
    pub(crate) emitters: Vec<usize>,
    /// Instance nodes past `MAX_INSTANCES` the scene asks for and doesn't get.
    pub(crate) dropped_instances: usize,
    pub(crate) total_units: usize,
    /// Nodes with a text buffer (a counter or a scramble).
    pub(crate) buffers: usize,
    /// The most tracks a frame can read (`layout`).
    pub(crate) worst_reads: usize,
    /// Past `MAX_TRACKS`: the still every frame shows, evaluated once, whole, at the hold point.
    pub(crate) still: Option<Box<SceneFrame>>,
    pub(crate) with_still: bool,
}

/// One drawn node of an instance: a compiled node, once per repeat instance it is in.
#[derive(Clone, Debug)]
pub(crate) struct INode {
    pub cnode: usize,
    pub parent: Option<usize>,
    /// `$i`: its nearest repeat's instance index, and `$row`: that repeat's table row.
    pub index: f64,
    pub row: Option<Box<TableRow>>,
    /// How far its repeat instance moves it (`gap` × index): fractions, then pixels, x and y.
    pub gap: [f64; 4],
    /// Its resting values, its own clip among them.
    pub base: NodeState,
    /// Its track list, in `SceneInstance::lists`.
    pub list: usize,
    /// Its texts: its own first, then holds, text keys and tween texts, with their characters.
    pub texts: Vec<String>,
    pub text_chars: Vec<u32>,
    pub units: usize,
    pub unit_start: usize,
    pub node_group: Option<usize>,
    pub anim: Option<Box<TextAnim>>,
    pub buffer: Option<usize>,
    pub emitter: Option<Box<Emitter>>,
    /// A group drawn offscreen whatever it animates: a blend other than normal, or a mask.
    pub isolated: bool,
}

impl SceneInstance {
    /// The scene with `values` (missing or misfitting settings take their defaults).
    pub fn new(
        compiled: &Arc<CompiledScene>,
        values: &BTreeMap<String, ParamValue>,
    ) -> SceneInstance {
        let values = Values::resolve(&compiled.params, values);
        let mut inst = expand::build(compiled, values);
        inst.relayout();
        inst
    }

    /// The compiler's own instance, with the settings' defaults: it never evaluates a still.
    pub(crate) fn defaults(compiled: &Arc<CompiledScene>) -> SceneInstance {
        let values = Values::resolve(&compiled.params, &BTreeMap::new());
        let mut inst = expand::build(compiled, values);
        inst.with_still = false;
        inst.relayout();
        inst
    }

    /// The most tracks a frame of this instance can read; past `MAX_TRACKS` every frame is its settled still.
    pub fn worst_reads(&self) -> usize {
        self.worst_reads
    }

    pub fn compiled(&self) -> &Arc<CompiledScene> {
        &self.compiled
    }

    /// Instance nodes, in paint order.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Tracks the instance holds, shared and its nodes' own (what a frame reads from).
    pub fn track_count(&self) -> usize {
        self.pool.len()
    }

    /// The compiled node instance node `node` draws.
    pub fn cnode(&self, node: usize) -> Option<usize> {
        self.nodes.get(node).map(|n| n.cnode)
    }

    /// Every instance node of the compiled node `cnode`.
    pub fn instances_of(&self, cnode: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(move |(_, n)| n.cnode == cnode)
            .map(|(i, _)| i)
    }

    /// How many units the renderer laid a split text's instance node out in: held to what the check counted (at most
    /// 500) and to 2,000 over the scene; its tweens are placed again on it. Not per frame.
    pub fn set_units(&mut self, node: usize, count: usize) {
        self.set_units_many(&[(node, count)]);
    }

    /// [`set_units`](Self::set_units) for several nodes at once, placing the tweens again once (and not at all when
    /// nothing changed).
    pub fn set_units_many(&mut self, units: &[(usize, usize)]) {
        let mut changed = false;
        for &(node, count) in units {
            let Some(n) = self.nodes.get(node) else {
                continue;
            };
            let most = self
                .compiled
                .nodes
                .get(n.cnode)
                .filter(|c| c.is_split())
                .map_or(0, |c| c.max_units);
            let others = self.total_units.saturating_sub(n.units);
            let count = count
                .min(most)
                .min(SceneLimits::MAX_TEXT_SCENE.saturating_sub(others));
            if let Some(n) = self.nodes.get_mut(node).filter(|n| n.units != count) {
                self.total_units = others.saturating_add(count);
                n.units = count;
                changed = true;
            }
        }
        if changed {
            self.relayout();
        }
    }

    /// [`set_units`](Self::set_units) for every instance of the node with this id; nothing when none has it.
    pub fn set_units_by_id(&mut self, id: &str, count: usize) {
        let Some(c) = self.compiled.find(id) else {
            return;
        };
        let units: Vec<(usize, usize)> = self.instances_of(c).map(|n| (n, count)).collect();
        self.set_units_many(&units);
    }

    /// The renderer says a generator fill's shader animates (from the registry's facts): no frame is then still.
    pub fn set_fills_animate(&mut self, animate: bool) {
        self.fills_animate = animate;
    }

    /// The units instance node `node` lays out.
    pub fn units(&self, node: usize) -> usize {
        self.nodes.get(node).map_or(0, |n| n.units)
    }

    /// Live particles at once at most.
    pub fn particle_capacity(&self) -> usize {
        self.particle_cap
    }

    /// Instance nodes past `MAX_INSTANCES` the scene asked for (not drawn; one diagnostic).
    pub fn dropped_instances(&self) -> usize {
        self.dropped_instances
    }

    /// Text `node` shows as its own text (`case` applied): its text or spans, settings resolved.
    pub fn own_text(&self, node: usize) -> &str {
        self.nodes
            .get(node)
            .and_then(|n| n.texts.first())
            .map_or("", String::as_str)
    }

    /// The particle shape an emitter's instance node draws each particle with.
    pub fn particle_shape(&self, emitter: usize) -> Option<usize> {
        self.nodes.get(emitter)?.emitter.as_ref()?.shape
    }

    /// Where member `node` of tween `ti`'s stagger starts counting: its first member there.
    pub(crate) fn base_of(&self, ti: usize, node: usize) -> usize {
        let (Some(targets), Some(bases)) = (self.targets.get(ti), self.bases.get(ti)) else {
            return 0;
        };
        targets
            .binary_search(&node)
            .ok()
            .and_then(|k| bases.get(k))
            .map_or(0, |&b| b as usize)
    }

    /// The clip a clip track's value names: 0 is the node's own.
    pub(crate) fn clip_of(&self, v: f64, own: &ClipState) -> ClipState {
        match v as usize {
            0 => *own,
            i => self.clips.get(i).copied().unwrap_or(*own),
        }
    }

    fn ctx(&self, node: usize) -> Ctx<'_> {
        fields::ctx_of(self, node)
    }

    /// The defaults' tracks of a compiled node's first instance (the compiler's own).
    pub(crate) fn first_tracks(&self, cnode: usize) -> Vec<Track> {
        let first = self.first.get(cnode).copied().unwrap_or(u32::MAX);
        let Some(n) = self.nodes.get(first as usize) else {
            return Vec::new();
        };
        let mods = self.mods.get(cnode).into_iter().flatten().map(|(t, _)| t);
        self.lists
            .get(n.list)
            .into_iter()
            .flatten()
            .filter_map(|&i| self.pool.get(i as usize))
            .chain(mods)
            .cloned()
            .collect()
    }

    fn relayout(&mut self) {
        layout::relayout(self);
        self.still = None;
        if self.with_still && self.worst_reads > SceneLimits::MAX_TRACKS {
            // At the hold point, before any exit: never a blank still.
            let at = hold::hold_point(self);
            let mut frame = SceneFrame::for_instance(self);
            super::eval::evaluate(self, at, &mut frame);
            self.still = Some(Box::new(frame));
        }
    }
}
