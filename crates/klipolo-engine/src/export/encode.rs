//! One format on its own thread: one VideoToolbox session for the whole run (zero-copy IOSurface input in hardware
//! mode, CPU frames in SoftwareSafe mode), a keyframe forced at each segment's first frame, and the segment files
//! (`SegmentWriter`). The GPU draws the next frame while VideoToolbox encodes this one; a bounded queue keeps memory
//! flat when the encoder is the slower side. The sound is a separate track (`sound.rs`). Editing copies use
//! the same thread, with their own ledger and background options.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use ffmpeg::Packet;
use tokio::sync::{mpsc, oneshot};

use super::codecs::{builder, input_format};
use super::protocol::ErrorCode;
use super::quality::{Container, EncodeSettings};
use super::segment::{SegmentLedger, SegmentPlan, SegmentWriter};
use super::shared::{FrameRate, WorkerError, ff_error, internal};
use crate::frames::buffer::FrameBytes;
use crate::render::yuv::PlaneLayout;
use crate::vendor::cap::enc_ffmpeg::h264::{H264Encoder, H264EncoderError, QueueFrameError};
use crate::vendor::cap::media_info::{FFRational, VideoInfo};

/// Frames queued per encoder (2–4 per format measured best).
const QUEUE: usize = 3;

/// A frame's picture for one encoder.
pub enum VideoIn {
    /// An IOSurface the GPU finished drawing (hardware mode).
    #[cfg(target_os = "macos")]
    Surface(crate::render::yuv::PixelBuffer),
    /// Planes read back to memory (SoftwareSafe mode).
    Planes(FrameBytes, PlaneLayout),
}

/// One frame for one encoder: its index on the timeline and its picture.
pub struct Input {
    pub index: u64,
    pub video: VideoIn,
}

/// Resolves once the encoder is open, with the CRC-32 of its parameter sets.
pub type Opened = oneshot::Receiver<Result<u32, WorkerError>>;

/// What one encoder thread encodes, and how its frames map to segments.
#[derive(Clone, Copy, Debug)]
pub struct EncoderSetup {
    /// Which of the ledger's outputs its segments belong to.
    pub output: usize,
    pub settings: EncodeSettings,
    /// VideoToolbox's software encoder (SoftwareSafe mode).
    pub software: bool,
    /// The picture's size in pixels.
    pub size: (u32, u32),
    /// Frame n has pts n in `rate.frame_base()`.
    pub rate: FrameRate,
    /// Frames per segment: frame n belongs to segment n / segment_frames.
    pub segment_frames: u64,
    /// Extra encoder options (`platform::encode::background_options` for editing copies; none for export).
    pub options: &'static [(&'static str, &'static str)],
}

pub struct EncoderThread {
    tx: Option<mpsc::Sender<Input>>,
    abandon: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<(), WorkerError>>>,
}

fn open_error(e: H264EncoderError) -> WorkerError {
    match e {
        // VideoToolbox couldn't create the session (busy, too many sessions, or no such hardware encoder).
        H264EncoderError::FFmpeg(ffmpeg::Error::External)
        | H264EncoderError::VideoToolboxHwInput(_)
        | H264EncoderError::CodecNotFound => WorkerError {
            code: ErrorCode::EncoderUnavailable,
            message: format!("the encoder couldn't start: {e}"),
            retryable: true,
        },
        H264EncoderError::FFmpeg(e) => ff_error(e, "the encoder couldn't start"),
        e => WorkerError {
            code: ErrorCode::Encode,
            message: format!("the encoder couldn't start: {e}"),
            retryable: true,
        },
    }
}

impl EncoderThread {
    /// Starts `setup.output`'s thread, which opens its encoder (VideoToolbox takes a while per session, so every
    /// format opens at once) and then writes `plans`, the segments this run renders, recording each in `ledger`.
    pub fn start(
        ledger: Arc<dyn SegmentLedger>,
        setup: EncoderSetup,
        plans: Vec<SegmentPlan>,
    ) -> Result<(Self, Opened), WorkerError> {
        let EncoderSetup {
            output,
            settings,
            software,
            size,
            rate,
            segment_frames,
            options,
        } = setup;
        let (tx, rx) = mpsc::channel(QUEUE);
        let (opened_tx, opened) = oneshot::channel();
        let abandon = Arc::new(AtomicBool::new(false));
        let flag = abandon.clone();
        let join = std::thread::Builder::new()
            .name("klipolo-export-encode".into())
            .spawn(move || {
                // Capped by the process's ceiling: utility in the editing-copy worker.
                crate::qos::set_current(crate::qos::Qos::UserInitiated);
                let encoder = match open(size, settings, rate, software, options) {
                    Ok(encoder) => encoder,
                    Err(e) => {
                        let _ = opened_tx.send(Err(e.clone()));
                        return Err(e);
                    }
                };
                let config = crc32fast::hash(encoder.extradata());
                let _ = opened_tx.send(Ok(config));
                let container = match settings.container {
                    Container::Mp4 => "mp4",
                    Container::Mov => "mov",
                };
                let stream = encoder.stream_template();
                let writer = SegmentWriter::new(
                    ledger,
                    output,
                    container,
                    plans,
                    segment_frames,
                    rate,
                    stream,
                    (config, software),
                );
                run(encoder, writer, rx, &flag)
            })
            .map_err(internal)?;
        Ok((
            Self {
                tx: Some(tx),
                abandon,
                join: Some(join),
            },
            opened,
        ))
    }

    /// Queues a frame (waits while the queue is full). A failed encoder reports its error.
    pub async fn push(&mut self, input: Input) -> Result<(), WorkerError> {
        let sent = match &self.tx {
            Some(tx) => tx.send(input).await.is_ok(),
            None => false,
        };
        if sent {
            return Ok(());
        }
        self.tx = None;
        Err(self
            .join()
            .err()
            .unwrap_or_else(|| internal("the encoder stopped")))
    }

    fn join(&mut self) -> Result<(), WorkerError> {
        match self.join.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err(internal("the encoder crashed")),
            None => Err(internal("the encoder already stopped")),
        }
    }

    /// Ends the input: flushes the encoder, closes the last segment if it is complete (else deletes it) and waits
    /// for every segment check. Blocking.
    pub fn finish(mut self) -> Result<(), WorkerError> {
        self.tx = None;
        self.join()
    }
}

impl Drop for EncoderThread {
    /// Dropped unfinished: the open segment is abandoned and deleted (segments already closed stay recorded).
    fn drop(&mut self) {
        if self.join.is_some() {
            self.abandon.store(true, Ordering::SeqCst);
            self.tx = None;
            let _ = self.join();
        }
    }
}

fn open(
    (width, height): (u32, u32),
    settings: EncodeSettings,
    rate: FrameRate,
    software: bool,
    options: &[(&str, &str)],
) -> Result<H264Encoder, WorkerError> {
    let mut info =
        VideoInfo::from_raw_ffmpeg(input_format(settings.codec), width, height, rate.num);
    // Frame n has pts n: an exact rate (24000/1001) keeps its fraction.
    info.frame_rate = FFRational(rate.num as i32, rate.den.max(1) as i32);
    info.time_base = FFRational(rate.den.max(1) as i32, rate.num as i32);
    let builder = builder(info, settings.codec)
        .ok_or_else(|| open_error(H264EncoderError::CodecNotFound))?
        .with_bitrate(settings.bitrate)
        .with_max_bitrate(settings.max_bitrate)
        .with_gop(settings.gop)
        .with_b_frames(settings.b_frames)
        .with_software_only(software)
        .with_options(options);
    #[cfg(target_os = "macos")]
    if !software {
        return builder.open_videotoolbox_hw().map_err(open_error);
    }
    builder.open().map_err(open_error)
}

/// Encodes one frame; packets go to the segment files.
fn encode(
    encoder: &mut H264Encoder,
    writer: &mut SegmentWriter,
    frame: &mut ffmpeg::frame::Video,
    index: u64,
) -> Result<(), WorkerError> {
    let key = writer.starts_segment(index);
    let mut failed = None;
    let result = {
        let mut sink = |p: &mut Packet| {
            writer.write(p).map_err(|e| {
                failed = Some(e);
                ffmpeg::Error::Exit
            })
        };
        encoder.encode_to(frame, index as i64, key, &mut sink)
    };
    if let Some(e) = failed {
        return Err(e);
    }
    result.map_err(|e| match e {
        QueueFrameError::Encode(e) | QueueFrameError::Converter(e) => {
            ff_error(e, "encoding failed")
        }
    })
}

fn flush(encoder: &mut H264Encoder, writer: &mut SegmentWriter) -> Result<(), WorkerError> {
    let mut failed = None;
    let result = {
        let mut sink = |p: &mut Packet| {
            writer.write(p).map_err(|e| {
                failed = Some(e);
                ffmpeg::Error::Exit
            })
        };
        encoder.flush_to(&mut sink)
    };
    if let Some(e) = failed {
        return Err(e);
    }
    result.map_err(|e| ff_error(e, "finishing the encoder"))
}

fn run(
    mut encoder: H264Encoder,
    mut writer: SegmentWriter,
    mut rx: mpsc::Receiver<Input>,
    abandon: &AtomicBool,
) -> Result<(), WorkerError> {
    let mut planes: Option<ffmpeg::frame::Video> = None;
    let result = (|| {
        while let Some(input) = rx.blocking_recv() {
            match input.video {
                #[cfg(target_os = "macos")]
                VideoIn::Surface(mut buffer) => {
                    buffer.fill_padding();
                    // A pool that bounds the frames VideoToolbox uses (ProRes) is woken when it lets this one go.
                    let released = buffer.in_flight().map(|f| f.handed());
                    // SAFETY: the buffer is a live IOSurface CVPixelBuffer of this encoder's size and format; the
                    // wrapped frame retains it until VideoToolbox is done with it.
                    let mut frame = unsafe {
                        encoder.wrap_videotoolbox_pixel_buffer_then(buffer.as_ptr(), released)
                    }
                    .map_err(|e| WorkerError {
                        code: ErrorCode::Encode,
                        message: e.to_string(),
                        retryable: true,
                    })?;
                    encode(&mut encoder, &mut writer, &mut frame, input.index)?;
                }
                VideoIn::Planes(bytes, layout) => {
                    let format = encoder.conversion_requirements().input_format;
                    let frame = planes.get_or_insert_with(|| {
                        ffmpeg::frame::Video::new(format, layout.width, layout.height)
                    });
                    copy_planes(&bytes, layout, frame);
                    encode(&mut encoder, &mut writer, frame, input.index)?;
                }
            }
        }
        Ok(())
    })();
    if result.is_err() || abandon.load(Ordering::SeqCst) {
        writer.abandon();
        return result;
    }
    let flushed = flush(&mut encoder, &mut writer);
    if flushed.is_err() {
        writer.abandon();
        return flushed;
    }
    writer.finish()
}

/// Copies read-back planes (rows of `layout.stride` bytes) into `frame`'s planes.
fn copy_planes(bytes: &[u8], layout: PlaneLayout, frame: &mut ffmpeg::frame::Video) {
    let rows = [
        (0usize, layout.height),
        (layout.uv_offset as usize, layout.uv_rows),
    ];
    for (plane, (offset, count)) in rows.into_iter().enumerate() {
        let dst_stride = frame.stride(plane);
        let width = dst_stride.min(layout.stride as usize);
        let dst = frame.data_mut(plane);
        for row in 0..count as usize {
            let src = offset + row * layout.stride as usize;
            let (Some(s), Some(d)) = (
                bytes.get(src..src + width),
                dst.get_mut(row * dst_stride..row * dst_stride + width),
            ) else {
                break;
            };
            d.copy_from_slice(s);
        }
    }
}
