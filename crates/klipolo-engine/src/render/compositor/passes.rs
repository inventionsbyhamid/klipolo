//! A frame's layers drawn back to front (step 4 of `Compositor::render`): one render pass per adjustment layer plus one,
//! each layer that reads what is under it (an adjustment, an overlay blend, a scene with a backdrop) ending a pass.

use super::frame::{FrameCtx, PassStart};
use super::{Compositor, Draw};
use crate::plan::Layer;

impl Compositor {
    /// Draws `draws` (prepared from `layers`) into `target` back to front, in one pass per adjustment layer plus one.
    /// Each adjustment ends a pass, re-draws what is under it through its effects, and the next pass starts with that
    /// result. `first` says how the first pass starts.
    pub(super) fn encode_passes(
        &mut self,
        f: &mut FrameCtx<'_>,
        target: &wgpu::TextureView,
        draws: &[Draw],
        layers: &[Layer],
        first: PassStart,
    ) {
        let (mut load, background) = match first {
            PassStart::Background => (wgpu::LoadOp::Clear(wgpu::Color::BLACK), true),
            PassStart::Clear(colour) => (wgpu::LoadOp::Clear(colour), false),
        };
        let mut next = 0;
        let mut carried: Option<usize> = None;
        loop {
            // A layer that reads what is under it ends the pass.
            let end = draws[next..]
                .iter()
                .position(|d| matches!(d, Draw::Adjust(_) | Draw::Overlay(_) | Draw::Backdrop(_)))
                .map_or(draws.len(), |i| next + i);
            self.passes += 1;
            // The frame's last pass, when it is timed (`FrameTimer`).
            let timed = (end == draws.len())
                .then(|| self.timing_end.clone())
                .flatten();
            {
                let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Klipolo layers"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: timed.as_ref().map(|set| wgpu::RenderPassTimestampWrites {
                        query_set: set,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(super::frame::END_INDEX),
                    }),
                    occlusion_query_set: None,
                });
                if next == 0 && background {
                    self.background.render(&mut pass);
                }
                if let Some(i) = carried.take() {
                    self.quads.render_draw(&mut pass, i);
                }
                for draw in &draws[next..end] {
                    match draw {
                        Draw::Video(i, lut) => self.video.render(*i, &mut pass, lut),
                        Draw::Black(i) => self.blacks[*i].render(&mut pass),
                        Draw::Quad(i) => self.quads.render_draw(&mut pass, *i),
                        Draw::Cursor(range) => {
                            if let Some(cursor) = &self.cursor {
                                cursor.draw(&mut pass, *range);
                            }
                        }
                        Draw::Caption(slot) => {
                            if let Some(captions) = &self.captions {
                                captions.draw(&mut pass, *slot);
                            }
                        }
                        Draw::Touch(i) => {
                            pass.set_scissor_rect(0, 0, 1, 1);
                            self.quads.render_draw(&mut pass, *i);
                            pass.set_scissor_rect(0, 0, f.size.0, f.size.1);
                        }
                        Draw::Mark(start) => {
                            if let Some(timing) = &mut self.text_timing {
                                timing.mark(&mut pass, *start);
                            }
                        }
                        Draw::Adjust(_) | Draw::Overlay(_) | Draw::Backdrop(_) => {
                            unreachable!("a pass ends before each layer that reads under it")
                        }
                    }
                }
            }
            match draws.get(end) {
                Some(Draw::Adjust(index)) => {
                    if let Layer::Adjustment(a) = &layers[*index] {
                        carried = self.adjust(f.encoder, target, a, f.size, f.info, f.warnings);
                    }
                }
                Some(Draw::Overlay(k)) => {
                    carried = self.overlay_blend(f.encoder, target, *k, f.size, f.info, f.warnings);
                }
                Some(Draw::Backdrop(k)) => carried = self.backdrop_scene(f, target, *k, layers),
                _ => break,
            }
            load = wgpu::LoadOp::Load;
            next = end + 1;
        }
    }
}
