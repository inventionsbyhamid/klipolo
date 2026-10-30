// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/yuv_converter.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted; CPU conversions removed (convert_nv12_cpu, convert_yuv420p_cpu, crate::cpu_yuv); Windows D3D11 interop (imports, error variants, staging fields, convert_nv12_from_d3d11_*, convert_nv12_with_fallback, zero-copy state) and Linux CUDA code (GpuInterop error, cuda_staging, convert_nv12_cuda*) removed; the IOSurface texture cache is made on wgpu's own Metal device; the IOSurface path imports the UV plane at half the frame rounded up (odd sizes).

#[cfg(target_os = "macos")]
use crate::vendor::cap::rendering::iosurface_texture::{
    IOSurfaceTextureCache, IOSurfaceTextureError, import_metal_texture_to_wgpu,
};

#[cfg(target_os = "macos")]
use cidre::cv;

#[derive(Debug, thiserror::Error)]
pub enum YuvConversionError {
    #[error("{plane} plane size mismatch: expected {expected}, got {actual}")]
    PlaneSizeMismatch {
        plane: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{plane} plane stride too small: expected at least {expected}, got {actual}")]
    PlaneStrideTooSmall {
        plane: &'static str,
        expected: u32,
        actual: u32,
    },
    #[error("{dimension} dimension ({value}) exceeds maximum allowed ({max})")]
    DimensionExceedsLimit {
        dimension: &'static str,
        value: u32,
        max: u32,
    },
    #[cfg(target_os = "macos")]
    #[error("IOSurface error: {0}")]
    IOSurfaceError(#[from] IOSurfaceTextureError),
}

fn upload_plane_with_stride(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    plane_name: &'static str,
) -> Result<(), YuvConversionError> {
    if stride < width {
        return Err(YuvConversionError::PlaneStrideTooSmall {
            plane: plane_name,
            expected: width,
            actual: stride,
        });
    }

    let expected_data_size = (stride * height) as usize;
    if data.len() < expected_data_size {
        return Err(YuvConversionError::PlaneSizeMismatch {
            plane: plane_name,
            expected: expected_data_size,
            actual: data.len(),
        });
    }

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

const MAX_TEXTURE_WIDTH: u32 = 7680;
const MAX_TEXTURE_HEIGHT: u32 = 4320;

const INITIAL_TEXTURE_WIDTH: u32 = 1920;
const INITIAL_TEXTURE_HEIGHT: u32 = 1080;

const TEXTURE_SIZE_PADDING: u32 = 64;

fn align_dimension(dim: u32) -> u32 {
    dim.div_ceil(TEXTURE_SIZE_PADDING) * TEXTURE_SIZE_PADDING
}

fn validate_dimensions(
    width: u32,
    height: u32,
    gpu_max_texture_size: u32,
) -> Result<(u32, u32, bool), YuvConversionError> {
    let effective_max_width = MAX_TEXTURE_WIDTH.min(gpu_max_texture_size);
    let effective_max_height = MAX_TEXTURE_HEIGHT.min(gpu_max_texture_size);

    if width <= effective_max_width && height <= effective_max_height {
        return Ok((width, height, false));
    }

    let scale_x = effective_max_width as f32 / width as f32;
    let scale_y = effective_max_height as f32 / height as f32;
    let scale = scale_x.min(scale_y).min(1.0);

    if scale < 0.1 {
        return Err(YuvConversionError::DimensionExceedsLimit {
            dimension: "resolution",
            value: width.max(height),
            max: effective_max_width.max(effective_max_height),
        });
    }

    let new_width = ((width as f32 * scale) as u32).max(2) & !1;
    let new_height = ((height as f32 * scale) as u32).max(2) & !1;

    Ok((new_width, new_height, true))
}

struct BindGroupCache {
    nv12_bind_groups: [Option<wgpu::BindGroup>; 2],
    yuv420p_bind_groups: [Option<wgpu::BindGroup>; 2],
    cached_width: u32,
    cached_height: u32,
}

impl BindGroupCache {
    fn new() -> Self {
        Self {
            nv12_bind_groups: [None, None],
            yuv420p_bind_groups: [None, None],
            cached_width: 0,
            cached_height: 0,
        }
    }

    fn invalidate(&mut self) {
        self.nv12_bind_groups = [None, None];
        self.yuv420p_bind_groups = [None, None];
        self.cached_width = 0;
        self.cached_height = 0;
    }

    #[allow(clippy::too_many_arguments)]
    fn get_or_create_nv12(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        y_view: &wgpu::TextureView,
        uv_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        output_index: usize,
        width: u32,
        height: u32,
    ) -> &wgpu::BindGroup {
        if self.cached_width != width || self.cached_height != height {
            self.invalidate();
            self.cached_width = width;
            self.cached_height = height;
        }

        if self.nv12_bind_groups[output_index].is_none() {
            self.nv12_bind_groups[output_index] =
                Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("NV12 Converter Bind Group (Cached)"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(y_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(uv_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(output_view),
                        },
                    ],
                }));
        }

        self.nv12_bind_groups[output_index].as_ref().unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn get_or_create_yuv420p(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        y_view: &wgpu::TextureView,
        u_view: &wgpu::TextureView,
        v_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        output_index: usize,
        width: u32,
        height: u32,
    ) -> &wgpu::BindGroup {
        if self.cached_width != width || self.cached_height != height {
            self.invalidate();
            self.cached_width = width;
            self.cached_height = height;
        }

        if self.yuv420p_bind_groups[output_index].is_none() {
            self.yuv420p_bind_groups[output_index] =
                Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("YUV420P Converter Bind Group (Cached)"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(y_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(u_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(v_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(output_view),
                        },
                    ],
                }));
        }

        self.yuv420p_bind_groups[output_index].as_ref().unwrap()
    }
}

use std::sync::Arc;

pub struct YuvConverterPipelines {
    pub nv12_pipeline: wgpu::ComputePipeline,
    pub yuv420p_pipeline: wgpu::ComputePipeline,
    pub nv12_bind_group_layout: wgpu::BindGroupLayout,
    pub yuv420p_bind_group_layout: wgpu::BindGroupLayout,
}

impl YuvConverterPipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        tracing::info!("Creating shared YUV converter pipelines (shader compilation)");

        let nv12_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("NV12 to RGBA Converter"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/nv12_to_rgba.wgsl"
            ))),
        });

        let yuv420p_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("YUV420P to RGBA Converter"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/yuv420p_to_rgba.wgsl"
            ))),
        });

        let nv12_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("NV12 Converter Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });

        let yuv420p_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("YUV420P Converter Bind Group Layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });

        let nv12_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("NV12 Converter Pipeline Layout"),
            bind_group_layouts: &[&nv12_bind_group_layout],
            push_constant_ranges: &[],
        });

        let yuv420p_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("YUV420P Converter Pipeline Layout"),
                bind_group_layouts: &[&yuv420p_bind_group_layout],
                push_constant_ranges: &[],
            });

        let nv12_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("NV12 Converter Pipeline"),
            layout: Some(&nv12_pipeline_layout),
            module: &nv12_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let yuv420p_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("YUV420P Converter Pipeline"),
            layout: Some(&yuv420p_pipeline_layout),
            module: &yuv420p_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        tracing::info!("Shared YUV converter pipelines created successfully");

        Self {
            nv12_pipeline,
            yuv420p_pipeline,
            nv12_bind_group_layout,
            yuv420p_bind_group_layout,
        }
    }
}

pub struct YuvToRgbaConverter {
    pipelines: Arc<YuvConverterPipelines>,
    y_texture: wgpu::Texture,
    y_view: wgpu::TextureView,
    uv_texture: wgpu::Texture,
    uv_view: wgpu::TextureView,
    u_texture: wgpu::Texture,
    u_view: wgpu::TextureView,
    v_texture: wgpu::Texture,
    v_view: wgpu::TextureView,
    output_textures: [wgpu::Texture; 2],
    output_views: [wgpu::TextureView; 2],
    current_output: usize,
    allocated_width: u32,
    allocated_height: u32,
    gpu_max_texture_size: u32,
    bind_group_cache: BindGroupCache,
    #[cfg(target_os = "macos")]
    iosurface_cache: Option<IOSurfaceTextureCache>,
}

impl YuvToRgbaConverter {
    pub fn new(device: &wgpu::Device) -> Self {
        let pipelines = Arc::new(YuvConverterPipelines::new(device));
        Self::new_with_shared_pipelines(device, pipelines)
    }

    pub fn new_with_shared_pipelines(
        device: &wgpu::Device,
        pipelines: Arc<YuvConverterPipelines>,
    ) -> Self {
        let gpu_max_texture_size = device.limits().max_texture_dimension_2d;

        tracing::info!(
            gpu_max_texture_size = gpu_max_texture_size,
            "Initializing YUV converter textures (using shared pipelines)"
        );

        let initial_width = INITIAL_TEXTURE_WIDTH;
        let initial_height = INITIAL_TEXTURE_HEIGHT;

        let (y_texture, y_view) = Self::create_y_texture(device, initial_width, initial_height);
        let (uv_texture, uv_view) = Self::create_uv_texture(device, initial_width, initial_height);
        let (u_texture, u_view) = Self::create_u_texture(device, initial_width, initial_height);
        let (v_texture, v_view) = Self::create_v_texture(device, initial_width, initial_height);
        let (output_textures, output_views) =
            Self::create_output_textures(device, initial_width, initial_height);

        Self {
            pipelines,
            y_texture,
            y_view,
            uv_texture,
            uv_view,
            u_texture,
            u_view,
            v_texture,
            v_view,
            output_textures,
            output_views,
            current_output: 0,
            allocated_width: initial_width,
            allocated_height: initial_height,
            gpu_max_texture_size,
            bind_group_cache: BindGroupCache::new(),
            #[cfg(target_os = "macos")]
            iosurface_cache: IOSurfaceTextureCache::new(device),
        }
    }

    fn create_y_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Y Plane Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    fn create_uv_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("UV Plane Texture"),
            size: wgpu::Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    fn create_u_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("U Plane Texture"),
            size: wgpu::Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    fn create_v_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("V Plane Texture"),
            size: wgpu::Extent3d {
                width: width / 2,
                height: height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    fn create_output_textures(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> ([wgpu::Texture; 2], [wgpu::TextureView; 2]) {
        let create_one = |label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };

        let texture_0 = create_one("RGBA Output Texture 0");
        let texture_1 = create_one("RGBA Output Texture 1");
        let view_0 = texture_0.create_view(&Default::default());
        let view_1 = texture_1.create_view(&Default::default());

        ([texture_0, texture_1], [view_0, view_1])
    }

    fn ensure_texture_size(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let required_width = align_dimension(width);
        let required_height = align_dimension(height);

        if required_width <= self.allocated_width && required_height <= self.allocated_height {
            return;
        }

        let new_width = required_width.max(self.allocated_width);
        let new_height = required_height.max(self.allocated_height);

        tracing::info!(
            old_width = self.allocated_width,
            old_height = self.allocated_height,
            new_width = new_width,
            new_height = new_height,
            "Reallocating YUV converter textures for larger video"
        );

        let (y_texture, y_view) = Self::create_y_texture(device, new_width, new_height);
        let (uv_texture, uv_view) = Self::create_uv_texture(device, new_width, new_height);
        let (u_texture, u_view) = Self::create_u_texture(device, new_width, new_height);
        let (v_texture, v_view) = Self::create_v_texture(device, new_width, new_height);
        let (output_textures, output_views) =
            Self::create_output_textures(device, new_width, new_height);

        self.y_texture = y_texture;
        self.y_view = y_view;
        self.uv_texture = uv_texture;
        self.uv_view = uv_view;
        self.u_texture = u_texture;
        self.u_view = u_view;
        self.v_texture = v_texture;
        self.v_view = v_view;
        self.output_textures = output_textures;
        self.output_views = output_views;
        self.allocated_width = new_width;
        self.allocated_height = new_height;
        self.bind_group_cache.invalidate();
    }

    pub fn gpu_max_texture_size(&self) -> u32 {
        self.gpu_max_texture_size
    }

    pub fn prepare_for_dimensions(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<(), YuvConversionError> {
        let (effective_width, effective_height, _) =
            validate_dimensions(width, height, self.gpu_max_texture_size)?;
        self.ensure_texture_size(device, effective_width, effective_height);
        Ok(())
    }

    fn swap_output_buffer(&mut self) {
        self.current_output = 1 - self.current_output;
    }

    fn current_output_texture(&self) -> &wgpu::Texture {
        &self.output_textures[self.current_output]
    }

    fn current_output_view(&self) -> &wgpu::TextureView {
        &self.output_views[self.current_output]
    }

    #[allow(clippy::too_many_arguments)]
    pub fn convert_nv12(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        y_data: &[u8],
        uv_data: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Result<&wgpu::TextureView, YuvConversionError> {
        let (effective_width, effective_height, _downscaled) =
            validate_dimensions(width, height, self.gpu_max_texture_size)?;
        self.ensure_texture_size(device, effective_width, effective_height);
        self.swap_output_buffer();

        upload_plane_with_stride(queue, &self.y_texture, y_data, width, height, y_stride, "Y")?;

        let half_height = height / 2;
        if uv_stride < width {
            return Err(YuvConversionError::PlaneStrideTooSmall {
                plane: "UV",
                expected: width,
                actual: uv_stride,
            });
        }

        let expected_uv_size = (uv_stride * half_height) as usize;
        if uv_data.len() < expected_uv_size {
            return Err(YuvConversionError::PlaneSizeMismatch {
                plane: "UV",
                expected: expected_uv_size,
                actual: uv_data.len(),
            });
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.uv_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            uv_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(uv_stride),
                rows_per_image: Some(half_height),
            },
            wgpu::Extent3d {
                width: width / 2,
                height: half_height,
                depth_or_array_layers: 1,
            },
        );

        let output_index = self.current_output;
        let bind_group = self.bind_group_cache.get_or_create_nv12(
            device,
            &self.pipelines.nv12_bind_group_layout,
            &self.y_view,
            &self.uv_view,
            &self.output_views[output_index],
            output_index,
            self.allocated_width,
            self.allocated_height,
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("NV12 Conversion Encoder"),
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NV12 Conversion Pass"),
                ..Default::default()
            });
            compute_pass.set_pipeline(&self.pipelines.nv12_pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);
            compute_pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        Ok(self.current_output_view())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn convert_nv12_to_encoder(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        y_data: &[u8],
        uv_data: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Result<&wgpu::TextureView, YuvConversionError> {
        let (effective_width, effective_height, _downscaled) =
            validate_dimensions(width, height, self.gpu_max_texture_size)?;
        self.ensure_texture_size(device, effective_width, effective_height);
        self.swap_output_buffer();

        upload_plane_with_stride(queue, &self.y_texture, y_data, width, height, y_stride, "Y")?;

        let half_height = height / 2;
        if uv_stride < width {
            return Err(YuvConversionError::PlaneStrideTooSmall {
                plane: "UV",
                expected: width,
                actual: uv_stride,
            });
        }

        let expected_uv_size = (uv_stride * half_height) as usize;
        if uv_data.len() < expected_uv_size {
            return Err(YuvConversionError::PlaneSizeMismatch {
                plane: "UV",
                expected: expected_uv_size,
                actual: uv_data.len(),
            });
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.uv_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            uv_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(uv_stride),
                rows_per_image: Some(half_height),
            },
            wgpu::Extent3d {
                width: width / 2,
                height: half_height,
                depth_or_array_layers: 1,
            },
        );

        let output_index = self.current_output;
        let bind_group = self.bind_group_cache.get_or_create_nv12(
            device,
            &self.pipelines.nv12_bind_group_layout,
            &self.y_view,
            &self.uv_view,
            &self.output_views[output_index],
            output_index,
            self.allocated_width,
            self.allocated_height,
        );

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NV12 Conversion Pass (Batched)"),
                ..Default::default()
            });
            compute_pass.set_pipeline(&self.pipelines.nv12_pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);
            compute_pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        Ok(self.current_output_view())
    }

    #[cfg(target_os = "macos")]
    pub fn convert_nv12_from_iosurface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image_buf: &cv::ImageBuf,
    ) -> Result<&wgpu::TextureView, YuvConversionError> {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("NV12 IOSurface Conversion Encoder"),
        });

        self.convert_nv12_from_iosurface_to_encoder(device, &mut encoder, image_buf)?;
        queue.submit(std::iter::once(encoder.finish()));

        Ok(self.current_output_view())
    }

    #[cfg(target_os = "macos")]
    pub fn convert_nv12_from_iosurface_to_encoder(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        image_buf: &cv::ImageBuf,
    ) -> Result<&wgpu::TextureView, YuvConversionError> {
        if self.iosurface_cache.is_none() {
            return Err(IOSurfaceTextureError::NoMetalDevice.into());
        }

        let io_surface = image_buf
            .io_surf()
            .ok_or(IOSurfaceTextureError::NoIOSurface)?;

        let width = image_buf.width() as u32;
        let height = image_buf.height() as u32;

        let (effective_width, effective_height, _downscaled) =
            validate_dimensions(width, height, self.gpu_max_texture_size)?;
        self.ensure_texture_size(device, effective_width, effective_height);
        self.swap_output_buffer();

        let cache = self.iosurface_cache.as_ref().unwrap();
        let y_metal_texture = cache.create_y_texture(io_surface, width, height)?;
        let uv_metal_texture = cache.create_uv_texture(io_surface, width, height)?;

        let y_wgpu_texture = import_metal_texture_to_wgpu(
            device,
            &y_metal_texture,
            wgpu::TextureFormat::R8Unorm,
            width,
            height,
            Some("IOSurface Y Plane"),
        )?;

        let uv_wgpu_texture = import_metal_texture_to_wgpu(
            device,
            &uv_metal_texture,
            wgpu::TextureFormat::Rg8Unorm,
            width.div_ceil(2),
            height.div_ceil(2),
            Some("IOSurface UV Plane"),
        )?;

        let y_view = y_wgpu_texture.create_view(&Default::default());
        let uv_view = uv_wgpu_texture.create_view(&Default::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("NV12 IOSurface Converter Bind Group"),
            layout: &self.pipelines.nv12_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&uv_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(self.current_output_view()),
                },
            ],
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("NV12 IOSurface Conversion Pass (Batched)"),
                ..Default::default()
            });
            compute_pass.set_pipeline(&self.pipelines.nv12_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);
            compute_pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        Ok(self.current_output_view())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn convert_yuv420p(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        y_data: &[u8],
        u_data: &[u8],
        v_data: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Result<&wgpu::TextureView, YuvConversionError> {
        let (effective_width, effective_height, _downscaled) =
            validate_dimensions(width, height, self.gpu_max_texture_size)?;
        self.ensure_texture_size(device, effective_width, effective_height);
        self.swap_output_buffer();

        upload_plane_with_stride(queue, &self.y_texture, y_data, width, height, y_stride, "Y")?;

        let half_width = width / 2;
        let half_height = height / 2;

        upload_plane_with_stride(
            queue,
            &self.u_texture,
            u_data,
            half_width,
            half_height,
            uv_stride,
            "U",
        )?;
        upload_plane_with_stride(
            queue,
            &self.v_texture,
            v_data,
            half_width,
            half_height,
            uv_stride,
            "V",
        )?;

        let output_index = self.current_output;
        let bind_group = self.bind_group_cache.get_or_create_yuv420p(
            device,
            &self.pipelines.yuv420p_bind_group_layout,
            &self.y_view,
            &self.u_view,
            &self.v_view,
            &self.output_views[output_index],
            output_index,
            self.allocated_width,
            self.allocated_height,
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("YUV420P Conversion Encoder"),
        });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("YUV420P Conversion Pass"),
                ..Default::default()
            });
            compute_pass.set_pipeline(&self.pipelines.yuv420p_pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);
            compute_pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        Ok(self.current_output_view())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn convert_yuv420p_to_encoder(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        y_data: &[u8],
        u_data: &[u8],
        v_data: &[u8],
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Result<&wgpu::TextureView, YuvConversionError> {
        let (effective_width, effective_height, _downscaled) =
            validate_dimensions(width, height, self.gpu_max_texture_size)?;
        self.ensure_texture_size(device, effective_width, effective_height);
        self.swap_output_buffer();

        upload_plane_with_stride(queue, &self.y_texture, y_data, width, height, y_stride, "Y")?;

        let half_width = width / 2;
        let half_height = height / 2;

        upload_plane_with_stride(
            queue,
            &self.u_texture,
            u_data,
            half_width,
            half_height,
            uv_stride,
            "U",
        )?;
        upload_plane_with_stride(
            queue,
            &self.v_texture,
            v_data,
            half_width,
            half_height,
            uv_stride,
            "V",
        )?;

        let output_index = self.current_output;
        let bind_group = self.bind_group_cache.get_or_create_yuv420p(
            device,
            &self.pipelines.yuv420p_bind_group_layout,
            &self.y_view,
            &self.u_view,
            &self.v_view,
            &self.output_views[output_index],
            output_index,
            self.allocated_width,
            self.allocated_height,
        );

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("YUV420P Conversion Pass (Batched)"),
                ..Default::default()
            });
            compute_pass.set_pipeline(&self.pipelines.yuv420p_pipeline);
            compute_pass.set_bind_group(0, bind_group, &[]);
            compute_pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }

        Ok(self.current_output_view())
    }

    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        Some(self.current_output_texture())
    }
}
