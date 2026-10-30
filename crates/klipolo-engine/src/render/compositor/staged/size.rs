//! Where a layer's effects run (split from `staged.rs`): the size of their targets, how many target pixels each drawn
//! pixel is, and whether the layer's texture needs a first pass to stand it upright or sample it down.

use crate::plan::EffectPass;
use crate::render::effects::{FrameInfo, MAX_EFFECT_SIDE};
use crate::vendor::cap::rendering::layers::image::ImageQuad;

/// Where a layer's effects run: the size of their targets, and how many target pixels each drawn pixel is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::render::compositor) struct EffectSize {
    pub(in crate::render::compositor) size: (u32, u32),
    /// 1, or less when the layer is drawn longer than `MAX_EFFECT_SIDE` and processed smaller.
    pub(in crate::render::compositor) scale: f32,
    /// The layer's own box in the target's uv (`FrameInfo::content`): all of it, except a text texture's filter margin.
    pub(in crate::render::compositor) content: [f32; 4],
    /// The factor a target longer than `MAX_EFFECT_SIDE` was shrunk by (1 otherwise), alone: a scene's text is laid
    /// out at the frame's pixels × the motion's largest scale × this, which is the same every frame.
    pub(in crate::render::compositor) cap: f64,
}

impl EffectSize {
    /// Effects run on a texture of `size` with `per_drawn` texture pixels to each drawn pixel (a text laid out for the
    /// largest size its motion reaches).
    pub(in crate::render::compositor) fn texture(size: (u32, u32), per_drawn: f64) -> Self {
        let per_drawn = if per_drawn.is_finite() && per_drawn > 0.0 {
            per_drawn
        } else {
            1.0
        };
        Self {
            size,
            scale: per_drawn as f32,
            content: FrameInfo::WHOLE,
            cap: 1.0,
        }
    }

    /// The same, the layer's own box being `content` of the target (a text texture holding a filter margin).
    pub(in crate::render::compositor) fn with_content(self, content: [f32; 4]) -> Self {
        Self { content, ..self }
    }

    /// Half of this size, `(w + 1) / 2` each way, with half the target pixels per drawn pixel: a `resolution: half`
    /// generator, placed by a quad that scales it back up.
    pub(in crate::render::compositor) fn halved(self) -> Self {
        Self {
            size: (self.size.0.div_ceil(2), self.size.1.div_ceil(2)),
            scale: self.scale * 0.5,
            ..self
        }
    }

    /// The frame's `info` for passes at this size: `kl.px` (sizes declared in 1080p pixels, such as a mask's feather
    /// and border) shrinks with a capped target, so they keep their size against the layer once it is scaled back up.
    pub(in crate::render::compositor) fn info(self, info: FrameInfo) -> FrameInfo {
        FrameInfo {
            px: info.px * self.scale,
            content: self.content,
            ..info
        }
    }
}

/// A quad's size in whole pixels, for the effect targets it is processed in: `reach` times its drawn size, the
/// largest box the layer's motion reaches (1 for a layer that doesn't grow), so a scaling layer keeps one pooled
/// target size every frame instead of reallocating. A layer bigger than `MAX_EFFECT_SIDE` is processed at that size
/// with its aspect kept (both sides scaled by one factor, so a shape mask's circle stays round). `kl.px` scales by the
/// same factor (target pixels per drawn pixel), so sizes in px keep their size against the drawn layer, and the quad
/// scales the result to it.
pub(in crate::render::compositor) fn effect_size(quad: &ImageQuad, reach: f64) -> EffectSize {
    let reach = if reach.is_finite() && reach > 1.0 {
        reach
    } else {
        1.0
    };
    // To a thousandth first: `reach` divides out the motion's scale this frame, so the same largest box lands on the
    // same whole pixels every frame.
    let grown = |v: f64| (v * reach * 1000.0).round() / 1000.0;
    let (w, h) = (grown(quad.size.x), grown(quad.size.y));
    let longest = w.max(h);
    let cap = if longest > f64::from(MAX_EFFECT_SIDE) {
        f64::from(MAX_EFFECT_SIDE) / longest
    } else {
        1.0
    };
    let side = |v: f64| ((v * cap).ceil() as u32).clamp(1, MAX_EFFECT_SIDE);
    EffectSize {
        size: (side(w), side(h)),
        scale: (reach * cap) as f32,
        content: FrameInfo::WHOLE,
        cap,
    }
}

/// A texture with mipmaps more than a quarter bigger than the size its effects run at (images: decoded up to the
/// next 256 pixels, or at full size).
pub(super) fn needs_resample(texture: &wgpu::Texture, local: (u32, u32)) -> bool {
    texture.mip_level_count() > 1
        && (texture.width() * 4 > local.0 * 5 || texture.height() * 4 > local.1 * 5)
}

/// The clockwise turn that stands a layer's texture upright before its effects run: the texture's stored
/// turn (phone video), to the nearest quarter, when there are effects to run. None (no extra pass) for a layer
/// without effects or a texture stored upright.
pub(in crate::render::compositor) fn upright_turn(
    source_rotation: u16,
    passes: &[EffectPass],
) -> Option<u16> {
    let turn = (source_rotation % 360 + 45) / 90 % 4 * 90;
    (turn != 0 && !passes.is_empty()).then_some(turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::cap::compat::XY;

    fn quad(w: f64, h: f64) -> ImageQuad {
        ImageQuad {
            center: XY::new(0.0, 0.0),
            size: XY::new(w, h),
            rotation_deg: 0.0,
            opacity: 1.0,
            rounding: 0.0,
            source_rotation: 0,
            mirror: false,
            blend: klipolo_model::motion::Blend::Normal,
        }
    }

    #[test]
    fn effect_targets_keep_the_layers_aspect_when_capped() {
        let size = |w, h| effect_size(&quad(w, h), 1.0).size;
        assert_eq!(size(320.0, 179.2), (320, 180));
        // 2:1 drawn 8192 wide: both sides halve, rather than only the long one.
        assert_eq!(size(8192.0, 4096.0), (4096, 2048));
        assert_eq!(size(1000.0, 6000.0), (683, 4096));
    }

    /// A capped target's `kl.px` shrinks with it: a 20 px feather on a layer drawn 8192
    /// wide runs as 10 target pixels, which the quad scales back to 20.
    #[test]
    fn a_capped_target_scales_kl_px_with_it() {
        let info = FrameInfo::new(7, 2.0);
        let capped = effect_size(&quad(8192.0, 4096.0), 1.0);
        assert_eq!(capped.info(info).px, 1.0);
        assert_eq!(capped.info(info).frame, 7);
        assert_eq!(effect_size(&quad(1920.0, 1080.0), 1.0).info(info).px, 2.0);
        assert_eq!(EffectSize::texture((300, 100), 1.0).info(info).px, 2.0);
    }

    /// A layer scaling up to 1.5× keeps one target, the size of its largest box, at every scale on the way,
    /// and its `kl.px` follows the target, so a 12 px blur stays 12 px on the drawn layer.
    #[test]
    fn a_scaling_layer_keeps_one_effect_target() {
        let at = |scale: f64| effect_size(&quad(320.0 * scale, 180.0 * scale), 1.5 / scale);
        for scale in [1.0, 1.1, 1.2345, 1.3, 1.4999, 1.5] {
            assert_eq!(at(scale).size, (480, 270), "{scale}");
        }
        let info = FrameInfo::new(0, 1.0);
        assert!((at(1.2).info(info).px - 1.25).abs() < 1e-6);
    }

    /// The upright pass runs only for a texture stored turned whose layer has effects.
    #[test]
    fn only_turned_layers_with_effects_get_the_upright_pass() {
        use crate::plan::ParamBlock;
        use crate::registry::Registry;
        use klipolo_model::contrib::builtin;

        let registry = Registry::builtin();
        let shader = registry.shader(&builtin::EFFECT_MASK.into()).unwrap();
        let pass = EffectPass::host(shader.clone(), ParamBlock::zeroed(shader.layout.size));
        let fx = std::slice::from_ref(&pass);
        assert_eq!(
            [90, 180, 270].map(|r| upright_turn(r, fx)),
            [Some(90), Some(180), Some(270)]
        );
        assert_eq!(upright_turn(0, fx), None, "stored upright: no extra pass");
        assert_eq!(upright_turn(360, fx), None);
        assert_eq!(upright_turn(90, &[]), None, "no effects: no extra pass");
    }
}
