// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/frame_pipeline.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted; `uniforms: &ProjectUniforms` parameters became `target: &FrameTarget`; Cap's native preview surface (SurfaceFrame, RgbaToBgraSurfaceConverter, PendingSurface, finish_encoder_bgra_surface) and its GPU tests (surface_output_tests) removed; readiness phases, the blur-cache COPY_DST session usage, the test-only surface failure hook and the Linux CUDA output (external ring, PendingNv12External, gpu fields) removed; the RGBA readback reserves 48 spare bytes (was 24) for Klipolo's frame trailer; the NV12 surface ring's IOSurface texture cache is made on wgpu's own Metal device; `RgbaToNv12Converter::encode_into_storage` (and its `external_readback` flag) added, so the preview's socket path converts into the storage buffer and reads it back through Klipolo's own staging ring; the converter's bind-group cache is keyed by texture identity (`cached_texture`) instead of the texture reference's address.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::oneshot;
use wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

use crate::vendor::cap::compat::FrameTarget;
use crate::vendor::cap::rendering::RenderingError;

#[cfg(target_os = "macos")]
use crate::vendor::cap::rendering::iosurface_texture::{
    IOSurfaceTextureCache, import_metal_texture_to_wgpu_with_usage,
};
#[cfg(target_os = "macos")]
use cidre::{arc, cf, cv, io, mtl};

const GPU_BUFFER_WAIT_TIMEOUT_SECS: u64 = 10;
const SOFTWARE_GPU_BUFFER_WAIT_TIMEOUT_SECS: u64 = 60;

/// Whether this process has selected a software (CPU rasterizer) wgpu adapter.
/// Software adapters like Windows WARP legitimately take tens of seconds for the
/// first cold render (pipeline compilation + full-frame rasterization on the CPU),
/// so readback waits get a much longer deadline before being treated as failures —
/// timing out the first frame is what used to leave the editor on a blank screen.
static SOFTWARE_ADAPTER_IN_USE: AtomicBool = AtomicBool::new(false);

pub(crate) fn note_software_adapter_in_use() {
    SOFTWARE_ADAPTER_IN_USE.store(true, Ordering::Release);
}

fn gpu_buffer_wait_timeout() -> std::time::Duration {
    let secs = if SOFTWARE_ADAPTER_IN_USE.load(Ordering::Acquire) {
        SOFTWARE_GPU_BUFFER_WAIT_TIMEOUT_SECS
    } else {
        GPU_BUFFER_WAIT_TIMEOUT_SECS
    };
    std::time::Duration::from_secs(secs)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FinishEncoderTimings {
    pub wait_previous_duration: std::time::Duration,
    pub resize_duration: std::time::Duration,
    pub submit_readback_duration: std::time::Duration,
}

pub struct NV12BufferPool {
    buffers: Arc<Mutex<Vec<Vec<u8>>>>,
}

const MAX_POOLED_BUFFERS: usize = 8;

struct PooledNv12Buffer {
    data: Vec<u8>,
    pool: Option<Arc<Mutex<Vec<Vec<u8>>>>>,
}

impl Drop for PooledNv12Buffer {
    fn drop(&mut self) {
        let Some(pool) = self.pool.take() else {
            return;
        };

        let mut data = std::mem::take(&mut self.data);
        data.clear();

        if let Ok(mut buffers) = pool.lock()
            && buffers.len() < MAX_POOLED_BUFFERS
        {
            buffers.push(data);
        }
    }
}

#[derive(Clone)]
pub struct SharedNv12Buffer(Arc<PooledNv12Buffer>);

impl SharedNv12Buffer {
    fn new(data: Vec<u8>, pool: Option<Arc<Mutex<Vec<Vec<u8>>>>>) -> Self {
        Self(Arc::new(PooledNv12Buffer { data, pool }))
    }

    pub fn from_vec(data: Vec<u8>) -> Self {
        Self::new(data, None)
    }

    pub fn from_arc_vec(data: Arc<Vec<u8>>) -> Self {
        Self::from_vec(Arc::unwrap_or_clone(data))
    }

    pub fn into_vec(self) -> Vec<u8> {
        match Arc::try_unwrap(self.0) {
            Ok(mut inner) => {
                inner.pool = None;
                std::mem::take(&mut inner.data)
            }
            Err(shared) => shared.data.clone(),
        }
    }
}

impl AsRef<[u8]> for SharedNv12Buffer {
    fn as_ref(&self) -> &[u8] {
        &self.0.data
    }
}

impl Deref for SharedNv12Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0.data
    }
}

impl NV12BufferPool {
    pub fn new(pre_alloc: usize) -> Self {
        Self {
            buffers: Arc::new(Mutex::new(Vec::with_capacity(pre_alloc))),
        }
    }

    pub fn acquire(&self, size: usize) -> Vec<u8> {
        if let Ok(mut buffers) = self.buffers.lock()
            && let Some(pos) = buffers.iter().position(|b| b.capacity() >= size)
        {
            let mut buf = buffers.swap_remove(pos);
            buf.clear();
            return buf;
        }

        Vec::with_capacity(size)
    }

    pub fn wrap(&self, data: Vec<u8>) -> SharedNv12Buffer {
        SharedNv12Buffer::new(data, Some(Arc::clone(&self.buffers)))
    }
}

pub struct RgbaToNv12Converter {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    nv12_buffer: Option<wgpu::Buffer>,
    readback_buffers: [Option<Arc<wgpu::Buffer>>; 2],
    current_readback: usize,
    pending: Option<PendingNv12Output>,
    cached_width: u32,
    cached_height: u32,
    cached_stride: u32,
    cached_bind_groups: Option<[wgpu::BindGroup; 2]>,
    cached_texture_view: Option<wgpu::TextureView>,
    /// Klipolo: the texture the cached bind groups read, compared by wgpu identity (Cap compared the reference's
    /// address, which stays the same when a render session recreates its texture in place: the converter then kept
    /// reading the old texture and the picture froze).
    cached_texture: Option<wgpu::Texture>,
    surface_output: bool,
    /// Klipolo: the caller reads the storage buffer back itself (`encode_into_storage`), so no readback buffers here.
    external_readback: bool,
    #[cfg(target_os = "macos")]
    surface_ring: Option<Nv12SurfaceRing>,
}

/// An NV12 CVPixelBuffer produced by the GPU converter, ready for zero-copy
/// VideoToolbox encoding. Consumers outside this crate use the raw-pointer and
/// locked-plane accessors so they need no CoreVideo bindings of their own.
#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct Nv12Surface(arc::R<cv::PixelBuf>);

#[cfg(target_os = "macos")]
unsafe impl Send for Nv12Surface {}
#[cfg(target_os = "macos")]
unsafe impl Sync for Nv12Surface {}

#[cfg(target_os = "macos")]
impl Nv12Surface {
    /// The underlying `CVPixelBufferRef`, valid while `self` is alive.
    pub fn as_pixel_buffer_ptr(&self) -> *mut std::ffi::c_void {
        (self.0.as_ref() as *const cv::PixelBuf as *const std::ffi::c_void).cast_mut()
    }

    /// Locks the buffer for CPU reads and hands the callback both NV12 planes
    /// with their strides: `(y, y_stride, uv, uv_stride)`.
    pub fn with_locked_planes<T>(
        &self,
        callback: impl FnOnce(&[u8], usize, &[u8], usize) -> T,
    ) -> Option<T> {
        let mut buf = self.0.clone();
        unsafe {
            buf.lock_base_addr(cv::pixel_buffer::LockFlags::READ_ONLY)
                .result()
                .ok()?;
        }
        let y_stride = buf.plane_bytes_per_row(0);
        let uv_stride = buf.plane_bytes_per_row(1);
        let y_height = buf.plane_height(0);
        let uv_height = buf.plane_height(1);
        let result = unsafe {
            let y_plane =
                std::slice::from_raw_parts(buf.plane_base_address(0), y_stride * y_height);
            let uv_plane =
                std::slice::from_raw_parts(buf.plane_base_address(1), uv_stride * uv_height);
            callback(y_plane, y_stride, uv_plane, uv_stride)
        };
        unsafe {
            buf.unlock_lock_base_addr(cv::pixel_buffer::LockFlags::READ_ONLY);
        }
        Some(result)
    }
}

#[cfg(target_os = "macos")]
fn surface_allocation_attributes(limit: usize) -> Result<arc::R<cf::Dictionary>, RenderingError> {
    let threshold = cf::Number::from_usize(limit);
    cf::Dictionary::with_keys_values(
        &[cv::pixel_buffer_pool::aux_attr_keys::allocation_threashold().as_ref()],
        &[threshold.as_ref()],
    )
    .ok_or_else(|| RenderingError::Surface("Failed to create surface allocation limit".to_string()))
}

#[cfg(target_os = "macos")]
async fn acquire_surface_buffer<T: Send>(
    owner: &mut T,
    mut allocate: impl FnMut(&mut T) -> Result<arc::R<cv::PixelBuf>, cidre::os::Error> + Send,
) -> Result<arc::R<cv::PixelBuf>, RenderingError> {
    let started = Instant::now();
    loop {
        let error = match allocate(owner) {
            Ok(buffer) => return Ok(buffer),
            Err(error) => error,
        };
        if error != cv::err::WOULD_EXCEED_ALLOCATION_THRESHOLD
            || started.elapsed() >= std::time::Duration::from_secs(2)
        {
            return Err(RenderingError::Surface(format!(
                "Surface pool is unavailable: {error}"
            )));
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

#[cfg(target_os = "macos")]
// The pool controls reuse; cached imports must not keep a CVPixelBuffer lease.
// CoreVideo also tracks CVMetalTexture consumers that do not retain that buffer.
struct Nv12SurfaceSlot {
    surface: arc::R<io::Surf>,
    y_texture: wgpu::Texture,
    uv_texture: wgpu::Texture,
}

#[cfg(target_os = "macos")]
struct Nv12SurfaceRing {
    texture_cache: IOSurfaceTextureCache,
    slots: Vec<Nv12SurfaceSlot>,
    pool: Option<arc::R<cv::PixelBufPool>>,
    allocation_attributes: arc::R<cf::Dictionary>,
    next: usize,
    size: (u32, u32),
}

// SAFETY: same contract as RgbaToBgraSurfaceConverter — the CF objects have
// atomic refcounts and all access is serialized by the owning converter.
#[cfg(target_os = "macos")]
unsafe impl Send for Nv12SurfaceRing {}

#[cfg(target_os = "macos")]
impl Nv12SurfaceRing {
    const SLOTS: usize = 8;
    const MAX_BUFFERS: usize = 32;

    fn new(device: &wgpu::Device) -> Result<Self, RenderingError> {
        let texture_cache = IOSurfaceTextureCache::new(device)
            .ok_or_else(|| RenderingError::Surface("Metal device is unavailable".to_string()))?;
        Ok(Self {
            texture_cache,
            slots: Vec::new(),
            pool: None,
            allocation_attributes: surface_allocation_attributes(Self::MAX_BUFFERS)?,
            next: 0,
            size: (0, 0),
        })
    }

    fn ensure_size(
        &mut self,
        _device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<(), RenderingError> {
        if self.size == (width, height) && self.pool.is_some() {
            return Ok(());
        }

        let minimum_count = cf::Number::from_usize(3);
        let width_number = cf::Number::from_usize(width as usize);
        let height_number = cf::Number::from_usize(height as usize);
        let io_surface_properties = cf::Dictionary::new();
        let pool_keys: [&cf::Type; 1] =
            [cv::pixel_buffer_pool::keys::minimum_buffer_count().as_ref()];
        let pool_values: [&cf::Type; 1] = [minimum_count.as_ref()];
        let pool_attributes = cf::Dictionary::with_keys_values(&pool_keys, &pool_values)
            .ok_or_else(|| {
                RenderingError::Surface("Failed to create pixel buffer pool attributes".to_string())
            })?;
        let pixel_buffer_keys: [&cf::Type; 5] = [
            cv::pixel_buffer::keys::pixel_format().as_ref(),
            cv::pixel_buffer::keys::width().as_ref(),
            cv::pixel_buffer::keys::height().as_ref(),
            cv::pixel_buffer::keys::io_surf_props().as_ref(),
            cv::pixel_buffer::keys::metal_compatibility().as_ref(),
        ];
        let pixel_buffer_values: [&cf::Type; 5] = [
            cv::PixelFormat::_420V.to_cf_number().as_ref(),
            width_number.as_ref(),
            height_number.as_ref(),
            io_surface_properties.as_ref(),
            cf::Boolean::value_true().as_ref(),
        ];
        let pixel_buffer_attributes =
            cf::Dictionary::with_keys_values(&pixel_buffer_keys, &pixel_buffer_values).ok_or_else(
                || RenderingError::Surface("Failed to create pixel buffer attributes".to_string()),
            )?;
        let pool = cv::PixelBufPool::new(
            Some(pool_attributes.as_ref()),
            Some(pixel_buffer_attributes.as_ref()),
        )
        .map_err(|error| RenderingError::Surface(error.to_string()))?;

        self.pool = Some(pool);
        self.slots.clear();
        self.next = 0;
        self.size = (width, height);
        Ok(())
    }

    async fn next_slot(
        &mut self,
        device: &wgpu::Device,
    ) -> Result<(arc::R<cv::PixelBuf>, &Nv12SurfaceSlot), RenderingError> {
        if self.pool.is_none() {
            return Err(RenderingError::Surface(
                "NV12 pool is unavailable".to_string(),
            ));
        }
        let pixel_buffer = acquire_surface_buffer(self, |this| {
            this.pool
                .as_ref()
                .expect("pool checked above")
                .pixel_buf_with_aux_attrs(Some(&this.allocation_attributes))
        })
        .await?;
        let io_surface = pixel_buffer
            .io_surf()
            .ok_or_else(|| RenderingError::Surface("Pixel buffer has no IOSurface".to_string()))?;
        let index = if let Some(index) = self
            .slots
            .iter()
            .position(|slot| std::ptr::eq(slot.surface.as_ref(), io_surface))
        {
            index
        } else {
            let (width, height) = self.size;
            let y_metal = self
                .texture_cache
                .create_y_texture(io_surface, width, height)
                .map_err(|error| RenderingError::Surface(error.to_string()))?;
            let y_texture = import_metal_texture_to_wgpu_with_usage(
                device,
                &y_metal,
                wgpu::TextureFormat::R8Unorm,
                width,
                height,
                wgpu::TextureUsages::COPY_DST,
                Some("NV12 IOSurface Y"),
            )
            .map_err(|error| RenderingError::Surface(error.to_string()))?;
            let uv_metal = self
                .texture_cache
                .create_uv_texture(io_surface, width, height)
                .map_err(|error| RenderingError::Surface(error.to_string()))?;
            let uv_texture = import_metal_texture_to_wgpu_with_usage(
                device,
                &uv_metal,
                wgpu::TextureFormat::Rg8Unorm,
                width / 2,
                height / 2,
                wgpu::TextureUsages::COPY_DST,
                Some("NV12 IOSurface UV"),
            )
            .map_err(|error| RenderingError::Surface(error.to_string()))?;
            let slot = Nv12SurfaceSlot {
                surface: io_surface.retained(),
                y_texture,
                uv_texture,
            };
            if self.slots.len() < Self::SLOTS {
                self.slots.push(slot);
                self.slots.len() - 1
            } else {
                let index = self.next;
                self.slots[index] = slot;
                self.next = (index + 1) % Self::SLOTS;
                index
            }
        };
        Ok((pixel_buffer, &self.slots[index]))
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Nv12Params {
    width: u32,
    height: u32,
    y_stride: u32,
    uv_stride: u32,
}

impl RgbaToNv12Converter {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RGBA to NV12 Converter"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "shaders/rgba_to_nv12.wgsl"
            ))),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RGBA to NV12 Bind Group Layout"),
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
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
            label: Some("RGBA to NV12 Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("RGBA to NV12 Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NV12 Params Buffer"),
            size: std::mem::size_of::<Nv12Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            params_buffer,
            nv12_buffer: None,
            readback_buffers: [None, None],
            current_readback: 0,
            pending: None,
            cached_width: 0,
            cached_height: 0,
            cached_stride: 0,
            cached_bind_groups: None,
            cached_texture_view: None,
            cached_texture: None,
            surface_output: false,
            external_readback: false,
            #[cfg(target_os = "macos")]
            surface_ring: None,
        }
    }

    /// Emit IOSurface-backed NV12 CVPixelBuffers instead of CPU readbacks:
    /// the same compute output is GPU-copied into imported IOSurface planes
    /// (byte-identical pixel values, 256-aligned stride for the buffer→texture
    /// copy). If the surface machinery fails at any point the converter falls
    /// back to readback mode permanently.
    #[cfg(target_os = "macos")]
    pub fn enable_surface_output(&mut self) {
        self.surface_output = true;
    }

    fn aligned_stride(&self, width: u32) -> u32 {
        if self.surface_output {
            width.next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT)
        } else {
            (width + 3) & !3
        }
    }

    fn nv12_size(&self, width: u32, height: u32) -> u64 {
        let stride = self.aligned_stride(width) as u64;
        let aligned_height = ((height + 1) & !1) as u64;
        let y_size = stride * aligned_height;
        let uv_size = stride * (aligned_height / 2);
        y_size + uv_size
    }

    fn ensure_buffers(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        #[cfg(target_os = "macos")]
        if self.surface_output {
            let ring_result = match self.surface_ring.as_mut() {
                Some(ring) => ring.ensure_size(device, width, height),
                None => Nv12SurfaceRing::new(device).and_then(|mut ring| {
                    ring.ensure_size(device, width, height)?;
                    self.surface_ring = Some(ring);
                    Ok(())
                }),
            };
            if let Err(error) = ring_result {
                tracing::warn!(
                    %error,
                    "NV12 IOSurface ring unavailable, falling back to CPU readback output"
                );
                self.surface_output = false;
                self.surface_ring = None;
                self.cached_width = 0;
            }
        }

        let stride = self.aligned_stride(width);
        if self.cached_width == width
            && self.cached_height == height
            && self.cached_stride == stride
        {
            #[cfg(target_os = "macos")]
            if self.surface_output {
                self.readback_buffers = [None, None];
            }
            return;
        }

        let nv12_size = self.nv12_size(width, height);
        let aligned_size = nv12_size.div_ceil(4) * 4;

        self.nv12_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("NV12 Storage Buffer"),
            size: aligned_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));

        let make_readback = || {
            Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NV12 Readback Buffer"),
                size: nv12_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }))
        };

        self.readback_buffers = if self.surface_output || self.external_readback {
            [None, None]
        } else {
            [Some(make_readback()), Some(make_readback())]
        };
        self.current_readback = 0;
        self.cached_width = width;
        self.cached_height = height;
        self.cached_stride = stride;
        self.cached_bind_groups = None;
        self.cached_texture_view = None;
        self.cached_texture = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn submit_conversion(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_texture: &wgpu::Texture,
        width: u32,
        height: u32,
        frame_number: u32,
        frame_rate: u32,
    ) -> Result<bool, RenderingError> {
        if width == 0 || height == 0 {
            return Ok(false);
        }

        self.ensure_buffers(device, width, height);

        let Some(nv12_buffer) = self.nv12_buffer.as_ref() else {
            return Ok(false);
        };

        let readback_idx = self.current_readback;
        self.current_readback = 1 - self.current_readback;

        let y_stride = self.aligned_stride(width);
        let uv_stride = self.aligned_stride(width);

        let params = Nv12Params {
            width,
            height,
            y_stride,
            uv_stride,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&[params]));

        let needs_rebind = self.cached_texture.as_ref() != Some(source_texture)
            || self.cached_bind_groups.is_none();

        if needs_rebind {
            let source_view = source_texture.create_view(&Default::default());

            let make_bind_group = |view: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("RGBA to NV12 Bind Group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: nv12_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.params_buffer.as_entire_binding(),
                        },
                    ],
                })
            };

            let bg0 = make_bind_group(&source_view);
            let bg1 = make_bind_group(&source_view);

            self.cached_texture_view = Some(source_view);
            self.cached_bind_groups = Some([bg0, bg1]);
            self.cached_texture = Some(source_texture.clone());
        }

        let bind_groups = self.cached_bind_groups.as_ref().unwrap();

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RGBA to NV12 Conversion"),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_groups[readback_idx], &[]);
            pass.dispatch_workgroups(width.div_ceil(4 * 8), height.div_ceil(2 * 8), 1);
        }

        #[cfg(target_os = "macos")]
        if self.surface_output
            && let Some(ring) = self.surface_ring.as_mut()
        {
            let (pixel_buffer, slot) = ring.next_slot(device).await?;
            let y_plane_bytes = (y_stride as u64) * (height as u64);
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: nv12_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(y_stride),
                        rows_per_image: None,
                    },
                },
                slot.y_texture.as_image_copy(),
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: nv12_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: y_plane_bytes,
                        bytes_per_row: Some(uv_stride),
                        rows_per_image: None,
                    },
                },
                slot.uv_texture.as_image_copy(),
                wgpu::Extent3d {
                    width: width / 2,
                    height: height / 2,
                    depth_or_array_layers: 1,
                },
            );
            self.pending = Some(PendingNv12Output::Surface(PendingNv12Surface {
                pixel_buffer,
                completed: None,
                width,
                height,
                y_stride,
                frame_number,
                frame_rate,
            }));
            return Ok(true);
        }

        let readback_buffer = match self.readback_buffers[readback_idx].as_ref() {
            Some(b) => b.clone(),
            None => return Ok(false),
        };
        let nv12_size = self.nv12_size(width, height);
        encoder.copy_buffer_to_buffer(nv12_buffer, 0, &readback_buffer, 0, nv12_size);

        self.pending = Some(PendingNv12Output::Readback(PendingNv12Readback {
            rx: None,
            buffer: readback_buffer,
            width,
            height,
            y_stride,
            frame_number,
            frame_rate,
        }));

        Ok(true)
    }

    /// Klipolo: converts `source_texture` into the converter's storage buffer at the end of `encoder` and returns that
    /// buffer, the NV12 byte length (Y plane, then interleaved UV) and the row stride both planes share. The caller
    /// copies it into its own readback ring; nothing else is pending here.
    pub fn encode_into_storage(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Option<(&wgpu::Buffer, u64, u32)> {
        if width == 0 || height == 0 {
            return None;
        }
        self.external_readback = true;
        self.ensure_buffers(device, width, height);
        let stride = self.aligned_stride(width);
        let params = Nv12Params {
            width,
            height,
            y_stride: stride,
            uv_stride: stride,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&[params]));
        if self.cached_texture.as_ref() != Some(source_texture) || self.cached_bind_groups.is_none()
        {
            let nv12_buffer = self.nv12_buffer.as_ref()?;
            let view = source_texture.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RGBA to NV12 Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: nv12_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                ],
            });
            self.cached_texture_view = Some(view);
            self.cached_bind_groups = Some([group.clone(), group]);
            self.cached_texture = Some(source_texture.clone());
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("RGBA to NV12 Conversion"),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.cached_bind_groups.as_ref()?[0], &[]);
            pass.dispatch_workgroups(width.div_ceil(4 * 8), height.div_ceil(2 * 8), 1);
        }
        let bytes = self.nv12_size(width, height);
        Some((self.nv12_buffer.as_ref()?, bytes, stride))
    }

    /// Arms the pending output after its command buffer has been submitted:
    /// readback mode starts the buffer map, surface mode registers a
    /// submitted-work-done completion flag.
    pub fn after_submit(&mut self, queue: &wgpu::Queue) {
        match self.pending.as_mut() {
            Some(PendingNv12Output::Readback(pending)) => {
                let (tx, rx) = oneshot::channel();
                pending
                    .buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = tx.send(result);
                    });
                pending.rx = Some(rx);
            }
            #[cfg(target_os = "macos")]
            Some(PendingNv12Output::Surface(pending)) => {
                let completed = Arc::new(AtomicBool::new(false));
                let callback_completed = Arc::clone(&completed);
                queue.on_submitted_work_done(move || {
                    callback_completed.store(true, Ordering::Release);
                });
                pending.completed = Some(completed);
            }
            None => {}
        }
        let _ = queue;
    }

    pub fn take_pending(&mut self) -> Option<PendingNv12Output> {
        self.pending.take()
    }
}

pub enum PendingNv12Output {
    Readback(PendingNv12Readback),
    #[cfg(target_os = "macos")]
    Surface(PendingNv12Surface),
}

impl PendingNv12Output {
    pub async fn wait_with_pool(
        self,
        device: &wgpu::Device,
        buffer_pool: Option<&mut NV12BufferPool>,
    ) -> Result<Nv12RenderedFrame, RenderingError> {
        match self {
            Self::Readback(pending) => pending.wait_with_pool(device, buffer_pool).await,
            #[cfg(target_os = "macos")]
            Self::Surface(pending) => pending.wait(device).await,
        }
    }
}

#[cfg(target_os = "macos")]
pub struct PendingNv12Surface {
    pixel_buffer: arc::R<cv::PixelBuf>,
    completed: Option<Arc<AtomicBool>>,
    width: u32,
    height: u32,
    y_stride: u32,
    frame_number: u32,
    frame_rate: u32,
}

#[cfg(target_os = "macos")]
unsafe impl Send for PendingNv12Surface {}

#[cfg(target_os = "macos")]
impl PendingNv12Surface {
    async fn wait(self, device: &wgpu::Device) -> Result<Nv12RenderedFrame, RenderingError> {
        let Some(completed) = self.completed.as_ref() else {
            return Err(RenderingError::BufferMapWaitingFailed);
        };
        let started = Instant::now();
        let mut poll_count = 0u32;
        while !completed.load(Ordering::Acquire) {
            if started.elapsed() > gpu_buffer_wait_timeout() {
                return Err(RenderingError::BufferMapWaitingFailed);
            }
            device.poll(wgpu::PollType::Poll)?;
            poll_count += 1;
            if poll_count < 10 {
                tokio::task::yield_now().await;
            } else if poll_count < 100 {
                tokio::time::sleep(std::time::Duration::from_micros(100)).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        }

        let target_time_ns =
            (self.frame_number as u64 * 1_000_000_000) / self.frame_rate.max(1) as u64;

        Ok(Nv12RenderedFrame {
            data: SharedNv12Buffer::from_vec(Vec::new()),
            width: self.width,
            height: self.height,
            y_stride: self.y_stride,
            frame_number: self.frame_number,
            target_time_ns,
            format: GpuOutputFormat::Nv12,
            surface: Some(Nv12Surface(self.pixel_buffer)),
        })
    }
}

pub struct PendingNv12Readback {
    rx: Option<oneshot::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    buffer: Arc<wgpu::Buffer>,
    pub width: u32,
    pub height: u32,
    pub y_stride: u32,
    pub frame_number: u32,
    pub frame_rate: u32,
}

impl PendingNv12Readback {
    fn cancel(self) -> RenderingError {
        self.buffer.unmap();
        RenderingError::BufferMapWaitingFailed
    }

    pub async fn wait_with_pool(
        mut self,
        device: &wgpu::Device,
        buffer_pool: Option<&mut NV12BufferPool>,
    ) -> Result<Nv12RenderedFrame, RenderingError> {
        let Some(mut rx) = self.rx.take() else {
            return Err(self.cancel());
        };

        let mut poll_count = 0u32;
        let start_time = Instant::now();
        let timeout_duration = gpu_buffer_wait_timeout();

        loop {
            if start_time.elapsed() > timeout_duration {
                return Err(self.cancel());
            }

            match rx.try_recv() {
                Ok(result) => match result {
                    Ok(()) => break,
                    Err(error) => {
                        self.buffer.unmap();
                        return Err(RenderingError::BufferMapFailed(error));
                    }
                },
                Err(oneshot::error::TryRecvError::Empty) => {
                    device.poll(wgpu::PollType::Poll)?;
                    poll_count += 1;
                    if poll_count < 10 {
                        tokio::task::yield_now().await;
                    } else if poll_count < 100 {
                        tokio::time::sleep(std::time::Duration::from_micros(100)).await;
                    } else {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    return Err(self.cancel());
                }
            }
        }

        let buffer_slice = self.buffer.slice(..);
        let data = buffer_slice.get_mapped_range();
        let data_len = data.len();

        let nv12_data = if let Some(pool) = buffer_pool {
            let mut buf = pool.acquire(data_len);
            buf.extend_from_slice(&data);
            pool.wrap(buf)
        } else {
            SharedNv12Buffer::from_vec(data.to_vec())
        };

        drop(data);
        self.buffer.unmap();

        let target_time_ns =
            (self.frame_number as u64 * 1_000_000_000) / self.frame_rate.max(1) as u64;

        Ok(Nv12RenderedFrame {
            data: nv12_data,
            width: self.width,
            height: self.height,
            y_stride: self.y_stride,
            frame_number: self.frame_number,
            target_time_ns,
            format: GpuOutputFormat::Nv12,
            #[cfg(target_os = "macos")]
            surface: None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GpuOutputFormat {
    Nv12,
    Rgba,
}

pub struct Nv12RenderedFrame {
    pub data: SharedNv12Buffer,
    pub width: u32,
    pub height: u32,
    pub y_stride: u32,
    pub frame_number: u32,
    pub target_time_ns: u64,
    pub format: GpuOutputFormat,
    /// When set, the frame lives in an IOSurface-backed CVPixelBuffer and
    /// `data` is empty (surface-output mode; macOS export path).
    #[cfg(target_os = "macos")]
    pub surface: Option<Nv12Surface>,
}

impl Nv12RenderedFrame {
    pub fn clone_metadata_with_data(&self) -> Self {
        Self {
            data: self.data.clone(),
            width: self.width,
            height: self.height,
            y_stride: self.y_stride,
            frame_number: self.frame_number,
            target_time_ns: self.target_time_ns,
            format: self.format,
            #[cfg(target_os = "macos")]
            surface: self.surface.clone(),
        }
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data.into_vec()
    }

    pub fn y_plane(&self) -> &[u8] {
        let y_size = (self.y_stride as usize) * (self.height as usize);
        &self.data[..y_size.min(self.data.len())]
    }

    pub fn uv_plane(&self) -> &[u8] {
        let y_size = (self.y_stride as usize) * (self.height as usize);
        if y_size < self.data.len() {
            &self.data[y_size..]
        } else {
            &[]
        }
    }
}

pub struct PendingReadback {
    rx: oneshot::Receiver<Result<(), wgpu::BufferAsyncError>>,
    buffer: Arc<wgpu::Buffer>,
    padded_bytes_per_row: u32,
    width: u32,
    height: u32,
    frame_number: u32,
    frame_rate: u32,
}

fn active_readback_byte_len(
    padded_bytes_per_row: usize,
    height: usize,
    mapped_bytes: usize,
) -> Option<usize> {
    padded_bytes_per_row
        .checked_mul(height)
        .filter(|&active_bytes| active_bytes > 0 && active_bytes <= mapped_bytes)
}

impl PendingReadback {
    fn cancel(&self) -> RenderingError {
        self.buffer.unmap();
        RenderingError::BufferMapWaitingFailed
    }

    pub async fn wait(mut self, device: &wgpu::Device) -> Result<RenderedFrame, RenderingError> {
        let mut poll_count = 0u32;
        let start_time = Instant::now();
        let timeout_duration = gpu_buffer_wait_timeout();

        loop {
            if start_time.elapsed() > timeout_duration {
                tracing::error!(
                    frame_number = self.frame_number,
                    elapsed_secs = start_time.elapsed().as_secs(),
                    poll_count = poll_count,
                    "GPU buffer mapping timed out after {}s",
                    timeout_duration.as_secs()
                );
                return Err(self.cancel());
            }

            match self.rx.try_recv() {
                Ok(result) => match result {
                    Ok(()) => break,
                    Err(error) => {
                        self.buffer.unmap();
                        return Err(RenderingError::BufferMapFailed(error));
                    }
                },
                Err(oneshot::error::TryRecvError::Empty) => {
                    device.poll(wgpu::PollType::Poll)?;
                    poll_count += 1;
                    if poll_count < 10 {
                        tokio::task::yield_now().await;
                    } else if poll_count < 100 {
                        tokio::time::sleep(std::time::Duration::from_micros(100)).await;
                    } else {
                        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    }
                    if poll_count.is_multiple_of(10000) {
                        tracing::warn!(
                            frame_number = self.frame_number,
                            poll_count = poll_count,
                            elapsed_ms = start_time.elapsed().as_millis() as u64,
                            "GPU buffer mapping taking longer than expected"
                        );
                    }
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    return Err(self.cancel());
                }
            }
        }

        let Some(active_bytes) =
            usize::try_from(self.buffer.size())
                .ok()
                .and_then(|buffer_bytes| {
                    active_readback_byte_len(
                        self.padded_bytes_per_row as usize,
                        self.height as usize,
                        buffer_bytes,
                    )
                })
        else {
            self.buffer.unmap();
            return Err(RenderingError::BufferMapWaitingFailed);
        };
        let buffer_slice = self.buffer.slice(..active_bytes as u64);
        let data = buffer_slice.get_mapped_range();
        // 48 spare bytes: Klipolo appends its frame trailer (frames::trailer) without copying the pixels.
        let mut data_vec = Vec::with_capacity(active_bytes + 48);
        data_vec.extend_from_slice(&data);

        drop(data);
        self.buffer.unmap();

        let target_time_ns =
            (self.frame_number as u64 * 1_000_000_000) / self.frame_rate.max(1) as u64;

        Ok(RenderedFrame {
            data: Arc::new(data_vec),
            padded_bytes_per_row: self.padded_bytes_per_row,
            width: self.width,
            height: self.height,
            frame_number: self.frame_number,
            target_time_ns,
        })
    }
}

pub struct PipelinedGpuReadback {
    buffers: [Arc<wgpu::Buffer>; 3],
    buffer_size: u64,
    current_index: usize,
    pending: Option<PendingReadback>,
    needs_resize: bool,
    pending_resize_size: u64,
}

impl PipelinedGpuReadback {
    pub fn new(device: &wgpu::Device, initial_size: u64) -> Self {
        let make_buffer = || {
            Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Pipelined Readback Buffer"),
                size: initial_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }))
        };

        Self {
            buffers: [make_buffer(), make_buffer(), make_buffer()],
            buffer_size: initial_size,
            current_index: 0,
            pending: None,
            needs_resize: false,
            pending_resize_size: 0,
        }
    }

    pub fn mark_for_resize(&mut self, required_size: u64) {
        if self.buffer_size < required_size {
            self.needs_resize = true;
            self.pending_resize_size = required_size;
        }
    }

    pub fn perform_resize_if_needed(&mut self, device: &wgpu::Device) {
        if self.needs_resize && self.pending.is_none() {
            let required_size = self.pending_resize_size;
            tracing::info!(
                old_size = self.buffer_size,
                new_size = required_size,
                "Resizing GPU readback buffers"
            );
            let make_buffer = || {
                Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Pipelined Readback Buffer"),
                    size: required_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }))
            };

            self.buffers = [make_buffer(), make_buffer(), make_buffer()];
            self.buffer_size = required_size;
            self.current_index = 0;
            self.needs_resize = false;
            self.pending_resize_size = 0;
        }
    }

    pub fn ensure_size(&mut self, device: &wgpu::Device, required_size: u64) {
        if self.buffer_size < required_size {
            if self.pending.is_some() {
                self.mark_for_resize(required_size);
            } else {
                let make_buffer = || {
                    Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Pipelined Readback Buffer"),
                        size: required_size,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }))
                };

                self.buffers = [make_buffer(), make_buffer(), make_buffer()];
                self.buffer_size = required_size;
                self.current_index = 0;
            }
        }
    }

    fn next_buffer(&mut self) -> Arc<wgpu::Buffer> {
        let buffer = self.buffers[self.current_index].clone();
        self.current_index = (self.current_index + 1) % self.buffers.len();
        buffer
    }

    pub fn submit_readback(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        target: &FrameTarget,
        mut render_encoder: wgpu::CommandEncoder,
    ) -> Result<(), RenderingError> {
        let padded_bytes_per_row = padded_bytes_per_row(target.output_size);
        let output_buffer_size = u64::from(padded_bytes_per_row) * u64::from(target.output_size.1);

        self.ensure_size(device, output_buffer_size);
        let buffer = self.next_buffer();

        let output_texture_size = wgpu::Extent3d {
            width: target.output_size.0,
            height: target.output_size.1,
            depth_or_array_layers: 1,
        };

        render_encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(target.output_size.1),
                },
            },
            output_texture_size,
        );

        queue.submit(std::iter::once(render_encoder.finish()));

        let (tx, rx) = oneshot::channel();
        buffer
            .slice(..output_buffer_size)
            .map_async(wgpu::MapMode::Read, move |result| {
                if let Err(e) = tx.send(result) {
                    tracing::error!("Failed to send map_async result: {:?}", e);
                }
            });

        self.pending = Some(PendingReadback {
            rx,
            buffer,
            padded_bytes_per_row,
            width: target.output_size.0,
            height: target.output_size.1,
            frame_number: target.frame_number,
            frame_rate: target.frame_rate,
        });

        Ok(())
    }

    pub fn take_pending(&mut self) -> Option<PendingReadback> {
        self.pending.take()
    }
}

pub struct RenderSession {
    pub textures: (wgpu::Texture, wgpu::Texture),
    texture_views: (wgpu::TextureView, wgpu::TextureView),
    pub current_is_left: bool,
    pipelined_readback: Option<PipelinedGpuReadback>,
    texture_width: u32,
    texture_height: u32,
}

/// The blur result cache copies into session textures, so only render hosts
/// that enable it need them to be copy destinations.
fn session_texture_usage() -> wgpu::TextureUsages {
    wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
}

impl RenderSession {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let make_texture = || {
            device.create_texture(&wgpu::TextureDescriptor {
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: session_texture_usage(),
                label: Some("Intermediate Texture"),
                view_formats: &[],
            })
        };

        let textures = (make_texture(), make_texture());

        Self {
            current_is_left: true,
            texture_views: (
                textures.0.create_view(&Default::default()),
                textures.1.create_view(&Default::default()),
            ),
            textures,
            pipelined_readback: None,
            texture_width: width,
            texture_height: height,
        }
    }

    pub fn update_texture_size(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.texture_width == width && self.texture_height == height {
            return;
        }

        tracing::info!(
            old_width = self.texture_width,
            old_height = self.texture_height,
            new_width = width,
            new_height = height,
            "Resizing render session textures"
        );

        let make_texture = || {
            device.create_texture(&wgpu::TextureDescriptor {
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: session_texture_usage(),
                label: Some("Intermediate Texture"),
                view_formats: &[],
            })
        };

        self.textures = (make_texture(), make_texture());
        self.texture_views = (
            self.textures.0.create_view(&Default::default()),
            self.textures.1.create_view(&Default::default()),
        );
        self.texture_width = width;
        self.texture_height = height;
    }

    pub fn current_texture(&self) -> &wgpu::Texture {
        if self.current_is_left {
            &self.textures.0
        } else {
            &self.textures.1
        }
    }

    pub fn current_texture_view(&self) -> &wgpu::TextureView {
        if self.current_is_left {
            &self.texture_views.0
        } else {
            &self.texture_views.1
        }
    }

    pub fn other_texture_view(&self) -> &wgpu::TextureView {
        if self.current_is_left {
            &self.texture_views.1
        } else {
            &self.texture_views.0
        }
    }

    pub fn swap_textures(&mut self) {
        self.current_is_left = !self.current_is_left;
    }
}

// pub struct FramePipelineState<'a> {
//     pub constants: &'a RenderVideoConstants,
//     pub uniforms: &'a ProjectUniforms,
//     pub texture: &'a wgpu::Texture,
//     pub texture_view: wgpu::TextureView,
// }

// impl<'a> FramePipelineState<'a> {
//     pub fn new(
//         constants: &'a RenderVideoConstants,
//         uniforms: &'a ProjectUniforms,
//         texture: &'a wgpu::Texture,
//     ) -> Self {
//         let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

//         Self {
//             constants,
//             uniforms,
//             texture,
//             texture_view,
//         }
//     }
// }

// pub struct FramePipelineEncoder {
//     pub encoder: wgpu::CommandEncoder,
// }

#[derive(Clone)]
pub struct RenderedFrame {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub padded_bytes_per_row: u32,
    pub frame_number: u32,
    pub target_time_ns: u64,
}

// impl FramePipelineEncoder {
//     pub fn new(state: &FramePipelineState) -> Self {
//         Self {
//             encoder: state.constants.device.create_command_encoder(
//                 &(wgpu::CommandEncoderDescriptor {
//                     label: Some("Render Encoder"),
//                 }),
//             ),
//         }
//     }
// }

pub fn padded_bytes_per_row(output_size: (u32, u32)) -> u32 {
    // Calculate the aligned bytes per row
    let align = COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded_bytes_per_row = output_size.0 * 4;
    let padding = (align - (unpadded_bytes_per_row % align)) % align;
    let padded_bytes_per_row = unpadded_bytes_per_row + padding;

    // Ensure the padded_bytes_per_row is a multiple of 4 (32 bits)
    (padded_bytes_per_row + 3) & !3
}

pub async fn finish_encoder(
    session: &mut RenderSession,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &FrameTarget,
    encoder: wgpu::CommandEncoder,
) -> Result<Option<RenderedFrame>, RenderingError> {
    finish_encoder_timed(session, device, queue, target, encoder)
        .await
        .map(|(frame, _)| frame)
}

pub async fn finish_encoder_timed(
    session: &mut RenderSession,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &FrameTarget,
    encoder: wgpu::CommandEncoder,
) -> Result<(Option<RenderedFrame>, FinishEncoderTimings), RenderingError> {
    let mut timings = FinishEncoderTimings::default();
    let initial_buffer_size =
        (padded_bytes_per_row(target.output_size) * target.output_size.1) as u64;
    let readback = session
        .pipelined_readback
        .get_or_insert_with(|| PipelinedGpuReadback::new(device, initial_buffer_size));

    let wait_start = Instant::now();
    let previous_frame = if let Some(prev) = readback.take_pending() {
        Some(prev.wait(device).await?)
    } else {
        None
    };
    timings.wait_previous_duration = wait_start.elapsed();

    let resize_start = Instant::now();
    readback.perform_resize_if_needed(device);
    timings.resize_duration = resize_start.elapsed();

    let texture = if session.current_is_left {
        &session.textures.0
    } else {
        &session.textures.1
    };

    let submit_start = Instant::now();
    readback.submit_readback(device, queue, texture, target, encoder)?;
    timings.submit_readback_duration = submit_start.elapsed();

    Ok((previous_frame, timings))
}

pub async fn finish_encoder_nv12_pooled(
    session: &mut RenderSession,
    nv12_converter: &mut RgbaToNv12Converter,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &FrameTarget,
    mut encoder: wgpu::CommandEncoder,
    buffer_pool: Option<&mut NV12BufferPool>,
) -> Result<Option<Nv12RenderedFrame>, RenderingError> {
    let width = target.output_size.0;
    let height = target.output_size.1;

    let previous_frame = if let Some(prev) = nv12_converter.take_pending() {
        Some(prev.wait_with_pool(device, buffer_pool).await?)
    } else {
        None
    };

    let texture = if session.current_is_left {
        &session.textures.0
    } else {
        &session.textures.1
    };

    let submitted = nv12_converter
        .submit_conversion(
            device,
            queue,
            &mut encoder,
            texture,
            width,
            height,
            target.frame_number,
            target.frame_rate,
        )
        .await?;

    if submitted {
        queue.submit(std::iter::once(encoder.finish()));
        nv12_converter.after_submit(queue);

        Ok(previous_frame)
    } else if let Some(prev_frame) = previous_frame {
        queue.submit(std::iter::once(encoder.finish()));
        Ok(Some(prev_frame))
    } else {
        let rgba_frame = finish_encoder(session, device, queue, target, encoder).await?;
        Ok(rgba_frame.map(|f| Nv12RenderedFrame {
            data: SharedNv12Buffer::from_arc_vec(f.data),
            width: f.width,
            height: f.height,
            y_stride: f.padded_bytes_per_row,
            frame_number: f.frame_number,
            target_time_ns: f.target_time_ns,
            format: GpuOutputFormat::Rgba,
            #[cfg(target_os = "macos")]
            surface: None,
        }))
    }
}

pub async fn flush_pending_readback(
    session: &mut RenderSession,
    device: &wgpu::Device,
) -> Option<Result<RenderedFrame, RenderingError>> {
    let readback = session.pipelined_readback.as_mut()?;
    if let Some(pending) = readback.take_pending() {
        Some(pending.wait(device).await)
    } else {
        None
    }
}

#[cfg(test)]
mod readback_output_tests {
    use super::active_readback_byte_len;

    #[test]
    fn oversized_readback_buffers_only_include_active_rows() {
        assert_eq!(
            active_readback_byte_len(2_048, 270, 3_594_240),
            Some(552_960)
        );
    }

    #[test]
    fn exact_readback_buffers_include_every_row() {
        assert_eq!(
            active_readback_byte_len(5_120, 702, 3_594_240),
            Some(3_594_240)
        );
    }

    #[test]
    fn undersized_readback_buffers_are_rejected() {
        assert_eq!(active_readback_byte_len(2_048, 270, 552_959), None);
    }

    #[test]
    fn overflowing_readback_dimensions_are_rejected() {
        assert_eq!(active_readback_byte_len(usize::MAX, 2, usize::MAX), None);
    }

    #[test]
    fn pooled_readback_buffers_preserve_grow_shrink_grow_frame_lengths() {
        let buffer_bytes = 3_594_240;
        let frame_lengths = [(5_120, 702), (2_048, 270), (5_120, 702)]
            .into_iter()
            .map(|(stride, height)| active_readback_byte_len(stride, height, buffer_bytes))
            .collect::<Vec<_>>();

        assert_eq!(
            frame_lengths,
            vec![Some(3_594_240), Some(552_960), Some(3_594_240)]
        );
    }

    #[test]
    fn empty_readback_dimensions_do_not_access_the_buffer() {
        assert_eq!(active_readback_byte_len(2_048, 0, 3_594_240), None);
        assert_eq!(active_readback_byte_len(0, 270, 3_594_240), None);
    }
}
