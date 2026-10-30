//! The caption renderer's draw pipeline: `captions.wgsl`'s `quad` and `shade` over a page's atlas,
//! instanced quads in one buffer (`Instance`), and the frame's uniform (`View`: the output's size and the line boxes).
//! Made once per drawer, off the render thread (`drawer`); a page's bind group once per atlas.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arrayvec::ArrayVec;
use bytemuck::{Pod, Zeroable};

use super::draw::MAX_LINE_BOXES;

/// Kept staging buffers for the quads: a frame writes the next one that is mapped (frames in flight hold
/// the others), copies it in its own encoder, and it is mapped again once the GPU is done with it.
const STAGING: usize = 3;

/// One kept staging buffer (the quads' buffer's size: they go when it grows): written while mapped, then copied into
/// the quads' buffer; `ready` once mapped again.
struct Staging {
    buffer: wgpu::Buffer,
    ready: Arc<AtomicBool>,
}

/// The output's format (the compositor's targets).
const TARGET: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One quad (`Inst` in the shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct Instance {
    pub(super) rect: [f32; 4],
    pub(super) uv: [f32; 4],
    pub(super) color: [f32; 4],
    pub(super) color2: [f32; 4],
    pub(super) params: [f32; 4],
    pub(super) extra: [f32; 4],
}

/// The frame's uniform (`View` in the shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct View {
    pub(super) size: [f32; 2],
    pub(super) pad: [f32; 2],
    pub(super) lines: [[f32; 4]; MAX_LINE_BOXES],
}

/// The draw pipeline, made once per drawer (off the render thread: `drawer`).
pub(super) struct DrawPipe {
    layout: wgpu::BindGroupLayout,
    pub(super) pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl DrawPipe {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("captions.wgsl"));
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Caption draw"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX | fragment,
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
            label: Some("Caption draw"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Caption draw"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("quad"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("shade"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Caption draw"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipeline,
            sampler,
        }
    }

    /// The bind group a page's atlas draws with (made once per atlas; `view` is the drawer's one uniform buffer).
    pub(super) fn bind(
        &self,
        device: &wgpu::Device,
        atlas: &wgpu::TextureView,
        view: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Caption draw"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(atlas),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: view.as_entire_binding(),
                },
            ],
        })
    }
}

/// The drawer's frame buffers, kept: the uniform, and the instance buffer, grown to the next power of two (at least 64
/// instances, 6 KB) only when a page needs more. Writing a frame allocates nothing of ours.
///
/// Each is uploaded only when it changed: every `queue.write_buffer` makes a staging buffer of its own
/// (wgpu-core 25: a new GPU buffer, mapped, filled, freed once used), which on Metal was most of the drawer's
/// render-thread time (about 300 µs a frame on Iris Plus for two writes a frame, where a title's steady frame takes
/// 15). A frame whose words didn't move uploads nothing. One whose words moved writes its quads into a kept staging
/// buffer (`STAGING` of them, mapped again after the GPU used them: `after_submit`) and copies them in the frame's own
/// encoder; only when none is mapped yet does it fall back to a queue write. The uniform (the output size and the line
/// boxes) is a queue write, only when it changes.
pub(super) struct Buffers {
    pub(super) view: wgpu::Buffer,
    instances: Option<(wgpu::Buffer, usize)>,
    /// What each buffer holds now (`held`'s room is kept).
    held: Vec<Instance>,
    held_view: Option<View>,
    staging: ArrayVec<Staging, STAGING>,
    /// The staging buffers this frame copied from, to map again once it is submitted.
    used: ArrayVec<usize, STAGING>,
    /// Uploads made, and those made through a queue write (a staging buffer of wgpu's own): the cost test and the
    /// perf harness.
    pub(super) uploads: u64,
    pub(super) queue_writes: u64,
}

impl Buffers {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            view: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Caption view"),
                size: std::mem::size_of::<View>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            instances: None,
            held: Vec::new(),
            held_view: None,
            staging: ArrayVec::new(),
            used: ArrayVec::new(),
            uploads: 0,
            queue_writes: 0,
        }
    }

    /// Uploads a frame's instances and uniform, those that changed: the instances through a kept staging buffer copied
    /// in `encoder` (the frame's, before its passes), the uniform with a queue write.
    pub(super) fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        instances: &[Instance],
        view: &View,
    ) {
        let n = instances.len();
        if self.instances.as_ref().is_none_or(|(_, room)| *room < n) {
            let room = n.next_power_of_two().max(64);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Caption instances"),
                size: (room * std::mem::size_of::<Instance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instances = Some((buffer, room));
            // A new buffer holds nothing yet, and staging buffers sized for the old one go (in flight, wgpu keeps
            // them until the GPU is done).
            self.held.clear();
            self.staging.clear();
            self.used.clear();
        }
        if let Some((buffer, room)) = &self.instances
            && self.held.as_slice() != instances
        {
            let data: &[u8] = bytemuck::cast_slice(instances);
            let bytes = data.len() as u64;
            let room = (*room * std::mem::size_of::<Instance>()) as u64;
            let free = self
                .staging
                .iter()
                .position(|s| s.ready.load(Ordering::Acquire));
            let free = match free {
                Some(i) => Some(i),
                None if !self.staging.is_full() && !self.used.is_full() => {
                    self.staging.push(Staging {
                        buffer: device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("Caption instances staging"),
                            size: room,
                            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                            mapped_at_creation: true,
                        }),
                        ready: Arc::new(AtomicBool::new(true)),
                    });
                    Some(self.staging.len() - 1)
                }
                None => None,
            };
            match free.filter(|_| bytes > 0 && !self.used.is_full()) {
                Some(i) => {
                    let staging = &self.staging[i];
                    staging
                        .buffer
                        .slice(..bytes)
                        .get_mapped_range_mut()
                        .copy_from_slice(data);
                    staging.buffer.unmap();
                    staging.ready.store(false, Ordering::Release);
                    encoder.copy_buffer_to_buffer(&staging.buffer, 0, buffer, 0, bytes);
                    self.used.push(i);
                }
                // Every staging buffer still in flight: wgpu's own staging, as before.
                None => {
                    queue.write_buffer(buffer, 0, data);
                    self.queue_writes += 1;
                }
            }
            self.held.clear();
            self.held.extend_from_slice(instances);
            self.uploads += 1;
        }
        if self.held_view != Some(*view) {
            queue.write_buffer(&self.view, 0, bytemuck::bytes_of(view));
            self.held_view = Some(*view);
            self.uploads += 1;
            self.queue_writes += 1;
        }
    }

    /// The frame that made the last uploads never reached the GPU: one that failed validation, whose
    /// whole submit wgpu drops with its copy, or one given up before its submit. What the buffers hold is no longer
    /// known, so the next frame uploads its quads and uniform again.
    pub(super) fn forget_uploads(&mut self) {
        self.held.clear();
        self.held_view = None;
    }

    /// After the frame that copied from them is submitted: its staging buffers are mapped again, ready once the GPU is
    /// done with them (the device's poll thread completes the mapping, as it does every frame's pixels).
    pub(super) fn after_submit(&mut self) {
        for i in self.used.drain(..) {
            let Some(staging) = self.staging.get(i) else {
                continue;
            };
            let ready = staging.ready.clone();
            staging
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Write, move |mapped| {
                    ready.store(mapped.is_ok(), Ordering::Release);
                });
        }
    }

    pub(super) fn instances(&self) -> Option<&wgpu::Buffer> {
        self.instances.as_ref().map(|(b, _)| b)
    }
}
