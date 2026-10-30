// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/video-decode/src/avassetreader.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: the track output asks for IOSurface-backed, Metal-compatible pixel buffers (kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey), so frames reach the GPU without a CPU copy; `new_with_keyframe_index` takes an optional output box and the reader scales larger sources down to fit it while decoding (`fit_output`; the preview's frames are then up to 16× smaller in the cache and on the GPU); the keyframe-index log lines and the open errors name the file, not its full path (the app log never holds folders); `KeyframeIndex::load_or_build` reads the index the thumbnail pass stored (`crate::media::keyframes`, rebuilt by `from_stored` with `build`'s arithmetic) and builds one only when none is stored, `build` counts its runs (`builds`, for tests), and `new_at_position` uses `load_or_build`. A reader's range starts 0.1 ms after the keyframe it seeks to, at a µs timescale (`time_range_from`, EDGE_S; `compute_seek_time` returns f64). Cap started it at the keyframe rounded to 1/100 s, which lands just before keyframes off that grid (every NTSC one), so AVFoundation decoded the GOP before and showed the frame before, re-timed to the start. `readers_created` counts readers made (measurement). `KeyframeIndex::coded_size`. (Reader reuse via `reset(forReadingTimeRanges:)` was tried and removed: it corrupts AVFoundation's heap.) 10-bit 4:2:0 HEVC (phones' Main 10) is read by AVAssetReader as 8-bit `420v` (VideoToolbox decodes and converts it; `output_pixel_format`) instead of falling back to FFmpeg's software decoder; other 10-bit sources (H.264 High 10 has no VideoToolbox decoder) still fall back. The stored index records the codec, so `from_stored` decides the reader's format once per file (`reader_format`, no log) instead of every new reader of a 10-bit HEVC file logging an error and opening the file with FFmpeg again. `reader_ns` measures what making readers costs (opening the asset, starting the reader, cancelling the one a seek replaces; `trial` builds only). `load_or_build_with`/`build_with` call a tick while they demux (the decoder's progress); in test and trial builds each reader is numbered by `decode_wait::hooks` (`reader_no`, for the stalled-reader hook). The index records where the video stream starts (`start_secs`: FFmpeg's start, from the stored index or the stream `build` reads) and holds its keyframes on the stream's own timeline (less that start), as the FFmpeg decoders and the thumbnails number frames; a reader is aimed at a stream time plus the start (`file_time`), and a vended sample's time is taken back to the stream's timeline by `stream_pts`, so every decoder shows the same frame of a file whose picture starts after 0. Assets are made with AVURLAssetReferenceRestrictionsKey = ForbidAll (`get_reader_track_output`), so a QuickTime file can't pull in other files or URLs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use cidre::{
    arc::{self, R},
    av::{self},
    cm,
    cv::{self},
    ns,
};
use ffmpeg::{codec as avcodec, format as avformat};
use tokio::runtime::Handle as TokioHandle;

/// Klipolo: indexes built by demuxing a whole file (a stored one is read instead when there is one).
static BUILDS: AtomicUsize = AtomicUsize::new(0);

/// Klipolo: a reader's range starts this long after the keyframe it seeks to. AVFoundation starts on the frame showing
/// at the range's start, so a start a hair before a keyframe decodes the whole GOP before it and shows the frame
/// before (re-timed to the start, so it is taken for the keyframe).
const EDGE_S: f64 = 1e-4;
/// Klipolo: the timescale of a range's start (µs: keyframe times stay exact to well within `EDGE_S`).
const RANGE_TIMESCALE: i32 = 1_000_000;

// Klipolo (security): every asset is made with `AVURLAssetReferenceRestrictionsKey` set to
// `AVAssetReferenceRestrictionForbidAll`, so a QuickTime file can't pull in other files or URLs. Apple's default only
// forbids a local file referencing remote ones; a local movie may reference other local media, which the FFmpeg
// guard (`crate::media::open`, `enable_drefs=0`) refuses on the other path.
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVURLAssetReferenceRestrictionsKey: &'static ns::String;
}

/// `AVAssetReferenceRestrictionForbidAll` (AVAsset.h): every kind of reference forbidden.
const FORBID_ALL_REFERENCES: u32 = 0xFFFF;

static READERS_CREATED: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "trial")]
static ASSET_NS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "trial")]
static START_NS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "trial")]
static CANCEL_NS: AtomicU64 = AtomicU64::new(0);

/// Klipolo: the AVAssetReaders this process has made (measurement).
pub fn readers_created() -> u64 {
    READERS_CREATED.load(Ordering::Relaxed)
}

/// Klipolo (measurement): nanoseconds spent making readers so far: opening the asset and loading its video track,
/// making, aiming and starting the reader, and cancelling the one a seek replaces (`trial` builds only).
#[cfg(feature = "trial")]
pub fn reader_ns() -> (u64, u64, u64) {
    (
        ASSET_NS.load(Ordering::Relaxed),
        START_NS.load(Ordering::Relaxed),
        CANCEL_NS.load(Ordering::Relaxed),
    )
}

#[derive(Clone)]
pub struct KeyframeIndex {
    /// Klipolo: on the stream's own timeline (seconds from `start_secs`), as frames are numbered.
    keyframes: Vec<(u32, f64)>,
    fps: f64,
    duration_secs: f64,
    /// Klipolo: where the video stream starts on the file's timeline (FFmpeg's start; 0 when unknown).
    start_secs: f64,
    pixel_format: Option<cv::PixelFormat>,
    width: u32,
    height: u32,
}

impl KeyframeIndex {
    /// Klipolo: the index the thumbnail pass stored for this file as it is now, or one built by demuxing it.
    pub fn load_or_build(path: &Path) -> Result<Self, String> {
        Self::load_or_build_with(path, &|| {})
    }

    /// Klipolo: `load_or_build`, calling `tick` as it demuxes (a long file takes seconds).
    pub fn load_or_build_with(path: &Path, tick: &dyn Fn()) -> Result<Self, String> {
        match crate::media::keyframes::load(path) {
            Some(stored) => Ok(Self::from_stored(stored)),
            None => Self::build_with(path, tick),
        }
    }

    /// Klipolo: the index from what the thumbnail pass recorded of the same stream, with `build`'s arithmetic.
    pub fn from_stored(stored: crate::media::keyframes::StoredKeys) -> Self {
        let (num, den) = stored.time_base;
        let keyframes = stored
            .pts
            .iter()
            .map(|&pts| {
                let time_secs = (pts - stored.start) as f64 * num as f64 / den as f64;
                ((time_secs * stored.fps).round() as u32, time_secs)
            })
            .collect();
        // Decided here, once per file (Klipolo): every reader of the index reuses it. A format the reader
        // can't take is None without a log, so the reader's open finds out (and falls back) as before.
        let pixel_format = stored
            .pixel_format
            .parse::<avformat::Pixel>()
            .ok()
            .and_then(|pixel| reader_format(codec_id(&stored.codec), pixel));
        Self {
            keyframes,
            fps: stored.fps,
            duration_secs: stored.duration_secs,
            start_secs: stored.start as f64 * num as f64 / den as f64,
            pixel_format,
            width: stored.width,
            height: stored.height,
        }
    }

    /// Klipolo: how many indexes `build` has made in this process.
    pub fn builds() -> usize {
        BUILDS.load(Ordering::SeqCst)
    }

    pub fn build(path: &Path) -> Result<Self, String> {
        Self::build_with(path, &|| {})
    }

    /// Klipolo: `build`, calling `tick` every 64 packets.
    pub fn build_with(path: &Path, tick: &dyn Fn()) -> Result<Self, String> {
        BUILDS.fetch_add(1, Ordering::SeqCst);
        let build_start = std::time::Instant::now();

        let mut input = crate::media::open::input(path)
            .map_err(|e| format!("Failed to open video for keyframe scan: {e}"))?;

        let (stream_index, time_base, start, fps, duration_secs, pixel_format, width, height) = {
            let video_stream = input
                .streams()
                .best(ffmpeg::media::Type::Video)
                .ok_or("No video stream found")?;

            let stream_index = video_stream.index();
            let time_base = video_stream.time_base();
            let start = crate::render::probe::stream_start(&video_stream);
            let fps = {
                let rate = video_stream.avg_frame_rate();
                if rate.denominator() == 0 {
                    30.0
                } else {
                    rate.numerator() as f64 / rate.denominator() as f64
                }
            };

            let duration_secs = {
                let duration = video_stream.duration();
                if duration > 0 {
                    duration as f64 * time_base.numerator() as f64 / time_base.denominator() as f64
                } else {
                    0.0
                }
            };

            let decoder = avcodec::Context::from_parameters(video_stream.parameters())
                .map_err(|e| format!("decoder context / {e}"))?
                .decoder()
                .video()
                .map_err(|e| format!("video decoder / {e}"))?;

            let pixel_format = output_pixel_format(decoder.id(), decoder.format()).ok();
            let width = decoder.width();
            let height = decoder.height();

            (
                stream_index,
                time_base,
                start,
                fps,
                duration_secs,
                pixel_format,
                width,
                height,
            )
        };

        let mut keyframes = Vec::new();

        for (n, (stream, packet)) in input.packets().enumerate() {
            if n % 64 == 0 {
                tick();
            }
            if stream.index() != stream_index {
                continue;
            }

            if packet.is_key() {
                let pts = packet.pts().unwrap_or(0);
                let time_secs = (pts - start) as f64 * time_base.numerator() as f64
                    / time_base.denominator() as f64;
                let frame_number = (time_secs * fps).round() as u32;
                keyframes.push((frame_number, time_secs));
            }
        }

        let elapsed = build_start.elapsed();
        tracing::info!(
            file = %crate::log_name(path),
            keyframe_count = keyframes.len(),
            fps = fps,
            duration_secs = duration_secs,
            build_ms = elapsed.as_millis(),
            "Built keyframe index"
        );

        Ok(Self {
            keyframes,
            fps,
            duration_secs,
            start_secs: start as f64 * time_base.numerator() as f64
                / time_base.denominator() as f64,
            pixel_format,
            width,
            height,
        })
    }

    pub fn nearest_keyframe_before(&self, target_frame: u32) -> Option<(u32, f64)> {
        if self.keyframes.is_empty() {
            return None;
        }

        let pos = self
            .keyframes
            .binary_search_by_key(&target_frame, |(frame, _)| *frame);

        match pos {
            Ok(i) => Some(self.keyframes[i]),
            Err(0) => None,
            Err(i) => Some(self.keyframes[i - 1]),
        }
    }

    pub fn nearest_keyframe_after(&self, target_frame: u32) -> Option<(u32, f64)> {
        if self.keyframes.is_empty() {
            return None;
        }

        let pos = self
            .keyframes
            .binary_search_by_key(&target_frame, |(frame, _)| *frame);

        let idx = match pos {
            Ok(i) => {
                if i + 1 < self.keyframes.len() {
                    i + 1
                } else {
                    i
                }
            }
            Err(i) => {
                if i < self.keyframes.len() {
                    i
                } else {
                    return None;
                }
            }
        };

        Some(self.keyframes[idx])
    }

    pub fn get_strategic_positions(&self, num_positions: usize) -> Vec<f64> {
        if self.keyframes.is_empty() || num_positions == 0 {
            return vec![0.0];
        }

        let total_keyframes = self.keyframes.len();
        if total_keyframes <= num_positions {
            return self.keyframes.iter().map(|(_, time)| *time).collect();
        }

        let mut positions: Vec<f64> = Vec::with_capacity(num_positions);

        positions.push(self.keyframes[0].1);

        if num_positions > 2 {
            let inner_count = num_positions - 2;
            let step = (total_keyframes - 1) as f64 / (inner_count + 1) as f64;
            for i in 1..=inner_count {
                let idx = (step * i as f64).round() as usize;
                let idx = idx.min(total_keyframes - 2);
                positions.push(self.keyframes[idx].1);
            }
        }

        if num_positions > 1 {
            positions.push(self.keyframes[total_keyframes - 1].1);
        }

        positions
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    /// Klipolo: where the video stream starts on the file's timeline, in seconds: frame 0's time there.
    pub fn start_secs(&self) -> f64 {
        self.start_secs
    }

    pub fn keyframe_count(&self) -> usize {
        self.keyframes.len()
    }

    pub fn keyframes(&self) -> &[(u32, f64)] {
        &self.keyframes
    }

    pub fn cached_video_info(&self) -> Option<(cv::PixelFormat, u32, u32)> {
        self.pixel_format.map(|pf| (pf, self.width, self.height))
    }

    /// Klipolo: the decoder's (coded) width and height, when known.
    pub fn coded_size(&self) -> Option<(u32, u32)> {
        (self.width > 0 && self.height > 0).then_some((self.width, self.height))
    }
}

/// Klipolo: `size` scaled down (keeping its shape, even sides) to fit `max_output`; unchanged when it fits.
pub fn fit_output(size: (u32, u32), max_output: Option<(u32, u32)>) -> (u32, u32) {
    let Some((bw, bh)) = max_output else {
        return size;
    };
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    // A portrait box for a portrait frame: the box is a size limit, not a shape.
    let (bw, bh) = if (w < h) == (bw < bh) {
        (bw, bh)
    } else {
        (bh, bw)
    };
    let scale = (f64::from(bw) / w).min(f64::from(bh) / h);
    if scale >= 1.0 {
        return size;
    }
    let even = |n: f64| ((n / 2.0).round() as u32 * 2).max(2);
    (even(w * scale), even(h * scale))
}

/// Klipolo: in f64 (was f32): a range starts on it (`time_range_from`), so it keeps the keyframe's time exactly.
fn compute_seek_time(keyframe_index: Option<&Arc<KeyframeIndex>>, requested_time: f32) -> f64 {
    if let Some(kf_index) = keyframe_index {
        let fps = kf_index.fps();
        let target_frame = (requested_time as f64 * fps).round() as u32;
        if let Some((_, keyframe_time)) = kf_index.nearest_keyframe_before(target_frame) {
            return keyframe_time;
        }
        if let Some((_, first_keyframe_time)) = kf_index.keyframes().first() {
            return *first_keyframe_time;
        }
    }
    f64::from(requested_time)
}

/// Klipolo: where `stream_time` (seconds on the video stream's timeline, as requests and the index have it)
/// is on the file's timeline, which AVFoundation reads by: the stream starts `start_secs` in. Never before the file's 0.
fn file_time(start_secs: f64, stream_time: f64) -> f64 {
    (start_secs + stream_time).max(0.0)
}

/// Klipolo: a vended sample's presentation time `pts` (the file's timeline) as a pts on the video stream's
/// own timeline, in `pts`'s timescale: the stream starts `start_secs` into the file. The rendering decoder numbers
/// frames from it, as the FFmpeg decoders and the thumbnails do.
pub fn stream_pts(pts: cm::Time, start_secs: f64) -> i64 {
    pts.value - (start_secs * f64::from(pts.scale)).round() as i64
}

/// Klipolo: the range a reader seeking to `start` reads: from `EDGE_S` after it to the end of the track.
fn time_range_from(start: f64) -> cm::TimeRange {
    cm::TimeRange {
        start: cm::Time::with_secs(start + EDGE_S, RANGE_TIMESCALE),
        duration: cm::Time::infinity(),
    }
}

pub struct AVAssetReaderDecoder {
    path: PathBuf,
    pixel_format: cv::PixelFormat,
    tokio_handle: TokioHandle,
    track_output: R<av::AssetReaderTrackOutput>,
    reader: R<av::AssetReader>,
    width: u32,
    height: u32,
    keyframe_index: Option<Arc<KeyframeIndex>>,
    /// Klipolo: the index's `start_secs` (0 without one).
    start_secs: f64,
    current_position_secs: f32,
    /// Klipolo: this reader's number among the test hooks' (test and trial builds).
    #[cfg(any(test, feature = "trial"))]
    reader_no: Option<usize>,
}

impl AVAssetReaderDecoder {
    pub fn new(path: PathBuf, tokio_handle: TokioHandle) -> Result<Self, String> {
        Self::new_at_position(path, tokio_handle, 0.0)
    }

    pub fn new_at_position(
        path: PathBuf,
        tokio_handle: TokioHandle,
        start_time: f32,
    ) -> Result<Self, String> {
        let keyframe_index = match KeyframeIndex::load_or_build(&path) {
            Ok(index) => Some(Arc::new(index)),
            Err(e) => {
                tracing::warn!(
                    file = %crate::log_name(&path),
                    error = %e,
                    "Failed to build keyframe index, seeking may be slower"
                );
                None
            }
        };

        Self::new_with_keyframe_index(path, tokio_handle, start_time, keyframe_index, None)
    }

    pub fn new_with_keyframe_index(
        path: PathBuf,
        tokio_handle: TokioHandle,
        start_time: f32,
        keyframe_index: Option<Arc<KeyframeIndex>>,
        max_output: Option<(u32, u32)>,
    ) -> Result<Self, String> {
        let (pixel_format, width, height) = if let Some(info) = keyframe_index
            .as_ref()
            .and_then(|ki| ki.cached_video_info())
        {
            info
        } else {
            let input = crate::media::open::input(&path).map_err(|e| {
                format!(
                    "Failed to open video input '{}': {e}",
                    crate::log_name(&path)
                )
            })?;

            let input_stream = input
                .streams()
                .best(ffmpeg::media::Type::Video)
                .ok_or_else(|| format!("No video stream in '{}'", crate::log_name(&path)))?;

            let decoder = avcodec::Context::from_parameters(input_stream.parameters())
                .map_err(|e| format!("decoder context / {e}"))?
                .decoder()
                .video()
                .map_err(|e| format!("video decoder / {e}"))?;

            (
                output_pixel_format(decoder.id(), decoder.format())?,
                decoder.width(),
                decoder.height(),
            )
        };

        let (width, height) = fit_output((width, height), max_output);
        let seek_time = compute_seek_time(keyframe_index.as_ref(), start_time);
        let start_secs = keyframe_index
            .as_ref()
            .map_or(0.0, |index| index.start_secs());

        let (track_output, reader) = Self::get_reader_track_output(
            &path,
            file_time(start_secs, seek_time),
            &tokio_handle,
            pixel_format,
            width,
            height,
        )?;

        Ok(Self {
            #[cfg(any(test, feature = "trial"))]
            reader_no: crate::decode_wait::hooks::reader_made(&path),
            path,
            pixel_format,
            tokio_handle,
            track_output,
            reader,
            width,
            height,
            keyframe_index,
            start_secs,
            current_position_secs: seek_time as f32,
        })
    }

    /// Klipolo: this reader's number among the test hooks' (None without hooks).
    #[cfg(any(test, feature = "trial"))]
    pub fn reader_no(&self) -> Option<usize> {
        self.reader_no
    }

    pub fn reset(&mut self, requested_time: f32) -> Result<(), String> {
        #[cfg(feature = "trial")]
        let cancelling = std::time::Instant::now();
        self.reader.cancel_reading();
        #[cfg(feature = "trial")]
        CANCEL_NS.fetch_add(cancelling.elapsed().as_nanos() as u64, Ordering::Relaxed);

        let seek_time = compute_seek_time(self.keyframe_index.as_ref(), requested_time);

        (self.track_output, self.reader) = Self::get_reader_track_output(
            &self.path,
            file_time(self.start_secs, seek_time),
            &self.tokio_handle,
            self.pixel_format,
            self.width,
            self.height,
        )?;

        self.current_position_secs = seek_time as f32;
        #[cfg(any(test, feature = "trial"))]
        {
            self.reader_no = crate::decode_wait::hooks::reader_made(&self.path);
        }

        Ok(())
    }

    pub fn current_position_secs(&self) -> f32 {
        self.current_position_secs
    }

    /// Klipolo: where the video stream starts on the file's timeline (`stream_pts` takes it).
    pub fn start_secs(&self) -> f64 {
        self.start_secs
    }

    pub fn update_position(&mut self, position_secs: f32) {
        self.current_position_secs = position_secs;
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn pixel_format(&self) -> cv::PixelFormat {
        self.pixel_format
    }

    pub fn take_keyframe_index(&mut self) -> Option<Arc<KeyframeIndex>> {
        self.keyframe_index.take()
    }

    pub fn keyframe_index(&self) -> Option<&Arc<KeyframeIndex>> {
        self.keyframe_index.as_ref()
    }

    pub fn keyframe_index_arc(&self) -> Option<Arc<KeyframeIndex>> {
        self.keyframe_index.clone()
    }

    fn get_reader_track_output(
        path: &Path,
        time: f64,
        handle: &TokioHandle,
        pixel_format: cv::PixelFormat,
        width: u32,
        height: u32,
    ) -> Result<(R<av::AssetReaderTrackOutput>, R<av::AssetReader>), String> {
        #[cfg(feature = "trial")]
        let opening = std::time::Instant::now();
        // SAFETY: an immutable NSString constant AVFoundation exports.
        let key = unsafe { AVURLAssetReferenceRestrictionsKey };
        let forbid_all = ns::Number::with_u32(FORBID_ALL_REFERENCES);
        let options: arc::R<ns::Dictionary<ns::String, ns::Id>> =
            ns::Dictionary::with_keys_values(&[key], &[forbid_all.as_id_ref()]);
        let asset = av::UrlAsset::with_url(
            &ns::Url::with_fs_path_str(
                path.to_str().ok_or_else(|| {
                    format!("Invalid UTF-8 in the path of '{}'", crate::log_name(path))
                })?,
                false,
            ),
            Some(&options),
        )
        .ok_or_else(|| format!("UrlAsset::with_url{{{}}}", crate::log_name(path)))?;

        let mut reader = av::AssetReader::with_asset(&asset)
            .map_err(|e| format!("AssetReader::with_asset: {e}"))?;

        reader.set_time_range(time_range_from(time));

        let tracks = handle
            .block_on(asset.load_tracks_with_media_type(av::MediaType::video()))
            .map_err(|e| format!("asset.load_tracks_with_media_type: {e}"))?;

        let track = tracks.get(0).map_err(|e| e.to_string())?;
        #[cfg(feature = "trial")]
        let starting = std::time::Instant::now();
        #[cfg(feature = "trial")]
        ASSET_NS.fetch_add(
            starting.duration_since(opening).as_nanos() as u64,
            Ordering::Relaxed,
        );

        let mut reader_track_output = av::AssetReaderTrackOutput::with_track(
            &track,
            Some(&ns::Dictionary::with_keys_values(
                &[
                    cv::pixel_buffer::keys::pixel_format().as_ns(),
                    cv::pixel_buffer::keys::width().as_ns(),
                    cv::pixel_buffer::keys::height().as_ns(),
                    cv::pixel_buffer::keys::io_surf_props().as_ns(),
                    cv::pixel_buffer::keys::metal_compatibility().as_ns(),
                ],
                &[
                    pixel_format.to_cf_number().as_ns().as_id_ref(),
                    ns::Number::with_u32(width).as_id_ref(),
                    ns::Number::with_u32(height).as_id_ref(),
                    ns::Dictionary::<ns::String, ns::Id>::new().as_id_ref(),
                    ns::Number::with_bool(true).as_id_ref(),
                ],
            )),
        )
        .map_err(|e| format!("asset.reader_track_output{{{pixel_format:?}}}): {e}"))?;

        reader_track_output.set_always_copies_sample_data(false);

        reader
            .add_output(&reader_track_output)
            .map_err(|e| format!("reader.add_output: {e}"))?;

        reader
            .start_reading()
            .map_err(|e| format!("reader.start_reading: {e}"))?;

        READERS_CREATED.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "trial")]
        START_NS.fetch_add(starting.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Ok((reader_track_output, reader))
    }

    pub fn frames(&mut self) -> FramesIter<'_> {
        FramesIter {
            track_output: &mut self.track_output,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

pub struct FramesIter<'a> {
    track_output: &'a mut av::AssetReaderTrackOutput,
}

impl Drop for AVAssetReaderDecoder {
    fn drop(&mut self) {
        self.reader.cancel_reading();
    }
}

impl<'a> Iterator for FramesIter<'a> {
    type Item = ns::ExResult<'static, arc::R<cm::SampleBuf>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.track_output.next_sample_buf().transpose()
    }
}

/// Klipolo: FFmpeg's codec id for its name (`hevc`); `Id::None` for an empty or unknown name.
fn codec_id(name: &str) -> ffmpeg::codec::Id {
    let Ok(name) = std::ffi::CString::new(name) else {
        return ffmpeg::codec::Id::None;
    };
    // SAFETY: a lookup in FFmpeg's static codec table by a NUL-terminated name; null when unknown.
    unsafe {
        let descriptor = ffmpeg::ffi::avcodec_descriptor_get_by_name(name.as_ptr());
        if descriptor.is_null() {
            ffmpeg::codec::Id::None
        } else {
            ffmpeg::codec::Id::from((*descriptor).id)
        }
    }
}

/// Klipolo: the format the reader asks for, or None (no log) for one it can't take.
fn reader_format(codec: ffmpeg::codec::Id, pixel: avformat::Pixel) -> Option<cv::PixelFormat> {
    match (codec, pixel) {
        (ffmpeg::codec::Id::HEVC, avformat::Pixel::YUV420P10LE | avformat::Pixel::P010LE) => {
            Some(cv::PixelFormat::_420V)
        }
        (_, avformat::Pixel::NV12 | avformat::Pixel::YUV420P) => Some(cv::PixelFormat::_420V),
        (_, avformat::Pixel::RGBA) => Some(cv::PixelFormat::_32_RGBA),
        (_, avformat::Pixel::BGRA) => Some(cv::PixelFormat::_32_BGRA),
        _ => None,
    }
}

/// Klipolo: the format the track output asks for, from the source's codec and pixel format. VideoToolbox
/// decodes 10-bit 4:2:0 HEVC (phones' Main 10) and hands it over as 8-bit `420v`, so it never falls back to FFmpeg's
/// software decoder (measured on the Intel Mac, 20 s of 4K HEVC Main 10 into an editing copy: 14 → about 125 frames a
/// second, 116 s → 2.5 s of CPU, peak memory 735 → 150 MB). Other 10-bit sources keep `pixel_to_pixel_format`'s answer
/// (H.264 High 10 has no VideoToolbox decoder).
pub fn output_pixel_format(
    codec: ffmpeg::codec::Id,
    pixel: avformat::Pixel,
) -> Result<cv::PixelFormat, String> {
    match reader_format(codec, pixel) {
        Some(format) => Ok(format),
        // Logs the format it can't take.
        None => pixel_to_pixel_format(pixel),
    }
}

pub fn pixel_to_pixel_format(pixel: avformat::Pixel) -> Result<cv::PixelFormat, String> {
    match pixel {
        avformat::Pixel::NV12 => Ok(cv::PixelFormat::_420V),
        avformat::Pixel::YUV420P => Ok(cv::PixelFormat::_420V),
        avformat::Pixel::RGBA => Ok(cv::PixelFormat::_32_RGBA),
        avformat::Pixel::BGRA => Ok(cv::PixelFormat::_32_BGRA),
        other => {
            tracing::error!(
                pixel_format = ?other,
                "Unhandled pixel format encountered - no mapping to cv::PixelFormat available"
            );
            Err(format!(
                "Unsupported pixel format: {other:?}. Supported formats: NV12, YUV420P, RGBA, BGRA"
            ))
        }
    }
}

pub fn pixel_format_to_pixel(format: cv::PixelFormat) -> avformat::Pixel {
    match format {
        cv::PixelFormat::_420V => avformat::Pixel::NV12,
        cv::PixelFormat::_32_RGBA => avformat::Pixel::RGBA,
        cv::PixelFormat::_32_BGRA => avformat::Pixel::BGRA,
        _ => avformat::Pixel::RGBA,
    }
}
