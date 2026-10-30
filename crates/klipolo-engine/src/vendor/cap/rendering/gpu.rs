// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/lib.rs (excerpts: 181–203, 220–240, 273–275, 575–615, 2148–2255, 7984–8042)
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: only these items copied (is_software_wgpu_adapter + markers, create_wgpu_instance_sync macOS branch, create_shared_device + SharedWgpuDevice, create_shader_render_pipeline, RenderingError); RenderingError keeps NoAdapter, RequestDeviceFailed, BufferMapWaitingFailed, BufferMapFailed, ImageLoadError, PollError and Surface (the NV12 IOSurface ring uses it); readiness::Phase timing and force_software_wgpu_adapter removed from create_shared_device; device label renamed; the hardware adapter is requested with PowerPreference::LowPower (was HighPerformance); the instance's backends come from crate::platform::gpu::instance_descriptor (Metal on macOS, DX12 on Windows, Vulkan or GL on Linux; was every backend). `ImageLoadError` carries `image_load::ImageError` (typed: missing or a message). `create_shared_device_with(wanted, limits)` asks for the optional features `wanted` when the adapter has them and for `limits` (the render worker's: timestamps, a 4096 px texture side, 256 MiB buffers); `create_shared_device` is it with none and the defaults. The adapter request is `choose_adapter`, which the app also runs to key this computer as the render worker does (`make::budget::this_computer`). `create_shared_device` also asks for the timestamp features when the perf harness asked (`render::text_timing::timestamp_features`; never in the app).

//! Device setup shared by the vendored layers (Cap kept these in `rendering/src/lib.rs`).

const NON_HARDWARE_WGPU_ADAPTER_MARKERS: &[&str] = &[
    "parsec",
    "displaylink",
    "splashtop",
    "synergy",
    "virtual display",
    "microsoft basic render",
    "microsoft basic",
    "warp",
];

pub fn is_software_wgpu_adapter(info: &wgpu::AdapterInfo) -> bool {
    matches!(
        info.device_type,
        wgpu::DeviceType::Cpu | wgpu::DeviceType::VirtualGpu
    ) || {
        let name = info.name.to_ascii_lowercase();
        NON_HARDWARE_WGPU_ADAPTER_MARKERS
            .iter()
            .any(|marker| name.contains(marker))
    }
}

pub fn create_wgpu_instance_sync() -> wgpu::Instance {
    // Klipolo: each OS's graphics API (Metal, DX12, Vulkan or GL) from the platform seam.
    wgpu::Instance::new(&crate::platform::gpu::instance_descriptor())
}

pub async fn create_wgpu_instance() -> wgpu::Instance {
    create_wgpu_instance_sync()
}

#[derive(thiserror::Error, Debug)]
pub enum RenderingError {
    #[error("No GPU adapter found")]
    NoAdapter,
    #[error(transparent)]
    RequestDeviceFailed(#[from] wgpu::RequestDeviceError),
    #[error("Failed to wait for buffer mapping")]
    BufferMapWaitingFailed,
    #[error(transparent)]
    BufferMapFailed(#[from] wgpu::BufferAsyncError),
    #[error("Failed to load image: {0}")]
    ImageLoadError(crate::render::image_load::ImageError),
    #[error("Error polling wgpu: {0}")]
    PollError(#[from] wgpu::PollError),
    #[error("Failed to create preview surface: {0}")]
    Surface(String),
}

/// Instance, adapter and device the way every renderer creates them. A
/// long-lived process can make one up front and hand it to
/// [`RenderVideoConstants::new_with_device`].
pub async fn create_shared_device() -> Result<SharedWgpuDevice, RenderingError> {
    create_shared_device_with(wgpu::Features::empty(), wgpu::Limits::default()).await
}

/// Klipolo: [`create_shared_device`] that also asks for the features of `wanted` the adapter has, with
/// `limits`.
pub async fn create_shared_device_with(
    wanted: wgpu::Features,
    limits: wgpu::Limits,
) -> Result<SharedWgpuDevice, RenderingError> {
    let instance = create_wgpu_instance().await;
    let (adapter, is_software_adapter) = choose_adapter(&instance).await?;

    let mut required_features = wgpu::Features::empty();
    if adapter.features().contains(wgpu::Features::PIPELINE_CACHE) {
        required_features |= wgpu::Features::PIPELINE_CACHE;
    }
    if adapter
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    {
        required_features |= wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    }
    // Klipolo: GPU timestamps only when the perf harness asked for them.
    required_features |= crate::render::text_timing::timestamp_features(adapter.features());

    required_features |= adapter.features() & wanted;

    let device_descriptor = wgpu::DeviceDescriptor {
        label: Some("klipolo-rendering-device"),
        required_features,
        required_limits: limits,
        ..Default::default()
    };

    let (device, queue) = adapter.request_device(&device_descriptor).await?;
    Ok(SharedWgpuDevice {
        instance,
        adapter,
        device,
        queue,
        is_software_adapter,
    })
}

pub struct SharedWgpuDevice {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub is_software_adapter: bool,
}

pub fn create_shader_render_pipeline(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
    shader: wgpu::ShaderModuleDescriptor,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(shader);

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Render Pipeline Layout"),
        bind_group_layouts: &[bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Render Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[],
                zero_initialize_workgroup_memory: false,
            },
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
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

/// Klipolo: the adapter every device of Klipolo's is made on (the preview's, the export's and the render
/// worker's): the hardware adapter `LowPower` prefers, else the software fallback, and whether it renders in software.
pub async fn choose_adapter(
    instance: &wgpu::Instance,
) -> Result<(wgpu::Adapter, bool), RenderingError> {
    let hardware_adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            // Klipolo: the integrated GPU, where VideoToolbox's IOSurfaces live (dual-GPU Intel Macs would otherwise
            // composite on the discrete GPU and keep it powered).
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
        .ok();

    let (adapter, is_software_adapter) = if let Some(adapter) = hardware_adapter {
        let adapter_info = adapter.get_info();
        let is_software = is_software_wgpu_adapter(&adapter_info);

        if is_software {
            tracing::warn!(
                adapter_name = adapter_info.name,
                adapter_backend = ?adapter_info.backend,
                adapter_device_type = ?adapter_info.device_type,
                "Hardware adapter behaves like a software renderer"
            );
        } else {
            tracing::info!(
                adapter_name = adapter_info.name,
                adapter_backend = ?adapter_info.backend,
                adapter_device_type = ?adapter_info.device_type,
                "Using hardware GPU adapter"
            );
        }

        (adapter, is_software)
    } else {
        tracing::warn!("No hardware GPU adapter found, attempting software fallback");
        let software_adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: true,
                compatible_surface: None,
            })
            .await
            .map_err(|_| RenderingError::NoAdapter)?;

        let adapter_info = software_adapter.get_info();
        tracing::info!(
            adapter_name = adapter_info.name,
            adapter_backend = ?adapter_info.backend,
            adapter_device_type = ?adapter_info.device_type,
            "Using software adapter (CPU rendering - performance may be reduced)"
        );
        (software_adapter, true)
    };
    Ok((adapter, is_software_adapter))
}
