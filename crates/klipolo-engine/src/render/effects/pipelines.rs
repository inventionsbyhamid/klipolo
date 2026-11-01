//! The effect stage's pipelines (one per shader key, compiled after naga validated the source) and the uniform
//! slots each pass reads. In the app's preview a checked shader's pipeline is compiled on the compile thread and drawn
//! only after the crash guard's handshake (`compile_thread`, `handshake`); every other one, and every one
//! in `Contained` mode, compiles here on first use, before the frame draws it. Every compile's error scopes take the
//! device's lock (`scopes`).

use super::frame_info::{KlFrame, kl_frame};
use super::{EFFECT_FORMAT, EffectStage, FrameInfo, made};
use crate::plan::{EffectPass, MAX_PARAM_BYTES};
use crate::registry::{Registry, ShaderEffect, ShaderKind, ShaderTrust};
use crate::render::device::RenderDevice;
use crate::render::mode::RenderMode;

/// Params sit at this offset in a pass's uniform buffer (wgpu's minimum uniform offset alignment).
const PARAMS_OFFSET: u64 = 256;
/// A pass's uniform buffer: `KlFrame`, then the params at `PARAMS_OFFSET`.
const UNIFORM_BYTES: usize = PARAMS_OFFSET as usize + MAX_PARAM_BYTES;

pub(super) struct UniformSlot {
    buffer: wgpu::Buffer,
    pub(super) group: wgpu::BindGroup,
    /// What the buffer holds: a pass whose uniforms didn't change (a still frame drawn again) writes nothing, and
    /// one that did writes once (each `write_buffer` makes a staging buffer).
    written: Option<[u8; UNIFORM_BYTES]>,
}

impl EffectStage {
    /// Compiles the built-in effects', generators' and transitions' pipelines now, so the first frame that uses one
    /// doesn't stall on its compile (`GpuBackend::warm_all`). One that fails is left to compile again, and be
    /// reported, on first use. Its error scopes take the device's lock.
    pub fn warm(&mut self) {
        let started = std::time::Instant::now();
        let registry = Registry::builtin();
        let internal = [
            self.grab.clone(),
            self.resample.clone(),
            self.overlay.clone(),
            super::padding::pad_shader().clone(),
        ];
        let mut shaders: Vec<&ShaderEffect> = internal.iter().map(|s| &**s).collect();
        for shader in registry.shaders() {
            shaders.push(shader);
            shaders.extend(shader.then.as_deref());
        }
        for shader in shaders {
            if self.pipelines.contains(shader) {
                continue;
            }
            self.sync_compiles += 1;
            match compile(&self.dev, &self.pipeline_layout, shader) {
                Ok(pipeline) => {
                    made();
                    self.pipelines.insert(shader, Some(pipeline), self.frame);
                }
                Err(error) => tracing::warn!(effect = %shader.id, %error, "effect shader"),
            }
        }
        tracing::debug!(
            ms = started.elapsed().as_secs_f64() * 1000.0,
            "built-in effect pipelines ready"
        );
    }

    /// Compiles `shaders`' pipelines now too (the scene drawer's own host passes), as `warm` does: on
    /// this thread, its error scopes under the device's lock.
    pub(crate) fn warm_also(&mut self, shaders: &[&ShaderEffect]) {
        for shader in shaders {
            if self.pipelines.contains(shader) {
                continue;
            }
            self.sync_compiles += 1;
            match compile(&self.dev, &self.pipeline_layout, shader) {
                Ok(pipeline) => {
                    made();
                    self.pipelines.insert(shader, Some(pipeline), self.frame);
                }
                Err(error) => tracing::warn!(effect = %shader.id, %error, "effect shader"),
            }
        }
    }

    /// The render worker's registry became `registry`: the pipelines of shaders it no longer holds
    /// go (built-ins and the stage's own stay), and so do the kept generator pictures, which were another draft's.
    pub fn keep_shaders_of(&mut self, registry: &Registry) {
        let own = [
            &self.grab,
            &self.resample,
            &self.overlay,
            super::padding::pad_shader(),
        ];
        let mut keep: Vec<&ShaderEffect> = own.iter().map(|s| &***s).collect();
        for shader in registry.shaders() {
            keep.push(shader);
            keep.extend(shader.then.as_deref());
        }
        self.pipelines.retain(&keep);
        self.let_go_statics();
    }

    /// Pipelines kept, built-ins included (tests).
    #[doc(hidden)]
    pub fn pipelines_kept(&self) -> usize {
        self.pipelines.len()
    }

    /// Whether `shader`'s pipeline can draw this frame: compiled now if it never was, except a checked one in the app's
    /// preview, which the compile thread is asked for and which draws only once its crash-guard mark is acknowledged
    /// (until then the layer draws without it, and this answers false).
    pub(super) fn ensure_pipeline(
        &mut self,
        _device: &wgpu::Device,
        shader: &ShaderEffect,
        warnings: &mut Vec<String>,
    ) -> bool {
        let guarded = self.mode == RenderMode::Interactive && shader.trust == ShaderTrust::Checked;
        if !self.pipelines.contains(shader) && guarded {
            self.ask(shader);
            return false;
        }
        // A built-in queued on the compile thread: handed back already, perhaps.
        if !self.pipelines.contains(shader) && self.builtin_queued(shader) {
            self.poll_compiles();
            // A shader preset still waiting there may sit behind an agent's compile: compiled
            // here it would wait on the scope lock for that compile, so the layer draws without it until
            // it is handed back and the wake draws again, as for a checked shader.
            if !self.pipelines.contains(shader) && super::guarded::is_preset(shader) {
                return false;
            }
        }
        if !self.pipelines.contains(shader) {
            made();
            self.sync_compiles += 1;
            let pipeline = compile(&self.dev, &self.pipeline_layout, shader)
                .map_err(|error| {
                    tracing::warn!(effect = %shader.id, %error, "effect shader");
                    warnings.push(match shader.kind {
                        // The compositor shows the nearer side whole instead.
                        ShaderKind::Transition => format!(
                            "The {} transition couldn't be drawn, so it plays as a cut.",
                            shader.name
                        ),
                        ShaderKind::Filter | ShaderKind::Generator => format!(
                            "The {} effect couldn't be drawn, so it's left out.",
                            shader.name
                        ),
                    });
                })
                .ok();
            self.pipelines.insert(shader, pipeline, self.frame);
        }
        // In the app's preview a checked effect draws only under its own crash-guard mark.
        self.pipelines.get(shader, self.frame).is_some()
            && (!guarded || self.handshake.may_draw(shader))
    }

    /// A uniform slot for one pass this frame, drawn `size` pixels big (a padded layer's inner rect may be fractional):
    /// `KlFrame` at 0 and the packed params at 256.
    pub(super) fn slot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &EffectPass,
        size: [f32; 2],
        info: FrameInfo,
    ) -> usize {
        if self.slots.len() == self.used_slots {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Klipolo effect uniforms"),
                size: UNIFORM_BYTES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Klipolo effect frame"),
                layout: &self.frame_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: wgpu::BufferSize::new(std::mem::size_of::<KlFrame>() as u64),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: PARAMS_OFFSET,
                            size: wgpu::BufferSize::new(MAX_PARAM_BYTES as u64),
                        }),
                    },
                ],
            });
            made();
            self.slots.push(UniformSlot {
                buffer,
                group,
                written: None,
            });
        }
        let i = self.used_slots;
        self.used_slots += 1;
        let frame = kl_frame(pass, size, info);
        let mut bytes = [0u8; UNIFORM_BYTES];
        let frame = bytemuck::bytes_of(&frame);
        bytes[..frame.len()].copy_from_slice(frame);
        let params = pass.params.bytes();
        bytes[PARAMS_OFFSET as usize..PARAMS_OFFSET as usize + params.len()]
            .copy_from_slice(params);
        let slot = &mut self.slots[i];
        if slot.written.as_ref() != Some(&bytes) {
            queue.write_buffer(&slot.buffer, 0, &bytes);
            slot.written = Some(bytes);
            self.uniform_writes += 1;
        }
        i
    }
}

pub(super) fn input_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    a: &wgpu::TextureView,
    b: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Klipolo effect input"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(a),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(b),
            },
        ],
    })
}

/// naga first (readable errors), then wgpu, whose default shader modules keep bounds and loop checks on, as
/// untrusted WGSL needs. A shader that isn't built in compiles only as its hardened module
/// (`guard::harden`: the shared loop counter and the sanitising entry), which `ShaderEffect::compilable` verifies
/// first; its loop bound doesn't rely on wgpu's. wgpu runs inside error scopes: a module or pipeline it refuses after naga
/// passed (a backend or driver limit) fails only this effect and never reaches the device's fault handler, which would
/// fail the frame. Scopes are one stack per device, so the pair holds the device's lock (`dev.scopes`): this runs on
/// the drawing thread and on the compile thread (`compile_thread`) alike.
pub(super) fn compile(
    dev: &RenderDevice,
    layout: &wgpu::PipelineLayout,
    shader: &ShaderEffect,
) -> Result<wgpu::RenderPipeline, String> {
    shader.compilable()?;
    let device = &dev.device;
    let filters = [wgpu::ErrorFilter::Internal, wgpu::ErrorFilter::Validation];
    let (pipeline, error) = dev.scopes.scoped(device, &filters, || {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(shader.id.as_str()),
            source: wgpu::ShaderSource::Wgsl(shader.source.as_ref().into()),
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(shader.id.as_str()),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("kl_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("kl_fs"),
                compilation_options: Default::default(),
                targets: &[Some(EFFECT_FORMAT.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    });
    match error {
        Some(error) => Err(error.to_string()),
        None => Ok(pipeline),
    }
}
