//! The caption atlas's pipelines: `captions.wgsl`'s `pack` and `blur_pass`, and for half-size blurs
//! `halve` and `combine` (one per set of masks it writes), their bind group layouts and samplers, made
//! once with the atlas builder.

use wgpu::util::DeviceExt;

use super::blur::Uniform;

/// The atlas's format: four 8-bit masks.
pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub(super) struct Pipes {
    sources: wgpu::BindGroupLayout,
    params: wgpu::BindGroupLayout,
    pack: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    halve: wgpu::RenderPipeline,
    /// `combine` writing the shadow's mask (B), the glow's (A), or both.
    combine: [wgpu::RenderPipeline; 3],
    /// Texel for texel (the pack, and a blur's own texel); linear for a blur's spread-out taps (`captions.wgsl`).
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
}

impl Pipes {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("captions.wgsl"));
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sources = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Caption atlas sources"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let params = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Caption atlas blur"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline = |entry: &str, groups: &[&wgpu::BindGroupLayout], write_mask| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Caption atlas"),
                bind_group_layouts: groups,
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: None,
                        write_mask,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let all = wgpu::ColorWrites::ALL;
        Self {
            pack: pipeline("pack", &[&sources, &params], all),
            blur: pipeline("blur_pass", &[&sources, &params], all),
            halve: pipeline("halve", &[&sources], all),
            combine: [
                wgpu::ColorWrites::BLUE,
                wgpu::ColorWrites::ALPHA,
                wgpu::ColorWrites::BLUE | wgpu::ColorWrites::ALPHA,
            ]
            .map(|mask| pipeline("combine", &[&sources], mask)),
            sources,
            params,
            nearest: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Caption atlas"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            }),
            linear: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Caption atlas blur"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    /// The textures a pass reads: `a` (`src_a`) and `b` (`src_b`), through the linear sampler for a blur.
    pub(super) fn sources(
        &self,
        device: &wgpu::Device,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
        blur: bool,
    ) -> wgpu::BindGroup {
        let sampler = if blur { &self.linear } else { &self.nearest };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Caption atlas sources"),
            layout: &self.sources,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// A pass's `Blur` uniform (`blur::Uniform`: the pack pass's, or a blur pass's taps).
    pub(super) fn blur_params(&self, device: &wgpu::Device, values: &Uniform) -> wgpu::BindGroup {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Caption atlas blur"),
            contents: bytemuck::bytes_of(values),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Caption atlas blur"),
            layout: &self.params,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }

    /// The pack pass into `into`, reading `sources` with `params` (`Uniform::pack`).
    pub(super) fn pack(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        into: &wgpu::TextureView,
        sources: &wgpu::BindGroup,
        params: &wgpu::BindGroup,
    ) {
        let mut p = pass(encoder, into);
        p.set_pipeline(&self.pack);
        p.set_bind_group(0, sources, &[]);
        p.set_bind_group(1, params, &[]);
        p.draw(0..3, 0..1);
    }

    /// The half-size copy of `sources`' first texture into `into` (`halve`).
    pub(super) fn halve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        into: &wgpu::TextureView,
        sources: &wgpu::BindGroup,
    ) {
        let mut p = pass(encoder, into);
        p.set_pipeline(&self.halve);
        p.set_bind_group(0, sources, &[]);
        p.draw(0..3, 0..1);
    }

    /// The half-size blurs of `sources` upscaled into the atlas `into`, writing the shadow's mask, the glow's, or both;
    /// the atlas's other masks are kept.
    pub(super) fn combine(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        into: &wgpu::TextureView,
        sources: &wgpu::BindGroup,
        shadow: bool,
        glow: bool,
    ) {
        let pipeline = match (shadow, glow) {
            (true, false) => &self.combine[0],
            (false, true) => &self.combine[1],
            (true, true) => &self.combine[2],
            (false, false) => return,
        };
        let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Caption atlas"),
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
        p.set_pipeline(pipeline);
        p.set_bind_group(0, sources, &[]);
        p.draw(0..3, 0..1);
    }

    /// One blur pass into `into`, reading `sources` with `params`.
    pub(super) fn blur(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        into: &wgpu::TextureView,
        sources: &wgpu::BindGroup,
        params: &wgpu::BindGroup,
    ) {
        let mut p = pass(encoder, into);
        p.set_pipeline(&self.blur);
        p.set_bind_group(0, sources, &[]);
        p.set_bind_group(1, params, &[]);
        p.draw(0..3, 0..1);
    }
}

/// A render pass into `view`, cleared to transparent.
pub(super) fn pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Caption atlas"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    })
}
