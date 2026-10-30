//! A scene text's blurring pieces drawn small (`text_blur.wgsl`): before the scene's passes,
//! each blurring piece's cell of the atlas is sampled down into its region of a pooled scratch target (`text_blur.rs`
//! placed them as the frame packed), blurred across into the same region of a second, and down back into the first,
//! three instanced draws in all; the text pass reads it back from there. The scratch targets are as tall as the row
//! class their regions need, the largest this text has needed (one size through a blur-in, so the pool keeps them), and
//! go back to the pool once the scene has drawn. Per instance text, made the first frame it blurs and kept: its slots'
//! uniform buffer and the first pass's bind group (its masks), and, again only when the pool hands out other targets,
//! the other two passes' bind groups and the text's with the blur bound. Once warm, nothing is made and a frame whose
//! slots didn't change writes nothing.

use std::borrow::Cow;

use super::text_blur::{BLUR_WIDTH, BlurBudget, BlurSlot, MAX_BLUR_PIECES};
use super::text_pipeline::{TextGpu, TextPipeline};
use crate::render::effects::{EFFECT_FORMAT, EffectStage, made};

/// The passes' three pipelines (sample, across, down) and their bind group layout (slots, what they read, sampler).
pub(super) struct BlurPass {
    layout: wgpu::BindGroupLayout,
    sample: wgpu::RenderPipeline,
    across: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

/// One instance text's blur passes: its slots, the first pass's bind group, what the buffer holds, the scratch rows it
/// takes, and what it bound for the scratch targets it was last handed.
pub(super) struct TextBlur {
    slots: wgpu::Buffer,
    group: wgpu::BindGroup,
    uploaded: [BlurSlot; MAX_BLUR_PIECES],
    rows: u32,
    pub(super) with: Option<Bound>,
}

/// The scratch targets an instance text was handed (the stage's ids, no clone of either), the across
/// and down passes' bind groups reading them, and the text's bind group reading the blur back from the first.
pub(super) struct Bound {
    targets: [u64; 2],
    across: wgpu::BindGroup,
    down: wgpu::BindGroup,
    pub(super) text: wgpu::BindGroup,
}

impl BlurPass {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Klipolo scene text blur"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("text_blur.wgsl"))),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo scene text blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo scene text blur"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Klipolo scene text blur"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_blur"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: EFFECT_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let (sample, across, down) = (
            pipeline("fs_sample"),
            pipeline("fs_across"),
            pipeline("fs_down"),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Klipolo scene text blur"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        for _ in 0..6 {
            made();
        }
        Self {
            layout,
            sample,
            across,
            down,
            sampler,
        }
    }

    /// A pass's bind group: `slots`, reading `source`.
    fn group(
        &self,
        device: &wgpu::Device,
        slots: &wgpu::Buffer,
        source: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        made();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Klipolo scene text blur"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: slots.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// An instance text's blur passes (its masks `masks`).
    fn text_blur(&self, device: &wgpu::Device, masks: &wgpu::TextureView) -> TextBlur {
        let slots = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Klipolo scene text blur slots"),
            size: std::mem::size_of::<[BlurSlot; MAX_BLUR_PIECES]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        made();
        TextBlur {
            group: self.group(device, &slots, masks),
            slots,
            uploaded: [BlurSlot {
                misc: [-1.0; 4],
                ..BlurSlot::default()
            }; MAX_BLUR_PIECES],
            rows: 0,
            with: None,
        }
    }
}

/// One of the passes: `n` regions drawn into `into` by `pipeline` with `group` bound. Not cleared:
/// each pass writes every texel of its regions, and nothing reads outside them (the text pass clamps to its region).
fn draw(
    encoder: &mut wgpu::CommandEncoder,
    into: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    n: u32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Klipolo scene text blur"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: into,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..6, 0..n);
}

/// The passes `run` draws when anything blurs.
pub(super) const PASSES: u32 = 3;

/// The frame's blurring pieces (`budget`) of instance text `gpu` blurred small into two pooled scratch targets, and
/// the text's draws set to read the blur back from the first: the two (the caller gives them back to the pool once the
/// scene has drawn), or None when nothing blurs (or the frame's budget has no target left: its pieces then draw
/// unblurred, the stage said so).
pub(super) fn run(
    (device, queue, encoder): (&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder),
    stage: &mut EffectStage,
    tp: &TextPipeline,
    gpu: &mut TextGpu,
    budget: &BlurBudget,
    warnings: &mut Vec<String>,
) -> Option<[usize; 2]> {
    gpu.use_blur = false;
    let n = budget.count();
    if n == 0 {
        return None;
    }
    let passes = &tp.across;
    let blur = gpu
        .blur
        .get_or_insert_with(|| passes.text_blur(device, &gpu.masks_view));
    blur.rows = blur.rows.max(budget.rows());
    let size = (BLUR_WIDTH, blur.rows);
    let first = stage.budgeted_target(device, size, warnings, false)?;
    let Some(second) = stage.budgeted_target(device, size, warnings, false) else {
        stage.free_target(first);
        return None;
    };
    let mut slots = [BlurSlot::default(); MAX_BLUR_PIECES];
    for (slot, s) in slots.iter_mut().zip(budget.slots()) {
        *slot = BlurSlot {
            size: [size.0 as f32, size.1 as f32, s.size[2], s.size[3]],
            ..s
        };
    }
    if slots != blur.uploaded {
        queue.write_buffer(&blur.slots, 0, bytemuck::cast_slice(&slots));
        blur.uploaded = slots;
    }
    let targets = [stage.id(first), stage.id(second)];
    if blur.with.as_ref().is_none_or(|b| b.targets != targets) {
        let (one, two) = (stage.view(first), stage.view(second));
        let (across, down) = (
            passes.group(device, &blur.slots, one),
            passes.group(device, &blur.slots, two),
        );
        let text = gpu.bind(tp, device, Some(one));
        let blur = gpu.blur.as_mut()?;
        blur.with = Some(Bound {
            targets,
            across,
            down,
            text,
        });
    }
    let blur = gpu.blur.as_ref()?;
    let bound = blur.with.as_ref()?;
    let n = n as u32;
    draw(encoder, stage.view(first), &passes.sample, &blur.group, n);
    draw(
        encoder,
        stage.view(second),
        &passes.across,
        &bound.across,
        n,
    );
    draw(encoder, stage.view(first), &passes.down, &bound.down, n);
    gpu.use_blur = true;
    Some([first, second])
}
