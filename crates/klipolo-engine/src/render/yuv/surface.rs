//! The macOS sink for export frames: a CVPixelBufferPool shaped like VideoToolbox's own (so it encodes the buffers
//! as they are), with each buffer's planes imported into wgpu once.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cidre::{arc, cf, cv, io, mtl};
use tokio::sync::Notify;

use super::Yuv;
use crate::vendor::cap::rendering::iosurface_texture::{
    IOSurfaceTextureCache, import_metal_texture_to_wgpu_with_usage,
};

/// An IOSurface-backed CVPixelBuffer the GPU drew a frame into; VideoToolbox reads it directly. A pool that bounds
/// the frames VideoToolbox holds (ProRes) hands its buffers out with its `InFlight`.
pub struct PixelBuffer(arc::R<cv::PixelBuf>, Option<Arc<InFlight>>);

/// A pool's wake-up as VideoToolbox lets each of its frames go (it output the frame, or dropped it).
#[derive(Default)]
pub struct InFlight {
    let_go: Notify,
}

impl InFlight {
    /// A frame goes to the encoder: the returned callback (`H264Encoder`'s `wrap_videotoolbox_pixel_buffer_then`
    /// runs it when VideoToolbox lets the frame go) wakes the pool.
    pub fn handed(self: &Arc<Self>) -> Box<dyn FnOnce() + Send> {
        let this = self.clone();
        Box::new(move || this.let_go.notify_one())
    }
}

/// While a ProRes pool waits, its buffers' use counts are looked at again this often between wake-ups.
/// VideoToolbox outputs a frame before it lets its IOSurface go (measured: counting output frames left the worker at
/// 1.17 GB, as with no bound), and nothing signals that, so each wake-up and this re-check look at the use counts.
const IN_USE_RECHECK: Duration = Duration::from_millis(10);

// SAFETY: CoreVideo buffers are reference counted atomically; a frame is written by the GPU before it is handed
// on, and only read afterwards.
unsafe impl Send for PixelBuffer {}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferFillExtendedPixels(buffer: *mut std::ffi::c_void) -> i32;
}

impl PixelBuffer {
    /// The pool's count of frames VideoToolbox holds, if it keeps one (ProRes).
    pub fn in_flight(&self) -> Option<&Arc<InFlight>> {
        self.1.as_ref()
    }

    /// The `CVPixelBufferRef`, valid while `self` is alive.
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        (self.0.as_ref() as *const cv::PixelBuf as *const std::ffi::c_void).cast_mut()
    }

    /// Repeats the edge pixels into the macroblock padding (the GPU writes only the frame), so the encoder's
    /// last row and column of blocks don't code stale memory. Call once the GPU is done with the frame.
    pub fn fill_padding(&mut self) {
        // SAFETY: the buffer is ours until it goes to the encoder; locking guards the CPU write.
        unsafe {
            if self
                .0
                .lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT)
                .result()
                .is_ok()
            {
                CVPixelBufferFillExtendedPixels(self.as_ptr());
                self.0
                    .unlock_lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT);
            }
        }
    }
}

struct Slot {
    surface: arc::R<io::Surf>,
    y: wgpu::Texture,
    uv: wgpu::Texture,
    /// When it was last handed out (`SurfacePool::tick`).
    used: u64,
}

/// A CVPixelBufferPool of one size and layout, with each buffer's planes imported into wgpu once.
pub struct SurfacePool {
    pool: arc::R<cv::PixelBufPool>,
    aux: arc::R<cf::Dictionary>,
    cache: IOSurfaceTextureCache,
    /// Imported buffers, at most `max` (least recently used first out).
    slots: Vec<Slot>,
    max: usize,
    /// How many of its buffers VideoToolbox may still be using (their IOSurfaces in use) when the next is handed
    /// out: looked at when this is below `max` (ProRes, which has `in_flight`), else not at all.
    max_busy: usize,
    in_flight: Option<Arc<InFlight>>,
    tick: u64,
    yuv: Yuv,
    size: (u32, u32),
    /// Distinct buffers the pool made (logged when the export ends).
    pub peak: usize,
}

// SAFETY: the CF objects are reference counted atomically; one export loop owns the pool.
unsafe impl Send for SurfacePool {}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferExtendedPixelsRightKey: &'static cf::String;
    static kCVPixelBufferBytesPerRowAlignmentKey: &'static cf::String;
}

fn pixel_format(yuv: Yuv) -> cv::PixelFormat {
    match yuv {
        Yuv::Nv12 => cv::PixelFormat::_420V,
        Yuv::P010 => cv::PixelFormat::_420_YP_CB_CR_10_BI_PLANAR_VIDEO_RANGE,
        Yuv::P210 => cv::PixelFormat::_422_YP_CB_CR_10_BI_PLANAR_VIDEO_RANGE,
    }
}

impl SurfacePool {
    /// Buffers of `size` pixels with room for `padded` (the encoder's macroblocks; `PixelBuffer::fill_padding`
    /// repeats the edge pixels into it). At most `max_buffers` exist at once (the GPU's, the encoder queue's and
    /// VideoToolbox's own).
    pub fn new(
        device: &wgpu::Device,
        yuv: Yuv,
        (width, height): (u32, u32),
        padded: (u32, u32),
        max_buffers: usize,
        max_busy: usize,
    ) -> Result<Self, String> {
        let cache =
            IOSurfaceTextureCache::new(device).ok_or("The graphics card isn't a Metal device.")?;
        // Buffers never age out (CoreVideo's default drops ones idle for a second): the pool keeps reusing the
        // same few, so their imported planes stay valid.
        let (min, age) = (cf::Number::from_usize(3), cf::Number::from_i32(0));
        let pool_attributes = cf::Dictionary::with_keys_values(
            &[
                cv::pixel_buffer_pool::keys::minimum_buffer_count().as_ref(),
                cv::pixel_buffer_pool::keys::maximum_buffer_age().as_ref(),
            ],
            &[min.as_ref(), age.as_ref()],
        )
        .ok_or("pixel buffer pool attributes")?;
        let (w, h) = (
            cf::Number::from_usize(width as usize),
            cf::Number::from_usize(height as usize),
        );
        // What VideoToolbox's own pool asks for (its pixel buffer pool attributes), so it encodes our buffers as
        // they are instead of copying each into one of its own (`VTPixelTransferSession`): the padding to
        // whole macroblocks (1080 → 1088), and for HEVC 64-byte rows and 4 KiB planes.
        let right = cf::Number::from_usize((padded.0 - width) as usize);
        let bottom = cf::Number::from_usize((padded.1 - height) as usize);
        let (row_align, plane_align) = (cf::Number::from_usize(64), cf::Number::from_usize(4096));
        let io_props = cf::Dictionary::new();
        let format = pixel_format(yuv).to_cf_number();
        // SAFETY: CoreVideo's constant keys, valid for the process's lifetime.
        let (right_key, row_key) = unsafe {
            (
                kCVPixelBufferExtendedPixelsRightKey,
                kCVPixelBufferBytesPerRowAlignmentKey,
            )
        };
        let keys: [&cf::Type; 9] = [
            cv::pixel_buffer::keys::pixel_format().as_ref(),
            cv::pixel_buffer::keys::width().as_ref(),
            cv::pixel_buffer::keys::height().as_ref(),
            cv::pixel_buffer::keys::io_surf_props().as_ref(),
            cv::pixel_buffer::keys::metal_compatibility().as_ref(),
            right_key.as_ref(),
            cv::pixel_buffer::keys::extended_pixels_bottom().as_ref(),
            row_key.as_ref(),
            cv::pixel_buffer::keys::plane_aligment().as_ref(),
        ];
        let values: [&cf::Type; 9] = [
            format.as_ref(),
            w.as_ref(),
            h.as_ref(),
            io_props.as_ref(),
            cf::Boolean::value_true().as_ref(),
            right.as_ref(),
            bottom.as_ref(),
            row_align.as_ref(),
            plane_align.as_ref(),
        ];
        let buffer_attributes =
            cf::Dictionary::with_keys_values(&keys, &values).ok_or("pixel buffer attributes")?;
        let pool = cv::PixelBufPool::new(
            Some(pool_attributes.as_ref()),
            Some(buffer_attributes.as_ref()),
        )
        .map_err(|e| format!("pixel buffer pool: {e}"))?;
        let threshold = cf::Number::from_usize(max_buffers);
        let aux = cf::Dictionary::with_keys_values(
            &[cv::pixel_buffer_pool::aux_attr_keys::allocation_threashold().as_ref()],
            &[threshold.as_ref()],
        )
        .ok_or("pixel buffer allocation limit")?;
        Ok(Self {
            pool,
            aux,
            cache,
            slots: Vec::new(),
            max: max_buffers,
            max_busy,
            in_flight: (max_busy < max_buffers).then(Arc::default),
            tick: 0,
            yuv,
            size: (width, height),
            peak: 0,
        })
    }

    /// The next free buffer and its imported Y and UV plane textures. Waits while every buffer is still with
    /// the encoder (VideoToolbox lets one go as it finishes a frame), or while VideoToolbox still uses `max_busy`
    /// of them (looked at again as it lets a frame go, and every `IN_USE_RECHECK`); gives up after 10 s.
    pub async fn next(
        &mut self,
        device: &wgpu::Device,
    ) -> Result<(PixelBuffer, &wgpu::Texture, &wgpu::Texture), String> {
        let started = Instant::now();
        if let Some(in_flight) = &self.in_flight {
            let give_up = tokio::time::Instant::now() + Duration::from_secs(10);
            while self.slots.iter().filter(|s| s.surface.is_in_use()).count() >= self.max_busy {
                let now = tokio::time::Instant::now();
                if now >= give_up {
                    return Err("the encoder kept every frame buffer".into());
                }
                // A frame let go between the check and the wait leaves a permit (`notify_one`): never missed.
                let _ = tokio::time::timeout_at(
                    give_up.min(now + IN_USE_RECHECK),
                    in_flight.let_go.notified(),
                )
                .await;
            }
        }
        let buffer = loop {
            match self.pool.pixel_buf_with_aux_attrs(Some(&self.aux)) {
                Ok(buffer) => break buffer,
                Err(e)
                    if e == cv::err::WOULD_EXCEED_ALLOCATION_THRESHOLD
                        && started.elapsed() < Duration::from_secs(10) =>
                {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                Err(e) => return Err(format!("no free frame buffer: {e}")),
            }
        };
        let surface = buffer
            .io_surf()
            .ok_or("the frame buffer has no IOSurface")?;
        self.tick += 1;
        let index = match self
            .slots
            .iter()
            .position(|s| std::ptr::eq(s.surface.as_ref(), surface))
        {
            Some(index) => index,
            None => {
                let slot = self.import(device, surface)?;
                self.peak += 1;
                if self.slots.len() < self.max {
                    self.slots.push(slot);
                    self.slots.len() - 1
                } else {
                    // A buffer the pool made in place of one it let go: forget the oldest import.
                    let oldest = (0..self.slots.len())
                        .min_by_key(|&i| self.slots[i].used)
                        .unwrap_or(0);
                    self.slots[oldest] = slot;
                    oldest
                }
            }
        };
        self.slots[index].used = self.tick;
        let slot = &self.slots[index];
        Ok((
            PixelBuffer(buffer, self.in_flight.clone()),
            &slot.y,
            &slot.uv,
        ))
    }

    fn import(&self, device: &wgpu::Device, surface: &io::Surf) -> Result<Slot, String> {
        let (width, height) = self.size;
        let (y_mtl, uv_mtl, y_wgpu, uv_wgpu) = match self.yuv {
            Yuv::Nv12 => (
                mtl::PixelFormat::R8UNorm,
                mtl::PixelFormat::Rg8UNorm,
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureFormat::Rg8Unorm,
            ),
            // The planes hold raw 16-bit codes: copied as integers, never sampled.
            Yuv::P010 | Yuv::P210 => (
                mtl::PixelFormat::R16UInt,
                mtl::PixelFormat::Rg16UInt,
                wgpu::TextureFormat::R16Uint,
                wgpu::TextureFormat::Rg16Uint,
            ),
        };
        let uv_size = (width.div_ceil(2), super::Yuv::chroma_rows(self.yuv, height));
        let usage = mtl::TextureUsage::SHADER_READ;
        let y = self
            .cache
            .create_surface_texture(surface, (width, height), y_mtl, 0, usage)
            .map_err(|e| e.to_string())?;
        let uv = self
            .cache
            .create_surface_texture(surface, uv_size, uv_mtl, 1, usage)
            .map_err(|e| e.to_string())?;
        let import = |texture: &mtl::Texture, format, (w, h): (u32, u32), label| {
            import_metal_texture_to_wgpu_with_usage(
                device,
                texture,
                format,
                w,
                h,
                wgpu::TextureUsages::COPY_DST,
                Some(label),
            )
            .map_err(|e| e.to_string())
        };
        Ok(Slot {
            used: 0,
            surface: surface.retained(),
            y: import(&y, y_wgpu, (width, height), "Export plane Y")?,
            uv: import(&uv, uv_wgpu, uv_size, "Export plane UV")?,
        })
    }
}
