//! Our own NV12 IOSurface pool: a capture buffer is copied into it at once on the GPU
//! (`VTPixelTransferSession`), so the capturer gets its buffer straight back and never waits on the encoder, which reads
//! the copy. The pool is bounded (`preset::pool_frames`: about 0.5 s of frames, at most 64 MiB): when every buffer is
//! in use, the frame is dropped and counted, never waited for and never allocated beyond the bound
//! (`AllocationThreshold`). The pixels never touch the CPU. Cap's ScreenCaptureKit source keeps a pool of its own the
//! same way (`recording/src/sources/screen_capture/macos.rs` @ d327051; design only, no code taken).
//!
//! Memory: `frames` IOSurfaces of `width × height × 1.5` bytes (plus row padding), allocated as needed up to the bound
//! and kept (`MinimumBufferCount`); none per copy on our side (a buffer is a retained CF reference).

use cidre::{arc, cf, cv, vt};

use crate::pipe::PipeError;
use crate::preset::pool_frames;

/// `CVPixelBuffer` attributes for IOSurface-backed `420v` (NV12, video range) at this size: what capture delivers,
/// what the pool holds and what the encoder is told it gets.
pub(super) fn nv12_attrs(width: u32, height: u32) -> Option<arc::R<cf::Dictionary>> {
    let (w, h) = (
        cf::Number::from_i64(width.into()),
        cf::Number::from_i64(height.into()),
    );
    let io_surface = cf::Dictionary::new();
    cf::Dictionary::with_keys_values(
        &[
            cv::pixel_buffer::keys::pixel_format().as_ref(),
            cv::pixel_buffer::keys::width().as_ref(),
            cv::pixel_buffer::keys::height().as_ref(),
            cv::pixel_buffer::keys::io_surf_props().as_ref(),
        ],
        &[
            cv::PixelFormat::_420V.to_cf_number().as_ref(),
            w.as_ref(),
            h.as_ref(),
            io_surface.as_ref(),
        ],
    )
}

pub struct SurfacePool {
    pool: arc::R<cv::PixelBufPool>,
    transfer: arc::R<vt::PixelTransferSession>,
    /// `AllocationThreshold = frames`, passed with every request: past it the pool refuses instead of growing.
    bound: arc::R<cf::Dictionary>,
    frames: usize,
    dropped: u64,
    /// A failed copy is logged once.
    copy_failed: bool,
}

// SAFETY: CVPixelBufferPool is thread-safe, and the transfer session is used by one thread at a time (`&mut self`);
// the pool moves to the capture callback's thread once.
unsafe impl Send for SurfacePool {}

impl SurfacePool {
    /// A pool of `frames` NV12 IOSurfaces at `width × height`.
    pub fn new(width: u32, height: u32, frames: usize) -> Result<Self, PipeError> {
        let os = |what: &str, e: cidre::os::Error| PipeError::Os(format!("{what}: {}", e.0));
        let count = cf::Number::from_i64(frames as i64);
        let pool_attrs = cf::Dictionary::with_keys_values(
            &[cv::pixel_buffer_pool::keys::minimum_buffer_count().as_ref()],
            &[count.as_ref()],
        );
        let bound = cf::Dictionary::with_keys_values(
            &[cv::pixel_buffer_pool::aux_attr_keys::allocation_threashold().as_ref()],
            &[count.as_ref()],
        );
        let (Some(pool_attrs), Some(bound), Some(buf_attrs)) =
            (pool_attrs, bound, nv12_attrs(width, height))
        else {
            return Err(PipeError::Os("the pool's attributes".into()));
        };
        let pool = cv::PixelBufPool::new(Some(&pool_attrs), Some(&buf_attrs))
            .map_err(|e| os("CVPixelBufferPoolCreate", e))?;
        let mut transfer =
            vt::PixelTransferSession::new().map_err(|e| os("VTPixelTransferSessionCreate", e))?;
        transfer
            .set_realtime(true)
            .map_err(|e| os("the transfer's RealTime", e))?;
        Ok(Self {
            pool,
            transfer,
            bound,
            frames,
            dropped: 0,
            copy_failed: false,
        })
    }

    /// The pool for a preset's size and rate: `preset::pool_frames` buffers.
    pub fn for_preset(width: u32, height: u32, fps: u32) -> Result<Self, PipeError> {
        Self::new(width, height, pool_frames(width, height, fps))
    }

    /// `from` copied into one of the pool's buffers on the GPU; None, counted, when every buffer is in use (or the copy
    /// failed). Never blocks.
    pub fn copy(&mut self, from: &cv::PixelBuf) -> Option<arc::R<cv::PixelBuf>> {
        let Ok(to) = self.pool.pixel_buf_with_aux_attrs(Some(&self.bound)) else {
            self.dropped += 1;
            return None;
        };
        if let Err(e) = self.transfer.transfer(from, &to) {
            if !std::mem::replace(&mut self.copy_failed, true) {
                tracing::warn!(
                    status = e.0.get(),
                    "pool: a capture buffer wasn't copied; it is dropped"
                );
            }
            self.dropped += 1;
            return None;
        }
        Some(to)
    }

    /// Frames dropped because the pool was full (or a copy failed): `VideoStats::dropped_pool`.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The pool's bound, in frames.
    pub fn frames(&self) -> usize {
        self.frames
    }
}
