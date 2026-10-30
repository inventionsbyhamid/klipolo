//! Overlay video frames converted from NV12 planes to RGBA at about the size they're drawn: one render pass per new
//! frame that averages each target pixel's whole footprint (bilinear taps spread over it), so shrunk b-roll doesn't
//! shimmer and the image quad then samples it nearly 1:1.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use crate::render::nv12::PlaneImporter;
use crate::vendor::cap::rendering::DecodedFrame;
use crate::vendor::cap::rendering::decoder::DecodedFrameStorageIdentity;

const SCALER: &str = r"
struct Params {
    // One target pixel's footprint in source uv, and the taps per axis that cover it.
    footprint: vec2<f32>,
    taps: vec2<f32>,
};

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var uv_plane: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

// BT.709 video range, as nv12_to_rgba.wgsl.
fn rgb(y_raw: f32, uv_raw: vec2<f32>) -> vec3<f32> {
    let y = (y_raw - 16.0 / 255.0) * (255.0 / 219.0);
    let u = uv_raw.x - 128.0 / 255.0;
    let v = uv_raw.y - 128.0 / 255.0;
    return clamp(vec3<f32>(y + 1.792741 * v, y - 0.213249 * u - 0.532909 * v, y + 2.112402 * u),
        vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let n = vec2<i32>(params.taps);
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < n.y; j++) {
        for (var i = 0; i < n.x; i++) {
            let offset = ((vec2<f32>(f32(i), f32(j)) + 0.5) / params.taps - 0.5) * params.footprint;
            let at = in.uv + offset;
            sum += rgb(textureSampleLevel(y_plane, linear, at, 0.0).r,
                textureSampleLevel(uv_plane, linear, at, 0.0).rg);
        }
    }
    return vec4<f32>(sum / f32(n.x * n.y), 1.0);
}
";

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct Params {
    footprint: [f32; 2],
    taps: [f32; 2],
}

/// The NV12 → RGBA box-filter pass overlay videos share.
pub struct Nv12Scaler {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Nv12Scaler {
    pub fn new(device: &wgpu::Device) -> Arc<Self> {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Klipolo NV12 scaler"),
            source: wgpu::ShaderSource::Wgsl(SCALER.into()),
        });
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo NV12 scaler"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo NV12 scaler"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Klipolo NV12 scaler"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Klipolo NV12 scaler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Arc::new(Self {
            pipeline,
            layout,
            sampler,
        })
    }
}

/// Target sizes are rounded up to this step (a resize drag doesn't make a texture per pixel).
const STEP: u32 = 64;
/// Taps per axis at most (a 16× shrink is still averaged over its whole footprint, two texels per bilinear tap).
const MAX_TAPS: f32 = 8.0;

/// One overlay video's frames as RGBA at about the size they're drawn (in the frame's stored orientation).
#[derive(Default)]
pub struct ScaledFrame {
    target: Option<(wgpu::Texture, wgpu::TextureView)>,
    params: Option<(wgpu::Buffer, Params)>,
    last: Option<DecodedFrameStorageIdentity>,
    /// The conversion `encode` records this frame.
    pending: Option<wgpu::BindGroup>,
}

impl ScaledFrame {
    /// The RGBA texture showing `frame` drawn `drawn` pixels large (stored orientation), converting it when it is a
    /// new frame or the size changed. None when the frame isn't an NV12 IOSurface (the caller converts it instead).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scaler: &Nv12Scaler,
        importer: &PlaneImporter,
        frame: &DecodedFrame,
        drawn: (f64, f64),
    ) -> Option<wgpu::Texture> {
        let (fw, fh) = (frame.width().max(1), frame.height().max(1));
        // Within the frame's size before rounding up, so a huge `drawn` (a layer scaled far past the frame) can't
        // overflow; the cast saturates and a NaN reads as 1.
        let bucket = |want: f64, max: u32| {
            (want.max(1.0).ceil() as u32)
                .min(max)
                .div_ceil(STEP)
                .saturating_mul(STEP)
                .clamp(1, max)
        };
        let size = (bucket(drawn.0, fw), bucket(drawn.1, fh));
        let storage = frame.storage_identity();
        let resized = self
            .target
            .as_ref()
            .is_none_or(|(t, _)| (t.width(), t.height()) != size);
        let same = !resized && self.last.as_ref().is_some_and(|l| l.matches(&storage));
        if !same {
            let planes = importer.import(device, frame)?;
            if resized {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Overlay video frame"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.target = Some((texture, view));
            }
            let scale = (
                planes.size.0 as f32 / size.0 as f32,
                planes.size.1 as f32 / size.1 as f32,
            );
            let params = Params {
                footprint: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
                taps: [
                    (scale.0 / 2.0).ceil().clamp(1.0, MAX_TAPS),
                    (scale.1 / 2.0).ceil().clamp(1.0, MAX_TAPS),
                ],
            };
            let buffer = match &mut self.params {
                Some((buffer, last)) => {
                    if *last != params {
                        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&params));
                        *last = params;
                    }
                    buffer
                }
                None => {
                    use wgpu::util::DeviceExt;
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Overlay video scale"),
                        contents: bytemuck::bytes_of(&params),
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    });
                    &self.params.insert((buffer, params)).0
                }
            };
            self.pending = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Overlay video scale"),
                layout: &scaler.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&planes.y),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&planes.uv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&scaler.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            }));
            self.last = Some(storage);
        }
        self.target.as_ref().map(|(t, _)| t.clone())
    }

    /// Records this frame's conversion (if `prepare` made one) into `encoder`, before the pass that draws it.
    pub fn encode(&mut self, scaler: &Nv12Scaler, encoder: &mut wgpu::CommandEncoder) {
        let (Some(group), Some((_, view))) = (self.pending.take(), &self.target) else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Overlay video scale"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&scaler.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
