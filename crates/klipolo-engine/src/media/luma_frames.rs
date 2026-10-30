//! Every frame of a file's video as a small grey picture with its time, for analysis that compares frames one by one
//! (the scene log's cut finder in `klipolo-analysis`). The activity pass's pictures (`platform::activity`) come one per
//! 100 ms tick; these come one per frame, so a change can be dated to the frame it happened on.
//!
//! **Decode:** software, on the caller's thread with FFmpeg's own frame threads, every stream but the video discarded at
//! the demuxer. Meant for small files (the scene log's 360p copies); a damaged packet is passed over, as the thumbnail
//! scan does.
//!
//! **Pictures:** each frame scaled to the size asked for with FFmpeg's fast bilinear scaler, of its Y plane alone where
//! it is 8-bit YUV (else of the whole frame to grey). Limited-range luma isn't widened: frames are compared within one
//! file.
//!
//! **Times** are on the file's clock, in microseconds: the video stream's first frame is 0, each frame
//! at its best-effort timestamp. Frames come in presentation order.

use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg::codec::threading;
use ffmpeg::format::Pixel;
use ffmpeg::format::stream::Disposition;
use ffmpeg::software::scaling;
use klipolo_model::time::Us;

use crate::media::scan::{next_packet, set_discards};
use crate::render::probe::stream_start;

/// Why a file's frames couldn't all be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LumaError {
    /// The file has no video stream (sound only, or a still's attached picture).
    #[error("the file has no video")]
    NoVideo,
    /// Stopped by its `cancel` (or by `see` returning false).
    #[error("stopped")]
    Stopped,
    /// Opening, reading or decoding failed.
    #[error("{0}")]
    Failed(String),
}

/// Calls `see` with every frame of `path`'s video, in order: its time (see the module notes) and its grey picture of
/// `width` × `height`, row by row. `see` returns false to stop. Returns how many frames it saw. `cancel` is checked
/// between packets.
pub fn each_frame(
    path: &Path,
    (width, height): (u32, u32),
    cancel: &AtomicBool,
    see: &mut dyn FnMut(Us, &[u8]) -> bool,
) -> Result<u64, LumaError> {
    if width == 0 || height == 0 {
        return Err(LumaError::Failed("a picture needs a size".into()));
    }
    let _ = ffmpeg::init();
    let failed = |what: &str, e: ffmpeg::Error| LumaError::Failed(format!("{what}: {e}"));
    let mut input = crate::media::open::input(path).map_err(|e| failed("open", e))?;
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .filter(|s| !s.disposition().contains(Disposition::ATTACHED_PIC))
        .ok_or(LumaError::NoVideo)?;
    let (index, tb, params, start) = (
        stream.index(),
        stream.time_base(),
        stream.parameters(),
        stream_start(&stream),
    );
    let mut context =
        ffmpeg::codec::Context::from_parameters(params).map_err(|e| failed("decoder", e))?;
    context.set_threading(threading::Config::kind(threading::Type::Frame));
    let mut decoder = context
        .decoder()
        .video()
        .map_err(|e| failed("decoder", e))?;
    decoder.set_packet_time_base(tb);
    set_discards(&mut input, Some(index), None);

    let seconds = f64::from(tb);
    let mut picture = Grey::new(width, height);
    let mut frame = ffmpeg::frame::Video::empty();
    let mut seen = 0u64;
    // Hands out every frame the decoder has ready; false once `see` says stop.
    let mut drain = |decoder: &mut ffmpeg::decoder::Video| -> Result<bool, LumaError> {
        while decoder.receive_frame(&mut frame).is_ok() {
            let Some(pts) = frame.timestamp().or_else(|| frame.pts()) else {
                continue;
            };
            let at = ((pts - start) as f64 * seconds * 1e6).round() as Us;
            picture.of(&frame).map_err(LumaError::Failed)?;
            seen += 1;
            if !see(at, &picture.pixels) {
                return Ok(false);
            }
        }
        Ok(true)
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(LumaError::Stopped);
        }
        match next_packet(&mut input) {
            Ok(Some((stream, packet))) if stream == index => {
                // A damaged packet: the decoder resyncs at a later one.
                let _ = decoder.send_packet(&packet);
                if !drain(&mut decoder)? {
                    return Err(LumaError::Stopped);
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => return Err(failed("read", e)),
        }
    }
    let _ = decoder.send_eof();
    if !drain(&mut decoder)? {
        return Err(LumaError::Stopped);
    }
    Ok(seen)
}

/// The grey picture of the frame being handed out, and the scaler that makes it.
struct Grey {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    /// The scaler and what it was made for (source format as read, width, height).
    scaler: Option<(scaling::Context, (Pixel, u32, u32))>,
}

impl Grey {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
            scaler: None,
        }
    }

    fn of(&mut self, frame: &ffmpeg::frame::Video) -> Result<(), String> {
        let (format, w, h) = (frame.format(), frame.width(), frame.height());
        let y_only = is_8bit_yuv(format);
        let key = (if y_only { Pixel::GRAY8 } else { format }, w, h);
        if self.scaler.as_ref().is_none_or(|(_, made)| *made != key) {
            let context = scaling::Context::get(
                key.0,
                w,
                h,
                Pixel::GRAY8,
                self.width,
                self.height,
                scaling::Flags::FAST_BILINEAR,
            )
            .map_err(|e| format!("scaler: {e}"))?;
            self.scaler = Some((context, key));
        }
        let (context, _) = self.scaler.as_mut().expect("made above");
        // SAFETY: the frame's planes and strides as FFmpeg filled them (only the first, read as grey, for a Y plane); the
        // output is our own buffer of `width` × `height` with its stride.
        let lines = unsafe {
            let f = &*frame.as_ptr();
            let mut data = [null::<u8>(); 4];
            let mut stride = [0; 4];
            let planes = if y_only { 1 } else { 4 };
            let from = f.data.iter().zip(&f.linesize).take(planes);
            for ((d, s), (&p, &line)) in data.iter_mut().zip(stride.iter_mut()).zip(from) {
                *d = p.cast_const();
                *s = line;
            }
            let dst = [self.pixels.as_mut_ptr(), null_mut(), null_mut(), null_mut()];
            let dst_stride = [self.width as i32, 0, 0, 0];
            ffmpeg::ffi::sws_scale(
                context.as_mut_ptr(),
                data.as_ptr(),
                stride.as_ptr(),
                0,
                h as i32,
                dst.as_ptr(),
                dst_stride.as_ptr(),
            )
        };
        if lines < self.height as i32 {
            return Err(format!("scaling gave {lines} lines"));
        }
        Ok(())
    }
}

/// 8-bit YUV, planar or semi-planar: its first plane is the grey picture.
fn is_8bit_yuv(format: Pixel) -> bool {
    use Pixel as P;
    matches!(
        format,
        P::YUV420P
            | P::YUVJ420P
            | P::YUV422P
            | P::YUVJ422P
            | P::YUV444P
            | P::YUVJ444P
            | P::NV12
            | P::NV21
            | P::YUVA420P
            | P::GRAY8
    )
}
