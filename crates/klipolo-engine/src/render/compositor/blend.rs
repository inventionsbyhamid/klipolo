//! Layer blend modes. Normal, multiply, screen and add are blend states on the same layer pipelines, so
//! they cost nothing extra; overlay needs the colour below and runs as one read-below pass (`overlay.wgsl`), like an
//! adjustment layer. The canvas under the layers is opaque (the background always draws), which makes these exact.

use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::motion::Blend;
use klipolo_model::params::{ParamKind, ParamSpec, PointN};

use crate::plan::{EffectPass, ParamBlock};
use crate::registry::{ShaderEffect, ShaderKind};
use crate::render::effects::PixelRect;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

/// The fixed-function blend modes, in the order the image quad keeps a pipeline for each (`pipeline_index`).
pub(crate) const FIXED: [Blend; 4] = [Blend::Normal, Blend::Multiply, Blend::Screen, Blend::Add];

/// Which of `FIXED` draws `blend` (overlay never reaches the image quad's pipelines; normal if it did).
pub(crate) fn pipeline_index(blend: Blend) -> usize {
    FIXED.iter().position(|b| *b == blend).unwrap_or(0)
}

/// The blend state of `blend`, on premultiplied colour over an opaque canvas.
pub(crate) fn state(blend: Blend) -> wgpu::BlendState {
    let colour = |src, dst| wgpu::BlendComponent {
        src_factor: src,
        dst_factor: dst,
        operation: wgpu::BlendOperation::Add,
    };
    let alpha = colour(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrcAlpha);
    let color = match blend {
        Blend::Normal | Blend::Overlay => {
            colour(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrcAlpha)
        }
        // src·dst + dst·(1 − srcA)
        Blend::Multiply => colour(wgpu::BlendFactor::Dst, wgpu::BlendFactor::OneMinusSrcAlpha),
        // src + dst·(1 − src)
        Blend::Screen => colour(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrc),
        Blend::Add => colour(wgpu::BlendFactor::One, wgpu::BlendFactor::One),
    };
    wgpu::BlendState { color, alpha }
}

/// The overlay pass: the frame drawn so far (`kl_a`, read texel for texel from `origin`) with the layer's texture
/// (`kl_b`) placed over it as its quad places it (rotation, rounding, opacity, the mirror, the stored turn),
/// overlay-blended.
const OVERLAY: &str = include_str!("../../registry/wgsl/overlay.wgsl");

/// The overlay pass's shader (the effect stage keeps it with its other internal passes and warms it with them).
pub(crate) fn overlay_shader() -> ShaderEffect {
    let float = |key: &str| ParamSpec {
        key: key.into(),
        label: key.into(),
        visible_when: None,
        kind: ParamKind::Float {
            min: f64::MIN,
            max: f64::MAX,
            default: 0.0,
            step: None,
            unit: None,
        },
    };
    let point = |key: &str| ParamSpec {
        key: key.into(),
        label: key.into(),
        visible_when: None,
        kind: ParamKind::Point {
            default: PointN { x: 0.0, y: 0.0 },
        },
    };
    let def = ContribDef {
        params: vec![
            point("origin"),
            point("center"),
            point("size"),
            float("cosine"),
            float("sine"),
            float("opacity"),
            float("rounding"),
            float("turn"),
            float("mirror"),
        ],
        ..ContribDef::new(
            "klipolo.builtin/internal.overlay",
            ContribKind::Effect,
            "Overlay",
        )
    };
    ShaderEffect::assemble(&def, ShaderKind::Transition, OVERLAY)
        .expect("the overlay shader assembles")
}

/// The overlay pass over `rect` (the frame's pixels the layer's `quad` covers), with its settings packed.
pub(super) fn overlay_pass(
    shader: &std::sync::Arc<ShaderEffect>,
    rect: PixelRect,
    quad: &ImageQuad,
) -> EffectPass {
    let mut params = ParamBlock::zeroed(shader.layout.size);
    let (sin, cos) = (quad.rotation_deg % 360.0).to_radians().sin_cos();
    let values: [&[f32]; 9] = [
        &[rect.x as f32, rect.y as f32],
        &[quad.center.x as f32, quad.center.y as f32],
        &[quad.size.x as f32, quad.size.y as f32],
        &[cos as f32],
        &[sin as f32],
        &[quad.opacity.clamp(0.0, 1.0) as f32],
        &[(quad.rounding.max(0.0) as f32).min(quad.size.x.min(quad.size.y) as f32 * 0.5)],
        &[f32::from(quad.source_rotation)],
        &[f32::from(u8::from(quad.mirror))],
    ];
    for &(i, _, at) in &shader.layout.fields {
        params.put_f32s(at, values[i]);
    }
    EffectPass::host(shader.clone(), params)
}

/// The frame's pixels `quad` covers once turned (its bounding box), clipped to an `out`-sized frame; None off it.
pub(super) fn covered(quad: &ImageQuad, out: (u32, u32)) -> Option<PixelRect> {
    let (sin, cos) = (quad.rotation_deg % 360.0).to_radians().sin_cos();
    let half_w = (cos.abs() * quad.size.x + sin.abs() * quad.size.y) / 2.0;
    let half_h = (sin.abs() * quad.size.x + cos.abs() * quad.size.y) / 2.0;
    let (w, h) = (f64::from(out.0), f64::from(out.1));
    PixelRect::of(
        klipolo_model::layout::BoxN {
            x: (quad.center.x - half_w) / w,
            y: (quad.center.y - half_h) / h,
            w: 2.0 * half_w / w,
            h: 2.0 * half_h / h,
        },
        out,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Premultiplied src over an opaque dst, as the GPU blends it: (src factor, dst factor) applied to colours.
    fn blended(blend: Blend, src: [f64; 4], dst: f64) -> f64 {
        let s = state(blend).color;
        let factor = |f: wgpu::BlendFactor| match f {
            wgpu::BlendFactor::One => 1.0,
            wgpu::BlendFactor::Dst => dst,
            wgpu::BlendFactor::OneMinusSrcAlpha => 1.0 - src[3],
            wgpu::BlendFactor::OneMinusSrc => 1.0 - src[0],
            other => panic!("{other:?}"),
        };
        src[0] * factor(s.src_factor) + dst * factor(s.dst_factor)
    }

    #[test]
    fn the_fixed_blend_states_compute_their_modes() {
        let grey = [0.5, 0.5, 0.5, 1.0];
        assert_eq!(blended(Blend::Multiply, grey, 0.8), 0.4);
        assert_eq!(blended(Blend::Screen, grey, 0.8), 0.5 + 0.8 * 0.5);
        assert_eq!(blended(Blend::Add, grey, 0.25), 0.75);
        assert_eq!(blended(Blend::Normal, grey, 0.8), 0.5);
        // Where the layer is transparent, every mode leaves what is under it.
        for blend in FIXED {
            assert_eq!(blended(blend, [0.0; 4], 0.3), 0.3, "{blend:?}");
        }
    }
}
