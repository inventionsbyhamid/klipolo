//! Backdrops: a `backdrop` node shows the frame under its layer inside its rounded box, blurred by
//! its `blur` and under its `tint` (a glass card). The compositor grabs the layer's box from the frame drawn so far
//! before it draws the scene (`EffectStage::grab`, as for an adjustment layer: `compositor/backdrop.rs`) and hands it
//! over with how this scene's target pixels map onto it; each backdrop node blurs a copy (the built-in blur, a pooled
//! target of the grab's size) and draws it as its fill (`shapes.wgsl`'s backdrop paint). A scene drawn with
//! nothing under it (a transition's side, the render worker's transparent frames) draws its tint alone. A scene with a
//! backdrop is never held still: what is under it may change every frame.

use std::sync::Arc;

use klipolo_model::scene::eval::NodeState;

use super::composite::blur_pass;
use super::pack::{Packed, Run};
use super::place::World;
use super::shape_pack::push_shape;
use super::statics::{Look, NodeStatic};
use crate::registry::ShaderEffect;
use crate::render::effects::{EffectStage, FrameInfo, Input};

/// The most backdrop nodes a frame draws (past them, the frame says `frame.sceneReduced` and the rest draw their tint).
pub(super) const MAX_BACKDROPS: usize = 4;

/// The frame under a scene layer, grabbed before the scene draws: its target in the effect stage, how this scene's
/// target pixels map onto its uv (`[a, b, c, d, e, f]`: u = a x + b y + e, v = c x + d y + f), and its pixels per 1080p
/// pixel (the frame's: a backdrop's blur is measured on the frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backdrop {
    pub target: usize,
    pub map: [f32; 6],
    pub px: f32,
}

/// What a frame's backdrop node needs: its Gaussian, in 1080p pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BackdropUse {
    pub sigma: f64,
}

/// Backdrop node `s` (place `here`) as its rounded box filled with the frame under the layer (mapped by `map`),
/// blurred and tinted, drawn alone with its copy bound; without a frame under it (`map` None), or past
/// [`MAX_BACKDROPS`], its tint alone.
pub(super) fn push(
    p: &mut Packed,
    here: &World,
    s: &NodeState,
    st: &NodeStatic,
    map: Option<[f32; 6]>,
    k: f64,
) {
    let tint = s.tint;
    let full = p.backdrop_uses.len() >= MAX_BACKDROPS;
    p.reduced |= full && map.is_some();
    let Some(map) = map.filter(|_| !full) else {
        if tint[3] > 0.0 {
            let tinted = NodeState { fill: tint, ..*s };
            push_shape(&mut p.shapes, here, &tinted, st, Look::Rect, k, None);
        }
        return;
    };
    p.close_shapes();
    let from = p.shapes.len() as u32;
    push_shape(&mut p.shapes, here, s, st, Look::Rect, k, None);
    let Some(last) = p.shapes.last_mut() else {
        return;
    };
    // The backdrop paint: this target's pixel → the copy's uv, under the tint (the stroke slot; a backdrop has none).
    last.paint = [-3.0, -1.0, map[4], map[5]];
    last.extra = [map[0], map[1], map[2], map[3]];
    last.stroke = tint;
    last.fill = [1.0; 4];
    last.misc[0] = 0.0;
    let to = p.shapes.len() as u32;
    p.runs.push(Run::Backdrop {
        from,
        to,
        slot: p.backdrop_uses.len() as u16,
    });
    p.run_start = to;
    p.backdrop_uses.push(BackdropUse {
        sigma: s.blur.max(0.0),
    });
}

/// This frame's backdrop nodes' copies of the frame under the layer (`under`): each blurred by its own Gaussian into a
/// pooled target of the grab's size (the scene's own blur, while the frame's budget has room: else drawn unblurred,
/// said as the stage says an effect left out), or the grab itself when it doesn't blur, into `out`; None without one.
/// The last blurred use reads the grab as its input, which lets it go (a glass card holds one target,
/// not two). What they hold the stage counts (`EffectStage::watch`).
#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    (device, queue, encoder): (&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder),
    stage: &mut EffectStage,
    blur: Option<&Arc<ShaderEffect>>,
    under: Option<Backdrop>,
    uses: &[BackdropUse],
    info: FrameInfo,
    warnings: &mut Vec<String>,
    out: &mut Vec<Option<usize>>,
) {
    out.clear();
    let Some(under) = under else {
        out.extend(uses.iter().map(|_| None));
        return;
    };
    let grabbed = stage.texture(under.target).clone();
    let size = (grabbed.width(), grabbed.height());
    let info = FrameInfo {
        px: under.px,
        content: FrameInfo::WHOLE,
        ..info
    };
    let mut left_out = false;
    let mut grab_read = false;
    for (k, u) in uses.iter().enumerate() {
        // The grab goes with its last use only when no use before reads it as it is.
        let last = k + 1 == uses.len() && !grab_read;
        let pass = blur.and_then(|b| blur_pass(b, u.sigma));
        left_out = left_out || (pass.is_some() && !stage.room_for(size, 2, warnings));
        let blurred = pass.filter(|_| !left_out).and_then(|pass| {
            let pass = std::slice::from_ref(&pass);
            let input = if last {
                Input::Target(under.target)
            } else {
                Input::Texture(&grabbed)
            };
            stage
                .apply(device, queue, encoder, input, size, pass, info, warnings)
                .ok()
                .flatten()
        });
        grab_read |= blurred.is_none();
        out.push(Some(blurred.unwrap_or(under.target)));
    }
}
