//! The quad lay-back (`quad_lay.wgsl`): a normal-blend offscreen node that doesn't tilt (masked or not:
//! its mask is applied at its own box before) goes back into the target it began in as one quad over
//! its own box, blended premultiplied, its opacity and clips in the shader: no full-target pass and no ping-pong partner
//! (`composite.rs` keeps the host pass for a tilt or another blend). It reads the node's target through the effect
//! stage's own bind group (`EffectStage::input_group`) and its slot (`slots.rs`) at a dynamic offset through one bind
//! group per instance, made on its first draw: once warm, no GPU object.

use std::borrow::Cow;

use super::slots::{LAY_BYTES, Slots, offset};
use crate::render::effects::{EFFECT_FORMAT, made};

const SOURCE: &str = concat!(
    include_str!("group_clip.wgsl"),
    include_str!("quad_lay.wgsl")
);

/// The pipeline, made once per drawer (at warm-up: `SceneDrawer::warm_passes`).
pub(super) struct QuadLay {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

impl QuadLay {
    /// The pipeline, reading the effect stage's targets through `input` (its input layout).
    pub(super) fn new(device: &wgpu::Device, input: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Klipolo scene lay"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SOURCE)),
        });
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo scene lay"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: both,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(LAY_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo scene lay"),
            bind_group_layouts: &[&layout, input],
            push_constant_ranges: &[],
        });
        // As the effect stage samples its targets: linear, clamped (`kl_sample_b` in the host pass).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Klipolo scene lay"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Klipolo scene lay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_lay"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_lay"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: EFFECT_FORMAT,
                    blend: Some(crate::render::compositor::blend::state(
                        klipolo_model::motion::Blend::Normal,
                    )),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        for _ in 0..4 {
            made();
        }
        Self {
            layout,
            sampler,
            pipeline,
        }
    }

    /// An instance's slots bound for the lay (made once per instance, on its first lay).
    pub(super) fn group(&self, device: &wgpu::Device, slots: &Slots) -> wgpu::BindGroup {
        made();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Klipolo scene lay"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: slots.binding(LAY_BYTES),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// The node in slot `slot` (its target read through `node`) laid over `onto` (cleared already).
    pub(super) fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        onto: &wgpu::TextureView,
        (group, slot): (&wgpu::BindGroup, usize),
        node: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Klipolo scene lay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: onto,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[offset(slot)]);
        pass.set_bind_group(1, node, &[]);
        pass.draw(0..4, 0..1);
    }
}
