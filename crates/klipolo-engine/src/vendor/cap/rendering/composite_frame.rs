// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/composite_frame.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted (XY, ColorCorrection from vendor::cap::compat); the on-disk pipeline cache (PIPELINE_CACHE_DATA, get_cache_path, load/save_pipeline_cache under Cap's data dir) removed, so the pipeline is built with `cache: None`. `source_rotation` takes the first of the three `_padding1` floats (layout and size unchanged; matches the WGSL). A `@group(1)` LUT bind group layout (3D texture, sampler, `lut_params` uniform) is added to the pipeline layout as `lut_bind_group_layout`. Klipolo also builds `nv12_pipeline` with `nv12_bind_group_layout`/`nv12_bind_group`: the same WGSL with its frame reads rewritten at load time to sample NV12 planes and convert BT.709 video range per sample (`nv12_shader_source`), so VideoToolbox frames are composited without an RGBA copy; pipeline creation is shared through `render_pipeline`. The RGBA pipeline is made on first use (`rgba_pipeline`; VideoToolbox frames take `nv12_pipeline`), so a first frame never waits for its compile. `rotation_deg` takes the second padding float (layout and size unchanged): the layer turns clockwise about its target's centre (the camera bubble's rotation and keys).

use crate::vendor::cap::compat::XY;
use bytemuck::{Pod, Zeroable};
use wgpu::{include_wgsl, util::DeviceExt};

pub struct CompositeVideoFramePipeline {
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Klipolo: `@group(1)`: the clip's `.cube` LUT (3D texture + sampler) and `lut_params` (enabled, strength, size).
    pub lut_bind_group_layout: wgpu::BindGroupLayout,
    /// Klipolo: made by `rgba_pipeline` the first time a frame without an IOSurface needs it (an FFmpeg decode,
    /// overlay video's converter path); about half a second of shader compile on a cold Intel Mac.
    render_pipeline: std::sync::OnceLock<wgpu::RenderPipeline>,
    /// Klipolo: the same shader reading an NV12 frame's planes (`@binding(1)` Y, `@binding(3)` UV) and converting
    /// BT.709 video range per sample, so VideoToolbox frames need no RGBA copy.
    pub nv12_bind_group_layout: wgpu::BindGroupLayout,
    pub nv12_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

/// Klipolo: the composite shader with its frame reads going through NV12 planes.
fn nv12_shader_source() -> String {
    let source = include_str!("shaders/composite-video-frame.wgsl")
        .replace(
            "@group(0) @binding(1) var frame_texture: texture_2d<f32>;",
            "@group(0) @binding(1) var frame_y: texture_2d<f32>;\n@group(0) @binding(3) var frame_uv: texture_2d<f32>;",
        )
        .replace("textureSampleLevel(frame_texture, frame_sampler, ", "sample_frame_level(")
        .replace("textureSample(frame_texture, frame_sampler, ", "sample_frame(");
    debug_assert!(
        !source.contains("frame_texture"),
        "every frame read goes through the planes"
    );
    source
        + r"
// Klipolo: NV12 (video range) to RGB with BT.709, as nv12_to_rgba.wgsl, per sample.
fn nv12_rgba(y_raw: f32, uv_raw: vec2<f32>) -> vec4<f32> {
    let y = (y_raw - 16.0 / 255.0) * (255.0 / 219.0);
    let u = uv_raw.x - 128.0 / 255.0;
    let v = uv_raw.y - 128.0 / 255.0;
    let rgb = vec3<f32>(y + 1.792741 * v, y - 0.213249 * u - 0.532909 * v, y + 2.112402 * u);
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

fn sample_frame(uv: vec2<f32>) -> vec4<f32> {
    return nv12_rgba(textureSample(frame_y, frame_sampler, uv).r, textureSample(frame_uv, frame_sampler, uv).rg);
}

fn sample_frame_level(uv: vec2<f32>, level: f32) -> vec4<f32> {
    return nv12_rgba(
        textureSampleLevel(frame_y, frame_sampler, uv, level).r,
        textureSampleLevel(frame_uv, frame_sampler, uv, level).rg,
    );
}
"
}

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct CompositeVideoFrameUniforms {
    pub crop_bounds: [f32; 4],
    pub target_bounds: [f32; 4],
    pub output_size: [f32; 2],
    pub frame_size: [f32; 2],
    pub motion_blur_vector: [f32; 2],
    pub motion_blur_zoom_center: [f32; 2],
    pub motion_blur_params: [f32; 4],
    pub target_size: [f32; 2],
    pub rounding_px: f32,
    pub rounding_type: f32,
    pub mirror_x: f32,
    pub shadow: f32,
    pub shadow_size: f32,
    pub shadow_opacity: f32,
    pub shadow_blur: f32,
    pub opacity: f32,
    pub border_enabled: f32,
    pub border_width: f32,
    pub preserve_source_alpha: f32,
    /// Clockwise degrees (0, 90, 180, 270) that turn the stored frame upright (phone video); `crop_bounds` and
    /// `frame_size` are then in upright orientation. Was the first of Cap's three padding floats.
    pub source_rotation: f32,
    /// Clockwise degrees the layer turns about the centre of `target_bounds` (0 = upright). Was Cap's second padding
    /// float.
    pub rotation_deg: f32,
    pub _padding1: f32,
    pub border_color: [f32; 4],
    /// Per-corner multipliers on `rounding_px` (top-left, top-right,
    /// bottom-left, bottom-right). All 1s = uniform rounding; the display
    /// squares its top corners against decorative frame chrome with
    /// `[0, 0, 1, 1]`.
    pub corner_radii: [f32; 4],
    /// (exposure stops, contrast, saturation, temperature).
    pub color_adjust_a: [f32; 4],
    /// (tint, fade, split_tone, vignette).
    pub color_adjust_b: [f32; 4],
    /// (grain amount, grain seed, grade-active flag, full-frame vignette
    /// flag). The active flag gates the shader's whole color pass in one
    /// uniform branch, keeping ungraded layers bit-identical to before.
    pub grain_params: [f32; 4],
}

impl Default for CompositeVideoFrameUniforms {
    fn default() -> Self {
        Self {
            crop_bounds: Default::default(),
            target_bounds: Default::default(),
            output_size: Default::default(),
            frame_size: Default::default(),
            motion_blur_vector: Default::default(),
            motion_blur_zoom_center: [0.5, 0.5],
            motion_blur_params: Default::default(),
            target_size: Default::default(),
            rounding_px: Default::default(),
            rounding_type: 0.0,
            mirror_x: Default::default(),
            shadow: Default::default(),
            shadow_size: Default::default(),
            shadow_opacity: Default::default(),
            shadow_blur: Default::default(),
            opacity: 1.0,
            border_enabled: 0.0,
            border_width: 5.0,
            preserve_source_alpha: 0.0,
            source_rotation: 0.0,
            rotation_deg: 0.0,
            _padding1: 0.0,
            border_color: [0.0, 0.0, 0.0, 0.0],
            corner_radii: [1.0; 4],
            color_adjust_a: [0.0; 4],
            color_adjust_b: [0.0; 4],
            grain_params: [0.0; 4],
        }
    }
}

/// Intensity scaling and range clamping happen here, Rust-side, so the
/// shader never sees unscaled values and a hand-edited config can't push it
/// outside its designed ranges.
#[derive(Debug, Clone, Copy)]
pub struct ColorGradeUniformParams {
    pub color_adjust_a: [f32; 4],
    pub color_adjust_b: [f32; 4],
    pub grain_params: [f32; 4],
}

impl ColorGradeUniformParams {
    pub const IDENTITY: Self = Self {
        color_adjust_a: [0.0; 4],
        color_adjust_b: [0.0; 4],
        grain_params: [0.0; 4],
    };

    /// Must mirror the shader's own gate on `grain_params.z`.
    pub fn is_active(&self) -> bool {
        self.grain_params[2] > 0.5
    }

    /// `full_frame_vignette` selects the vignette's coordinate space: full
    /// output frame for the screen (one continuous field across card and
    /// backdrop), card-local for the camera.
    pub fn from_config(
        config: &crate::vendor::cap::compat::ColorCorrection,
        frame_number: u32,
        full_frame_vignette: bool,
    ) -> Self {
        let intensity = config.intensity.clamp(0.0, 1.0);
        let exposure_stops = config.exposure.clamp(-1.0, 1.0) * 1.5 * intensity;
        let contrast = config.contrast.clamp(-1.0, 1.0) * intensity;
        let saturation = config.saturation.clamp(-1.0, 1.0) * intensity;
        let temperature = config.temperature.clamp(-1.0, 1.0) * intensity;
        let tint = config.tint.clamp(-1.0, 1.0) * intensity;
        let fade = config.fade.clamp(0.0, 1.0) * intensity;
        let split_tone = config.split_tone.clamp(-1.0, 1.0) * intensity;
        let vignette = config.vignette.clamp(0.0, 1.0) * intensity;
        let grain = config.grain.clamp(0.0, 1.0);

        let active = [
            exposure_stops,
            contrast,
            saturation,
            temperature,
            tint,
            fade,
            split_tone,
            vignette,
            grain,
        ]
        .iter()
        .any(|v| v.abs() > 1e-4);

        // Seed is deterministic per frame number so preview and export match.
        let grain_seed = (frame_number % 600) as f32;

        Self {
            color_adjust_a: [exposure_stops, contrast, saturation, temperature],
            color_adjust_b: [tint, fade, split_tone, vignette],
            grain_params: [
                grain,
                grain_seed,
                if active { 1.0 } else { 0.0 },
                if full_frame_vignette { 1.0 } else { 0.0 },
            ],
        }
    }
}

impl CompositeVideoFrameUniforms {
    pub(crate) fn for_source_frame(mut self, source_size: XY<u32>) -> Self {
        let scale_x = source_size.x as f32 / self.frame_size[0].max(1.0);
        let scale_y = source_size.y as f32 / self.frame_size[1].max(1.0);

        self.crop_bounds = [
            self.crop_bounds[0] * scale_x,
            self.crop_bounds[1] * scale_y,
            self.crop_bounds[2] * scale_x,
            self.crop_bounds[3] * scale_y,
        ];
        self.frame_size = [source_size.x as f32, source_size.y as f32];

        let crop_w = self.crop_bounds[2] - self.crop_bounds[0];
        let crop_h = self.crop_bounds[3] - self.crop_bounds[1];
        let target_w = self.target_bounds[2] - self.target_bounds[0];
        let target_h = self.target_bounds[3] - self.target_bounds[1];

        if crop_w > 0.0 && crop_h > 0.0 && target_w > 0.0 && target_h > 0.0 {
            let source_aspect = crop_w / crop_h;
            let target_aspect = target_w / target_h;

            if (source_aspect - target_aspect).abs() > 0.001 {
                let scale = (target_w / crop_w).min(target_h / crop_h);
                let fitted_w = crop_w * scale;
                let fitted_h = crop_h * scale;
                let new_x0 = self.target_bounds[0] + (target_w - fitted_w) * 0.5;
                let new_y0 = self.target_bounds[1] + (target_h - fitted_h) * 0.5;

                self.target_bounds = [new_x0, new_y0, new_x0 + fitted_w, new_y0 + fitted_h];
                self.target_size = [fitted_w, fitted_h];
            }
        }

        self
    }

    pub(crate) fn source_uv_to_target(&self, position: XY<f64>) -> XY<f64> {
        let crop_size = XY::new(
            (self.crop_bounds[2] - self.crop_bounds[0]).max(f32::EPSILON) as f64,
            (self.crop_bounds[3] - self.crop_bounds[1]).max(f32::EPSILON) as f64,
        );
        let source_position =
            position * XY::new(self.frame_size[0] as f64, self.frame_size[1] as f64);
        let mut position_in_crop = (source_position
            - XY::new(self.crop_bounds[0] as f64, self.crop_bounds[1] as f64))
            / crop_size;
        if self.mirror_x != 0.0 {
            position_in_crop.x = 1.0 - position_in_crop.x;
        }

        XY::new(self.target_bounds[0] as f64, self.target_bounds[1] as f64)
            + position_in_crop * XY::new(self.target_size[0] as f64, self.target_size[1] as f64)
    }

    pub fn to_buffer(self, device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_init(
            &(wgpu::util::BufferInitDescriptor {
                label: Some("CompositeVideoFrameUniforms Buffer"),
                contents: bytemuck::cast_slice(&[self]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
        )
    }

    pub fn write_to_buffer(&self, queue: &wgpu::Queue, buffer: &wgpu::Buffer) {
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(self));
    }
}

// pub struct CompositeFrameResources {
//     pub bind_group: wgpu::BindGroup,
//     pub bind_group_layout: wgpu::BindGroupLayout,
//     pub uniforms_buffer: wgpu::Buffer,
//     pub sampler: wgpu::Sampler,
// }

impl CompositeVideoFramePipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = Self::bind_group_layout(device, false);
        let lut_bind_group_layout = Self::lut_bind_group_layout(device);
        let nv12_bind_group_layout = Self::bind_group_layout(device, true);
        let nv12_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite-video-frame.wgsl (NV12)"),
            source: wgpu::ShaderSource::Wgsl(nv12_shader_source().into()),
        });
        let nv12_pipeline = Self::render_pipeline(
            device,
            &nv12_shader,
            &nv12_bind_group_layout,
            &lut_bind_group_layout,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            bind_group_layout,
            lut_bind_group_layout,
            render_pipeline: std::sync::OnceLock::new(),
            nv12_bind_group_layout,
            nv12_pipeline,
            sampler,
        }
    }

    /// Klipolo: the pipeline drawing an RGBA frame texture, made on first use.
    pub fn rgba_pipeline(&self, device: &wgpu::Device) -> &wgpu::RenderPipeline {
        self.render_pipeline.get_or_init(|| {
            let shader =
                device.create_shader_module(include_wgsl!("shaders/composite-video-frame.wgsl"));
            Self::render_pipeline(
                device,
                &shader,
                &self.bind_group_layout,
                &self.lut_bind_group_layout,
            )
        })
    }

    /// Klipolo: the RGBA pipeline, if `rgba_pipeline` made it.
    pub fn made_rgba_pipeline(&self) -> Option<&wgpu::RenderPipeline> {
        self.render_pipeline.get()
    }

    fn render_pipeline(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        bind_group_layout: &wgpu::BindGroupLayout,
        lut_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> wgpu::RenderPipeline {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Composite Render Pipeline Layout"),
            bind_group_layouts: &[bind_group_layout, lut_bind_group_layout],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Composite Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[],
                    zero_initialize_workgroup_memory: false,
                },
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[],
                    zero_initialize_workgroup_memory: false,
                },
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    }

    fn lut_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite-video-frame.wgsl LUT Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        })
    }

    fn bind_group_layout(device: &wgpu::Device, nv12: bool) -> wgpu::BindGroupLayout {
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
        let entries = [
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            texture(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // Klipolo: the UV plane (NV12 layout only).
            texture(3),
        ];
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite-video-frame.wgsl Bind Group Layout"),
            entries: &entries[..if nv12 { 4 } else { 3 }],
        })
    }

    /// Klipolo: a frame given as NV12 planes (Y `R8`, UV `RG8`) for `nv12_pipeline`.
    pub fn nv12_bind_group(
        &self,
        device: &wgpu::Device,
        uniforms: &wgpu::Buffer,
        y: &wgpu::TextureView,
        uv: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &self.nv12_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(y),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(uv),
                },
            ],
            label: Some("nv12 bind_group"),
        })
    }

    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        uniforms: &wgpu::Buffer,
        frame: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(frame),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
            label: Some("bind_group"),
        })
    }

    pub fn create_frame_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(
            &(wgpu::TextureDescriptor {
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_DST,
                label: Some("Frame Composite texture"),
                view_formats: &[],
            }),
        )
    }
}
