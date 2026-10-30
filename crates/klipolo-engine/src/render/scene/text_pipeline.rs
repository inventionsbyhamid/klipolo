//! The scene's text pipeline (`sdf.wgsl`, `paint.wgsl` then `text.wgsl`): instanced quads for the pieces
//! of text and the span backgrounds, read from a storage buffer by instance index (few varyings, as the paths'). Group 0
//! is the shapes' (the target's size and the scene's gradients), group 1 an instance's text: its draws and their paints
//! (what each node's pieces of one look draw with, once a frame), its atlas's masks, colour and baked
//! shadows, and a sampler. Made once per drawer, at warm-up; an instance's atlas and buffers when its text is laid
//! out (`text_gpu.rs`), never per frame.

use std::borrow::Cow;

use super::text_blur_pass::BlurPass;
use crate::render::effects::{EFFECT_FORMAT, made};

pub(super) use super::text_gpu::{TextDraw, TextGpu, TextPaintDraw};

/// The shader: the distances, what shapes, paths and text share, then the pipeline's own code.
const SOURCE: &str = concat!(
    include_str!("sdf.wgsl"),
    include_str!("paint.wgsl"),
    include_str!("text.wgsl")
);

pub(super) struct TextPipeline {
    layout: wgpu::BindGroupLayout,
    /// For a pass with a stencil, and its twin for one without.
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) plain: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// The colour, shadow or blur an instance's text doesn't have: one transparent texel.
    none: wgpu::TextureView,
    /// Its blurring pieces' passes (drawn small).
    pub(super) across: BlurPass,
}

/// What an instance text's bind group binds: its draws and paints, its atlas's masks, colours and shadows (None: a
/// transparent texel), and the frame's blur read back (None: a transparent texel).
pub(super) struct Bound<'a> {
    pub draws: &'a wgpu::Buffer,
    pub paints: &'a wgpu::Buffer,
    pub masks: &'a wgpu::TextureView,
    pub colors: Option<&'a wgpu::TextureView>,
    pub shadows: Option<&'a wgpu::TextureView>,
    pub blurred: Option<&'a wgpu::TextureView>,
}

impl TextPipeline {
    /// The pipeline, sharing the shapes' group 0 `shared`.
    pub(super) fn new(device: &wgpu::Device, shared: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Klipolo scene text"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SOURCE)),
        });
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let image = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: both,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo scene text"),
            entries: &[
                storage(0),
                image(1),
                image(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                image(4),
                storage(5),
                image(6),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo scene text"),
            bind_group_layouts: &[shared, &layout],
            push_constant_ranges: &[],
        });
        let make = |depth_stencil| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Klipolo scene text"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_text"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_text"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: EFFECT_FORMAT,
                        blend: Some(crate::render::compositor::blend::state(
                            klipolo_model::motion::Blend::Normal,
                        )),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // A mirrored unit turns its triangles around: none are culled.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        // A pass drawing a translucent stroke has a stencil, which text neither tests nor writes; any other pass has
        // none: a twin without it.
        let pipeline = make(Some(super::path_pipeline::stencil(
            wgpu::CompareFunction::Always,
            false,
        )));
        let plain = make(None);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Klipolo scene text atlas"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // A texture starts zeroed (wgpu clears it before its first use): one transparent texel.
        let none = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Klipolo scene text none"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        for _ in 0..5 {
            made();
        }
        Self {
            layout,
            pipeline,
            plain,
            sampler,
            none: none.create_view(&wgpu::TextureViewDescriptor::default()),
            across: BlurPass::new(device),
        }
    }

    /// An instance text's bind group, binding `b`.
    pub(super) fn group(&self, device: &wgpu::Device, b: Bound) -> wgpu::BindGroup {
        made();
        fn view(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(v),
            }
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Klipolo scene text"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: b.draws.as_entire_binding(),
                },
                view(1, b.masks),
                view(2, b.colors.unwrap_or(&self.none)),
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                view(4, b.blurred.unwrap_or(&self.none)),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: b.paints.as_entire_binding(),
                },
                view(6, b.shadows.unwrap_or(&self.none)),
            ],
        })
    }
}
