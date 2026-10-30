//! Indexing an instance's nodes once they are built: each compiled node's first instance, each tween's targets, the
//! stagger groups (one per tween, then one per split text with its own staggered keys), text buffers, emitters and
//! their particle shapes.

use super::super::timeline::{CompiledScene, Track};
use super::super::{NodeType, SceneLimits};
use super::{Group, INode};

pub(super) struct Index {
    pub first: Vec<u32>,
    pub targets: Vec<Vec<usize>>,
    pub groups: Vec<Group>,
    pub emitters: Vec<usize>,
    pub has_backdrop: bool,
    pub buffers: usize,
    pub particle_cap: usize,
}

pub(super) fn index(
    cs: &CompiledScene,
    nodes: &mut [INode],
    lists: &[Vec<u32>],
    pool: &[Track],
) -> Index {
    let mut first = vec![u32::MAX; cs.nodes.len()];
    let mut targets = vec![Vec::new(); cs.tweens.len()];
    let mut groups = vec![Group::default(); cs.tweens.len()];
    let (mut emitters, mut has_backdrop, mut buffers) = (Vec::new(), false, 0usize);
    for i in 0..nodes.len() {
        let c = nodes[i].cnode;
        if let Some(slot) = first.get_mut(c).filter(|s| **s == u32::MAX) {
            *slot = i as u32;
        }
        let cn = &cs.nodes[c];
        for &ti in &cn.tweens {
            if let Some(list) = targets.get_mut(ti) {
                list.push(i);
            }
        }
        has_backdrop |= cn.node.kind == NodeType::Backdrop;
        let staggered_own = lists.get(nodes[i].list).is_some_and(|list| {
            list.iter().any(|&t| {
                pool.get(t as usize)
                    .is_some_and(|t| t.unit && t.tween.is_none() && t.stagger.is_some())
            })
        });
        if staggered_own {
            nodes[i].node_group = Some(groups.len());
            groups.push(Group::default());
        }
        if nodes[i]
            .anim
            .as_ref()
            .is_some_and(|a| a.count.is_some() || a.scramble.is_some())
        {
            nodes[i].buffer = Some(buffers);
            buffers += 1;
        }
        if nodes[i].emitter.is_some() {
            // Its particle shape is its last child.
            let shape = cn
                .children
                .last()
                .filter(|&&ch| cs.nodes[ch].particle)
                .and_then(|&ch| {
                    (i + 1..nodes.len())
                        .find(|&j| nodes[j].cnode == ch && nodes[j].parent == Some(i))
                });
            if let Some(e) = nodes[i].emitter.as_mut() {
                e.shape = shape;
            }
            emitters.push(i);
        }
    }
    let particle_cap = emitters
        .iter()
        .filter_map(|&e| nodes[e].emitter.as_ref().map(|em| em.cap))
        .fold(0usize, usize::saturating_add)
        .min(SceneLimits::MAX_PARTICLES);
    Index {
        first,
        targets,
        groups,
        emitters,
        has_backdrop,
        buffers,
        particle_cap,
    }
}
