//! A small copy of part of a clip for a model to watch and hear (the scene log's proxy in `klipolo-analysis`): H.264 at
//! a low bitrate and AAC mono in an MP4 whose index comes first (`+faststart`), so an upload stays small and a server
//! can read it as it arrives.
//!
//! - **Picture:** the source decoded on the activity pass's hardware path (`platform::activity`: VideoToolbox on a Mac)
//!   when the codec has one, else in software on every core (FFmpeg's frame threads); should the hardware not start
//!   (FFmpeg then falls back to software by itself, on one thread) or fail part-way, the copy is made again in
//!   software. Each frame is scaled the way the person sees it (anamorphic pixels widened, a phone's quarter turn
//!   applied) to fit `SmallCopy::fit`, at most `max_fps` frames a second (frames dropped evenly, never repeated), and
//!   encoded by the platform's H.264 encoder (`platform::encode`: VideoToolbox's hardware encoder, then its software
//!   one; Linux has none yet, so the copy is a Mac feature).
//! - **Sound:** the engine's own decode (`audio::decode`, 48 kHz), mixed to mono and encoded as AAC (Apple's encoder
//!   where there is one), interleaved with the picture as it goes.
//! - **Every frame:** an `EveryFrame` caller sees each decoded frame of the source at its own rate, before the copy
//!   thins it, as a grey picture (`luma_frames`'s): the scene log finds its cuts there, to the source's frame, whatever
//!   frame rate the copy keeps.
//! - **Times:** the copy's time 0 is `range.start` on the source's file clock (its picture's: the video stream's first
//!   frame is 0), and every frame keeps its own time from there, so a time in the copy plus
//!   `range.start` is the same moment in the source. The picture on screen at `range.start` is the copy's first frame.
//!   The sound is placed by the source's `audio_lead_us` as the preview plays it: silence where it starts after the
//!   picture, nothing from before the copy's 0. AAC's priming is left to the MP4's edit list (FFmpeg's default).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use ffmpeg::codec::threading;
use ffmpeg::format::Pixel;
use ffmpeg::format::context::Output;
use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::format::stream::Disposition;
use ffmpeg::software::scaling;
use ffmpeg::{ChannelLayout, Packet, Rational};
use klipolo_model::time::{RangeUs, Us};

use crate::audio::decode;
use crate::audio::mix::RATE;
use crate::media::luma_frames::Grey;
use crate::media::scan::{next_packet, set_discards};
use crate::platform::activity::ffmpeg::luma::Hardware;
use crate::platform::encode::{Backend, CodecRole, background_options, candidates};
use crate::render::probe::{MediaKind, probe_media, stream_start};
use crate::vendor::cap::enc_ffmpeg::aac::AACEncoder;
use crate::vendor::cap::enc_ffmpeg::h264::H264Encoder;
use crate::vendor::cap::media_info::{AudioInfo, VideoInfo};

/// Sound samples (48 kHz mono) handed from the decode thread at a time.
const BLOCK: usize = 4_096;
/// Seconds between keyframes.
const GOP_SECONDS: f64 = 2.0;

/// What to copy and how small.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmallCopy {
    /// The part of the source, on its file clock.
    pub range: RangeUs,
    /// The box the picture fits in, as (long side, short side) and turned the picture's way: (640, 360) fits a
    /// landscape picture in 640 × 360 and a portrait one in 360 × 640. Never enlarged.
    pub fit: (u32, u32),
    /// Frames a second at most (a faster source drops frames evenly).
    pub max_fps: f64,
    /// The picture's average bitrate (bits a second).
    pub video_bps: u32,
    /// The sound's bitrate; 0 leaves the sound out.
    pub audio_bps: u32,
}

/// A caller that sees every decoded frame of the source over the range, to measure frames one by one: from the
/// keyframe before the range to its end, at the source's own rate (before `SmallCopy::max_fps` thins them), each as a
/// grey picture (as `luma_frames::each_frame` makes them).
pub trait EveryFrame {
    /// The grey picture's size (width, height).
    fn size(&self) -> (u32, u32);
    /// The next frame: its time on the source's file clock and its grey picture, row by row.
    fn see(&mut self, at: Us, pixels: &[u8]);
    /// `write` is making the copy again from the start (in software): forget every frame seen so far.
    fn again(&mut self);
}

/// What a copy holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmallCopyInfo {
    pub width: u32,
    pub height: u32,
    /// Its frame rate: the source's, or `max_fps` when the source is faster.
    pub fps: f64,
    pub frames: u64,
    /// False when the source has no sound (or none that decodes), or `audio_bps` is 0.
    pub has_audio: bool,
    /// The file's size.
    pub bytes: u64,
}

/// Why no copy was made. The messages name no file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmallCopyError {
    /// The source has no picture.
    #[error("the file has no video")]
    NoVideo,
    /// This computer has no H.264 encoder (Linux, for now).
    #[error("this computer has no H.264 encoder")]
    NoEncoder,
    /// Stopped by its `cancel`.
    #[error("stopped")]
    Stopped,
    /// Reading, decoding, encoding or writing failed.
    #[error("{0}")]
    Failed(String),
}

/// The copy's picture size for a source shown at `shown` (width, height): fitted in `fit` turned the source's way,
/// never enlarged, each side even (H.264's 4:2:0 needs it) and at least 2.
pub fn fitted_size(shown: (u32, u32), fit: (u32, u32)) -> (u32, u32) {
    let (w, h) = (f64::from(shown.0.max(1)), f64::from(shown.1.max(1)));
    let (long, short) = (f64::from(fit.0), f64::from(fit.1));
    let (bw, bh) = if w >= h { (long, short) } else { (short, long) };
    let scale = (bw / w).min(bh / h).min(1.0);
    let even = |x: f64| ((x.round() as u32) / 2 * 2).max(2);
    (even(w * scale), even(h * scale))
}

/// Writes the copy of `spec.range` of `src` to `out` (an MP4), first to a temporary file beside it that is renamed
/// into place once whole (removed on failure), handing `every` each decoded frame. Blocking; `cancel` is checked between
/// packets.
pub fn write(
    src: &Path,
    out: &Path,
    spec: &SmallCopy,
    cancel: &AtomicBool,
    mut every: Option<&mut dyn EveryFrame>,
) -> Result<SmallCopyInfo, SmallCopyError> {
    let _ = ffmpeg::init();
    if spec.range.end <= spec.range.start || spec.range.start < 0 {
        return Err(SmallCopyError::Failed("an empty range".into()));
    }
    let tmp = partial_path(out);
    let mut written = write_to(
        src,
        &tmp,
        spec,
        cancel,
        cfg!(target_os = "macos"),
        every.as_deref_mut(),
    );
    if matches!(written, Err(SmallCopyError::Failed(_))) && cfg!(target_os = "macos") {
        // The hardware decoder may refuse a stream part-way: once more in software.
        tracing::info!(error = ?written, "small copy: again with the software decoder");
        if let Some(every) = every.as_deref_mut() {
            every.again();
        }
        written = write_to(src, &tmp, spec, cancel, false, every);
    }
    let finished = written.and_then(|info| {
        std::fs::rename(&tmp, out).map_err(|e| SmallCopyError::Failed(format!("rename: {e}")))?;
        Ok(info)
    });
    if finished.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    finished
}

/// `<out>.partial.mp4`: the same container, so the muxer is the same.
fn partial_path(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".partial.mp4");
    out.with_file_name(name)
}

fn failed(what: &str, e: impl std::fmt::Display) -> SmallCopyError {
    SmallCopyError::Failed(format!("{what}: {e}"))
}

/// Everything one attempt holds while it writes.
struct Writer<'a> {
    output: Output,
    video: H264Encoder,
    video_tb: Rational,
    encoder_tb: Rational,
    /// (encoder, the decode thread's blocks, samples written).
    sound: Option<(AACEncoder, Receiver<Vec<f32>>, u64)>,
    spec: &'a SmallCopy,
    /// The decoded frame's size and format the scaler was made for.
    scaler: Option<(scaling::Context, (Pixel, u32, u32))>,
    /// The picture before its turn (stored orientation), and after it.
    scaled: ffmpeg::frame::Video,
    upright: ffmpeg::frame::Video,
    rotation: u16,
    /// Drop frames to keep to `max_fps`; the last kept frame's slot (`max_fps` slots a second from the copy's 0).
    thin: bool,
    last_slot: Option<i64>,
    frames: u64,
}

fn write_to(
    src: &Path,
    tmp: &Path,
    spec: &SmallCopy,
    cancel: &AtomicBool,
    hardware_decode: bool,
    every: Option<&mut (dyn EveryFrame + '_)>,
) -> Result<SmallCopyInfo, SmallCopyError> {
    let probe = probe_media(src).map_err(|e| failed("probe", e))?;
    if probe.kind != MediaKind::Video {
        return Err(SmallCopyError::NoVideo);
    }
    let choice = *candidates(CodecRole::H264)
        .iter()
        .find(|c| ffmpeg::encoder::find_by_name(c.ffmpeg_name).is_some())
        .ok_or(SmallCopyError::NoEncoder)?;
    let (width, height) = fitted_size((probe.width, probe.height), spec.fit);
    let rotation = probe.rotation % 360;
    let stored = if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    };
    let fps = if probe.fps > 0.0 {
        probe.fps.min(spec.max_fps)
    } else {
        spec.max_fps
    };
    let thin = probe.fps > spec.max_fps * 1.01;

    let mut output = ffmpeg::format::output_as(tmp, "mp4").map_err(|e| failed("create", e))?;
    let info = VideoInfo {
        pixel_format: Pixel::NV12,
        width,
        height,
        time_base: Rational(1, 1_000_000),
        frame_rate: Rational::from(fps),
    };
    let build = |software: bool, output: &mut Output| {
        H264Encoder::builder(info)
            .with_encoder_name(choice.ffmpeg_name)
            .with_bitrate(spec.video_bps)
            .with_gop(((fps * GOP_SECONDS).round() as u32).max(1))
            .with_options(background_options(&choice))
            .with_software_only(software)
            .build(output)
    };
    let video = match build(false, &mut output) {
        Ok(encoder) => encoder,
        // A Mac without a media engine for this size: VideoToolbox's software encoder.
        Err(_) if choice.backend == Backend::VideoToolbox => {
            build(true, &mut output).map_err(|e| failed("encoder", e))?
        }
        Err(e) => return Err(failed("encoder", e)),
    };
    let sound_decodes = probe.has_audio && spec.audio_bps > 0 && decode::probe(src).is_ok();
    let sound_encoder = if sound_decodes {
        let info = AudioInfo::new_raw(Sample::F32(SampleType::Packed), RATE, 1);
        Some(
            AACEncoder::init(info, spec.audio_bps as usize, &mut output)
                .map_err(|e| failed("sound encoder", e))?,
        )
    } else {
        None
    };
    let mut options = ffmpeg::Dictionary::new();
    options.set("movflags", "+faststart");
    output
        .write_header_with(options)
        .map_err(|e| failed("header", e))?;
    let video_tb = output
        .stream(0)
        .map(|s| s.time_base())
        .unwrap_or(Rational(1, 90_000));
    let encoder_tb = video.time_base();

    // The sound is decoded on its own thread and handed over in blocks, so the two tracks interleave.
    let (feeder, sound) = match sound_encoder {
        Some(encoder) => {
            let (tx, rx) = sync_channel(4);
            let (src, range, lead) = (src.to_path_buf(), spec.range, probe.audio_lead_us);
            let thread = std::thread::Builder::new()
                .name("klipolo-small-copy-sound".into())
                .spawn(move || feed_sound(&src, range, lead, &tx))
                .map_err(|e| failed("sound thread", e))?;
            (Some(thread), Some((encoder, rx, 0)))
        }
        None => (None, None),
    };
    let mut copy = Writer {
        output,
        video,
        video_tb,
        encoder_tb,
        sound,
        spec,
        scaler: None,
        scaled: ffmpeg::frame::Video::new(Pixel::NV12, stored.0, stored.1),
        upright: ffmpeg::frame::Video::new(Pixel::NV12, width, height),
        rotation,
        thin,
        last_slot: None,
        frames: 0,
    };
    let pictures = copy.pictures(src, cancel, hardware_decode, every);
    let finished = pictures.and_then(|()| copy.finish());
    // Ends the decode thread (its next block has nowhere to go), whatever happened here.
    let has_audio = copy.sound.as_ref().is_some_and(|(_, _, n)| *n > 0);
    drop(copy.sound.take());
    if let Some(thread) = feeder {
        match thread.join() {
            Ok(Err(e)) => tracing::warn!(%e, "small copy: the sound stopped early"),
            Err(_) => tracing::warn!("small copy: the sound thread panicked"),
            Ok(Ok(())) => {}
        }
    }
    finished?;
    let bytes = std::fs::metadata(tmp).map_err(|e| failed("size", e))?.len();
    Ok(SmallCopyInfo {
        width,
        height,
        fps,
        frames: copy.frames,
        has_audio,
        bytes,
    })
}

impl Writer<'_> {
    /// Decodes the source's picture over the range and encodes each kept frame, the sound alongside.
    fn pictures(
        &mut self,
        src: &Path,
        cancel: &AtomicBool,
        hardware_decode: bool,
        mut every: Option<&mut (dyn EveryFrame + '_)>,
    ) -> Result<(), SmallCopyError> {
        let range = self.spec.range;
        let mut grey = every.as_ref().map(|e| {
            let (width, height) = e.size();
            Grey::new(width, height)
        });
        let mut input = crate::media::open::input(src).map_err(|e| failed("open", e))?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .filter(|s| !s.disposition().contains(Disposition::ATTACHED_PIC))
            .ok_or(SmallCopyError::NoVideo)?;
        let (index, tb, params, start) = (
            stream.index(),
            stream.time_base(),
            stream.parameters(),
            stream_start(&stream),
        );
        let seconds = f64::from(tb);
        let (mut decoder, hardware) = open_decoder(&params, tb, hardware_decode)?;
        // Whether a frame has come out yet: the first tells whether the hardware decoder started.
        let mut decoded = false;
        set_discards(&mut input, Some(index), None);
        if range.start > 0 {
            // In AV_TIME_BASE (µs), as seeks are: the range's start on the container's clock.
            let ts = range.start + (start as f64 * seconds * 1e6).round() as i64;
            if input.seek(ts, ..ts).is_err() {
                // It can't seek there: read from the start (the frames before the range are passed over).
                input = crate::media::open::input(src).map_err(|e| failed("open", e))?;
                set_discards(&mut input, Some(index), None);
            }
        }
        let mut frame = ffmpeg::frame::Video::empty();
        // The latest frame before the range: on screen at its start.
        let mut held: Option<ffmpeg::frame::Video> = None;
        let mut started = false;
        let mut ended = false;
        let mut flushed = false;
        while !ended {
            if cancel.load(Ordering::Relaxed) {
                return Err(SmallCopyError::Stopped);
            }
            if !flushed {
                match next_packet(&mut input) {
                    Ok(Some((stream, packet))) if stream == index => {
                        if decoder.send_packet(&packet).is_err() && hardware.is_some() {
                            return Err(SmallCopyError::Failed(
                                "the hardware decoder refused".into(),
                            ));
                        }
                    }
                    Ok(Some(_)) => continue,
                    Ok(None) => {
                        let _ = decoder.send_eof();
                        flushed = true;
                    }
                    Err(e) => return Err(failed("read", e)),
                }
            }
            loop {
                match decoder.receive_frame(&mut frame) {
                    Ok(()) => {}
                    Err(ffmpeg::Error::Eof) => {
                        ended = true;
                        break;
                    }
                    Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                        if flushed {
                            ended = true;
                        }
                        break;
                    }
                    Err(e) if flushed => {
                        tracing::debug!(%e, "small copy: the decoder's last frames");
                        ended = true;
                        break;
                    }
                    Err(e) => return Err(failed("decode", e)),
                }
                if !decoded {
                    decoded = true;
                    // SAFETY: a plain field of a live frame.
                    let on_gpu = unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() };
                    if hardware.is_some() && !on_gpu {
                        // FFmpeg fell back to software by itself, on one thread: make the copy again on all cores.
                        return Err(SmallCopyError::Failed(
                            "the hardware decoder didn't start".into(),
                        ));
                    }
                }
                let Some(pts) = frame.timestamp().or_else(|| frame.pts()) else {
                    continue;
                };
                let at = ((pts - start) as f64 * seconds * 1e6).round() as Us;
                let software = take_software(&mut frame);
                if at < range.end
                    && let (Some(every), Some(grey)) = (every.as_deref_mut(), grey.as_mut())
                {
                    grey.of(&software).map_err(SmallCopyError::Failed)?;
                    every.see(at, grey.pixels());
                }
                if at < range.start {
                    held = Some(software);
                    continue;
                }
                if at >= range.end {
                    ended = true;
                    break;
                }
                if !started {
                    started = true;
                    if at > range.start
                        && let Some(first) = held.take()
                    {
                        self.frame(&first, 0)?;
                    }
                }
                self.frame(&software, at - range.start)?;
            }
        }
        if !started && let Some(last) = held.take() {
            // A range inside one long frame (a still image's or a slideshow's).
            self.frame(&last, 0)?;
        }
        if self.frames == 0 {
            return Err(SmallCopyError::Failed("no picture in this range".into()));
        }
        Ok(())
    }

    /// Scales, turns and encodes `frame` at `at` (µs from the copy's 0), unless it is thinned out; the sound first, up
    /// to its time.
    fn frame(&mut self, frame: &ffmpeg::frame::Video, at: Us) -> Result<(), SmallCopyError> {
        if self.thin {
            let slot = (at as f64 * self.spec.max_fps / 1e6).floor() as i64;
            if self.last_slot.is_some_and(|last| slot <= last) {
                return Ok(());
            }
            self.last_slot = Some(slot);
        }
        self.sound_until(at)?;
        let key = (frame.format(), frame.width(), frame.height());
        if self.scaler.as_ref().is_none_or(|(_, made)| *made != key) {
            let context = scaling::Context::get(
                key.0,
                key.1,
                key.2,
                Pixel::NV12,
                self.scaled.width(),
                self.scaled.height(),
                scaling::Flags::BILINEAR,
            )
            .map_err(|e| failed("scaler", e))?;
            self.scaler = Some((context, key));
        }
        let (scaler, _) = self.scaler.as_mut().expect("made above");
        scaler
            .run(frame, &mut self.scaled)
            .map_err(|e| failed("scale", e))?;
        let picture = if self.rotation == 0 {
            &mut self.scaled
        } else {
            turn_nv12(&self.scaled, &mut self.upright, self.rotation);
            &mut self.upright
        };
        let pts = (at as f64 * f64::from(self.encoder_tb.invert()) / 1e6).round() as i64;
        let (output, video_tb, encoder_tb) = (&mut self.output, self.video_tb, self.encoder_tb);
        self.video
            .encode_to(picture, pts, false, &mut |packet| {
                write_packet(packet, output, encoder_tb, video_tb)
            })
            .map_err(|e| failed("encode", e))?;
        self.frames += 1;
        Ok(())
    }

    /// Encodes the sound up to `at` (µs from the copy's 0), as far as the decode thread has it.
    fn sound_until(&mut self, at: Us) -> Result<(), SmallCopyError> {
        let want = (at.max(0) as u64) * u64::from(RATE) / 1_000_000;
        let Some((encoder, blocks, written)) = &mut self.sound else {
            return Ok(());
        };
        while *written < want {
            let Ok(block) = blocks.recv() else {
                // The sound ended (or stopped early): the picture goes on.
                break;
            };
            let n = block.len();
            let mut frame =
                ffmpeg::frame::Audio::new(Sample::F32(SampleType::Packed), n, ChannelLayout::MONO);
            frame.set_rate(RATE);
            frame.set_pts(Some(*written as i64));
            frame.data_mut(0)[..n * 4].copy_from_slice(bytemuck::cast_slice(&block));
            encoder
                .send_frame(frame, Duration::MAX, &mut self.output)
                .map_err(|e| failed("encode sound", e))?;
            *written += n as u64;
        }
        Ok(())
    }

    /// The rest of the sound, both encoders' last packets and the MP4's index.
    fn finish(&mut self) -> Result<(), SmallCopyError> {
        self.sound_until(self.spec.range.end - self.spec.range.start)?;
        let (output, video_tb, encoder_tb) = (&mut self.output, self.video_tb, self.encoder_tb);
        self.video
            .flush_to(&mut |packet| write_packet(packet, output, encoder_tb, video_tb))
            .map_err(|e| failed("finish picture", e))?;
        if let Some((encoder, _, _)) = &mut self.sound {
            encoder
                .flush(&mut self.output)
                .map_err(|e| failed("finish sound", e))?;
        }
        self.output
            .write_trailer()
            .map_err(|e| failed("finish file", e))
    }
}

/// One picture packet into the MP4 (stream 0), in its time base.
fn write_packet(
    packet: &mut Packet,
    output: &mut Output,
    from: Rational,
    to: Rational,
) -> Result<(), ffmpeg::Error> {
    packet.set_stream(0);
    packet.rescale_ts(from, to);
    packet.write_interleaved(output)
}

/// The source's decoder: the platform's hardware one when asked for and the codec has one there (`Hardware`), else
/// software with FFmpeg's frame threads, on every core. Threads are set before the decoder opens, so the hardware try
/// goes without them; should the hardware not start, the first frame says so and `write` makes the copy again here.
fn open_decoder(
    params: &ffmpeg::codec::Parameters,
    tb: Rational,
    hardware: bool,
) -> Result<(ffmpeg::decoder::Video, Option<Hardware>), SmallCopyError> {
    let open = |threads: bool| {
        let mut context = ffmpeg::codec::Context::from_parameters(params.clone())
            .map_err(|e| failed("decoder", e))?;
        if threads {
            context.set_threading(threading::Config::kind(threading::Type::Frame));
        }
        let mut decoder = context
            .decoder()
            .video()
            .map_err(|e| failed("decoder", e))?;
        decoder.set_packet_time_base(tb);
        Ok::<_, SmallCopyError>(decoder)
    };
    if hardware {
        let mut decoder = open(false)?;
        if let Some(device) = Hardware::attach(&mut decoder) {
            return Ok((decoder, Some(device)));
        }
    }
    Ok((open(true)?, None))
}

/// The decoded frame as a software frame of our own: a hardware frame downloaded, else the decoder's frame itself (moved
/// out, no copy; the decoder fills an empty one next).
fn take_software(frame: &mut ffmpeg::frame::Video) -> ffmpeg::frame::Video {
    // SAFETY: a plain field of a live frame.
    let on_gpu = unsafe { !(*frame.as_ptr()).hw_frames_ctx.is_null() };
    if on_gpu {
        let mut download = ffmpeg::frame::Video::empty();
        // SAFETY: both frames are live; FFmpeg allocates the download's buffers on the transfer.
        let copied = unsafe {
            ffmpeg::ffi::av_hwframe_transfer_data(download.as_mut_ptr(), frame.as_ptr(), 0)
        };
        if copied >= 0 {
            return download;
        }
    }
    std::mem::replace(frame, ffmpeg::frame::Video::empty())
}

/// Decodes the sound for the copy's `range` (48 kHz, mixed to mono) and sends it in blocks, from the copy's time 0:
/// sound time = file time − `lead_us`. Ends when the range is sent, the sound ends, or nobody takes the blocks.
fn feed_sound(
    src: &Path,
    range: RangeUs,
    lead_us: Us,
    tx: &SyncSender<Vec<f32>>,
) -> Result<(), String> {
    let frames = |us: Us| (i128::from(us) * i128::from(RATE) / 1_000_000) as i64;
    let (from, until) = (frames(range.start - lead_us), frames(range.end - lead_us));
    if until <= 0 {
        return Ok(());
    }
    // The sound starts after the copy's 0: silence until it does.
    let mut silence = (-from).max(0) as usize;
    while silence > 0 {
        let n = silence.min(BLOCK);
        if tx.send(vec![0.0; n]).is_err() {
            return Ok(());
        }
        silence -= n;
    }
    let channels = usize::from(decode::probe(src)?.channels.max(1));
    let (from, until) = (from.max(0) as u64, until as u64);
    // The next source frame to send: what a block holds before it is trimmed, a gap before a block is silence.
    let mut next = from;
    let mut gone = false;
    let mut emit = |first: u64, samples: &[f32]| -> Result<bool, String> {
        while next < first.min(until) {
            let n = (first.min(until) - next).min(BLOCK as u64) as usize;
            if tx.send(vec![0.0; n]).is_err() {
                gone = true;
                return Ok(false);
            }
            next += n as u64;
        }
        let count = (samples.len() / channels) as u64;
        let skip = next.saturating_sub(first) as usize;
        let take = (first + count).min(until).saturating_sub(next);
        let mut mono = samples
            .chunks_exact(channels)
            .skip(skip)
            .take(take as usize)
            .map(|f| f.iter().sum::<f32>() / channels as f32);
        loop {
            let block: Vec<f32> = mono.by_ref().take(BLOCK).collect();
            if block.is_empty() {
                break;
            }
            if tx.send(block).is_err() {
                gone = true;
                return Ok(false);
            }
        }
        next += take;
        Ok(next < until)
    };
    let decoded = match decode::decode(src, from, Some(until), &mut emit) {
        // The container can't seek there: from the start, trimmed to the range.
        Err(decode::DecodeError::CantSeek) => decode::decode(src, 0, Some(until), &mut emit),
        other => other,
    };
    match decoded {
        Ok(()) => Ok(()),
        Err(_) if gone => Ok(()),
        Err(decode::DecodeError::CantSeek) => Err("the sound can't be read from there".into()),
        Err(decode::DecodeError::Unreadable(e) | decode::DecodeError::Failed(e)) => Err(e),
    }
}

/// Turns an NV12 picture `degrees` clockwise (90, 180 or 270) into `dst` (its sides swapped for a quarter turn).
fn turn_nv12(src: &ffmpeg::frame::Video, dst: &mut ffmpeg::frame::Video, degrees: u16) {
    let (w, h) = (src.width() as usize, src.height() as usize);
    for (plane, size, px) in [(0, (w, h), 1), (1, (w / 2, h / 2), 2)] {
        let (src_stride, dst_stride) = (src.stride(plane), dst.stride(plane));
        turn_plane(
            src.data(plane),
            src_stride,
            size,
            px,
            degrees,
            dst.data_mut(plane),
            dst_stride,
        );
    }
}

/// Turns a plane of `size` (width, height) pixels of `px` bytes each (1 or 2) `degrees` clockwise into `dst`.
fn turn_plane(
    src: &[u8],
    src_stride: usize,
    size: (usize, usize),
    px: usize,
    degrees: u16,
    dst: &mut [u8],
    dst_stride: usize,
) {
    if px == 2 {
        turn::<2>(src, src_stride, size, degrees, dst, dst_stride);
    } else {
        turn::<1>(src, src_stride, size, degrees, dst, dst_stride);
    }
}

/// Each source row goes to a column (a quarter turn) or a row (a half turn) of `dst`. The bounds are checked once per
/// plane and the pixels copied through raw pointers: a checked copy per pixel made a 360p frame take about 10 ms in a
/// dev build, where this takes a fraction of one.
fn turn<const PX: usize>(
    src: &[u8],
    src_stride: usize,
    (w, h): (usize, usize),
    degrees: u16,
    dst: &mut [u8],
    dst_stride: usize,
) {
    if w == 0 || h == 0 {
        return;
    }
    let (dw, dh) = if degrees % 180 == 90 { (h, w) } else { (w, h) };
    assert!(w * PX <= src_stride && (h - 1) * src_stride + w * PX <= src.len());
    assert!(dw * PX <= dst_stride && (dh - 1) * dst_stride + dw * PX <= dst.len());
    let (ds, px) = (dst_stride as isize, PX as isize);
    let (from, to) = (src.as_ptr(), dst.as_mut_ptr());
    for y in 0..h {
        // Where pixel 0 of source row `y` goes, and how far apart its pixels land.
        let (first, step) = match degrees {
            90 => ((h - 1 - y) * PX, ds),
            180 => ((h - 1 - y) * dst_stride + (w - 1) * PX, -px),
            270 => ((w - 1) * dst_stride + y * PX, -ds),
            _ => (y * dst_stride, px),
        };
        let mut s = from.wrapping_add(y * src_stride);
        let mut d = to.wrapping_add(first);
        let mut x = 0;
        while x < w {
            // SAFETY: x < w and y < h, so the source byte lies inside the source plane, and the turned pixel inside the
            // destination plane of `dw` × `dh`: both checked by the asserts above.
            unsafe {
                *d = *s;
                if PX == 2 {
                    *d.wrapping_add(1) = *s.wrapping_add(1);
                }
            }
            s = s.wrapping_add(PX);
            d = d.wrapping_offset(step);
            x += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_fits_its_box_turned_its_way_and_is_never_enlarged() {
        let box_ = (640, 360);
        assert_eq!(fitted_size((1920, 1080), box_), (640, 360));
        assert_eq!(fitted_size((1080, 1920), box_), (360, 640));
        assert_eq!(fitted_size((3840, 2160), box_), (640, 360));
        // 4:3 is limited by its short side; a wide picture by its long side.
        assert_eq!(fitted_size((1440, 1080), box_), (480, 360));
        assert_eq!(fitted_size((2560, 1080), box_), (640, 270));
        assert_eq!(fitted_size((1000, 1000), box_), (360, 360));
        // Small and odd sizes: never enlarged, always even.
        assert_eq!(fitted_size((320, 240), box_), (320, 240));
        // 641 × 361 to 360 tall is 639.2 wide: 638, even.
        assert_eq!(fitted_size((641, 361), box_), (638, 360));
        assert_eq!(fitted_size((333, 177), box_), (332, 176));
        assert_eq!(fitted_size((1, 1), box_), (2, 2));
    }

    /// A 3 × 2 plane of one-byte pixels, and of two-byte pixels (NV12's chroma pairs), turned each way.
    #[test]
    fn planes_turn_clockwise() {
        // a b c
        // d e f
        let src = *b"abcdef";
        let turned = |degrees: u16| {
            let (w, h) = if degrees % 180 == 90 { (2, 3) } else { (3, 2) };
            let mut dst = vec![b'.'; w * h];
            turn_plane(&src, 3, (3, 2), 1, degrees, &mut dst, w);
            String::from_utf8(dst).unwrap()
        };
        // d a
        // e b
        // f c
        assert_eq!(turned(90), "daebfc");
        assert_eq!(turned(180), "fedcba");
        assert_eq!(turned(270), "cfbead");

        // Two bytes a pixel stay together: [ab cd] over [ef gh], turned a quarter clockwise.
        let pairs = *b"abcdefgh";
        let mut dst = vec![b'.'; 8];
        turn_plane(&pairs, 4, (2, 2), 2, 90, &mut dst, 4);
        assert_eq!(&dst, b"efabghcd");

        // Padded rows, as FFmpeg's planes have: each row's padding is read past and left alone.
        let padded = *b"abc#def#";
        for (degrees, want) in [(90, "da#eb#fc#"), (180, "fed##cba##"), (270, "cf#be#ad#")] {
            let (w, stride) = if degrees % 180 == 90 { (2, 3) } else { (3, 5) };
            let rows = if degrees % 180 == 90 { 3 } else { 2 };
            let mut dst = vec![b'#'; stride * rows];
            turn_plane(&padded, 4, (3, 2), 1, degrees, &mut dst, stride);
            assert_eq!(
                String::from_utf8(dst).unwrap(),
                want,
                "{degrees} with {w} a row"
            );
        }
    }
}
