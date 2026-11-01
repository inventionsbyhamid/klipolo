//! Layers drawn through the effect stage: the main footage with its clip's effects, images, overlay video and
//! text with their own, generators (`padded.rs`), and adjustment layers. Each is drawn alone at its drawn size (a
//! moving layer at the largest size its motion reaches), run through its passes and placed by the image quad like any
//! other overlay, or, blended by overlay, by the read-below pass (`overlay_blend`). Layers without effects keep the
//! single-pass path. A layer whose filters draw past its box runs them in a grown target (`padded.rs`).
//!
//! A layer whose blur, pixelation or shape mask couldn't run (`Withheld`) is never drawn as it is: an overlay or the
//! main footage isn't drawn at all, and an adjustment draws black over its box.

use klipolo_model::motion::Blend;

use super::frame::FrameCtx;
use super::{Compositor, Draw, Overlaid, blend};
use crate::plan::{AdjustmentLayer, EffectPass, VideoLayer, VideoRole};
use crate::render::effects::{FrameInfo, Input, PixelRect, Withheld, protects};
use crate::render::uniforms::quad_for;
use crate::render::video::VideoDraw;
use crate::vendor::cap::rendering::DecodedFrame;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

mod size;
use size::needs_resample;
pub(super) use size::{EffectSize, effect_size, upright_turn};

impl Compositor {
    /// `texture` (a layer whose effects run at `at`, upright) placed as `quad`, through `passes` first when it has
    /// any.
    pub(super) fn push_quad(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        texture: &wgpu::Texture,
        at: EffectSize,
        passes: &[EffectPass],
        mut quad: ImageQuad,
    ) {
        // Its protection couldn't run: nothing of the layer is drawn (the stage said so).
        let Ok(out) = self.own_passes(f, texture, at, passes, &mut quad) else {
            return;
        };
        let drawn = out.map_or(texture, |out| self.effects.texture(out)).clone();
        self.place(f, draws, &drawn, quad);
    }

    /// `texture` through `passes` at `at` (its own size): the target holding the result, None when no pass ran (the
    /// texture is drawn as it is), or `Withheld`. A result already stood upright sets `quad`'s turn to 0.
    pub(super) fn own_passes(
        &mut self,
        f: &mut FrameCtx<'_>,
        texture: &wgpu::Texture,
        at: EffectSize,
        passes: &[EffectPass],
        quad: &mut ImageQuad,
    ) -> Result<Option<usize>, Withheld> {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let (local, info) = (at.size, at.info(f.info));
        if passes.is_empty() {
            return Ok(None);
        }
        // One first pass when needed: a texture stored turned (phone video) is turned upright, and a mirrored one
        // flipped, so effects that depend on position (a mask's centre, a vignette's aspect, a blur's direction) act on
        // the picture as it's shown; an image decoded bigger than it's drawn is
        // sampled down through its mipmaps, so effects reading texels at the drawn size don't alias.
        let turn = upright_turn(quad.source_rotation, passes).filter(|_| self.upright);
        let first = if turn.is_some() || quad.mirror || needs_resample(texture, local) {
            self.effects.resample(
                device,
                queue,
                f.encoder,
                texture,
                local,
                (turn.unwrap_or(0), quad.mirror),
                info,
                f.warnings,
            )
        } else {
            None
        };
        match first {
            Some(upright) => {
                let out = self.effects.apply(
                    device,
                    queue,
                    f.encoder,
                    Input::Target(upright),
                    local,
                    passes,
                    info,
                    f.warnings,
                );
                if matches!(out, Ok(Some(_))) {
                    // Already upright and as shown: placed without turning or flipping again.
                    quad.source_rotation = 0;
                    quad.mirror = false;
                }
                out
            }
            // No first pass (or it couldn't run): the effects read the texture as stored.
            None => {
                let stored = if quad.source_rotation % 180 == 90 {
                    (local.1, local.0)
                } else {
                    local
                };
                self.effects.apply(
                    device,
                    queue,
                    f.encoder,
                    Input::Texture(texture),
                    stored,
                    passes,
                    info,
                    f.warnings,
                )
            }
        }
    }

    /// `texture` placed as `quad`: a draw of this pass, or (blended by overlay) one for the read-below pass, which
    /// runs once what is under it is drawn (`encode_passes`).
    pub(super) fn place(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        texture: &wgpu::Texture,
        quad: ImageQuad,
    ) {
        if quad.blend == Blend::Overlay {
            draws.push(Draw::Overlay(self.overlays.len()));
            self.overlays.push(Overlaid {
                texture: texture.clone(),
                quad,
            });
            return;
        }
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        if let Some(i) = self.quads.push_draw(device, queue, texture, quad, f.size) {
            draws.push(Draw::Quad(i));
        }
    }

    /// A video layer drawn alone (`VideoLayer::drawn_alone`) at its drawn size through the composite pipeline (its
    /// crop, mirror, look and LUT included), through its effects (the main footage's clip effects), then placed like an
    /// image, mixed by its blend. The camera, and footage without effects, keep the composite path. The blurred
    /// backdrop is drawn at a quarter of its size each way (a sixteenth of the pixels): its blur hides the size, and
    /// `kl.px` shrinks with it, so the blur reaches as far against the frame.
    pub(super) async fn video_alone(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        v: &VideoLayer,
        frame: &DecodedFrame,
        rotation: u16,
    ) {
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        let quad = ImageQuad {
            blend: v.blend,
            ..quad_for(v.dest, v.rotation, v.opacity, v.radius, f.size)
        };
        let at = match v.role {
            VideoRole::Backdrop => effect_size(&quad, 1.0).halved().halved(),
            _ => effect_size(&quad, 1.0),
        };
        let local = at.size;
        let alone = v.alone();
        let Some(VideoDraw::Composite(index)) = self.video.prepare(
            device,
            queue,
            f.encoder,
            &alone,
            frame,
            rotation,
            local,
            f.grain_frame,
        ) else {
            return;
        };
        let lut = v.look.as_ref().and_then(|l| l.lut.as_ref());
        let group = self
            .luts
            .bind(
                device,
                queue,
                &self.video.composite,
                lut,
                f.warnings,
                f.wait,
            )
            .await;
        let target = self.effects.target(device, local);
        self.passes += 1;
        {
            let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Klipolo layer alone"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.effects.view(target),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.video.render(index, &mut pass, &group);
        }
        // Its effects (in a grown target from its first padded filter on: `padded.rs`), placed; not drawn at all when
        // its protection couldn't run.
        self.through_padded(f, draws, target, at, &v.effects, quad, v.pad_px, 1.0);
    }

    /// An adjustment layer: what's drawn under `a.dest` so far (in `frame`), through its effects, as a quad draw
    /// for the next pass. Rotation is ignored for now: the box is snapped to whole pixels. One with a protective effect
    /// (a blur box) whose copy or effect couldn't run draws opaque black over its box instead: what's under it is never
    /// shown uncovered.
    pub(super) fn adjust(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
        a: &AdjustmentLayer,
        size: (u32, u32),
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Option<usize> {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let rect = PixelRect::of(a.dest, size)?;
        let guarded = a.effects.iter().any(|p| protects(&p.shader));
        let out = match self
            .effects
            .grab(device, queue, encoder, frame, rect, info, warnings, guarded)
        {
            Ok(Some(grabbed)) => self.effects.apply(
                device,
                queue,
                encoder,
                Input::Target(grabbed),
                (rect.w, rect.h),
                &a.effects,
                info,
                warnings,
            ),
            other => other,
        };
        match out {
            Ok(Some(out)) => {
                let quad = quad_for(rect.to_box(size), 0.0, a.opacity, 0.0, size);
                self.quads
                    .push_draw(device, queue, self.effects.texture(out), quad, size)
            }
            // Nothing ran and nothing protects: what's under it shows as it is.
            Ok(None) => None,
            Err(Withheld) => {
                let black = self.effects.black(device, queue).clone();
                let opaque = quad_for(rect.to_box(size), 0.0, 1.0, 0.0, size);
                self.quads.push_draw(device, queue, &black, opaque, size)
            }
        }
    }

    /// An overlay-blended layer (`self.overlays[k]`): the frame's pixels its quad covers (in `frame`, drawn so far),
    /// overlay-blended with it in one pass, as a quad draw for the next pass (opaque, so it replaces those pixels).
    pub(super) fn overlay_blend(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
        k: usize,
        size: (u32, u32),
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Option<usize> {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let Overlaid { texture, quad } = self.overlays.get(k)?;
        let rect = blend::covered(quad, size)?;
        let pass = blend::overlay_pass(self.effects.overlay_shader(), rect, quad);
        let out = self
            .effects
            .apply(
                device,
                queue,
                encoder,
                Input::Under(frame, texture),
                (rect.w, rect.h),
                std::slice::from_ref(&pass),
                info,
                warnings,
            )
            .ok()
            .flatten()?;
        let back = quad_for(rect.to_box(size), 0.0, 1.0, 0.0, size);
        self.quads
            .push_draw(device, queue, self.effects.texture(out), back, size)
    }
}
