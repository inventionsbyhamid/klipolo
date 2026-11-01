//! Padding: a filter may draw a declared margin outside its layer's box, so the layer's
//! passes from its first padded filter on run in a target grown by that margin (sized and clipped by the
//! compositor). The layer is drawn into the grown target's inner rect first: an image, video or anything already
//! run through the passes before the padded filter by the PAD pass, transparent around it; a square generator whose
//! first filter is the padded one straight from its own pass, so it costs no extra pass. A text's texture already
//! holds the margin (`render::text`). The grown targets are the layer's own memory: one padded filter holds
//! two (its ping-pong pair), counted against `MAX_FRAME_TARGET_BYTES` like any other.

use std::sync::{Arc, LazyLock};

use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::params::{ParamKind, ParamSpec};

use super::{EffectStage, FrameInfo, Input, Withheld, protects};
use crate::plan::{EffectPass, ParamBlock};
use crate::registry::{ShaderEffect, ShaderKind};

/// The PAD pass, drawn into the grown target's inner rect (a viewport, so `uv` and `kl.size` are the layer's): the
/// layer's texture from the mip level that fits and turned `turn`° clockwise, as `RESAMPLE` does, with the image quad's
/// rounded corners (`radius` in this pass's pixels) and its edge, since the grown quad that places it draws neither.
/// A side `open` (1) has no margin: the grown quad's own edge falls there and draws it, so PAD leaves it whole.
const PAD: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var stored = uv;
    var source = vec2<f32>(textureDimensions(kl_a));
    if params.turn == 90i {
        stored = vec2<f32>(uv.y, 1.0 - uv.x);
        source = source.yx;
    } else if params.turn == 180i {
        stored = vec2<f32>(1.0) - uv;
    } else if params.turn == 270i {
        stored = vec2<f32>(1.0 - uv.y, uv.x);
        source = source.yx;
    }
    let lod = max(log2(max(source.x / kl.size.x, source.y / kl.size.y)), 0.0);
    let colour = textureSampleLevel(kl_a, kl_samp, stored, lod);
    let low = -params.open.xy;
    let high = kl.size + params.open.zw;
    let half = (high - low) * 0.5;
    let q = abs(uv * kl.size - (low + half)) - half + vec2<f32>(params.radius);
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - params.radius;
    return colour * (1.0 - smoothstep(-1.0, 0.0, distance));
}";

/// The PAD shader, assembled once per process (host-internal, like the stage's resample pass).
pub(super) fn pad_shader() -> &'static Arc<ShaderEffect> {
    static PAD_SHADER: LazyLock<Arc<ShaderEffect>> = LazyLock::new(|| {
        let setting = |key: &str, kind| ParamSpec {
            key: key.into(),
            label: key.into(),
            visible_when: None,
            kind,
        };
        let def = ContribDef {
            params: vec![
                setting(
                    "turn",
                    ParamKind::Int {
                        min: 0,
                        max: 270,
                        default: 0,
                    },
                ),
                setting(
                    "radius",
                    ParamKind::Float {
                        min: 0.0,
                        max: f64::from(super::MAX_EFFECT_SIDE),
                        default: 0.0,
                        unit: None,
                        step: None,
                    },
                ),
                // A vec4 slot: left, top, right, bottom, each 0 or 1 (packed by `apply_padded`, never by a person).
                setting(
                    "open",
                    ParamKind::Color {
                        default: "#00000000".into(),
                        alpha: true,
                    },
                ),
            ],
            ..ContribDef::new(
                "klipolo.builtin/internal.pad",
                ContribKind::Effect,
                "Padding",
            )
        };
        Arc::new(
            ShaderEffect::assemble(&def, ShaderKind::Filter, PAD)
                .expect("the pad shader assembles"),
        )
    });
    &PAD_SHADER
}

/// Where a layer lands in its grown target, in target pixels (fractional while its motion draws it smaller than the
/// largest box the target is sized for).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inner {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// How the PAD pass draws a layer into its grown target besides placing it: the clockwise turn that stands it upright
/// (0, 90, 180 or 270: phone video), its corner radius in target pixels, and which sides have no margin (left, top,
/// right, bottom: 1 when the inner rect meets the target's edge there).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadLook {
    pub turn: u16,
    pub radius: f32,
    pub open: [f32; 4],
}

/// A padded layer's target for this frame: its size (the same on every frame of a motion), where the layer
/// lands in it, and how PAD draws it there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Padding {
    pub grown: (u32, u32),
    pub inner: Inner,
    pub look: PadLook,
}

impl EffectStage {
    /// Draws `input` into the inner rect of a new target of `padding.grown` (transparent around it), then runs
    /// `passes` on that grown target with `info` (its `kl.px`, and `kl.content` the inner rect); the result is the
    /// grown size, or half of it when a pass halves the stack.
    ///
    /// - A texture or target (an image, video, or what the passes before the first padded filter made): the PAD pass
    ///   draws it with `padding.look`'s turn, corners and edges. When PAD can't run (the frame's budget), nothing runs:
    ///   None, and the caller draws the layer as it is, or [`Withheld`] when a pass protects (the caller then draws
    ///   none of it).
    /// - Nothing (`Input::Empty`, a square generator): the first pass is the generator's, drawn straight into the
    ///   inner rect (`uv` on its box, `kl.size` its size; it has no second pass), and the rest run on the grown target.
    ///   The compositor draws a generator's own pass through `generate` instead, which does the same and caches it
    ///   (`statics`).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_padded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        input: Input<'_>,
        padding: &Padding,
        passes: &[EffectPass],
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Result<Option<usize>, Withheld> {
        let look = padding.look;
        let (first, shader, rest, guarded) = match input {
            Input::Empty => {
                let Some((first, rest)) = passes.split_first() else {
                    return Ok(None);
                };
                (
                    first.clone(),
                    first.shader.clone(),
                    rest,
                    protects(&first.shader),
                )
            }
            _ => {
                let shader = pad_shader().clone();
                let mut params = ParamBlock::zeroed(shader.layout.size);
                // `turn`, an i32 at 0, `radius`, an f32 at 4, and `open`, a vec4 at 16.
                params.put(0, &i32::from(look.turn).to_le_bytes());
                params.put_f32s(4, &[look.radius.max(0.0)]);
                params.put_f32s(16, &look.open);
                let first = EffectPass::host(shader.clone(), params);
                // Guarded when anything after it protects: its target then gets the protective ceiling, and if it
                // still can't run, none of the layer is shown.
                (
                    first,
                    shader,
                    passes,
                    passes.iter().any(|p| protects(&p.shader)),
                )
            }
        };
        // The inner pass draws the layer itself: its own box is all of its viewport.
        let inner_info = FrameInfo {
            content: FrameInfo::WHOLE,
            ..info
        };
        let Some(out) = self.draw(
            device,
            queue,
            encoder,
            &shader,
            &first,
            input,
            padding.grown,
            Some(padding.inner),
            inner_info,
            warnings,
            guarded,
        ) else {
            if guarded {
                self.note_withheld(warnings);
                return Err(Withheld);
            }
            return Ok(None);
        };
        self.release(input);
        let rest = self.run(
            device,
            queue,
            encoder,
            Input::Target(out),
            padding.grown,
            rest,
            info,
            warnings,
            false,
        )?;
        Ok(Some(rest.unwrap_or(out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pad_shader_assembles_and_validates() {
        let shader = pad_shader();
        crate::registry::validate_wgsl(&shader.source).unwrap();
        // `apply_padded` packs `turn` at 0, `radius` at 4 and `open` at 16.
        assert_eq!(
            shader
                .layout
                .fields
                .iter()
                .map(|&(_, _, at)| at)
                .collect::<Vec<_>>(),
            [0, 4, 16]
        );
    }
}
