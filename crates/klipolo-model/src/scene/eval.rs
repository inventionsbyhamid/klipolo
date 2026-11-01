//! Evaluating a frame: every instance node's transform, opacity, paint, trim, clip, blur and text, each
//! split unit's offsets and each live particle, written into a [`SceneFrame`] whose space is reserved once
//! ([`SceneFrame::for_instance`]): once warm, a frame allocates nothing (`tests/scene_alloc.rs`). Counters and
//! scrambles write into the frame's fixed buffers; holds, text keys and tween texts pick one of the instance's texts.
//! The renderer lays texts out and draws units from glyph sets; nothing here measures text.
//!
//! The frame also counts what it draws ([`FrameCounts`], against the runtime caps): blurred
//! units on visible text, characters per node and per scene, drawn instances, live particles and groups drawn
//! offscreen. The evaluator itself never goes past the instance and particle caps (the rest isn't built or written);
//! the renderer degrades the rest: units past 12 draw unblurred, characters past 500 a node or
//! 2,000 a scene are cut, groups past 4 draw flattened, each with one diagnostic per scene.

use std::cell::Cell;
use std::sync::Arc;

use arrayvec::ArrayString;

use super::instance::SceneInstance;
use super::timeline::{CompiledScene, Prop};
use super::NodeType;

mod access;
mod apply;
mod counts;
mod motion;
mod particles;
mod state;
mod still;
mod text;
pub(crate) mod tracks;

use apply::{apply, apply_units};
pub use counts::{CapsOver, FrameCounts};
pub(crate) use motion::tables as motion_tables;
pub use motion::{motion_path, MotionTable, MOTION_SAMPLES};
pub use particles::ParticleState;
pub use state::{ClipShape, ClipState, NodeState, ShadowState, TextNow, UnitState};
pub use still::static_until;
use tracks::Reader;

/// A frame's text for one node: a counter's number or a scramble, up to 500 characters of 4 bytes.
pub type TextBuf = ArrayString<2048>;

/// One frame of an instance, reused frame after frame.
pub struct SceneFrame {
    nodes: Vec<NodeState>,
    units: Vec<UnitState>,
    /// Per node, its units' first index and count.
    unit_ranges: Vec<(u32, u32)>,
    particles: Vec<ParticleState>,
    bufs: Vec<TextBuf>,
    compiled: Arc<CompiledScene>,
    first: Arc<[u32]>,
    counts: FrameCounts,
}

impl SceneFrame {
    /// A frame with room for everything `inst` draws.
    pub fn for_instance(inst: &SceneInstance) -> SceneFrame {
        let mut f = SceneFrame {
            nodes: Vec::new(),
            units: Vec::new(),
            unit_ranges: Vec::new(),
            particles: Vec::new(),
            bufs: Vec::new(),
            compiled: inst.compiled().clone(),
            first: inst.first.clone(),
            counts: FrameCounts::default(),
        };
        f.fit(inst);
        f
    }

    /// Makes room for `inst` (allocating only when it grew, as after `set_units`).
    fn fit(&mut self, inst: &SceneInstance) {
        if !Arc::ptr_eq(&self.first, &inst.first) {
            self.first = inst.first.clone();
            self.compiled = inst.compiled().clone();
        }
        self.nodes.resize(inst.nodes.len(), NodeState::REST);
        self.unit_ranges.resize(inst.nodes.len(), (0, 0));
        self.units.resize(inst.total_units, UnitState::REST);
        self.bufs.resize(inst.buffers, TextBuf::new());
        if self.particles.capacity() < inst.particle_cap {
            self.particles
                .reserve(inst.particle_cap - self.particles.len());
        }
    }

    pub fn nodes(&self) -> &[NodeState] {
        &self.nodes
    }

    /// The first instance of the node with this id.
    pub fn get(&self, id: &str) -> Option<&NodeState> {
        self.nodes.get(self.first_of(id)?)
    }

    /// [`get`](Self::get), for tests and tools: panics when no node has the id.
    pub fn node(&self, id: &str) -> &NodeState {
        self.get(id)
            .unwrap_or_else(|| panic!("no node has the id {id:?}"))
    }

    /// Instance node `node`'s units.
    pub fn units(&self, node: usize) -> &[UnitState] {
        let (start, count) = self.unit_ranges.get(node).copied().unwrap_or((0, 0));
        let (start, count) = (start as usize, count as usize);
        self.units.get(start..start + count).unwrap_or(&[])
    }

    /// The units of the first instance of the node with this id (none when no node has it).
    pub fn units_of(&self, id: &str) -> &[UnitState] {
        self.first_of(id).map_or(&[], |n| self.units(n))
    }

    pub fn particles(&self) -> &[ParticleState] {
        &self.particles
    }

    pub fn counts(&self) -> FrameCounts {
        self.counts
    }

    /// About how many bytes the frame holds on the heap, all reserved up front.
    pub fn approx_bytes(&self) -> usize {
        use std::mem::size_of;
        self.nodes.capacity() * size_of::<NodeState>()
            + self.units.capacity() * size_of::<UnitState>()
            + self.unit_ranges.capacity() * 8
            + self.particles.capacity() * size_of::<ParticleState>()
            + self.bufs.capacity() * size_of::<TextBuf>()
    }

    /// The text instance node `node` shows this frame.
    pub fn text<'a>(&'a self, inst: &'a SceneInstance, node: usize) -> &'a str {
        let Some(n) = inst.nodes.get(node) else {
            return "";
        };
        match self.nodes.get(node).map_or(TextNow::Base, |s| s.text) {
            TextNow::Base => inst.own_text(node),
            TextNow::Text(i) => n.texts.get(usize::from(i)).map_or("", String::as_str),
            TextNow::Buffer(_) => n
                .buffer
                .and_then(|b| self.bufs.get(b))
                .map_or("", |b| b.as_str()),
        }
    }

    /// `still`, copied into this frame's reserved space.
    fn copy_still(&mut self, inst: &SceneInstance, still: &SceneFrame) {
        self.fit(inst);
        self.nodes.clear();
        self.nodes.extend_from_slice(&still.nodes);
        self.units.clear();
        self.units.extend_from_slice(&still.units);
        self.unit_ranges.clear();
        self.unit_ranges.extend_from_slice(&still.unit_ranges);
        self.bufs.clear();
        self.bufs.extend_from_slice(&still.bufs);
        self.particles.clear();
        let room = self.particles.capacity();
        self.particles
            .extend_from_slice(&still.particles[..still.particles.len().min(room)]);
        self.counts = FrameCounts {
            still: true,
            track_evals: 0,
            ..still.counts
        };
    }

    fn first_of(&self, id: &str) -> Option<usize> {
        let c = self.compiled.find(id)?;
        let first = *self.first.get(c)?;
        (first != u32::MAX).then_some(first as usize)
    }
}

/// Writes every node, unit and live particle of `inst` at scene time `t` into `out`; an instance past the track cap
/// writes its settled still, the same whole frame every time (the renderer draws it with one diagnostic).
pub fn eval_into(inst: &SceneInstance, t: f64, out: &mut SceneFrame) {
    match inst.still.as_deref() {
        Some(still) => out.copy_still(inst, still),
        None => evaluate(inst, t, out),
    }
}

/// [`eval_into`], reading every track.
pub(crate) fn evaluate(inst: &SceneInstance, t: f64, out: &mut SceneFrame) {
    out.fit(inst);
    let t = if t.is_finite() { t } else { 0.0 };
    let mut counts = FrameCounts {
        dropped_instances: inst.dropped_instances,
        ..FrameCounts::default()
    };
    let reads = Cell::new(0usize);
    let compiled = inst.compiled();
    for (i, n) in inst.nodes.iter().enumerate() {
        let list = inst.lists.get(n.list).map_or(&[][..], Vec::as_slice);
        let unit_from = inst
            .list_unit_from
            .get(n.list)
            .copied()
            .unwrap_or(0)
            .min(list.len());
        let node_reader = Reader {
            inst,
            node: i,
            unit: None,
            reads: &reads,
        };
        let mut s = n.base;
        apply(&node_reader, &list[..unit_from], t, &mut s, n);
        for (m, reseed) in inst.mods.get(n.cnode).into_iter().flatten() {
            if let Some(f) = m.modifier {
                let f = if *reseed {
                    f.reseeded(n.index as u32)
                } else {
                    f
                };
                s.set(m.prop, s.get(m.prop) + f.at(t));
            }
        }
        s.set(Prop::X, s.x + n.gap[0]);
        s.set(Prop::Y, s.y + n.gap[1]);
        s.set(Prop::XPx, s.x_px + n.gap[2]);
        s.set(Prop::YPx, s.y_px + n.gap[3]);
        let buf = n.buffer.and_then(|b| out.bufs.get_mut(b));
        let chars = text::animate(n, t, &mut s, buf);
        let parent_drawn = n
            .parent
            .and_then(|p| out.nodes.get(p))
            .is_none_or(|p| p.drawn);
        s.drawn = parent_drawn && s.visible && s.opacity > 0.0;
        let cn = compiled.nodes.get(n.cnode);
        let kind = cn.map(|c| c.node.kind);
        if kind == Some(NodeType::Group) {
            s.isolated = s.drawn
                && (n.isolated
                    || s.blur > 0.0
                    || s.rotate_x != 0.0
                    || s.rotate_y != 0.0
                    || s.opacity < 1.0);
            counts.isolated_groups += usize::from(s.isolated);
        }
        if s.drawn && kind != Some(NodeType::Repeat) && !cn.is_some_and(|c| c.particle) {
            counts.instances += 1;
        }
        if s.drawn && kind == Some(NodeType::Text) {
            counts.chars = counts.chars.saturating_add(chars);
            counts.most_chars = counts.most_chars.max(chars);
        }
        let range = (n.unit_start as u32, n.units as u32);
        if let Some(r) = out.unit_ranges.get_mut(i) {
            *r = range;
        }
        let unit_tracks = &list[unit_from..];
        for j in 0..n.units {
            let reader = Reader {
                inst,
                node: i,
                unit: Some(j),
                reads: &reads,
            };
            let mut u = UnitState::REST;
            apply_units(&reader, unit_tracks, t, &mut u);
            if s.drawn && u.opacity > 0.0 && (s.blur > 0.0 || u.blur > 0.0) {
                counts.blurring_units += 1;
            }
            if let Some(slot) = out.units.get_mut(n.unit_start + j) {
                *slot = u;
            }
        }
        if let Some(slot) = out.nodes.get_mut(i) {
            *slot = s;
        }
    }
    counts.track_evals = reads.get();
    particles::write(inst, t, out, &mut counts);
    out.counts = counts;
}
