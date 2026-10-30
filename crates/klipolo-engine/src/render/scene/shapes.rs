//! The scene's shapes pipeline (`sdf.wgsl` then `shapes.wgsl`): instanced SDF quads drawn into a scene's target, one
//! draw per run of shapes and one per image (its picture bound). Made once per drawer, at warm-up
//! (`SceneDrawer::warm`); each cached scene instance owns its buffers (`ShapeBuffers`), made when it is built, its
//! shapes written only when a frame's values change and its targets' slots when they move (`slots.rs`).

use std::borrow::Cow;

use bytemuck::{Pod, Zeroable};
use klipolo_model::scene::SceneLimits;

use super::pictures::PictureBinding;
use super::slots::{GLOBALS_BYTES, Slots};
use crate::render::effects::{EFFECT_FORMAT, made};

/// The most gradients one scene instance draws (the check holds scenes to it), and the stops each keeps.
pub const MAX_GRADIENTS: usize = SceneLimits::MAX_GRADIENTS;
pub const MAX_STOPS: usize = 16;

/// One shape, image or shadow: `Instance` in the shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(super) struct ShapeInstance {
    pub xf: [f32; 4],
    pub at_half: [f32; 4],
    pub shape: [f32; 4],
    pub radius: [f32; 4],
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    pub paint: [f32; 4],
    pub misc: [f32; 4],
    pub clip_m: [f32; 4],
    pub clip_t: [f32; 4],
    pub clip: [f32; 4],
    pub clip2_m: [f32; 4],
    pub clip2_t: [f32; 4],
    pub clip2: [f32; 4],
    pub extra: [f32; 4],
}

/// The shader's kinds.
pub(super) mod kind {
    pub const RECT: f32 = 0.0;
    pub const ELLIPSE: f32 = 1.0;
    pub const STAR: f32 = 2.0;
    pub const LINE: f32 = 3.0;
    pub const SHADOW: f32 = 4.0;
    pub const IMAGE: f32 = 5.0;
}

/// One gradient of the scene's table (`Gradient` in the shader): premultiplied stops.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(super) struct GpuGradient {
    pub head: [f32; 4],
    pub geom: [f32; 4],
    pub pos: [[f32; 4]; 4],
    pub color: [[f32; 4]; MAX_STOPS],
}

/// The pipeline (and its twin for passes without a stencil), its shapes' bind group layout, and how pictures
/// are bound (`pictures.rs`).
pub(super) struct ShapePipeline {
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) pictures: PictureBinding,
    pub(super) pipeline: wgpu::RenderPipeline,
    pub(super) plain: wgpu::RenderPipeline,
}

/// One scene instance's GPU side: its shapes (room for every node twice, a shadow and the shape), its targets' slots
/// (`slots.rs`: each target's size and place, read at a dynamic offset) and its gradient table, and their
/// bind group.
pub(super) struct ShapeBuffers {
    pub(super) instances: wgpu::Buffer,
    capacity: usize,
    pub(super) slots: Slots,
    pub(super) group: wgpu::BindGroup,
    /// What `instances` holds: a frame whose shapes didn't change writes nothing.
    uploaded: Vec<ShapeInstance>,
}

/// The shader: the distances, what shapes and paths share, then the pipeline's own code.
const SOURCE: &str = concat!(
    include_str!("sdf.wgsl"),
    include_str!("paint.wgsl"),
    include_str!("shapes.wgsl")
);

impl ShapePipeline {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Klipolo scene shapes"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SOURCE)),
        });
        let uniform = |binding, visibility, dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: if dynamic {
                    std::num::NonZeroU64::new(GLOBALS_BYTES)
                } else {
                    None
                },
            },
            count: None,
        };
        let fragment = wgpu::ShaderStages::FRAGMENT;
        // The target's slot: where it sits in the scene's (the vertices and the clips read it).
        let both = wgpu::ShaderStages::VERTEX | fragment;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo scene shapes"),
            entries: &[uniform(0, both, true), uniform(1, fragment, false)],
        });
        let pictures = PictureBinding::new(device);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Klipolo scene shapes"),
            bind_group_layouts: &[&layout, &pictures.layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4,
            6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32x4,
            12 => Float32x4, 13 => Float32x4, 14 => Float32x4
        ];
        let make = |depth_stencil| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Klipolo scene shapes"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ShapeInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: EFFECT_FORMAT,
                        blend: Some(crate::render::compositor::blend::state(
                            klipolo_model::motion::Blend::Normal,
                        )),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // A mirrored node (a negative scale) turns its triangles around: none are culled.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        // A pass drawing a translucent stroke has a stencil, which shapes neither test nor write; any
        // other pass has none: a twin without it.
        let pipeline = make(Some(super::path_pipeline::stencil(
            wgpu::CompareFunction::Always,
            false,
        )));
        let plain = make(None);
        made();
        made();
        Self {
            layout,
            pictures,
            pipeline,
            plain,
        }
    }

    /// One instance's buffers for `nodes` nodes, with its gradient table: made when the instance is built (its slots
    /// are written by each frame drawn: `Slots::write_frame`).
    pub(super) fn buffers(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        nodes: usize,
        gradients: &[GpuGradient],
    ) -> ShapeBuffers {
        let capacity = (2 * nodes).max(1);
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Klipolo scene shapes"),
            size: (capacity * std::mem::size_of::<ShapeInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform = |label, bytes: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let slots = Slots::new(device);
        let table = uniform(
            "Klipolo scene gradients",
            (MAX_GRADIENTS * std::mem::size_of::<GpuGradient>()) as u64,
        );
        if !gradients.is_empty() {
            let n = gradients.len().min(MAX_GRADIENTS);
            queue.write_buffer(&table, 0, bytemuck::cast_slice(&gradients[..n]));
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Klipolo scene shapes"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: slots.binding(GLOBALS_BYTES),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: table.as_entire_binding(),
                },
            ],
        });
        for _ in 0..3 {
            made();
        }
        ShapeBuffers {
            instances,
            capacity,
            slots,
            group,
            uploaded: Vec::with_capacity(capacity),
        }
    }
}

impl ShapeBuffers {
    /// The GPU bytes these buffers hold.
    pub(super) fn bytes(&self) -> u64 {
        (self.capacity * std::mem::size_of::<ShapeInstance>()
            + MAX_GRADIENTS * std::mem::size_of::<GpuGradient>()) as u64
            + Slots::bytes()
    }

    /// The CPU bytes they keep (the uploaded copy).
    pub(super) fn cpu_bytes(&self) -> u64 {
        (self.uploaded.capacity() * std::mem::size_of::<ShapeInstance>()) as u64
    }

    /// Writes `shapes` (at most the capacity), unless the buffer already holds them: no allocation once warm.
    pub(super) fn upload(&mut self, queue: &wgpu::Queue, shapes: &[ShapeInstance]) -> usize {
        let n = shapes.len().min(self.capacity);
        if self.uploaded[..] != shapes[..n] {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&shapes[..n]));
            self.uploaded.clear();
            self.uploaded.extend_from_slice(&shapes[..n]);
        }
        n
    }
}

#[cfg(test)]
#[path = "shapes_tests.rs"]
mod tests;
