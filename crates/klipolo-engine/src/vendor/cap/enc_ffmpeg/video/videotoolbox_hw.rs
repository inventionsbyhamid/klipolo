// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/enc-ffmpeg/src/video/videotoolbox_hw.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: the frames context takes the software format of the wrapped buffers (NV12, P010 or P210) instead of always NV12; the unit test removed; `wrap_pixel_buffer_then` calls back once the encoder lets the wrapped buffer go (`Released`), and `wrap_pixel_buffer` is it with none.

//! VideoToolbox hardware-frame input for ffmpeg encoders.
//!
//! ScreenCaptureKit and AVFoundation vend IOSurface-backed NV12
//! CVPixelBuffers; feeding them to `h264_videotoolbox` as software `AVFrame`s
//! costs a full-frame lock + memcpy per frame (and VideoToolbox copies the
//! planes again internally). Wrapping the CVPixelBuffer as an
//! `AV_PIX_FMT_VIDEOTOOLBOX` frame lets the encoder read the IOSurface
//! directly — zero CPU pixel work.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use ffmpeg::sys::{
    AVBufferRef, AVCodecContext, AVHWDeviceType, AVHWFramesContext, AVPixelFormat,
    av_buffer_create, av_buffer_ref, av_buffer_unref, av_hwdevice_ctx_create, av_hwframe_ctx_alloc,
    av_hwframe_ctx_init,
};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

#[derive(thiserror::Error, Debug)]
pub enum VideoToolboxHwError {
    #[error("Failed to create VideoToolbox hardware device context")]
    DeviceCreation,
    #[error("Failed to allocate VideoToolbox hardware frames context")]
    FramesAllocation,
    #[error("Failed to initialize VideoToolbox hardware frames context: {0}")]
    FramesInit(i32),
    #[error("Failed to wrap CVPixelBuffer into an AVFrame")]
    FrameWrap,
}

/// An `AVHWFramesContext` for wrapping externally-owned CVPixelBuffers as
/// `AV_PIX_FMT_VIDEOTOOLBOX` frames. The context never allocates buffers of
/// its own; every frame references a caller-provided CVPixelBuffer.
pub struct VideoToolboxHwFrames {
    device_ctx: *mut AVBufferRef,
    frames_ctx: *mut AVBufferRef,
    width: u32,
    height: u32,
}

// SAFETY: AVBufferRef reference counting is thread-safe, the wrapped contexts
// carry no thread affinity, and every method either takes ownership or only
// performs atomic refcount operations.
unsafe impl Send for VideoToolboxHwFrames {}
unsafe impl Sync for VideoToolboxHwFrames {}

/// Klipolo: called once the encoder lets a wrapped buffer go (FFmpeg's VideoToolbox encoder keeps a reference to the
/// frame's buffer until VideoToolbox outputs that frame, or drops it).
pub type Released = Box<dyn FnOnce() + Send>;

/// `opaque`: a `Box<Released>` from `wrap_pixel_buffer_then`, or null.
unsafe extern "C" fn release_pixel_buffer(opaque: *mut c_void, data: *mut u8) {
    unsafe { CFRelease(data as *const c_void) };
    if !opaque.is_null() {
        // SAFETY: made by `Box::into_raw` in `wrap_pixel_buffer_then`; FFmpeg calls this once per buffer.
        let released = unsafe { Box::from_raw(opaque.cast::<Released>()) };
        released();
    }
}

impl VideoToolboxHwFrames {
    pub fn new(
        width: u32,
        height: u32,
        sw_format: ffmpeg::format::Pixel,
    ) -> Result<Self, VideoToolboxHwError> {
        unsafe {
            let mut device_ctx: *mut AVBufferRef = null_mut();
            if av_hwdevice_ctx_create(
                &mut device_ctx,
                AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                null(),
                null_mut(),
                0,
            ) < 0
            {
                return Err(VideoToolboxHwError::DeviceCreation);
            }

            let frames_ctx = av_hwframe_ctx_alloc(device_ctx);
            if frames_ctx.is_null() {
                let mut device_ctx = device_ctx;
                av_buffer_unref(&mut device_ctx);
                return Err(VideoToolboxHwError::FramesAllocation);
            }

            let ctx = (*frames_ctx).data as *mut AVHWFramesContext;
            (*ctx).format = AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;
            (*ctx).sw_format = sw_format.into();
            (*ctx).width = width as i32;
            (*ctx).height = height as i32;

            let init_result = av_hwframe_ctx_init(frames_ctx);
            if init_result < 0 {
                let mut frames_ctx = frames_ctx;
                av_buffer_unref(&mut frames_ctx);
                let mut device_ctx = device_ctx;
                av_buffer_unref(&mut device_ctx);
                return Err(VideoToolboxHwError::FramesInit(init_result));
            }

            Ok(Self {
                device_ctx,
                frames_ctx,
                width,
                height,
            })
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Attaches the frames context to an encoder that will be opened with
    /// `AV_PIX_FMT_VIDEOTOOLBOX`. Must run before `avcodec_open2`.
    ///
    /// # Safety
    /// `encoder_ctx` must be a valid, not-yet-opened `AVCodecContext`.
    pub(crate) unsafe fn attach_to_encoder(&self, encoder_ctx: *mut AVCodecContext) {
        unsafe {
            (*encoder_ctx).hw_frames_ctx = av_buffer_ref(self.frames_ctx);
        }
    }

    /// Wraps a retained CVPixelBuffer as an `AV_PIX_FMT_VIDEOTOOLBOX` frame.
    /// The buffer is CFRetained for the frame's lifetime and released when the
    /// frame's backing `AVBufferRef` drops.
    ///
    /// # Safety
    /// `pixel_buffer` must be a valid `CVPixelBufferRef` whose dimensions
    /// match this context and whose pixel format is this context's software
    /// format (`420v` for NV12, `x420` for P010, `x422` for P210).
    pub unsafe fn wrap_pixel_buffer(
        &self,
        pixel_buffer: *mut c_void,
    ) -> Result<ffmpeg::frame::Video, VideoToolboxHwError> {
        unsafe { self.wrap_pixel_buffer_then(pixel_buffer, None) }
    }

    /// Klipolo: `wrap_pixel_buffer`, calling `released` once the buffer is let go (by the frame and by the encoder,
    /// whichever holds it last), also when wrapping fails.
    ///
    /// # Safety
    /// As `wrap_pixel_buffer`.
    pub unsafe fn wrap_pixel_buffer_then(
        &self,
        pixel_buffer: *mut c_void,
        released: Option<Released>,
    ) -> Result<ffmpeg::frame::Video, VideoToolboxHwError> {
        if pixel_buffer.is_null() {
            if let Some(released) = released {
                released();
            }
            return Err(VideoToolboxHwError::FrameWrap);
        }
        let opaque = released.map_or(null_mut(), |r| Box::into_raw(Box::new(r)).cast::<c_void>());
        let mut frame = ffmpeg::frame::Video::empty();
        unsafe {
            let ptr = frame.as_mut_ptr();
            let retained = CFRetain(pixel_buffer);
            let buf = av_buffer_create(
                retained as *mut u8,
                size_of::<*mut c_void>(),
                Some(release_pixel_buffer),
                opaque,
                0,
            );
            if buf.is_null() {
                // As the buffer's own release would: the pixel buffer, then the callback.
                release_pixel_buffer(opaque, retained as *mut u8);
                return Err(VideoToolboxHwError::FrameWrap);
            }
            (*ptr).buf[0] = buf;
            (*ptr).data[3] = retained as *mut u8;
            (*ptr).format = AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX as i32;
            (*ptr).width = self.width as i32;
            (*ptr).height = self.height as i32;
            (*ptr).hw_frames_ctx = av_buffer_ref(self.frames_ctx);
            if (*ptr).hw_frames_ctx.is_null() {
                return Err(VideoToolboxHwError::FrameWrap);
            }
        }
        Ok(frame)
    }
}

impl Drop for VideoToolboxHwFrames {
    fn drop(&mut self) {
        unsafe {
            av_buffer_unref(&mut self.frames_ctx);
            av_buffer_unref(&mut self.device_ctx);
        }
    }
}
