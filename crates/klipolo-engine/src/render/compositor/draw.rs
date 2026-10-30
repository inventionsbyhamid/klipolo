//! One frame's drawing (steps 2–4 of `Compositor::render`): the background, every layer prepared in plan order
//! (`prepare_layers`, each through `prepare::prepare_one`), then the passes that draw them back to front
//! (`encode_passes`, `passes.rs`).

use std::sync::Arc;
use std::time::Instant;

use super::frame::{FrameCtx, PassStart};
use super::{Compositor, Draw};
use crate::plan::{BackgroundPlan, FramePlan, GRAIN_FPS, Layer, frame_index};
use crate::render::captions::CaptionDrawer;
use crate::render::effects::FrameInfo;
use crate::render::image_load::ImageError;
use crate::render::pool::DecodedSet;
use crate::render::pressure::{self, Pressure};
use crate::render::shared_fonts::SharedFonts;
use crate::vendor::cap::rendering::RenderingError;

mod prepare;

impl Compositor {
    /// Steps 2–4: draws `plan` at `size` into `target` and returns the frame's command encoder (not submitted yet),
    /// its warnings (the decoder's first) and the files it needs that aren't there (videos', then images').
    pub(super) async fn draw(
        &mut self,
        plan: &FramePlan,
        frames: &DecodedSet,
        size: (u32, u32),
        target: &wgpu::TextureView,
    ) -> (wgpu::CommandEncoder, Vec<String>, Vec<Arc<str>>) {
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        let mut warnings = frames.warnings.clone();
        let mut missing = frames.missing.clone();
        self.last_size = size;
        let wait = self.wait_for_assets;

        // 2. Background (an image that can't be loaded draws black and is reported); none in the render worker's
        //    transparent frames.
        let background = if self.transparent {
            Ok(())
        } else {
            self.background
                .prepare(device, queue, &plan.background, size)
                .await
        };
        if let Err(error) = background {
            tracing::warn!(%error, "background not drawn");
            if let RenderingError::ImageLoadError(error) = error {
                match (error, &plan.background) {
                    (ImageError::Missing(_), BackgroundPlan::Image { path, .. }) => {
                        missing.push(path.clone())
                    }
                    (error, _) => warnings.push(error.to_string()),
                }
            }
        }

        // 3. Prepare every layer; overlay video conversions go first in the frame's encoder.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Klipolo compositor"),
        });
        if let Some(timer) = self.timer.as_ref().filter(|_| self.timing) {
            timer.begin(&mut encoder, target);
        }
        self.quads.begin_frame();
        self.effects.begin_frame();
        // The checked shaders that couldn't be compiled, and the effects the preview's sampler slowed down.
        self.effects.frame_notes(&mut warnings);
        self.scenes.begin_frame(device, queue, &mut self.effects);
        self.overlays.clear();
        self.backdrops_used = 0;
        self.passes = 0;
        let lut_tick = self.luts.frame_tick();
        let mut draws = std::mem::take(&mut self.draws);
        draws.clear();
        self.video.begin_frame();
        // Shapes decoded since the last frame enter the cursor atlas before anything draws.
        if let Some(cursor) = &mut self.cursor {
            cursor.begin_frame();
            cursor.upload_ready(device, queue);
        }
        // Grain changes with the timeline frame (not the render count), so preview and export match.
        let grain_frame = frame_index(plan.at_us, GRAIN_FPS) as u32;
        let mut f = FrameCtx {
            encoder: &mut encoder,
            info: FrameInfo::new(grain_frame, size.0.min(size.1) as f32 / 1080.0),
            size,
            canvas: plan.canvas,
            warnings: &mut warnings,
            missing: &mut missing,
            wait,
            grain_frame,
            blacks: 0,
            video: 0,
            target: size,
        };
        // The fonts first, once a frame: when the scan finished or a font was registered, every text and caption is
        // drawn again.
        if self.fonts.as_mut().is_some_and(SharedFonts::refresh) {
            if let Some(texts) = &mut self.texts {
                texts.fonts_changed();
            }
            if let Some(captions) = &mut self.captions {
                captions.fonts_changed();
            }
            self.scenes.fonts_changed();
        }
        // The perf harness's text timing: the drawers' own CPU and GPU from here.
        let timed = self.text_timing.as_mut().map(|t| {
            t.begin_frame();
            Instant::now()
        });
        // Only this frame's texts count toward the text ceiling: the last scene's go first. The caption
        // atlases count toward it too.
        let caption_bytes = self.captions.as_ref().map_or(0, CaptionDrawer::atlas_bytes);
        if let Some(texts) = &mut self.texts {
            texts.reserve(caption_bytes);
            for layer in &plan.layers {
                let sides = match layer {
                    Layer::Transition(t) => [t.outgoing.as_slice(), t.incoming.as_slice()],
                    one => [std::slice::from_ref(one), &[]],
                };
                for l in sides.into_iter().flatten() {
                    if let Layer::Text(text) = l {
                        texts.in_frame(text);
                    }
                }
            }
            texts.begin_frame();
        }
        if let (Some(timing), Some(started)) = (&mut self.text_timing, timed) {
            timing.add_cpu(started);
        }
        self.prepare_layers(&plan.layers, frames, &mut f, &mut draws)
            .await;
        // Titles built ahead that landed this frame are touched once, a second or so before they enter: one draw of one
        // F-1): one draw of one pixel at zero opacity each, so the GPU has their textures resident and sampled before
        // their first frame. Nothing changes on screen.
        if self.texts.is_some() {
            // The drawer's own work for the perf harness's timing.
            let timed = self.text_timing_starts(&mut draws);
            let texts = self.texts.as_mut().expect("checked");
            while let Some(texture) = texts.take_touch() {
                if let Some(i) = self.quads.push_touch(device, queue, &texture, f.size) {
                    draws.push(Draw::Touch(i));
                }
            }
            self.text_timing_ends(&mut draws, timed);
        }
        // Under memory pressure only what this frame draws is kept (titles not in it give up their textures too).
        if pressure::level() != Pressure::Normal {
            self.quads.trim();
            self.luts.trim(lut_tick);
            self.effects.trim();
            if let Some(texts) = &mut self.texts {
                texts.trim();
            }
        }
        let timed = self.text_timing.as_ref().map(|_| Instant::now());
        if let Some(texts) = &mut self.texts {
            texts.end_frame();
        }
        if let (Some(timing), Some(started)) = (&mut self.text_timing, timed) {
            timing.add_cpu(started);
        }

        // 4. Back to front.
        let first = if self.transparent {
            PassStart::Clear(wgpu::Color::TRANSPARENT)
        } else {
            PassStart::Background
        };
        // The frame's last render pass writes the timer's second timestamp at its end.
        self.timing_end = self
            .timer
            .as_ref()
            .filter(|_| self.timing)
            .map(|t| t.query_set());
        self.encode_passes(&mut f, target, &draws, &plan.layers, first);
        if let Some(timer) = self.timing_end.take().and(self.timer.as_ref()) {
            timer.resolve(f.encoder);
        }
        // The passes the preview's sampler timed this frame, resolved for a later frame to read.
        self.effects.end_frame(f.encoder);
        draws.clear();
        self.draws = draws;
        if let Some(timing) = &mut self.text_timing {
            timing.end_frame(f.encoder);
        }
        self.passes_last = self.passes + self.effects.passes() + self.scenes.passes();
        // The cursor slots this frame wrote go up before the frame is submitted.
        if let Some(cursor) = &self.cursor {
            cursor.finish_frame(queue);
        }
        // Contributions that aren't installed draw as nothing; each is reported once until the project closes.
        self.effects.note_missing(&plan.missing, f.warnings);
        (encoder, warnings, missing)
    }

    /// Prepares `layers` in order into `draws`: textures uploaded, effects run, quads placed. Video layers take their
    /// frames from `frames` in turn (`f.video`). A transition's sides are prepared through `prepare_one` too
    /// (`Compositor::transition`), so nothing here recurses: every future has a fixed size and none is boxed.
    pub(super) async fn prepare_layers(
        &mut self,
        layers: &[Layer],
        frames: &DecodedSet,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
    ) {
        for (index, layer) in layers.iter().enumerate() {
            match layer {
                Layer::Transition(t) => self.transition(t, frames, f, draws).await,
                other => self.prepare_one(index, other, frames, f, draws).await,
            }
        }
    }

    /// The perf harness's text timing, before a title or the captions are prepared: when it started and
    /// where its draws start (marked in the pass, for its GPU time). None when off.
    fn text_timing_starts(&mut self, draws: &mut Vec<Draw>) -> Option<(Instant, usize)> {
        self.text_timing.as_ref()?;
        draws.push(Draw::Mark(true));
        Some((Instant::now(), draws.len()))
    }

    /// After a title or the captions were prepared: their CPU counted, and their draws closed by a mark (none when
    /// nothing was drawn).
    fn text_timing_ends(&mut self, draws: &mut Vec<Draw>, timed: Option<(Instant, usize)>) {
        let (Some(timing), Some((started, first))) = (&mut self.text_timing, timed) else {
            return;
        };
        timing.add_cpu(started);
        if draws.len() == first {
            draws.pop();
        } else {
            timing.drew();
            draws.push(Draw::Mark(false));
        }
    }
}
