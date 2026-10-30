//! Placing an instance's tweens on its laid-out texts: a stagger spreads over the members it
//! actually has (each target instance once, a split text's laid-out units each, at most what the check counted), so
//! the tweens after it start where it really ends; its members start at whole steps as GSAP's `distribute` spreads
//! them, `random` a seeded shuffle of the same starts. Each tween's first member on each target is kept (`bases`), the
//! shared tracks are moved to where their tweens start, and every track list is ordered by property and start. Runs
//! when the instance is built and when units change; allocates only a `random` stagger's shuffle.

use super::super::eval::tracks::us;
use super::super::timeline::run_reads;
use super::super::timeline::{at::Clock, CompiledScene, Stagger, Track};
use super::super::{SceneLimits, StaggerArg, StaggerFrom};
use super::SceneInstance;
use crate::scene::modifiers::hash;

/// Each tween placed with the check's counts, as the compiler did.
pub(super) fn first_placement(cs: &CompiledScene) -> Vec<(f64, f64)> {
    let out = out_start(cs);
    let mut clock = Clock::new(cs.labels);
    cs.tweens
        .iter()
        .map(|t| match t {
            Some(t) => {
                let spread = t.stagger.map_or(0.0, |st| st.total(t.members));
                clock.place(&t.timing, spread, cs.duration, out)
            }
            None => (0.0, 0.0),
        })
        .collect()
}

/// When the exit segment starts.
pub(crate) fn out_start(cs: &CompiledScene) -> f64 {
    cs.segments
        .map_or(cs.duration, |s| cs.duration - s.out.clamp(0.0, cs.duration))
}

/// The members one stagger spreads over and where each starts.
#[derive(Clone, Debug, Default)]
pub(crate) struct Group {
    pub stagger: Option<Stagger>,
    pub members: usize,
    /// The spread: each × (members − 1), or the amount.
    pub total: f64,
    /// `random`: the start step each member takes.
    perm: Vec<u32>,
}

impl Group {
    fn set(&mut self, stagger: Option<Stagger>, members: usize) {
        self.stagger = stagger;
        self.members = members;
        self.total = stagger.map_or(0.0, |st| st.total(members));
        self.perm.clear();
        if let Some(st) = stagger.filter(|st| st.from == StaggerFrom::Random) {
            self.perm.extend(0..members as u32);
            for i in (1..members).rev() {
                let j = hash(st.seed ^ hash(i as u32)) as usize % (i + 1);
                self.perm.swap(i, j);
            }
        }
    }

    /// When member `i` starts after the first, in seconds.
    pub fn offset(&self, i: usize) -> f64 {
        let Some(st) = self.stagger else {
            return 0.0;
        };
        let n = self.members;
        if n <= 1 || self.total <= 0.0 {
            return 0.0;
        }
        let i = i.min(n - 1);
        let last = (n - 1) as f64;
        let share = match st.from {
            StaggerFrom::Start => i as f64 / last,
            StaggerFrom::End => (last - i as f64) / last,
            StaggerFrom::Random => f64::from(self.perm.get(i).copied().unwrap_or(i as u32)) / last,
            StaggerFrom::Center | StaggerFrom::Edges => {
                // Distances from the middle, scaled so the farthest takes the whole spread (GSAP's distribute).
                let mid = last / 2.0;
                let near = if n % 2 == 1 { 0.0 } else { 0.5 };
                let d = (i as f64 - mid).abs();
                let l = if mid - near > 0.0 {
                    (d - near) / (mid - near)
                } else {
                    0.0
                };
                if st.from == StaggerFrom::Edges {
                    1.0 - l
                } else {
                    l
                }
            }
        };
        share * self.total
    }
}

/// How many members a node is in a stagger: a split text's units (at least one), anything else once.
fn members_of(inst: &SceneInstance, n: usize) -> usize {
    let Some(node) = inst.nodes.get(n) else {
        return 0;
    };
    let split = inst
        .compiled
        .nodes
        .get(node.cnode)
        .is_some_and(|c| c.is_split());
    if split {
        node.units.max(1)
    } else {
        1
    }
}

/// The order a list's tracks run in: node-level first, then by property, then timeline order: where the timeline
/// placed a tween (not its first key, which a `set` or a 0 s tween puts a microsecond earlier), a node's own keys at
/// their first, and list order among tweens placed at the same moment.
fn order_key(t: &Track) -> (bool, super::super::timeline::Prop, i64, i64) {
    (
        t.unit,
        t.prop,
        t.tween.map_or(t.start_us(), |_| t.placed_us),
        t.tween.map_or(-1, |ti| ti as i64),
    )
}

pub(super) fn relayout(inst: &mut SceneInstance) {
    let cs = inst.compiled.clone();
    // At most 2,000 units over the scene (the runtime cap on characters): later texts get what is left.
    let mut total = 0usize;
    for n in &mut inst.nodes {
        n.units = n
            .units
            .min(SceneLimits::MAX_TEXT_SCENE.saturating_sub(total));
        n.unit_start = total;
        total = total.saturating_add(n.units);
    }
    inst.total_units = total;
    let out = out_start(&cs);
    let mut clock = Clock::new(cs.labels);
    for (ti, t) in cs.tweens.iter().enumerate() {
        let Some(t) = t else {
            continue;
        };
        // Each target's first member, and how many there are.
        let mut bases = std::mem::take(&mut inst.bases[ti]);
        bases.clear();
        let mut members = 0usize;
        for &n in inst.targets.get(ti).map_or(&[][..], Vec::as_slice) {
            bases.push(members as u32);
            members = members.saturating_add(members_of(inst, n));
        }
        inst.bases[ti] = bases;
        if let Some(g) = inst.groups.get_mut(ti) {
            g.set(t.stagger, members);
        }
        let spread = inst.groups.get(ti).map_or(0.0, |g| g.total);
        if let Some(p) = inst.placed.get_mut(ti) {
            *p = clock.place(&t.timing, spread, cs.duration, out);
        }
    }
    for n in 0..inst.nodes.len() {
        let Some(g) = inst.nodes[n].node_group else {
            continue;
        };
        let mut stagger = inst.lists[inst.nodes[n].list]
            .iter()
            .filter_map(|&t| inst.pool.get(t as usize))
            .find(|t| t.unit && t.tween.is_none() && t.stagger.is_some())
            .and_then(|t| t.stagger);
        // An unseeded random order is each instance's own, as an unseeded wiggle is.
        let unseeded = cs.nodes.get(inst.nodes[n].cnode).is_some_and(|c| {
            matches!(
                c.node.props.stagger,
                Some(StaggerArg::Spec { seed: None, .. })
            )
        });
        if let Some(st) = stagger.as_mut().filter(|_| unseeded) {
            st.seed ^= hash((inst.nodes[n].index as u32).wrapping_add(0x51ed));
        }
        let units = inst.nodes[n].units;
        if let Some(group) = inst.groups.get_mut(g) {
            group.set(stagger, units);
        }
    }
    let placed = &inst.placed;
    for tr in inst.pool.iter_mut() {
        let Some(ti) = tr.tween else {
            continue;
        };
        let at = us(placed.get(ti).map_or(0.0, |p| p.0));
        let delta = at - tr.placed_us;
        if delta != 0 {
            for k in tr.keys.iter_mut() {
                k.t_us += delta;
            }
            tr.placed_us = at;
        }
    }
    let pool = &inst.pool;
    for (list, unit_from) in inst.lists.iter_mut().zip(inst.list_unit_from.iter_mut()) {
        list.sort_unstable_by_key(|&i| pool.get(i as usize).map(order_key));
        *unit_from = list.partition_point(|&i| pool.get(i as usize).is_none_or(|t| !t.unit));
    }
    // The most tracks a frame can read: each list's runs once per instance node, its unit runs
    // once per unit. An instance past the cap draws its settled still (`SceneInstance::relayout`).
    let per_list: Vec<(usize, usize)> = inst
        .lists
        .iter()
        .zip(&inst.list_unit_from)
        .map(|(list, &unit_from)| {
            let track = |k: usize| list.get(k).and_then(|&i| pool.get(i as usize));
            let run_of = |k: usize| track(k).map(|t| (t.unit, t.prop));
            let (mut own, mut each, mut i) = (0usize, 0usize, 0usize);
            while i < list.len() {
                let mut j = i;
                let mut chained = 0usize;
                while j < list.len() && run_of(j) == run_of(i) {
                    chained += usize::from(track(j).is_some_and(|t| t.chain.is_some()));
                    j += 1;
                }
                let reads = run_reads(j - i, chained);
                if i >= unit_from {
                    each = each.saturating_add(reads);
                } else {
                    own = own.saturating_add(reads);
                }
                i = j;
            }
            (own, each)
        })
        .collect();
    inst.worst_reads = inst
        .nodes
        .iter()
        .map(|n| {
            let (own, each) = per_list.get(n.list).copied().unwrap_or((0, 0));
            own.saturating_add(n.units.saturating_mul(each))
        })
        .fold(0usize, usize::saturating_add);
}
