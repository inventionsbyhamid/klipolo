// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/decoder/mod.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: Windows (media_foundation module, MediaFoundation decoder type, SendableD3D11Texture, D3D11 frame constructors/accessors) and Linux (CUDA NV12 frames) code removed; managed/segmented decoding removed (ManagedVideo*, CancelManagedReadiness, spawn_managed_decoder, managed_worker_tests, the path.is_dir() segmented branch in spawn_decoder); SCREEN_/CAMERA_MAX_FALLBACK_DISTANCE copied in from Cap rendering/src/lib.rs:352-353 (made pub); DecoderTuning added and passed from spawn_decoder (new last parameter) to the AVAssetReader decoder; DecoderTuning also carries `max_output`, the box the AVAssetReader decodes oversize sources into; off macOS spawn_decoder uses the FFmpeg decoder (spawn_ffmpeg_decoder, no longer macOS-only), since the removed Windows/Linux arms had left it without a body there; the decoder start-up timeout errors name the file, never its folder (`crate::log_name`). `DecodedFrame` carries the time of the frame it holds (`time_s`, set by the AVAssetReader decoder; NaN where a decoder doesn't record it). Frames are numbered on the source's exact frame rate (`fps: f64`, 30000/1001 for 29.97; it was a whole-number u32 grid that drifted against NTSC content), converted by `time_to_frame`/`frame_to_time`. The AVAssetReader handle never starts its software fallback itself (Cap started it in `get_frame_with_timeout` once a request returned nothing, and a synchronous `path.exists()` there ran on the engine's task and hung it when a volume stopped answering): a request the reader doesn't answer returns None, and the caller starts the fallback with `start_fallback` (`can_fall_back` says whether there is one left to start) once it has asked, off its own task, that the file is still there; `software_fallbacks` counts the fallbacks started. A request may carry `Superseded`, which says when a newer one replaced it (a scrub moved on); the AVAssetReader decoder then stops reading toward it, in a GOP of at most a second, and answers with the nearest frame of that GOP it has read (`get_frame_unless`). A request's wait follows `DecoderTuning::wait` (`crate::decode_wait`: Cap's fixed deadline for the preview, or as long as the decoder makes progress for export), each handle shares a `DecodeProgress` with its decoder thread, and `read_frame` tells a stalled request (`Waited::Stalled`) from one the decoder dropped; `start_fallback(at)` starts the software decoder at the frame it is asked for, and FFmpeg decoders start at `spawn_decoder_at`'s time (no frame-0 pre-decode mid-stream); the FFmpeg decoder's start waits under the same policy, with `decode_wait::START_WINDOW` before progress counts, and `start_fallback` sets the export heartbeat's beat meanwhile.

use ::ffmpeg::Rational;
use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Weak, mpsc},
    time::Duration,
};
use tokio::sync::oneshot;
use tracing::info;

use crate::decode_wait::{DecodeBeat, DecodeProgress, DecodeWait, Waited};

#[cfg(target_os = "macos")]
use cidre::{arc::R, cv};
#[cfg(target_os = "macos")]
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "macos")]
mod avassetreader;
#[cfg(all(target_os = "macos", feature = "trial"))]
pub use avassetreader::frames_read;
mod ffmpeg;
mod frame_converter;
#[cfg(target_os = "macos")]
pub mod multi_position;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderType {
    #[cfg(target_os = "macos")]
    AVAssetReader,
    FFmpegHardware,
    FFmpegSoftware,
}

impl DecoderType {
    pub fn is_hardware_accelerated(&self) -> bool {
        match self {
            #[cfg(target_os = "macos")]
            DecoderType::AVAssetReader => true,
            DecoderType::FFmpegHardware => true,
            DecoderType::FFmpegSoftware => false,
        }
    }
}

impl fmt::Display for DecoderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(target_os = "macos")]
            DecoderType::AVAssetReader => write!(f, "AVAssetReader (hardware)"),
            DecoderType::FFmpegHardware => write!(f, "FFmpeg (hardware)"),
            DecoderType::FFmpegSoftware => write!(f, "FFmpeg (software)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecoderStatus {
    pub decoder_type: DecoderType,
    pub video_width: u32,
    pub video_height: u32,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DecoderInitResult {
    pub width: u32,
    pub height: u32,
    pub decoder_type: DecoderType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba,
    Nv12,
    Yuv420p,
}

#[derive(Clone)]
pub struct DecodedFrame {
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    y_stride: u32,
    uv_stride: u32,
    /// Klipolo: the frame's presentation time in seconds (NaN when the decoder doesn't record it).
    time_s: f32,
    #[cfg(target_os = "macos")]
    image_buf_backing: Option<Arc<SendableImageBuf>>,
}

#[cfg(target_os = "macos")]
struct SendableImageBuf {
    image_buf: Mutex<Option<R<cv::ImageBuf>>>,
    raw_data: OnceLock<Vec<u8>>,
}

#[cfg(target_os = "macos")]
// SAFETY: all CVPixelBuffer access is serialized by `image_buf`, and the Core
// Foundation object remains retained for the lifetime of this wrapper.
unsafe impl Send for SendableImageBuf {}
#[cfg(target_os = "macos")]
// SAFETY: see the `Send` implementation above.
unsafe impl Sync for SendableImageBuf {}

#[cfg(target_os = "macos")]
impl SendableImageBuf {
    fn new(image_buf: R<cv::ImageBuf>) -> Self {
        Self {
            image_buf: Mutex::new(Some(image_buf)),
            raw_data: OnceLock::new(),
        }
    }

    fn with_image_buf<T>(&self, f: impl FnOnce(&cv::ImageBuf) -> T) -> Option<T> {
        let image_buf = self
            .image_buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        image_buf.as_deref().map(f)
    }

    fn raw_data(&self) -> &[u8] {
        self.raw_data.get_or_init(|| {
            let mut image_buf_guard = self
                .image_buf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(image_buf) = image_buf_guard.as_mut() else {
                return Vec::new();
            };

            unsafe {
                image_buf
                    .lock_base_addr(cv::pixel_buffer::LockFlags::READ_ONLY)
                    .result()
                    .unwrap();
            }

            let y_size = image_buf.plane_bytes_per_row(0) * image_buf.plane_height(0);
            let uv_size = image_buf.plane_bytes_per_row(1) * image_buf.plane_height(1);
            let y_plane =
                unsafe { std::slice::from_raw_parts(image_buf.plane_base_address(0), y_size) };
            let uv_plane =
                unsafe { std::slice::from_raw_parts(image_buf.plane_base_address(1), uv_size) };

            let mut data = Vec::with_capacity(y_size + uv_size);
            data.extend_from_slice(y_plane);
            data.extend_from_slice(uv_plane);

            unsafe {
                image_buf.unlock_lock_base_addr(cv::pixel_buffer::LockFlags::READ_ONLY);
            }

            // CPU consumers no longer need the IOSurface after its planes have
            // been copied. Releasing it keeps cache accounting at one frame
            // instead of retaining both the CVPixelBuffer and its materialized
            // bytes for the lifetime of the cache entry.
            *image_buf_guard = None;

            data
        })
    }
}

#[derive(Clone)]
pub(crate) struct DecodedFrameStorageIdentity {
    data: Weak<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    y_stride: u32,
    uv_stride: u32,
}

impl DecodedFrameStorageIdentity {
    pub(crate) fn matches(&self, other: &Self) -> bool {
        self.data.ptr_eq(&other.data)
            && self.width == other.width
            && self.height == other.height
            && self.format == other.format
            && self.y_stride == other.y_stride
            && self.uv_stride == other.uv_stride
    }
}

impl fmt::Debug for DecodedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedFrame")
            .field("data_len", &self.data.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("y_stride", &self.y_stride)
            .field("uv_stride", &self.uv_stride)
            .field("time_s", &self.time_s)
            .finish()
    }
}

impl DecodedFrame {
    pub fn new(data: Vec<u8>, width: u32, height: u32) -> Self {
        Self {
            data: Arc::new(data),
            width,
            height,
            format: PixelFormat::Rgba,
            y_stride: width * 4,
            uv_stride: 0,
            time_s: f32::NAN,
            #[cfg(target_os = "macos")]
            image_buf_backing: None,
        }
    }

    pub fn new_with_arc(data: Arc<Vec<u8>>, width: u32, height: u32) -> Self {
        Self {
            data,
            width,
            height,
            format: PixelFormat::Rgba,
            y_stride: width * 4,
            uv_stride: 0,
            time_s: f32::NAN,
            #[cfg(target_os = "macos")]
            image_buf_backing: None,
        }
    }

    pub fn new_nv12(data: Vec<u8>, width: u32, height: u32, y_stride: u32, uv_stride: u32) -> Self {
        Self {
            data: Arc::new(data),
            width,
            height,
            format: PixelFormat::Nv12,
            y_stride,
            uv_stride,
            time_s: f32::NAN,
            #[cfg(target_os = "macos")]
            image_buf_backing: None,
        }
    }

    pub fn new_nv12_with_arc(
        data: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Self {
        Self {
            data,
            width,
            height,
            format: PixelFormat::Nv12,
            y_stride,
            uv_stride,
            time_s: f32::NAN,
            #[cfg(target_os = "macos")]
            image_buf_backing: None,
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn new_nv12_with_image_buf(
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
        image_buf: R<cv::ImageBuf>,
    ) -> Self {
        Self {
            data: Arc::new(Vec::new()),
            width,
            height,
            format: PixelFormat::Nv12,
            y_stride,
            uv_stride,
            time_s: f32::NAN,
            image_buf_backing: Some(Arc::new(SendableImageBuf::new(image_buf))),
        }
    }

    pub fn new_yuv420p(
        data: Vec<u8>,
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Self {
        Self {
            data: Arc::new(data),
            width,
            height,
            format: PixelFormat::Yuv420p,
            y_stride,
            uv_stride,
            time_s: f32::NAN,
            #[cfg(target_os = "macos")]
            image_buf_backing: None,
        }
    }

    pub fn new_yuv420p_with_arc(
        data: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        y_stride: u32,
        uv_stride: u32,
    ) -> Self {
        Self {
            data,
            width,
            height,
            format: PixelFormat::Yuv420p,
            y_stride,
            uv_stride,
            time_s: f32::NAN,
            #[cfg(target_os = "macos")]
            image_buf_backing: None,
        }
    }

    pub fn data(&self) -> &[u8] {
        #[cfg(target_os = "macos")]
        if self.data.is_empty()
            && let Some(backing) = &self.image_buf_backing
        {
            return backing.raw_data();
        }

        &self.data
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn with_image_buf<T>(&self, f: impl FnOnce(&cv::ImageBuf) -> T) -> Option<T> {
        self.image_buf_backing
            .as_ref()
            .and_then(|backing| backing.with_image_buf(f))
    }

    /// Klipolo: the presentation time of this frame in seconds (NaN when the decoder doesn't record it; the FFmpeg
    /// decoder doesn't).
    pub fn time_s(&self) -> f32 {
        self.time_s
    }

    /// Klipolo: this frame, at `time_s`.
    #[cfg(target_os = "macos")]
    pub(crate) fn at_time(mut self, time_s: f32) -> Self {
        self.time_s = time_s;
        self
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn byte_len(&self) -> usize {
        if !self.data.is_empty() {
            return self.data.len();
        }

        let y_bytes = (self.y_stride as usize).saturating_mul(self.height as usize);
        let uv_bytes = (self.uv_stride as usize).saturating_mul((self.height / 2) as usize);

        match self.format {
            PixelFormat::Rgba => (self.width as usize)
                .saturating_mul(self.height as usize)
                .saturating_mul(4),
            PixelFormat::Nv12 => y_bytes.saturating_add(uv_bytes),
            PixelFormat::Yuv420p => y_bytes.saturating_add(uv_bytes.saturating_mul(2)),
        }
    }

    pub(crate) fn storage_identity(&self) -> DecodedFrameStorageIdentity {
        DecodedFrameStorageIdentity {
            data: Arc::downgrade(&self.data),
            width: self.width,
            height: self.height,
            format: self.format,
            y_stride: self.y_stride,
            uv_stride: self.uv_stride,
        }
    }

    pub fn y_plane(&self) -> Option<&[u8]> {
        match self.format {
            PixelFormat::Nv12 | PixelFormat::Yuv420p => {
                let y_size = self
                    .y_stride
                    .checked_mul(self.height)
                    .and_then(|v| usize::try_from(v).ok())?;
                self.data().get(..y_size)
            }
            PixelFormat::Rgba => None,
        }
    }

    pub fn uv_plane(&self) -> Option<&[u8]> {
        match self.format {
            PixelFormat::Nv12 => {
                let y_size = self
                    .y_stride
                    .checked_mul(self.height)
                    .and_then(|v| usize::try_from(v).ok())?;
                self.data().get(y_size..)
            }
            PixelFormat::Yuv420p | PixelFormat::Rgba => None,
        }
    }

    pub fn u_plane(&self) -> Option<&[u8]> {
        match self.format {
            PixelFormat::Yuv420p => {
                let y_size = self
                    .y_stride
                    .checked_mul(self.height)
                    .and_then(|v| usize::try_from(v).ok())?;
                let u_size = self
                    .uv_stride
                    .checked_mul(self.height / 2)
                    .and_then(|v| usize::try_from(v).ok())?;
                let u_end = y_size.checked_add(u_size)?;
                self.data().get(y_size..u_end)
            }
            _ => None,
        }
    }

    pub fn v_plane(&self) -> Option<&[u8]> {
        match self.format {
            PixelFormat::Yuv420p => {
                let y_size = self
                    .y_stride
                    .checked_mul(self.height)
                    .and_then(|v| usize::try_from(v).ok())?;
                let u_size = self
                    .uv_stride
                    .checked_mul(self.height / 2)
                    .and_then(|v| usize::try_from(v).ok())?;
                let v_start = y_size.checked_add(u_size)?;
                self.data().get(v_start..)
            }
            _ => None,
        }
    }

    pub fn y_stride(&self) -> u32 {
        self.y_stride
    }

    pub fn uv_stride(&self) -> u32 {
        self.uv_stride
    }
}

pub enum VideoDecoderMessage {
    /// Klipolo: the last field says when the request was superseded (`Superseded`).
    GetFrame(
        f32,
        u32,
        tokio::sync::oneshot::Sender<DecodedFrame>,
        Option<Superseded>,
    ),
}

/// Klipolo: true once a newer request replaced this one (the preview moved on during a scrub). In a GOP of at
/// most a second (an editing copy's, a short-GOP original's) the AVAssetReader decoder then stops reading toward it and
/// answers with the nearest frame of that GOP it has read; long-GOP sources are always read to their exact frame.
pub type Superseded = Arc<dyn Fn() -> bool + Send + Sync>;

pub fn pts_to_frame(pts: i64, time_base: Rational, fps: f64) -> u32 {
    (fps * ((pts as f64 * time_base.numerator() as f64) / (time_base.denominator() as f64))).round()
        as u32
}

/// The frame of a `fps` grid showing at `time` seconds (`fps` exact: 30000/1001 for 29.97).
pub fn time_to_frame(time: f32, fps: f64) -> u32 {
    (f64::from(time) * fps).floor() as u32
}

/// When frame `frame` of a `fps` grid starts, in seconds.
pub fn frame_to_time(frame: u32, fps: f64) -> f32 {
    (f64::from(frame) / fps) as f32
}

pub const FRAME_CACHE_SIZE: usize = 90;
const DEFAULT_MAX_FALLBACK_DISTANCE: u32 = 90;
// From Cap rendering/src/lib.rs:352-353 (RecordingSegmentDecoders' per-track fallback distances).
pub const SCREEN_MAX_FALLBACK_DISTANCE: u32 = 4;
pub const CAMERA_MAX_FALLBACK_DISTANCE: u32 = 2;

/// Records a pts hole discovered from a decode-order vend jump (frames vend
/// in pts order, so a jump means no samples exist in between). The map stays
/// bounded by dropping the narrowest hole — wide static-screen holds matter
/// most.
pub(super) fn record_pts_hole(
    holes: &mut std::collections::BTreeMap<u32, u32>,
    start: u32,
    end: u32,
) {
    const MAX_TRACKED_HOLES: usize = 64;
    holes.insert(start, end);
    if holes.len() > MAX_TRACKED_HOLES
        && let Some(narrowest) = holes
            .iter()
            .min_by_key(|&(&s, &e)| e.saturating_sub(s))
            .map(|(&s, _)| s)
    {
        holes.remove(&narrowest);
    }
}

#[derive(Clone)]
pub struct AsyncVideoDecoderHandle {
    sender: mpsc::Sender<VideoDecoderMessage>,
    offset: f64,
    status: DecoderStatus,
    max_fallback_distance: u32,
    /// Klipolo: how a request waits, and its decoder thread's progress.
    wait: DecodeWait,
    progress: DecodeProgress,
    #[cfg(target_os = "macos")]
    fallback: Option<Arc<MacOsDecoderFallback>>,
}

/// Klipolo: software fallbacks started by AVAssetReader handles in this process (a diagnostic for tests).
#[cfg(target_os = "macos")]
static SOFTWARE_FALLBACKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Klipolo: how many AVAssetReader handles switched to their FFmpeg software fallback so far in this process.
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn software_fallbacks() -> usize {
    SOFTWARE_FALLBACKS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(target_os = "macos")]
struct MacOsDecoderFallback {
    name: &'static str,
    path: PathBuf,
    fps: f64,
    /// Klipolo: the reader's wait, which the software decoder keeps.
    wait: DecodeWait,
    decoder: tokio::sync::OnceCell<Result<AsyncVideoDecoderHandle, String>>,
}

impl AsyncVideoDecoderHandle {
    const INITIAL_SEEK_TIMEOUT_MS: u64 = 10000;
    const INITIAL_MAX_FALLBACK_DISTANCE: u32 = 2;

    fn normal_timeout_ms(&self) -> u64 {
        let pixels = (self.status.video_width as u64) * (self.status.video_height as u64);
        if pixels > 4_000_000 {
            4000
        } else if pixels > 2_000_000 {
            3000
        } else {
            2000
        }
    }

    pub async fn get_frame(&self, time: f32) -> Option<DecodedFrame> {
        self.get_frame_unless(time, None).await
    }

    /// Klipolo: `get_frame`, answered early with a nearby frame once `superseded` says a newer request replaced it.
    pub async fn get_frame_unless(
        &self,
        time: f32,
        superseded: Option<Superseded>,
    ) -> Option<DecodedFrame> {
        self.read_frame(time, superseded, None).await.got()
    }

    /// Klipolo: `get_frame_unless`, saying whether a frame that didn't come was dropped by the decoder or
    /// stalled (no answer within the fixed deadline, or no progress for the wait's window). `beat` hears a wait on a
    /// decoder that still makes progress (export's heartbeat).
    pub async fn read_frame(
        &self,
        time: f32,
        superseded: Option<Superseded>,
        beat: Option<&DecodeBeat>,
    ) -> Waited<DecodedFrame> {
        self.get_frame_with_timeout(
            time,
            self.normal_timeout_ms(),
            self.max_fallback_distance,
            superseded,
            beat,
        )
        .await
    }

    pub async fn get_frame_initial(&self, time: f32) -> Option<DecodedFrame> {
        self.get_frame_with_timeout(
            time,
            Self::INITIAL_SEEK_TIMEOUT_MS,
            self.max_fallback_distance
                .min(Self::INITIAL_MAX_FALLBACK_DISTANCE),
            None,
            None,
        )
        .await
        .got()
    }

    async fn get_frame_with_timeout(
        &self,
        time: f32,
        timeout_ms: u64,
        max_fallback_distance: u32,
        superseded: Option<Superseded>,
        beat: Option<&DecodeBeat>,
    ) -> Waited<DecodedFrame> {
        // Klipolo: a started fallback serves every request; a request the reader doesn't answer returns
        // None, and the caller decides whether to start the fallback (`start_fallback`).
        #[cfg(target_os = "macos")]
        if let Some(result) = self
            .fallback
            .as_ref()
            .and_then(|fallback| fallback.decoder.get())
        {
            return match result {
                Ok(decoder) => {
                    decoder
                        .request_frame(time, timeout_ms, max_fallback_distance, None, beat)
                        .await
                }
                Err(_) => Waited::Dropped,
            };
        }
        self.request_frame(time, timeout_ms, max_fallback_distance, superseded, beat)
            .await
    }

    /// Klipolo: whether `start_fallback` has a software decoder left to start (an AVAssetReader handle
    /// whose fallback wasn't started yet).
    pub fn can_fall_back(&self) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(fallback) = &self.fallback {
            return !fallback.decoder.initialized();
        }
        false
    }

    /// Klipolo: switches an AVAssetReader handle whose reader stopped returning frames to FFmpeg
    /// software decoding, for every later request. Cap did this inside `get_frame_with_timeout`; here the caller first
    /// asks, off its own task, whether the file is still there: one that is gone can't be read in software either (the
    /// reader reads it again if it comes back), and a stat here could hang the caller's task on a volume that stopped
    /// answering. True when the handle now reads through the fallback. Klipolo: it starts at `at` (the
    /// time it is first asked for), seeking to the keyframe before it rather than decoding frame 0 first, and `beat`
    /// hears its start (export's heartbeat).
    pub async fn start_fallback(&self, at: f32, beat: Option<&DecodeBeat>) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(fallback) = &self.fallback {
            return fallback
                .decoder
                .get_or_init(|| async {
                    SOFTWARE_FALLBACKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        name = fallback.name,
                        "Video reader stopped returning frames; switching to software decoding"
                    );
                    match spawn_ffmpeg_decoder(
                        fallback.name,
                        fallback.path.clone(),
                        fallback.fps,
                        self.offset,
                        self.get_time(at),
                        fallback.wait,
                        false,
                        beat,
                    )
                    .await
                    {
                        Ok(mut decoder) => {
                            decoder.status.fallback_reason =
                                Some("AVAssetReader returned no frame".to_string());
                            Ok(decoder)
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Software video decoder fallback failed");
                            Err(error)
                        }
                    }
                })
                .await
                .is_ok();
        }
        false
    }

    async fn request_frame(
        &self,
        time: f32,
        timeout_ms: u64,
        max_fallback_distance: u32,
        superseded: Option<Superseded>,
        beat: Option<&DecodeBeat>,
    ) -> Waited<DecodedFrame> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let adjusted_time = self.get_time(time);

        if self
            .sender
            .send(VideoDecoderMessage::GetFrame(
                adjusted_time,
                max_fallback_distance,
                tx,
                superseded,
            ))
            .is_err()
        {
            tracing::warn!(
                time = adjusted_time,
                "decoder thread is gone; frame request dropped"
            );
            return Waited::Dropped;
        }

        let waited = self
            .wait
            .wait(rx, Duration::from_millis(timeout_ms), &self.progress, beat)
            .await;
        if let Waited::Stalled(after) = &waited {
            match self.wait {
                DecodeWait::Fixed => tracing::warn!(
                    time = adjusted_time,
                    timeout_ms = timeout_ms,
                    "Frame decode request timed out"
                ),
                // Klipolo: the progress wait says how long it waited, not the preview's deadline.
                DecodeWait::Progress { .. } => tracing::warn!(
                    time = adjusted_time,
                    waited_ms = after.as_millis() as u64,
                    "Frame decode request stalled: its decoder made no progress"
                ),
            }
        }
        waited
    }

    pub fn get_time(&self, time: f32) -> f32 {
        time + self.offset as f32
    }

    pub fn decoder_status(&self) -> &DecoderStatus {
        #[cfg(target_os = "macos")]
        if let Some(Ok(decoder)) = self
            .fallback
            .as_ref()
            .and_then(|fallback| fallback.decoder.get())
        {
            return &decoder.status;
        }
        &self.status
    }

    pub fn decoder_type(&self) -> DecoderType {
        self.decoder_status().decoder_type
    }

    pub fn is_hardware_accelerated(&self) -> bool {
        self.decoder_type().is_hardware_accelerated()
    }

    pub fn video_dimensions(&self) -> (u32, u32) {
        (self.status.video_width, self.status.video_height)
    }

    pub fn fallback_reason(&self) -> Option<&str> {
        self.decoder_status().fallback_reason.as_deref()
    }

    pub fn with_max_fallback_distance(mut self, max_fallback_distance: u32) -> Self {
        self.max_fallback_distance = max_fallback_distance;
        self
    }
}

/// Klipolo: starts at `start_at` (the decoder's own time), seeking before its first frame; its start waits
/// 30 s (`decode_wait::START_WINDOW`), and with `wait` by progress then on while it makes progress; `beat` hears it.
#[allow(clippy::too_many_arguments)]
async fn spawn_ffmpeg_decoder(
    name: &'static str,
    path: PathBuf,
    fps: f64,
    offset: f64,
    start_at: f32,
    wait: DecodeWait,
    use_hw_acceleration: bool,
    beat: Option<&DecodeBeat>,
) -> Result<AsyncVideoDecoderHandle, String> {
    let path_display = crate::log_name(&path).into_owned();
    let (ready_tx, ready_rx) = oneshot::channel::<Result<DecoderInitResult, String>>();
    let (tx, rx) = mpsc::channel();
    let progress = DecodeProgress::default();

    ffmpeg::FfmpegDecoder::spawn_with_hw_config(
        name,
        path,
        fps,
        rx,
        ready_tx,
        use_hw_acceleration,
        start_at,
        progress.clone(),
    )
    .map_err(|e| format!("'{name}' FFmpeg decoder / {e}"))?;

    match wait.wait_start(ready_rx, &progress, beat).await {
        Waited::Got(Ok(init_result)) => {
            info!(
                "Video '{}' using {} decoder ({}x{})",
                name, init_result.decoder_type, init_result.width, init_result.height
            );
            let status = DecoderStatus {
                decoder_type: init_result.decoder_type,
                video_width: init_result.width,
                video_height: init_result.height,
                fallback_reason: None,
            };
            Ok(AsyncVideoDecoderHandle {
                sender: tx,
                offset,
                status,
                max_fallback_distance: DEFAULT_MAX_FALLBACK_DISTANCE,
                wait,
                progress,
                #[cfg(target_os = "macos")]
                fallback: None,
            })
        }
        Waited::Got(Err(e)) => Err(format!(
            "'{name}' FFmpeg decoder initialization failed: {e}"
        )),
        Waited::Dropped => Err(format!("'{name}' FFmpeg decoder channel closed")),
        Waited::Stalled(after) => Err(format!(
            "'{name}' FFmpeg decoder timed out after {} s initializing: {path_display}",
            after.as_secs()
        )),
    }
}

/// Per-handle cache and idle policy (Cap hard-coded 256 MiB and 250 ms; Klipolo keeps preview decoders warm).
#[derive(Clone, Copy, Debug)]
pub struct DecoderTuning {
    /// Readers and the frame cache are dropped after this long without a request.
    pub idle_timeout: std::time::Duration,
    /// Upper bound of this handle's decoded-frame cache.
    pub max_cache_bytes: usize,
    /// Klipolo: the AVAssetReader scales larger sources down to fit this box while decoding (preview); None decodes
    /// at full size (export).
    pub max_output: Option<(u32, u32)>,
    /// Klipolo: how long a frame request waits (Cap's fixed deadline, or while the decoder progresses).
    pub wait: DecodeWait,
}

impl Default for DecoderTuning {
    fn default() -> Self {
        Self {
            idle_timeout: std::time::Duration::from_millis(250),
            max_cache_bytes: 256 << 20,
            max_output: None,
            wait: DecodeWait::Fixed,
        }
    }
}

pub async fn spawn_decoder(
    name: &'static str,
    path: PathBuf,
    fps: f64,
    offset: f64,
    force_ffmpeg: bool,
    tuning: DecoderTuning,
) -> Result<AsyncVideoDecoderHandle, String> {
    spawn_decoder_at(name, path, fps, offset, force_ffmpeg, tuning, 0.0).await
}

/// Klipolo: `spawn_decoder` for a first request at `start_at` (the source's time): an FFmpeg decoder seeks
/// there before its first frame (the AVAssetReader makes its first reader at its first request's time anyway).
pub async fn spawn_decoder_at(
    name: &'static str,
    path: PathBuf,
    fps: f64,
    offset: f64,
    force_ffmpeg: bool,
    tuning: DecoderTuning,
    start_at: f32,
) -> Result<AsyncVideoDecoderHandle, String> {
    let path_display = crate::log_name(&path).into_owned();
    let timeout_duration = Duration::from_secs(30);
    let start_at = start_at + offset as f32;

    #[cfg(target_os = "macos")]
    {
        if force_ffmpeg {
            info!(force_ffmpeg, "Video '{}' using FFmpeg decoder", name);
            return spawn_ffmpeg_decoder(
                name,
                path,
                fps,
                offset,
                start_at,
                tuning.wait,
                true,
                None,
            )
            .await;
        }

        let progress = DecodeProgress::default();
        let avasset_result = {
            let (ready_tx, ready_rx) = oneshot::channel::<Result<DecoderInitResult, String>>();
            let (tx, rx) = mpsc::channel();

            avassetreader::AVAssetReaderDecoder::spawn(
                name,
                path.clone(),
                fps,
                rx,
                ready_tx,
                tuning,
                progress.clone(),
            );

            match tokio::time::timeout(timeout_duration, ready_rx).await {
                Ok(Ok(Ok(init_result))) => {
                    info!(
                        "Video '{}' using {} decoder ({}x{})",
                        name, init_result.decoder_type, init_result.width, init_result.height
                    );
                    let status = DecoderStatus {
                        decoder_type: init_result.decoder_type,
                        video_width: init_result.width,
                        video_height: init_result.height,
                        fallback_reason: None,
                    };
                    Ok(AsyncVideoDecoderHandle {
                        sender: tx,
                        offset,
                        status,
                        max_fallback_distance: DEFAULT_MAX_FALLBACK_DISTANCE,
                        wait: tuning.wait,
                        progress: progress.clone(),
                        #[cfg(target_os = "macos")]
                        fallback: Some(Arc::new(MacOsDecoderFallback {
                            name,
                            path: path.clone(),
                            fps,
                            wait: tuning.wait,
                            decoder: tokio::sync::OnceCell::new(),
                        })),
                    })
                }
                Ok(Ok(Err(e))) => Err(format!("AVAssetReader initialization failed: {e}")),
                Ok(Err(e)) => Err(format!("AVAssetReader channel closed: {e}")),
                Err(_) => Err(format!(
                    "AVAssetReader timed out after 30s initializing: {path_display}"
                )),
            }
        };

        match avasset_result {
            Ok(handle) => Ok(handle),
            Err(avasset_error) => {
                tracing::warn!(
                    name = name,
                    error = %avasset_error,
                    "AVAssetReader failed, falling back to FFmpeg decoder"
                );

                match spawn_ffmpeg_decoder(
                    name,
                    path,
                    fps,
                    offset,
                    start_at,
                    tuning.wait,
                    true,
                    None,
                )
                .await
                {
                    Ok(mut handle) => {
                        info!(
                            "Video '{}' using {} decoder after AVAssetReader failure",
                            name, handle.status.decoder_type
                        );
                        handle.status.fallback_reason = Some(avasset_error);
                        Ok(handle)
                    }
                    Err(e) => Err(format!(
                        "'{name}' decoder failed - AVAssetReader: {avasset_error}, FFmpeg: {e}"
                    )),
                }
            }
        }
    }

    // Klipolo: elsewhere FFmpeg decodes, with the hardware device crate::platform::decode picks (software for now).
    #[cfg(not(target_os = "macos"))]
    {
        let _ = timeout_duration;
        spawn_ffmpeg_decoder(name, path, fps, offset, start_at, tuning.wait, true, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_frame_storage_identity_matches_clones() {
        let frame = DecodedFrame::new_nv12(vec![0; 6], 2, 2, 2, 2);
        let cloned = frame.clone();

        assert!(frame.storage_identity().matches(&cloned.storage_identity()));
    }

    #[test]
    fn decoded_frame_storage_identity_rejects_distinct_or_reinterpreted_frames() {
        let data = Arc::new(vec![0; 16]);
        let original = DecodedFrame::new_with_arc(Arc::clone(&data), 2, 2);
        let reinterpreted = DecodedFrame::new_with_arc(Arc::clone(&data), 1, 4);
        let distinct = DecodedFrame::new(vec![0; 16], 2, 2);

        assert!(
            !original
                .storage_identity()
                .matches(&reinterpreted.storage_identity())
        );
        assert!(
            !original
                .storage_identity()
                .matches(&distinct.storage_identity())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn iosurface_backed_nv12_materializes_raw_planes_only_on_demand() {
        let mut image_buf = cv::ImageBuf::new(4, 2, cv::PixelFormat::_420V, None).unwrap();

        unsafe {
            image_buf
                .lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT)
                .result()
                .unwrap();
        }

        let y_stride = image_buf.plane_bytes_per_row(0);
        let uv_stride = image_buf.plane_bytes_per_row(1);
        let y_size = y_stride * image_buf.plane_height(0);
        let uv_size = uv_stride * image_buf.plane_height(1);
        unsafe {
            std::ptr::write_bytes(image_buf.plane_base_address(0).cast_mut(), 0x11, y_size);
            std::ptr::write_bytes(image_buf.plane_base_address(1).cast_mut(), 0x22, uv_size);
            image_buf.unlock_lock_base_addr(cv::pixel_buffer::LockFlags::DEFAULT);
        }

        let frame = DecodedFrame::new_nv12_with_image_buf(
            4,
            2,
            y_stride as u32,
            uv_stride as u32,
            image_buf,
        );

        assert_eq!(frame.byte_len(), y_size + uv_size);
        assert!(frame.with_image_buf(|_| ()).is_some());
        assert_eq!(frame.y_plane().unwrap(), vec![0x11; y_size]);
        assert_eq!(frame.uv_plane().unwrap(), vec![0x22; uv_size]);
        assert_eq!(frame.data().len(), y_size + uv_size);
        assert!(frame.with_image_buf(|_| ()).is_none());
    }

    #[test]
    fn pts_to_frame_maps_real_presentation_time_to_index() {
        let tb = Rational::new(1, 600);
        assert_eq!(pts_to_frame(0, tb, 30.0), 0);
        assert_eq!(pts_to_frame(300, tb, 30.0), 15); // 0.5s
        assert_eq!(pts_to_frame(600, tb, 30.0), 30); // 1.0s
    }

    // The frame index is `round(fps * real_pts_seconds)` — a function of the
    // frame's real presentation time, independent of the stream time_base and of
    // the frame's ordinal position. This is why selection on the nominal-fps grid
    // does not accumulate A/V drift: a request for output time T (`floor(T*fps)`)
    // resolves to the source frame whose real PTS is nearest T.
    #[test]
    fn pts_to_frame_is_anchored_to_time_not_timebase() {
        // Same real time (1.0s) in two different time bases -> same index...
        assert_eq!(
            pts_to_frame(600, Rational::new(1, 600), 30.0),
            pts_to_frame(1000, Rational::new(1, 1000), 30.0),
        );
        // ...and that index is the real one (round(30 * 1.0)), not a constant.
        assert_eq!(pts_to_frame(600, Rational::new(1, 600), 30.0), 30);
        assert_ne!(
            pts_to_frame(600, Rational::new(1, 600), 30.0),
            pts_to_frame(900, Rational::new(1, 600), 30.0),
        );
    }

    // A source whose real rate (26.44fps) differs from the nominal grid (30fps):
    // each source frame's index tracks its real time, so frames are held/dropped
    // to the grid (a cadence artifact) but never drift out of time alignment.
    #[test]
    fn pts_to_frame_follows_real_time_for_off_nominal_source() {
        let real_fps = 26.44_f64;
        let tb = Rational::new(1, 1_000_000); // microseconds
        let mut prev = 0u32;
        for ordinal in 0..200u32 {
            let t = ordinal as f64 / real_fps;
            let pts = (t * 1_000_000.0).round() as i64;
            let idx = pts_to_frame(pts, tb, 30.0);
            assert_eq!(idx, (30.0 * t).round() as u32, "ordinal {ordinal}");
            assert!(idx >= prev, "index must be monotonic non-decreasing");
            prev = idx;
        }
    }
}
