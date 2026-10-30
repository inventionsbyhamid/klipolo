//! One full-screen draw of a pass into a new target (moved from `passes.rs`): the pass's uniforms, the bind group of
//! what it reads, and, for a padded layer (`padding.rs`), the inner rect it is drawn into. A checked pass is timed when
//! the preview samples it (`sampling`), counted toward the crash guard's 60 frames (`handshake`), and drawn in tiles in
//! an export when it is predicted slow (`tiles`).

use super::inputs::ExternalKey;
use super::padding::Inner;
use super::tiles::{TiledPass, tiled_pass, tiles_for};
use super::{EffectStage, FrameInfo, Input, MAX_EFFECT_SIDE};
use crate::plan::EffectPass;
use crate::registry::{ShaderEffect, ShaderTrust};
use crate::render::mode::RenderMode;

impl EffectStage {
    /// One full-screen draw of `shader` with `pass`'s settings over `source` into a new target of `size`; None when
    /// the shader couldn't be compiled or the frame's targets would pass their budget (`budgeted_target`; `guarded`: a
    /// protective pass's higher ceiling).
    ///
    /// With `inner`, the pass draws only into that rect of the target (a viewport, fractional when the layer is drawn
    /// smaller than its target is sized for: its `uv` runs 0–1 across it and `kl.size` is its size), the rest of the
    /// target left transparent: a padded layer's first pass. An `inner` outside the target, or under a hundredth
    /// of a pixel, draws nothing.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        shader: &ShaderEffect,
        pass: &EffectPass,
        source: Input<'_>,
        size: (u32, u32),
        inner: Option<Inner>,
        info: FrameInfo,
        warnings: &mut Vec<String>,
        guarded: bool,
    ) -> Option<usize> {
        let side = |v: u32| v.clamp(1, MAX_EFFECT_SIDE) as f32;
        // Within the target (to a thousandth of a pixel; wgpu refuses a viewport past it), never empty.
        let inner = match inner {
            Some(r) => {
                let (tw, th) = (side(size.0), side(size.1));
                let fits = r.x >= 0.0 && r.y >= 0.0 && r.x < tw && r.y < th;
                let (w, h) = (r.w.min(tw - r.x), r.h.min(th - r.y));
                if !(fits
                    && w >= 0.01
                    && h >= 0.01
                    && r.x + r.w <= tw + 1e-3
                    && r.y + r.h <= th + 1e-3)
                {
                    return None;
                }
                Some(Inner { w, h, ..r })
            }
            None => None,
        };
        if !self.ensure_pipeline(device, shader, warnings) {
            return None;
        }
        // Over the frame's budget, the pass doesn't run (the layer shows without it, or not at all when guarded).
        let out = self.budgeted_target(device, size, warnings, guarded)?;
        let target = self.targets[out].size;
        let drawn = inner.map_or([target.0 as f32, target.1 as f32], |r| [r.w, r.h]);
        let slot = self.slot(device, queue, pass, drawn, info);
        let external = match source {
            Input::Texture(texture) => {
                Some(
                    self.external(device, ExternalKey::Texture(texture.clone()), || {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        (view, None)
                    }),
                )
            }
            Input::View(view) => {
                Some(self.external(device, ExternalKey::View(view.clone()), || {
                    (view.clone(), None)
                }))
            }
            Input::Under(frame, layer) => Some(self.external(
                device,
                ExternalKey::Under(frame.clone(), layer.clone()),
                || {
                    let view = layer.create_view(&wgpu::TextureViewDescriptor::default());
                    (frame.clone(), Some(view))
                },
            )),
            Input::Target(_) | Input::Pair(..) | Input::Empty => None,
        };
        let pair = match source {
            Input::Pair(a, b) => Some(self.pair(device, a, b)),
            _ => None,
        };
        let inputs = match (external, pair, source) {
            (Some(i), _, _) => &self.externals[i].group,
            (None, Some(i), _) => &self.pairs[i].group,
            (None, None, Input::Target(i)) => &self.targets[i].input,
            (None, None, _) => &self.empty_input,
        };
        let pipeline = self
            .pipelines
            .get(shader, self.frame)
            .expect("ensured above")
            .clone();
        self.passes += 1;
        super::watch::count_pass(drawn);
        let checked = shader.trust == ShaderTrust::Checked;
        if checked && self.mode == RenderMode::Interactive {
            self.handshake
                .drew(shader, self.frame, std::time::Instant::now());
        }
        let tiles = if checked && self.tiled {
            tiles_for(shader, target)
        } else {
            1
        };
        if tiles > 1 {
            let pass = TiledPass {
                pipeline: &pipeline,
                view: &self.targets[out].view,
                frame: &self.slots[slot].group,
                inputs,
                inner,
                size: target,
            };
            self.checked_submissions += tiled_pass(&self.dev, encoder, pass, tiles);
            return Some(out);
        }
        if checked {
            self.checked_submissions += 1;
        }
        let pixels = f64::from(target.0) * f64::from(target.1);
        let timed = self.pass_sampler.sample(&self.dev, shader, pixels);
        let timestamp_writes = match (timed, self.pass_sampler.query_set()) {
            (Some(i), Some(query_set)) => Some(wgpu::RenderPassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: Some(i),
                end_of_pass_write_index: Some(i + 1),
            }),
            _ => None,
        };
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Klipolo effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets[out].view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&pipeline);
            if let Some(r) = inner {
                rp.set_viewport(r.x, r.y, r.w, r.h, 0.0, 1.0);
            }
            rp.set_bind_group(0, &self.slots[slot].group, &[]);
            rp.set_bind_group(1, inputs, &[]);
            rp.draw(0..3, 0..1);
        }
        Some(out)
    }
}
