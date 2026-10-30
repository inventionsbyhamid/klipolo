//! Generator fills: a rect, ellipse, star or polygon filled with a generator is drawn with that
//! generator's pass, rendered at the shape's box (its own cost is the scene's) through `EffectStage::apply` with
//! `Input::Empty`, and laid inside the shape's coverage: the shape samples the pass's target as its fill (`shapes.wgsl`,
//! a picture fill), so its corners, clips, stroke and shadow stay its own. Only the registry's shader for the id draws,
//! a built-in or a library generator the one runnable predicate let through; the plan resolves them
//! (`SceneLayer::fills`). A fill's settings are resolved once, with the instance (`$key`, `$i`, `$row.*`), and held to
//! its verdict's envelope; a frame only sets its time. Its target is pooled by size class (the box rounded up to 16
//! pixels), the same every frame of a shape that only moves, turns or scales.

use std::collections::BTreeMap;

use klipolo_model::contrib::Resolution;
use klipolo_model::params::ParamValue;
use klipolo_model::scene::Paint;
use klipolo_model::scene::instance::SceneInstance;

use super::pack::{Packed, Run};
use super::place::World;
use super::shape_pack::push_shape;
use super::statics::{Look, NodeStatic};
use crate::plan::{EffectPass, SceneFills, clamped, pack_params};
use crate::render::effects::{EffectStage, FrameInfo, Input, MAX_EFFECT_SIDE};

/// The most generator-filled shapes a frame draws (past them, a shape draws without its fill and the frame says
/// `frame.sceneReduced`).
pub(super) const MAX_FILLS: usize = 8;

/// A node's `fill` index when its generator doesn't draw in this layer (`SceneLayer::fills` has none for it): its shape
/// draws unfilled.
pub(super) const NOT_DRAWN: i16 = -2;

/// The instance's nodes whose generator doesn't draw in this layer marked [`NOT_DRAWN`] (with the instance).
pub(super) fn mark_undrawn(statics: &mut [NodeStatic], fills: &SceneFills) {
    for st in statics {
        if usize::try_from(st.fill).is_ok_and(|g| fills.get(g).is_none()) {
            st.fill = NOT_DRAWN;
        }
    }
}

/// One generator-filled node of an instance and its pass.
pub(super) struct FillPass {
    pub node: u32,
    pass: EffectPass,
    fps: Option<u32>,
    half: bool,
}

/// What a frame fills: node `node`'s shape, in a target of `size`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FillUse {
    pub node: u32,
    pub size: (u32, u32),
}

/// Every generator-filled node's pass (in node order), its settings resolved with its instance and held to its
/// verdict's envelope; `seed` names the layer. Made with the instance, never per frame.
pub(super) fn passes(
    inst: &SceneInstance,
    statics: &[NodeStatic],
    fills: &SceneFills,
    seed: u32,
) -> Vec<FillPass> {
    let mut out = Vec::new();
    let duration = inst.compiled().duration as f32;
    for (i, st) in statics.iter().enumerate() {
        let Some(shader) = usize::try_from(st.fill).ok().and_then(|g| fills.get(g)) else {
            continue;
        };
        let Some(cn) = inst.cnode(i).and_then(|c| inst.compiled().nodes.get(c)) else {
            continue;
        };
        let Some(Paint::Generator(g)) = &cn.node.props.fill else {
            continue;
        };
        let values: BTreeMap<String, ParamValue> = g
            .params
            .iter()
            .map(|(k, v)| (k.clone(), inst.fill_value(i, v)))
            .collect();
        let values = clamped(&values, &shader.params, shader.envelope.as_deref());
        out.push(FillPass {
            node: i as u32,
            pass: EffectPass {
                shader: shader.clone(),
                params: pack_params(shader, &values),
                time_s: 0.0,
                progress: 0.0,
                frame: None,
                duration_s: duration,
                fps: 0.0,
                step_fps: shader.fps.map_or(0.0, |f| f as f32),
                seed: seed.wrapping_add((i as u32).wrapping_mul(0x9E37_79B9)),
                pad_px: 0.0,
            },
            fps: shader.fps,
            half: shader.resolution == Resolution::Half,
        });
    }
    out
}

/// The target size a box of `w` × `h` pixels is filled at: each side rounded up to 16, at most the largest target.
pub(super) fn size_class(w: f64, h: f64) -> (u32, u32) {
    let side = |v: f64| {
        let v = if v.is_finite() {
            v.ceil().max(1.0)
        } else {
            1.0
        };
        ((v as u32).div_ceil(16) * 16).clamp(16, MAX_EFFECT_SIDE)
    };
    (side(w), side(h))
}

/// Node `i`'s shape (`look`) filled by its generator, drawn alone with the generator's target bound; past
/// [`MAX_FILLS`] a frame, it draws unfilled (its stroke and shadow).
#[allow(clippy::too_many_arguments)]
pub(super) fn push(
    p: &mut Packed,
    i: usize,
    here: &World,
    s: &klipolo_model::scene::eval::NodeState,
    st: &NodeStatic,
    look: Look,
    k: f64,
) {
    if p.fill_uses.len() >= MAX_FILLS {
        p.reduced = true;
        let unfilled = klipolo_model::scene::eval::NodeState {
            fill: [0.0; 4],
            ..*s
        };
        push_shape(&mut p.shapes, here, &unfilled, st, look, k, None);
        return;
    }
    p.close_shapes();
    let from = p.shapes.len() as u32;
    push_shape(&mut p.shapes, here, s, st, look, k, None);
    let Some(last) = p.shapes.last_mut() else {
        return;
    };
    // A picture fill: the generator's target across the box (`shapes.wgsl`).
    last.paint[0] = -2.0;
    last.fill = [1.0; 4];
    let to = p.shapes.len() as u32;
    p.runs.push(Run::Fill {
        from,
        to,
        fill: p.fill_uses.len() as u16,
    });
    p.run_start = to;
    p.fill_uses.push(FillUse {
        node: i as u32,
        size: size_class(here.w, here.h),
    });
}

/// This frame's fills (`uses`) drawn: each generator's pass at scene time `t` (held on its `fps` steps), into a pooled
/// target of its size (half of it for a `resolution: half` generator), into `out` (None where it couldn't run).
#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    (device, queue, encoder): (&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder),
    stage: &mut EffectStage,
    passes: &mut [FillPass],
    uses: &[FillUse],
    t: f64,
    info: FrameInfo,
    warnings: &mut Vec<String>,
    out: &mut Vec<Option<usize>>,
) {
    out.clear();
    for u in uses {
        let Ok(k) = passes.binary_search_by_key(&u.node, |f| f.node) else {
            out.push(None);
            continue;
        };
        let f = &mut passes[k];
        let time = match f.fps.filter(|&fps| fps > 0) {
            Some(fps) => (t * f64::from(fps)).floor() / f64::from(fps),
            None => t,
        };
        f.pass.time_s = time as f32;
        let duration = f64::from(f.pass.duration_s).max(1e-6);
        f.pass.progress = (t / duration).clamp(0.0, 1.0) as f32;
        let size = if f.half {
            (u.size.0.div_ceil(2), u.size.1.div_ceil(2))
        } else {
            u.size
        };
        let pass = std::slice::from_ref(&f.pass);
        let made = stage.apply(
            device,
            queue,
            encoder,
            Input::Empty,
            size,
            pass,
            info,
            warnings,
        );
        out.push(made.ok().flatten());
    }
}

/// Whether a shape of `look` takes a generator fill (an SDF shape's: its coverage holds the generator's target).
pub(super) fn fillable(look: Look) -> bool {
    matches!(
        look,
        Look::Rect | Look::Ellipse | Look::Star { .. } | Look::Polygon { .. }
    )
}
