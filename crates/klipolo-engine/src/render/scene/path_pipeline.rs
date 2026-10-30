//! The scene's paths pipeline (`sdf.wgsl`, `paint.wgsl` then `paths.wgsl`): tessellated meshes drawn with
//! their trim and dashes cut by distance along the path, antialiased across the stroke and at a fill's edge (its
//! fringe). It is its own pipeline beside the shapes', so the shapes keep their 56 varyings (it uses 6); group 0 is
//! the shapes' (the target's size and the scene's gradients, one bind group for both), group 1 each instance's draws
//! (a storage buffer read by the draw's instance index). Made once per drawer, at warm-up.

use std::borrow::Cow;

use bytemuck::{Pod, Zeroable};

use crate::render::effects::{EFFECT_FORMAT, made};

/// One vertex of a path's mesh: where it is (on the centre line, for a stroke), the way it moves out (`dir` times the
/// draw's reach: a stroke's unit offset, a fill's outward normal at its edge), which side it is on (−1 or 1), and
/// along the path (a stroke's distance from its start and its sub-path's start; a fill's coverage in `along`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct PathVertex {
    pub pos: [f32; 2],
    pub dir: [f32; 2],
    pub side: f32,
    pub along: f32,
    pub sub: f32,
}

/// One draw of a path's fill or stroke (`PathDraw` in the shader): see `paths.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct PathDraw {
    pub m: [f32; 4],
    pub t: [f32; 4],
    pub uv: [f32; 4],
    pub box_hw: [f32; 4],
    pub color: [f32; 4],
    pub paint: [f32; 4],
    pub trim: [f32; 4],
    pub dash0: [f32; 4],
    pub dash1: [f32; 4],
    pub dash: [f32; 4],
    pub clip_m: [f32; 4],
    pub clip_t: [f32; 4],
    pub clip: [f32; 4],
    pub clip2_m: [f32; 4],
    pub clip2_t: [f32; 4],
    pub clip2: [f32; 4],
}

/// The shader: the distances, what shapes and paths share, then the pipeline's own code.
const SOURCE: &str = concat!(
    include_str!("sdf.wgsl"),
    include_str!("paint.wgsl"),
    include_str!("paths.wgsl")
);

pub(super) struct PathPipeline {
    draws_layout: wgpu::BindGroupLayout,
    /// Fills and opaque strokes; a translucent stroke's whole pixels, then its edge, through the stencil. `plain`:
    /// fills and opaque strokes in a pass without a stencil.
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) solid: wgpu::RenderPipeline,
    pub(super) edge: wgpu::RenderPipeline,
    pub(super) plain: wgpu::RenderPipeline,
}

/// The scene pass's stencil: every pipeline in it states one.
pub(super) const STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Stencil8;

/// A stencil test of `compare` against the draw's reference, writing it where the test passes (`replace`), no depth.
pub(super) fn stencil(compare: wgpu::CompareFunction, replace: bool) -> wgpu::DepthStencilState {
    let face = wgpu::StencilFaceState {
        compare,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: if replace {
            wgpu::StencilOperation::Replace
        } else {
            wgpu::StencilOperation::Keep
        },
    };
    wgpu::DepthStencilState {
        format: STENCIL_FORMAT,
        depth_write_enabled: false,
        depth_compare: wgpu::CompareFunction::Always,
        stencil: wgpu::StencilState {
            front: face,
            back: face,
            read_mask: 0xff,
            write_mask: 0xff,
        },
        bias: wgpu::DepthBiasState::default(),
    }
}

impl PathPipeline {
    /// The pipeline, sharing the shapes' group 0 `shared`.
    pub(super) fn new(device: &wgpu::Device, shared: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Klipolo scene paths"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SOURCE)),
        });
        let draws_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo scene path draws"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo scene paths"),
            bind_group_layouts: &[shared, &draws_layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2, 1 => Float32x2, 2 => Float32, 3 => Float32, 4 => Float32
        ];
        let make = |entry: &str, depth: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Klipolo scene paths"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_path"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<PathVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: EFFECT_FORMAT,
                        blend: Some(crate::render::compositor::blend::state(
                            klipolo_model::motion::Blend::Normal,
                        )),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // A mirrored node turns its triangles around: none are culled.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Always, NotEqual};
        let pipeline = make("fs_path", Some(stencil(Always, false)));
        let solid = make("fs_path_solid", Some(stencil(NotEqual, true)));
        let edge = make("fs_path_edge", Some(stencil(NotEqual, true)));
        let plain = make("fs_path", None);
        for _ in 0..4 {
            made();
        }
        Self {
            draws_layout,
            pipeline,
            solid,
            edge,
            plain,
        }
    }

    /// An instance's draws buffer, room for `draws` of them (two per path drawn: its fill and its stroke).
    pub(super) fn buffers(&self, device: &wgpu::Device, draws: usize) -> PathBuffers {
        let capacity = draws.max(1);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Klipolo scene path draws"),
            size: (capacity * std::mem::size_of::<PathDraw>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Klipolo scene path draws"),
            layout: &self.draws_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        made();
        made();
        PathBuffers {
            buffer,
            capacity,
            group,
            uploaded: Vec::with_capacity(capacity),
        }
    }
}

/// One instance's path draws on the GPU, written only when a frame's differ from what it holds.
pub(super) struct PathBuffers {
    buffer: wgpu::Buffer,
    capacity: usize,
    pub(super) group: wgpu::BindGroup,
    uploaded: Vec<PathDraw>,
}

impl PathBuffers {
    /// The bytes they take, GPU and the CPU copy.
    pub(super) fn bytes(&self) -> u64 {
        (2 * self.capacity * std::mem::size_of::<PathDraw>()) as u64
    }

    /// Writes `draws` (at most the capacity) unless the buffer holds them: no allocation once warm.
    pub(super) fn upload(&mut self, queue: &wgpu::Queue, draws: &[PathDraw]) -> usize {
        let n = draws.len().min(self.capacity);
        if n > 0 && self.uploaded[..] != draws[..n] {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&draws[..n]));
            self.uploaded.clear();
            self.uploaded.extend_from_slice(&draws[..n]);
        }
        n
    }
}
