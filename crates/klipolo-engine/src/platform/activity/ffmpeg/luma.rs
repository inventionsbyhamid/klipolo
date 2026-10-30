//! A decoded frame as the activity pass's 128 × 72 luma (`Picture`), and the platform's hardware decoder (`Hardware`).
//!
//! **Scaling:** `sws_scale` with `SWS_FAST_BILINEAR`, of the Y plane alone where the frame is planar or semi-planar YUV
//! (read as grey of its depth), else of the whole frame to grey. Into an aligned buffer the scaler writes fast, then
//! copied out (9 KB). Limited-range luma isn't widened: changes are compared within one file.
//!
//! **Hardware:** VideoToolbox on macOS, D3D11VA on Windows, VA-API on Linux, used when the
//! codec has a configuration for it and the device opens (a container without a GPU decodes in software). The format
//! callback looks for the platform's one hardware format, so it keeps no thread state (the vendored helper keeps it in
//! a thread-local, which a decoder moved between threads would lose). A hardware frame is downloaded into a reused
//! frame only when a tick shows it.

use std::ptr::{null, null_mut};

use ffmpeg::ffi::{
    AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX, AVBufferRef, AVCodecContext, AVHWDeviceType,
    AVPixelFormat, av_buffer_ref, av_buffer_unref, av_hwdevice_ctx_create,
    av_hwframe_transfer_data, avcodec_default_get_format, avcodec_get_hw_config, sws_scale,
};
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;

use super::LUMA_BYTES;

const W: u32 = 128;
const H: u32 = 72;

/// The platform's hardware decoder and the frame format it decodes into.
const DEVICE: Option<(AVHWDeviceType, AVPixelFormat)> = if cfg!(target_os = "macos") {
    Some((
        AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
        AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX,
    ))
} else if cfg!(windows) {
    Some((
        AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
        AVPixelFormat::AV_PIX_FMT_D3D11,
    ))
} else if cfg!(target_os = "linux") {
    Some((
        AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
        AVPixelFormat::AV_PIX_FMT_VAAPI,
    ))
} else {
    None
};

/// The decoder asks which format to decode into: the platform's hardware one when it is offered, else FFmpeg's default
/// (a software format).
unsafe extern "C" fn hardware_format(
    ctx: *mut AVCodecContext,
    offered: *const AVPixelFormat,
) -> AVPixelFormat {
    // SAFETY: FFmpeg passes its context and a list ended by AV_PIX_FMT_NONE.
    unsafe {
        let mut p = offered;
        while *p != AVPixelFormat::AV_PIX_FMT_NONE {
            if DEVICE.is_some_and(|(_, format)| *p == format) {
                return *p;
            }
            p = p.add(1);
        }
        avcodec_default_get_format(ctx, offered)
    }
}

/// The hardware device a decoder uses; released when dropped.
pub(crate) struct Hardware(*mut AVBufferRef);

impl Hardware {
    /// Points `decoder` at the platform's hardware decoder when its codec has one and the device opens.
    pub(crate) fn attach(decoder: &mut ffmpeg::decoder::Video) -> Option<Hardware> {
        let (kind, _) = DEVICE?;
        let codec = decoder.codec()?;
        // SAFETY: the codec's static configuration table, read until its end (null); then a device created and handed
        // to the open context before its first packet, as FFmpeg's hw_decode example does.
        unsafe {
            let mut i = 0;
            loop {
                let config = avcodec_get_hw_config(codec.as_ptr(), i);
                if config.is_null() {
                    return None;
                }
                let device_ctx =
                    (*config).methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0;
                if device_ctx && (*config).device_type == kind {
                    break;
                }
                i += 1;
            }
            let mut device = null_mut();
            if av_hwdevice_ctx_create(&mut device, kind, null(), null_mut(), 0) < 0 {
                return None;
            }
            let ctx = decoder.as_mut_ptr();
            (*ctx).hw_device_ctx = av_buffer_ref(device);
            (*ctx).get_format = Some(hardware_format);
            // The two frames the pass holds (on screen, and the next) come from the decoder's surface pool.
            (*ctx).extra_hw_frames = 2;
            Some(Hardware(device))
        }
    }
}

impl Drop for Hardware {
    fn drop(&mut self) {
        // SAFETY: our own reference, made by `av_hwdevice_ctx_create`.
        unsafe { av_buffer_unref(&mut self.0) };
    }
}

/// An aligned 128 × 72 picture, as SIMD scalers like it.
#[repr(C, align(64))]
struct Aligned([u8; LUMA_BYTES]);

/// The frame on screen as 128 × 72 luma, made once per frame however many ticks show it.
pub(super) struct Picture {
    luma: Box<Aligned>,
    ok: bool,
    /// The scaler and what it was made for (source format as read, width, height).
    scaler: Option<(scaling::Context, (Pixel, u32, u32))>,
    /// A hardware frame's CPU copy, reused.
    download: ffmpeg::frame::Video,
}

impl Default for Picture {
    fn default() -> Self {
        Self {
            luma: Box::new(Aligned([0; LUMA_BYTES])),
            ok: false,
            scaler: None,
            download: ffmpeg::frame::Video::empty(),
        }
    }
}

impl Picture {
    /// Another frame is on screen.
    pub(super) fn stale(&mut self) {
        self.ok = false;
    }

    pub(super) fn luma(&self) -> &[u8; LUMA_BYTES] {
        &self.luma.0
    }

    /// Makes the picture of `frame` (the frame on screen), unless it is made already.
    pub(super) fn of(&mut self, frame: &ffmpeg::frame::Video) -> Result<(), String> {
        if self.ok {
            return Ok(());
        }
        // SAFETY: a plain field of a live frame.
        let hardware = unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() };
        let src = if hardware {
            // SAFETY: both frames are live; the download frame is reused (FFmpeg allocates it on the first transfer and
            // writes into it after), and made afresh when a transfer into it fails (the stream changed size).
            unsafe {
                if av_hwframe_transfer_data(self.download.as_mut_ptr(), frame.as_ptr(), 0) < 0 {
                    self.download = ffmpeg::frame::Video::empty();
                    if av_hwframe_transfer_data(self.download.as_mut_ptr(), frame.as_ptr(), 0) < 0 {
                        return Err("a hardware frame couldn't be read".into());
                    }
                }
            }
            &self.download
        } else {
            frame
        };
        scale(&mut self.scaler, src, &mut self.luma.0)?;
        self.ok = true;
        Ok(())
    }
}

/// The grey format of a YUV frame's Y plane (None: scale the whole frame to grey).
fn y_plane(format: Pixel) -> Option<Pixel> {
    // Not a glob import: `Pixel::None` would hide `Option::None`.
    use Pixel as P;
    match format {
        P::YUV420P
        | P::YUVJ420P
        | P::YUV422P
        | P::YUVJ422P
        | P::YUV444P
        | P::YUVJ444P
        | P::NV12
        | P::NV21
        | P::YUVA420P
        | P::GRAY8 => Some(P::GRAY8),
        P::YUV420P10LE | P::YUV422P10LE | P::YUV444P10LE => Some(P::GRAY10LE),
        // 10 or 16 bits in the high bits of each 16-bit sample.
        P::P010LE | P::P016LE => Some(P::GRAY16LE),
        _ => None,
    }
}

fn scale(
    scaler: &mut Option<(scaling::Context, (Pixel, u32, u32))>,
    frame: &ffmpeg::frame::Video,
    out: &mut [u8; LUMA_BYTES],
) -> Result<(), String> {
    let (format, w, h) = (frame.format(), frame.width(), frame.height());
    let plane = y_plane(format);
    let key = (plane.unwrap_or(format), w, h);
    if scaler.as_ref().is_none_or(|(_, made)| *made != key) {
        let context = scaling::Context::get(
            key.0,
            w,
            h,
            Pixel::GRAY8,
            W,
            H,
            scaling::Flags::FAST_BILINEAR,
        )
        .map_err(|e| format!("scaler: {e}"))?;
        *scaler = Some((context, key));
    }
    let (context, _) = scaler.as_mut().expect("made above");
    // SAFETY: the frame's planes and strides as FFmpeg filled them (only the first, read as grey, for a Y plane); the
    // output is our own 128 × 72 buffer with its stride.
    let lines = unsafe {
        let f = &*frame.as_ptr();
        let mut data = [null::<u8>(); 4];
        let mut stride = [0; 4];
        let planes = if plane.is_some() { 1 } else { 4 };
        let from = f.data.iter().zip(&f.linesize).take(planes);
        for ((d, s), (&p, &line)) in data.iter_mut().zip(stride.iter_mut()).zip(from) {
            *d = p.cast_const();
            *s = line;
        }
        let dst = [out.as_mut_ptr(), null_mut(), null_mut(), null_mut()];
        let dst_stride = [W as i32, 0, 0, 0];
        sws_scale(
            context.as_mut_ptr(),
            data.as_ptr(),
            stride.as_ptr(),
            0,
            h as i32,
            dst.as_ptr(),
            dst_stride.as_ptr(),
        )
    };
    if lines < H as i32 {
        return Err(format!("scaling gave {lines} lines"));
    }
    Ok(())
}
