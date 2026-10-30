//! Masks: a group shows only where another node (`"#id"`) is opaque (`alpha`) or bright (`luma`),
//! or the reverse (`invert`). The mask node is found once, with the instance (inside a repeat, the copy with the
//! group's own `$i`, else the first), and drawn each frame its group is offscreen into a pooled target of the scene's
//! size (its own segment of the frame's runs, drawn before the rest: `encode.rs`; a mask the mask pass reads is let go
//! there), where it stands in the scene, its subtree as it shows. The mask node itself draws in the scene too, where it is; one
//! hidden there (opacity 0 or not visible) still masks, as an SVG mask's content does.

use klipolo_model::scene::MaskMode;
use klipolo_model::scene::instance::SceneInstance;

use super::groups::MaskOf;
use super::pack::{PackIn, Packed, Run};

/// Instance node `i`'s mask, found by its id.
pub(super) fn mask_of(inst: &SceneInstance, i: usize) -> Option<MaskOf> {
    let cn = inst.cnode(i).and_then(|c| inst.compiled().nodes.get(c))?;
    let m = cn.node.props.mask.as_ref()?;
    let c = inst.compiled().find(m.node.strip_prefix('#')?)?;
    let index = inst.repeat_index(i);
    let node = inst
        .instances_of(c)
        .find(|&n| inst.repeat_index(n) == index)
        .or_else(|| inst.instances_of(c).next())?;
    (node != i).then_some(MaskOf {
        node: node as u32,
        luma: m.mode == Some(MaskMode::Luma),
        invert: m.invert == Some(true),
    })
}

/// The frame's mask segments: each offscreen group's mask node, packed after the frame's own runs, between `Run::Mask`
/// and `Run::MaskEnd` (the group's slot).
pub(super) fn segments(p: &mut Packed, f: &PackIn) {
    p.mask_from = p.runs.len();
    for slot in 0..p.isos.len() {
        if let Some(m) = p.isos[slot].mask {
            segment(p, f, slot as u8, m.node as usize);
        }
    }
}

/// Mask node `root`'s subtree, as it shows under it (its own visibility and opacity left out), into slot `slot`'s
/// segment. Particles and offscreen nodes inside draw in place.
fn segment(p: &mut Packed, f: &PackIn, slot: u8, root: usize) {
    let end = f
        .statics
        .get(root)
        .map_or(root + 1, |s| s.end as usize)
        .min(f.frame.nodes().len());
    p.runs.push(Run::Mask(slot));
    p.run_start = p.shapes.len() as u32;
    for j in root..end {
        let (Some(s), Some(st), Some(here)) = (
            f.frame.nodes().get(j),
            f.statics.get(j),
            p.world.get(j).copied(),
        ) else {
            continue;
        };
        if st.particle {
            continue;
        }
        let Some(opacity) = seen(f, root, j) else {
            continue;
        };
        let here = super::place::World { opacity, ..here };
        let rest = p.rest_world.get(j).copied().unwrap_or(here);
        p.node(f, j, &here, &rest, s, st);
    }
    p.close_shapes();
    p.runs.push(Run::MaskEnd(slot));
}

/// The opacity node `j` shows with in the mask of `root`: its own and its ancestors' up to `root` (not `root`'s); None
/// when one of them hides it.
fn seen(f: &PackIn, root: usize, j: usize) -> Option<f64> {
    let mut opacity = 1.0;
    let mut n = j;
    for _ in 0..=klipolo_model::scene::SceneLimits::MAX_DEPTH + 1 {
        if n == root {
            return Some(opacity);
        }
        let s = f.frame.nodes().get(n)?;
        if !s.visible || s.opacity <= 0.0 {
            return None;
        }
        opacity *= s.opacity;
        n = f.inst.parent(n)?;
    }
    None
}
