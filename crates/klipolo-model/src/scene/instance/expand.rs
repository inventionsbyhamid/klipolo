//! Expanding a compiled scene into instance nodes: each node once, a `repeat`'s children once per instance (`times`,
//! a bound count drawn at its ceiling, or one per table row), moved by `gap` × index. At most `MAX_INSTANCES` drawn
//! nodes (repeats and particle shapes don't count): past it nothing more is built, and what the scene asked for beyond
//! is counted, without walking it, for the renderer's one diagnostic.
//!
//! Tracks are made once per compiled node and shared by its instances (one track per tween and property whatever it
//! targets); an instance node gets tracks of its own only for values bound to its index or row, and for texts.

use std::sync::Arc;

use super::super::eval::ClipState;
use super::super::timeline::{CompiledScene, Track};
use super::super::{NodeType, SceneLimits};
use super::emitter::emitter;
use super::index::{index, Index};
use super::layout::first_placement;
use super::lower::{has_own, lower, Sink};
use super::own::modifiers;
use super::resolve::Values;
use super::statics::base;
use super::text_anim::{own_text, text_anim, units_estimate};
use super::{Ctx, INode, SceneInstance};

mod share;
use crate::params::TableRow;

/// Instance nodes of any kind at most: the drawn ones, and the repeats and particle shapes around them.
const MAX_BUILT: usize = 2 * SceneLimits::MAX_INSTANCES + SceneLimits::MAX_NODES;

/// A compiled node's seed for what it leaves to Klipolo (a random stagger, a wiggle): instance `index`'s.
pub(super) fn seed_of(c: usize, index: f64) -> u32 {
    (c as u32).wrapping_mul(0x9e37_79b9) ^ (index as u32)
}

struct Builder<'a> {
    cs: &'a CompiledScene,
    vals: &'a Values,
    placed: &'a [(f64, f64)],
    nodes: Vec<INode>,
    pool: Vec<Track>,
    lists: Vec<Vec<u32>>,
    list_cnode: Vec<usize>,
    clips: Vec<ClipState>,
    has_own: Vec<bool>,
    /// Drawn nodes still allowed.
    left: usize,
    dropped: usize,
}

pub(super) fn build(cs: &Arc<CompiledScene>, values: Values) -> SceneInstance {
    let placed = first_placement(cs);
    let mut b = Builder {
        cs,
        vals: &values,
        placed: &placed,
        nodes: Vec::new(),
        pool: Vec::new(),
        lists: Vec::new(),
        list_cnode: Vec::new(),
        // Index 0 stands for a node's own clip.
        clips: vec![ClipState::default()],
        has_own: (0..cs.nodes.len()).map(|c| has_own(cs, c)).collect(),
        left: SceneLimits::MAX_INSTANCES,
        dropped: 0,
    };
    b.share();
    let mods = (0..cs.nodes.len())
        .map(|c| modifiers(cs, c, seed_of(c, 0.0)))
        .collect();
    let roots: Vec<usize> = (0..cs.nodes.len())
        .filter(|&i| cs.nodes[i].parent.is_none())
        .collect();
    for c in roots {
        b.expand(c, None, 0.0, None, [0.0; 4]);
    }
    let Builder {
        mut nodes,
        pool,
        lists,
        list_cnode,
        clips,
        dropped,
        ..
    } = b;
    let Index {
        first,
        targets,
        groups,
        emitters,
        has_backdrop,
        buffers,
        particle_cap,
    } = index(cs, &mut nodes, &lists, &pool);
    let list_unit_from = vec![0; lists.len()];
    let bases = vec![Vec::new(); cs.tweens.len()];
    SceneInstance {
        compiled: cs.clone(),
        values,
        nodes,
        first: Arc::from(first),
        targets,
        bases,
        groups,
        placed,
        pool,
        lists,
        list_unit_from,
        list_cnode,
        mods,
        clips,
        motions: super::super::eval::motion_tables(cs),
        fills_animate: false,
        has_backdrop,
        particle_cap,
        emitters,
        dropped_instances: dropped,
        worst_reads: 0,
        still: None,
        with_still: true,
        total_units: 0,
        buffers,
    }
}

impl Builder<'_> {
    fn expand(
        &mut self,
        c: usize,
        parent: Option<usize>,
        index: f64,
        row: Option<&TableRow>,
        gap: [f64; 4],
    ) {
        let cs = self.cs;
        let Some(cn) = cs.nodes.get(c) else {
            return;
        };
        let ctx = Ctx {
            vals: self.vals,
            index,
            row,
        };
        let counts = cn.node.kind != NodeType::Repeat && !cn.particle;
        if self.nodes.len() >= MAX_BUILT || (counts && self.left == 0) {
            self.dropped = self.dropped.saturating_add(self.asked(c, &ctx));
            return;
        }
        if counts {
            self.left -= 1;
        }
        let me = self.nodes.len();
        let made = self.make(c, parent, &ctx, gap);
        self.nodes.push(made);
        if cn.node.kind != NodeType::Repeat {
            for &child in &cn.children {
                self.expand(child, Some(me), index, row, [0.0; 4]);
            }
            return;
        }
        let rows = cn.node.props.rows.as_deref().and_then(|r| ctx.rows(r));
        let n = repeat_count(&cn.node, &ctx, rows);
        let step = cn.node.props.gap.as_ref().map_or([0.0; 4], |[gx, gy]| {
            let x = ctx.len(gx, false).unwrap_or([0.0; 2]);
            let y = ctx.len(gy, false).unwrap_or([0.0; 2]);
            [x[0], y[0], x[1], y[1]]
        });
        for k in 0..n {
            let row_k = rows.and_then(|r| r.get(k));
            let gap_k = step.map(|g| g * k as f64);
            for &child in &cn.children {
                self.expand(child, Some(me), k as f64, row_k, gap_k);
            }
        }
    }

    /// The drawn nodes a node and everything under it would make, counted without building them.
    fn asked(&self, c: usize, ctx: &Ctx) -> usize {
        let Some(cn) = self.cs.nodes.get(c) else {
            return 0;
        };
        let own = usize::from(cn.node.kind != NodeType::Repeat && !cn.particle);
        let inner = cn
            .children
            .iter()
            .map(|&ch| self.asked(ch, ctx))
            .fold(0usize, usize::saturating_add);
        if cn.node.kind == NodeType::Repeat {
            let rows = cn.node.props.rows.as_deref().and_then(|r| ctx.rows(r));
            repeat_count(&cn.node, ctx, rows).saturating_mul(inner)
        } else {
            own.saturating_add(inner)
        }
    }

    fn make(&mut self, c: usize, parent: Option<usize>, ctx: &Ctx, gap: [f64; 4]) -> INode {
        let cs = self.cs;
        let cn = &cs.nodes[c];
        let node = &cn.node;
        let seed = seed_of(c, ctx.index);
        let (base_state, _) = base(node, ctx);
        let mut texts = vec![own_text(node, ctx)];
        let anim = text_anim(node, ctx, seed, &mut texts).map(Box::new);
        let mut list = c;
        if self.has_own.get(c) == Some(&true) {
            let own = {
                let mut sink = Sink {
                    ctx,
                    case: node.props.case,
                    texts: &mut texts,
                    clips: &mut self.clips,
                };
                lower(
                    cs,
                    c,
                    &mut sink,
                    self.placed,
                    seed_of(c, 0.0),
                    true,
                    &|_, _| false,
                )
            };
            let mut indices = self.lists.get(c).cloned().unwrap_or_default();
            for tr in own {
                self.pool.push(tr);
                indices.push((self.pool.len() - 1) as u32);
            }
            self.lists.push(indices);
            self.list_cnode.push(c);
            list = self.lists.len() - 1;
        }
        let units = units_estimate(node, &texts, anim.as_deref()).min(cn.max_units);
        let text_chars = texts.iter().map(|t| t.chars().count() as u32).collect();
        INode {
            cnode: c,
            parent,
            index: ctx.index,
            row: ctx.row.cloned().map(Box::new),
            gap,
            base: base_state,
            list,
            texts,
            text_chars,
            units,
            unit_start: 0,
            node_group: None,
            anim,
            buffer: None,
            emitter: emitter(node, ctx, seed).map(Box::new),
            isolated: node.kind == NodeType::Group
                && (node
                    .props
                    .blend
                    .is_some_and(|b| b != crate::motion::Blend::Normal)
                    || node.props.mask.is_some()),
        }
    }
}

/// How many instances a repeat makes: one per row, or `times` (a bound number at its ceiling), at most 1,000.
fn repeat_count(node: &super::super::Node, ctx: &Ctx, rows: Option<&[TableRow]>) -> usize {
    if node.props.rows.is_some() {
        return rows.map_or(0, <[TableRow]>::len);
    }
    let times = node
        .props
        .times
        .as_ref()
        .and_then(|v| ctx.num(v))
        .unwrap_or(0.0);
    times.ceil().clamp(0.0, SceneLimits::MAX_INSTANCES as f64) as usize
}
