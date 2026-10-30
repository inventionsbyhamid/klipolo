//! Running a layer's passes: each full-screen draw ping-pongs between the stage's targets, reading the layer's own
//! texture (or the frame under an adjustment, or a transition's two sides) through a kept bind group.
//!
//! **Protective effects fail closed.** A blur, pixelation or shape mask hides what it covers ([`protects`]): its passes
//! get targets past the frame's budget for other effects (up to `MAX_PROTECTED_TARGET_BYTES`), and when one still can't
//! run (that ceiling, a shader that doesn't compile) the run is [`Withheld`]: the caller draws nothing of the layer, or
//! black over an adjustment's box, never what it was meant to hide.

use super::sampling::Degrade;
use super::{EffectStage, FrameInfo, Input, PixelRect};
use crate::plan::{EffectPass, ParamBlock};
use crate::registry::ShaderEffect;

/// A protective effect couldn't run: what it protects must not be drawn as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withheld;

/// What a frame says once (until the project closes) when it hid a layer whose protection couldn't run (a code).
pub(crate) const WITHHELD: &str = crate::api::FRAME_PROTECTION_HIDDEN;

/// Whether `shader` hides what it covers: its contribution's `protective` marker (blur, pixelate, the shape mask), the
/// one source of truth the app's MCP protection reads too. The blur's second pass is the same contribution's.
pub fn protects(shader: &ShaderEffect) -> bool {
    shader.protective
}

/// Layers smaller than this on either side always run at full size (halving them saves next to nothing).
const HALVE_FROM_SIDE: u32 = 64;

impl EffectStage {
    /// `texture` at `size` as the input of its effects (`RESAMPLE`): from the mip level that fits (an image decoded
    /// bigger than it's drawn), turned `turn`° clockwise (0, 90, 180 or 270) when it is stored turned (phone video),
    /// so `size` is the upright size, and flipped when `mirror` (a mirrored video layer, as it's shown). One pass and
    /// one pooled target either way.
    #[allow(clippy::too_many_arguments)]
    pub fn resample(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        size: (u32, u32),
        (turn, mirror): (u16, bool),
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Option<usize> {
        let mut params = ParamBlock::zeroed(self.resample.layout.size);
        // Its settings: `turn`, an i32 at 0, and `mirror`, an i32 at 4.
        params.put(0, &i32::from(turn).to_le_bytes());
        params.put(4, &i32::from(mirror).to_le_bytes());
        let pass = EffectPass::host(self.resample.clone(), params);
        self.run(
            device,
            queue,
            encoder,
            Input::Texture(texture),
            size,
            std::slice::from_ref(&pass),
            info,
            warnings,
            false,
        )
        .unwrap_or(None)
    }

    /// Copies `rect` of `frame` (the frame drawn so far, `out` pixels) into a target, for an adjustment layer.
    /// `guarded`: the adjustment's effects protect what's under it, so the copy counts as protective too.
    #[allow(clippy::too_many_arguments)]
    pub fn grab(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
        rect: PixelRect,
        info: FrameInfo,
        warnings: &mut Vec<String>,
        guarded: bool,
    ) -> Result<Option<usize>, Withheld> {
        let mut params = ParamBlock::zeroed(self.grab.layout.size);
        params.put_f32s(0, &[rect.x as f32, rect.y as f32]);
        let pass = EffectPass::host(self.grab.clone(), params);
        self.run(
            device,
            queue,
            encoder,
            Input::View(frame),
            (rect.w, rect.h),
            std::slice::from_ref(&pass),
            info,
            warnings,
            guarded,
        )
    }

    /// Runs `passes` over `input` (a layer `size` pixels big, premultiplied) and returns the target holding the
    /// result, or None when no pass could run (the caller draws the layer as it was). A protective pass ([`protects`])
    /// that can't run makes it [`Withheld`] (the caller draws nothing of the layer); other passes that can't run are
    /// skipped. Each pass frees the targets it read for the rest of the frame (input targets included, both of a pair,
    /// which the caller no longer needs once a pass ran), so a stack ping-pongs between two targets.
    ///
    /// The first pass whose shader may run at half size (`ShaderEffect::downsample`) and whose settings make
    /// its result smooth enough halves the stack: the current picture is sampled down to half size and that pass and
    /// every later one run there (`px` halved with it). The result is then half size; every caller places it with
    /// the image quad, which scales it back up.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        input: Input<'_>,
        size: (u32, u32),
        passes: &[EffectPass],
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Result<Option<usize>, Withheld> {
        self.run(
            device, queue, encoder, input, size, passes, info, warnings, false,
        )
    }

    /// [`EffectStage::apply`], every pass counted as protective when `guard_all` (an adjustment's copy of what's
    /// under a protective effect).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        input: Input<'_>,
        size: (u32, u32),
        passes: &[EffectPass],
        info: FrameInfo,
        warnings: &mut Vec<String>,
        guard_all: bool,
    ) -> Result<Option<usize>, Withheld> {
        let (mut size, mut info) = (size, info);
        let mut halved = false;
        let mut source = input;
        let mut wrote = None;
        for pass in passes {
            let guarded = guard_all || protects(&pass.shader);
            // A filter the preview's sampler turned off draws the layer without it; one that protects hides it.
            if self.degraded(&pass.shader) == Some(Degrade::Off) {
                if guarded {
                    self.note_withheld(warnings);
                    return Err(Withheld);
                }
                continue;
            }
            // The pass's shader, then its host-run second pass if it has one (separable blur).
            let mut shader = Some(&*pass.shader);
            while let Some(current) = shader {
                if !halved && self.may_halve(device, current, pass, size, info, warnings) {
                    let half = (size.0.div_ceil(2), size.1.div_ceil(2));
                    if let Some(out) =
                        self.halve(device, queue, encoder, source, half, info, warnings)
                    {
                        self.release(source);
                        source = Input::Target(out);
                        wrote = Some(out);
                        size = half;
                        info = info.halved();
                        halved = true;
                    }
                }
                match self.draw(
                    device, queue, encoder, current, pass, source, size, None, info, warnings,
                    guarded,
                ) {
                    Some(out) => {
                        self.release(source);
                        source = Input::Target(out);
                        wrote = Some(out);
                    }
                    // A protection that didn't run: nothing of the layer may be shown.
                    None if guarded => {
                        self.note_withheld(warnings);
                        return Err(Withheld);
                    }
                    None => {}
                }
                shader = current.then.as_deref();
            }
        }
        Ok(wrote)
    }

    /// Says once (until the project closes) that a layer was hidden because its protection couldn't run.
    pub fn note_withheld(&mut self, warnings: &mut Vec<String>) {
        if !self.withheld_said {
            self.withheld_said = true;
            warnings.push(WITHHELD.into());
        }
    }

    /// Frees the targets `input` read for the rest of the frame (never a cached generator's: `statics`).
    pub(super) fn release(&mut self, input: Input<'_>) {
        match input {
            Input::Target(t) => self.free(t),
            Input::Pair(a, b) => {
                self.free(a);
                self.free(b);
            }
            Input::Texture(_) | Input::View(_) | Input::Under(..) | Input::Empty => {}
        }
    }

    /// Whether `shader` (with `pass`'s settings, on a layer `size` pixels big) should run at half size: it allows it
    /// and its setting is big enough (or the preview's sampler slowed it down to half size), the layer
    /// isn't tiny, and the shader compiles (one that can't leaves the layer at full size).
    fn may_halve(
        &mut self,
        device: &wgpu::Device,
        shader: &ShaderEffect,
        pass: &EffectPass,
        size: (u32, u32),
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> bool {
        self.downsample
            && size.0 >= HALVE_FROM_SIDE
            && size.1 >= HALVE_FROM_SIDE
            && (shader
                .downsample
                .as_ref()
                .is_some_and(|rule| rule.applies(&pass.params, info.px))
                || self.degraded(shader) == Some(Degrade::Half))
            && self.ensure_pipeline(device, shader, warnings)
    }

    /// `source` sampled down to `size` (half the layer's) through `RESAMPLE`: a 2 × 2 average from a target, the
    /// mip level that fits from a texture with mipmaps.
    #[allow(clippy::too_many_arguments)]
    fn halve(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: Input<'_>,
        size: (u32, u32),
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Option<usize> {
        let shader = self.resample.clone();
        let pass = EffectPass::host(shader.clone(), ParamBlock::zeroed(shader.layout.size));
        // Unguarded: a halving that can't get a target leaves the pass at full size.
        self.draw(
            device, queue, encoder, &shader, &pass, source, size, None, info, warnings, false,
        )
    }
}
