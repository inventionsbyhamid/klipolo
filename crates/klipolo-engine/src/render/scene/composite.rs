//! The host passes that put a scene's offscreen groups back (`composite.wgsl`), run by the effect stage
//! like its own grab and resample: `lay` lays a group's target over the target it began in (opacity, blend, clips, the
//! 2.5D tilt), `mask` keeps it where another node is opaque or bright, `tint` makes its silhouette in a shadow's colour,
//! and the built-in blur blurs it. Each pass is one full-screen draw into a pooled target, at the size of the node's own
//! box (`bounds.rs`) or, for `lay`, of the target the node began in; the stage's pipelines, uniform slots and
//! bind groups are cached, so once warm a frame makes no GPU object here, and a pass's settings are written into a
//! fixed block (no allocation). A normal-blend node that doesn't tilt or mask is laid back as a quad instead
//! (`quad_lay.rs`).

use std::sync::Arc;

use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::motion::Blend;
use klipolo_model::params::{ParamKind, ParamSpec};

use super::place::GpuClip;
use crate::plan::{EffectPass, ParamBlock};
use crate::registry::{ShaderEffect, ShaderKind};

const LAY: &str = concat!(
    include_str!("group_clip.wgsl"),
    include_str!("composite.wgsl")
);
/// `kl_a` (a group) kept where `kl_b` (its mask node, drawn alone) is opaque (`mode.x` 0) or bright (1), or the
/// reverse (`mode.y` 1).
const MASK: &str = "fn transition(uv: vec2<f32>) -> vec4<f32> {
    let g = kl_sample_a(uv);
    let m = kl_sample_b(uv);
    var v = select(m.a, kl_luma(m.rgb), params.mode.x > 0.5);
    if params.mode.y > 0.5 {
        v = 1.0 - v;
    }
    return g * clamp(v, 0.0, 1.0);
}";
/// `kl_a`'s silhouette in `color` (straight RGBA): a shadow before its blur.
const TINT: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    return kl_premultiply(params.color) * kl_sample_a(uv).a;
}";

/// The three host passes, assembled once per drawer, and the scene's own blur: the built-in blur's twin that isn't
/// protective, made once for the built-in it twins.
pub(super) struct Composites {
    lay: Arc<ShaderEffect>,
    mask: Arc<ShaderEffect>,
    tint: Arc<ShaderEffect>,
    blur: Option<(Arc<ShaderEffect>, Arc<ShaderEffect>)>,
}

/// How a group is laid back: its blend mode (`mode`), its opacity, how far what is below moves (a shadow's offset, in
/// target pixels), its tilt and the clips that hold it (its own and the two nearest around it, scene target pixels to
/// each owner's node space), where its target is in the one it is laid into (`gbox`: x, y, w, h) and that one's top
/// left in the scene's target (`porigin`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Lay {
    pub mode: f32,
    pub opacity: f32,
    pub offset: [f32; 2],
    pub tilt: Option<Tilt>,
    pub clips: [GpuClip; 3],
    /// `clips[0]` is the node's own: a tilted node's turns with it.
    pub own_clip: bool,
    pub gbox: [f32; 4],
    pub porigin: [f32; 2],
}

/// A group's 2.5D turn (`rotateX`, `rotateY`) about its origin, seen through `perspective` (target pixels): the
/// rotation's top 2 × 2 and the start of its third row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tilt {
    pub origin: [f32; 2],
    pub d: f32,
    pub r2: [f32; 4],
    pub r3: [f32; 2],
}

impl Tilt {
    /// CSS's `perspective(d) rotateX(x) rotateY(y)` about `origin`: None when it doesn't turn.
    pub(super) fn of(origin: [f64; 2], rotate_x: f64, rotate_y: f64, d: f64) -> Option<Tilt> {
        let ok = |v: f64| v.is_finite();
        if (rotate_x == 0.0 && rotate_y == 0.0) || !ok(rotate_x) || !ok(rotate_y) || !ok(d) {
            return None;
        }
        let (sa, ca) = rotate_x.to_radians().sin_cos();
        let (sb, cb) = rotate_y.to_radians().sin_cos();
        // Rx · Ry, y down and z towards the viewer, as CSS turns.
        Some(Tilt {
            origin: origin.map(|v| v as f32),
            d: d.max(1.0) as f32,
            r2: [cb, 0.0, sa * sb, ca].map(|v| v as f32),
            r3: [-ca * sb, sa].map(|v| v as f32),
        })
    }
}

/// The shader's number for a blend mode.
pub(super) fn mode(blend: Blend) -> f32 {
    match blend {
        Blend::Normal => 0.0,
        Blend::Multiply => 1.0,
        Blend::Screen => 2.0,
        Blend::Overlay => 3.0,
        Blend::Add => 4.0,
    }
}

/// Settings that are each a `vec4<f32>` (a colour slot), in this order.
fn vec4s(keys: &[&str]) -> Vec<ParamSpec> {
    keys.iter()
        .map(|k| ParamSpec {
            key: (*k).into(),
            label: (*k).into(),
            visible_when: None,
            kind: ParamKind::Color {
                default: "#000000".into(),
                alpha: true,
            },
        })
        .collect()
}

fn host(local: &str, kind: ShaderKind, keys: &[&str], body: &str) -> Arc<ShaderEffect> {
    let def = ContribDef {
        params: vec4s(keys),
        ..ContribDef::new(
            format!("klipolo.builtin/internal.{local}").as_str(),
            ContribKind::Effect,
            "Scene group",
        )
    };
    Arc::new(ShaderEffect::assemble(&def, kind, body).expect("a scene's host pass assembles"))
}

/// `values` into `shader`'s settings, each a `vec4<f32>` in schema order.
fn block(shader: &Arc<ShaderEffect>, values: &[[f32; 4]]) -> EffectPass {
    let mut params = ParamBlock::zeroed(shader.layout.size);
    for (&(_, _, at), v) in shader.layout.fields.iter().zip(values) {
        params.put_f32s(at, v);
    }
    EffectPass::host(shader.clone(), params)
}

impl Composites {
    pub(super) fn new() -> Self {
        let lay_keys = [
            "misc", "tilto", "tiltr", "tiltz", "c0m", "c0t", "c0c", "c1m", "c1t", "c1c", "c2m",
            "c2t", "c2c", "gbox", "porig",
        ];
        Self {
            lay: host("scene.lay", ShaderKind::Transition, &lay_keys, LAY),
            mask: host("scene.mask", ShaderKind::Transition, &["mode"], MASK),
            tint: host("scene.tint", ShaderKind::Filter, &["color"], TINT),
            blur: None,
        }
    }

    /// The scene's own twin of `blur` (the built-in blur): the same passes and pipeline, but not protective, so a
    /// scene's blurs are held to the frame's budget for layers' passes and never take the headroom kept for privacy
    /// blurs, nor say a protection failed. Made the first time, then the same `Arc`.
    pub(super) fn scene_blur(&mut self, blur: &Arc<ShaderEffect>) -> Arc<ShaderEffect> {
        match &self.blur {
            Some((of, twin)) if Arc::ptr_eq(of, blur) => twin.clone(),
            _ => {
                let twin = Arc::new(unprotected(blur));
                self.blur = Some((blur.clone(), twin.clone()));
                twin
            }
        }
    }

    /// The pass that lays `kl_b` over `kl_a` as `lay` says.
    pub(super) fn lay(&self, lay: &Lay) -> EffectPass {
        let t = lay.tilt;
        let [c0, c1, c2] = lay.clips;
        let values = [
            [lay.mode, lay.opacity, lay.offset[0], lay.offset[1]],
            t.map_or([0.0; 4], |t| [t.origin[0], t.origin[1], t.d, 1.0]),
            t.map_or([0.0; 4], |t| t.r2),
            t.map_or([0.0; 4], |t| {
                [t.r3[0], t.r3[1], if lay.own_clip { 1.0 } else { 0.0 }, 0.0]
            }),
            c0.m,
            c0.t,
            c0.c,
            c1.m,
            c1.t,
            c1.c,
            c2.m,
            c2.t,
            c2.c,
            lay.gbox,
            [lay.porigin[0], lay.porigin[1], 0.0, 0.0],
        ];
        block(&self.lay, &values)
    }

    /// The pass that keeps `kl_a` where `kl_b` is opaque (or bright: `luma`), or the reverse (`invert`).
    pub(super) fn mask(&self, luma: bool, invert: bool) -> EffectPass {
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        block(&self.mask, &[[on(luma), on(invert), 0.0, 0.0]])
    }

    /// The pass that draws `kl_a`'s silhouette in `color` (straight RGBA).
    pub(super) fn tint(&self, color: [f32; 4]) -> EffectPass {
        block(&self.tint, &[color])
    }

    /// The three, for the warm-up.
    pub(super) fn all(&self) -> [&Arc<ShaderEffect>; 3] {
        [&self.lay, &self.mask, &self.tint]
    }
}

/// `shader` and the pass it draws after (a separable blur's second), neither protective.
fn unprotected(shader: &ShaderEffect) -> ShaderEffect {
    ShaderEffect {
        protective: false,
        then: shader.then.as_deref().map(|t| Arc::new(unprotected(t))),
        ..shader.clone()
    }
}

/// The built-in blur at a Gaussian of `sigma_px` 1080p pixels (its radius is twice the deviation): None when there is
/// nothing to blur or the shader has no radius.
pub(super) fn blur_pass(blur: &Arc<ShaderEffect>, sigma_px: f64) -> Option<EffectPass> {
    if !(sigma_px.is_finite() && sigma_px > 0.0) {
        return None;
    }
    let mut params = ParamBlock::zeroed(blur.layout.size);
    let &(i, _, at) = blur.layout.fields.first()?;
    let ParamKind::Float { min, max, .. } = blur.params[i].kind else {
        return None;
    };
    params.put_f32s(at, &[(2.0 * sigma_px).clamp(min, max) as f32]);
    Some(EffectPass::host(blur.clone(), params))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene's blur is the built-in blur's own passes (the same text, so the same pipeline), and neither pass is
    /// protective; every pass the drawer runs is a built-in.
    #[test]
    fn a_scenes_blur_isnt_protective() {
        let builtin = crate::registry::Registry::builtin().blur().clone();
        assert!(builtin.protective, "the built-in blur protects");
        let mut c = Composites::new();
        let twin = c.scene_blur(&builtin);
        assert!(!twin.protective && !twin.then.as_ref().unwrap().protective);
        assert_eq!(twin.key, builtin.key);
        assert!(Arc::ptr_eq(&twin, &c.scene_blur(&builtin)), "made once");
        // The host passes (and the twin) are built-ins, whose pipelines a registry change keeps.
        let builtin = crate::registry::ShaderTrust::Builtin;
        for pass in [&c.lay, &c.mask, &c.tint, &twin] {
            assert_eq!(pass.trust, builtin, "{}", pass.id);
        }
    }
}
