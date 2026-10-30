//! The check's one pass (`analyze`, which `check_scene` runs) and the compiler's reading of it ([`plan`]):
//! every node in the order the check lists it (document order, parents before children, an emitter's particle shape
//! after its children), how many split units each can lay out at most, which tweens the check could place, with their
//! targets and the members their staggers spread over, and the facts. The compiler drops the problems: it takes any
//! scene, checked or not, and never panics.

use std::collections::BTreeMap;

use super::super::problems::{at, Problems};
use super::super::{Node, Scene, SceneFacts, SceneLimits};
use super::{cost, nodes, paint, params, scene_fields, shown, timeline, Flat, Place, Scope};
use crate::contrib::Catalog;

/// What the check's pass learns about a scene: every node in document order, what each costs, where each tween
/// lands and the facts. The compiler reads the same pass ([`plan`]), so tweens target and spread over what
/// the check counted.
pub(super) struct Analysis<'a> {
    pub(super) flat: Vec<Flat<'a>>,
    pub(super) costs: Vec<nodes::NodeCost>,
    pub(super) times: Vec<Option<timeline::Placed>>,
    pub(super) facts: SceneFacts,
}

/// The check's one pass, its problems in `out`; None where it stops early (too many settings or nodes).
pub(super) fn analyze<'a>(
    scene: &'a Scene,
    catalog: &'a Catalog,
    out: &mut Problems,
) -> Option<Analysis<'a>> {
    scene_fields(scene, out);
    let params = params::check(scene, out);
    if scene.params.len() > SceneLimits::MAX_PARAMS {
        // Named first and alone, like too many nodes: nothing reads settings the scene can't have.
        return None;
    }
    let mut s = Scope {
        scene,
        catalog,
        params,
        flat: Vec::new(),
        ids: BTreeMap::new(),
    };
    let top = Place {
        depth: 1,
        mult: 1,
        repeat: None,
        owner: None,
        parent: None,
        particle: false,
    };
    for (i, node) in scene.nodes.iter().enumerate() {
        s.add(node, at("/nodes", i), top, out);
    }
    if s.flat.len() > SceneLimits::MAX_NODES {
        // Named first and alone: the rest is the agent's to redo, and checking it costs time in proportion.
        out.push(
            "/nodes",
            "scene.tooManyNodes",
            format!(
                "A scene has at most {} nodes, counting children; this one has {}.",
                SceneLimits::MAX_NODES,
                s.flat.len()
            ),
        );
        return None;
    }
    s.ids(out);
    let mut facts = SceneFacts::default();
    let mut costs: Vec<nodes::NodeCost> = s
        .flat
        .iter()
        .map(|f| nodes::check(&s, out, &mut facts, f))
        .collect();
    let mut times = timeline::check(&s, out, &costs);
    shown::tween_texts(&s, out, &times, &mut costs);
    timeline::refresh(&s, &mut times, &costs);
    cost::check(&s, out, &mut facts, &costs, &times);
    let gradients = paint::gradients(scene);
    if gradients > SceneLimits::MAX_GRADIENTS {
        out.push(
            "/nodes",
            "scene.tooManyGradients",
            format!(
                "A scene draws at most {} different gradients; this one has {gradients}. Shapes that look alike can \
                 share one gradient (an identical gradient counts once), or use a solid colour.",
                SceneLimits::MAX_GRADIENTS
            ),
        );
    }
    Some(Analysis {
        flat: s.flat,
        costs,
        times,
        facts,
    })
}

/// One node as the check saw it.
pub(in crate::scene) struct PlanNode<'a> {
    pub node: &'a Node,
    /// The enclosing node, by index in [`Plan::nodes`].
    pub parent: Option<usize>,
    /// An emitter's particle shape.
    pub particle: bool,
    /// A split text's units at the most its texts can show (0 for every other node).
    pub units: usize,
}

/// A tween the check placed: its targets in document order and its stagger's members, as it counted them.
pub(in crate::scene) struct PlanTween {
    pub targets: Vec<usize>,
    pub members: usize,
}

pub(in crate::scene) struct Plan<'a> {
    pub nodes: Vec<PlanNode<'a>>,
    /// One per tween (at most `MAX_TWEENS`); None where the check couldn't place it.
    pub tweens: Vec<Option<PlanTween>>,
    pub facts: SceneFacts,
}

/// The check's pass over `scene`, with the built-in catalog. A scene the check stops early on (too many settings or
/// nodes) plans as empty.
pub(in crate::scene) fn plan(scene: &Scene) -> Plan<'_> {
    let mut out = Problems::default();
    let Some(a) = analyze(scene, Catalog::builtin(), &mut out) else {
        return Plan {
            nodes: Vec::new(),
            tweens: Vec::new(),
            facts: SceneFacts::default(),
        };
    };
    let nodes = a
        .flat
        .iter()
        .zip(&a.costs)
        .map(|(f, cost)| PlanNode {
            node: f.node,
            parent: f.parent,
            particle: f.particle,
            units: cost.units,
        })
        .collect();
    let tweens = a
        .times
        .into_iter()
        .map(|placed| {
            placed.map(|p| PlanTween {
                targets: p.targets,
                members: p.members,
            })
        })
        .collect();
    Plan {
        nodes,
        tweens,
        facts: a.facts,
    }
}
