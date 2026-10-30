//! The FFmpeg activity decoder: every frame of the video stream decoded (every other stream discarded), on the
//! platform's hardware decoder where there is one (`luma::Hardware`: VideoToolbox, D3D11VA, VA-API), else in software.
//! Only the frame on screen at a tick is downloaded and scaled, and only once however many ticks show it (`luma`).
//!
//! **Cost:** one decode per frame (unavoidable for inter-coded video), then per tick at most one download of a hardware
//! frame and one `sws_scale` of the Y plane to 128 × 72. Nothing of ours is allocated per frame: three reused frames
//! (the one on screen, the next one, the one being received), a reused download frame, the scaler, and a 9 KB picture.
//! FFmpeg's demuxer still allocates each packet's data. **Memory:** the decoder's frames (its references plus the two
//! held here: about 25 MB for 1080p H.264 in software) and FFmpeg's buffers.

pub(crate) mod luma;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg::format::stream::Disposition;
use klipolo_cursor::activity::TICK_US;
use klipolo_model::time::Us;

use super::{ActivityError, ActivityFrames, LUMA_BYTES};
use crate::media::scan::{next_packet, set_discards, start_us};
// **The file's clock**, the one origin of every tick, seek and length here: where the video stream starts, in its time
// base (its first pts, or 0 when the file doesn't say), so tick 0 is its first frame: the rule from which every
// every decoder numbers frames (the FFmpeg decoders, AVAssetReader, the thumbnail tiles), and the cursor timeline maps ticks on it.
// Sound that starts before the picture is the recording's `audioLeadUs`, never a shift of this clock.
use crate::render::probe::stream_start;
use luma::{Hardware, Picture};

pub(super) struct FfmpegFrames {
    input: ffmpeg::format::context::Input,
    index: usize,
    decoder: ffmpeg::decoder::Video,
    /// The hardware device the decoder uses (None: software).
    hw: Option<Hardware>,
    /// To reopen the decoder in software when the hardware one refuses the stream.
    params: ffmpeg::codec::Parameters,
    packet_time_base: ffmpeg::Rational,
    /// Seconds per pts unit, and where the file's clock starts (`stream_start`).
    time_base: f64,
    start: i64,
    duration_us: Us,
    cancel: Arc<AtomicBool>,
    /// The next tick to give.
    tick: Us,
    /// The latest frame at or before `tick` (`held_at`: its time; None before the first).
    held: ffmpeg::frame::Video,
    held_at: Option<Us>,
    /// A frame decoded past `tick`, on screen at a later tick.
    ahead: ffmpeg::frame::Video,
    ahead_at: Option<Us>,
    /// Receives each decoded frame.
    frame: ffmpeg::frame::Video,
    /// `held` as 128 × 72 luma, once a tick has needed it.
    picture: Picture,
    /// The end of the stream was sent to the decoder, and it has given its last frame.
    flushed: bool,
    ended: bool,
}

// SAFETY: every FFmpeg object here (the demuxer, the decoder, frames, the hardware device and the scaler) is owned by
// this value alone, and used by one thread at a time; the decoder's format callback keeps no thread state.
unsafe impl Send for FfmpegFrames {}

/// An FFmpeg error opening the file, by what it says: what the file is (a container FFmpeg
/// rejects or has no demuxer for, no decoder for its codec) can't be analysed, for good; anything else (an OS error: a
/// drive gone, no access yet, out of handles) may pass.
fn open_error(e: ffmpeg::Error) -> ActivityError {
    use ffmpeg::Error as E;
    match e {
        E::InvalidData
        | E::DemuxerNotFound
        | E::DecoderNotFound
        | E::StreamNotFound
        | E::ProtocolNotFound
        | E::PatchWelcome => ActivityError::Unsupported(e.to_string()),
        _ => ActivityError::Unavailable(e.to_string()),
    }
}

/// `open_error` for opening `path`'s container, where a refusal of the file (the media guard's, while the OS reads the
/// file: `media::open::refused`) can't be analysed either.
fn input_error(e: ffmpeg::Error, path: &Path) -> ActivityError {
    if crate::media::open::refused(&e, path) {
        ActivityError::Unsupported(e.to_string())
    } else {
        open_error(e)
    }
}

/// What the file is can't be analysed (no video, no length).
fn unsupported(why: &str) -> ActivityError {
    ActivityError::Unsupported(why.into())
}

impl FfmpegFrames {
    /// `path`'s video from the tick at `from_us` (a seek to the keyframe before it).
    pub(super) fn open(
        path: &Path,
        from_us: Us,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, ActivityError> {
        let _ = ffmpeg::init();
        let mut input = crate::media::open::input(&path).map_err(|e| input_error(e, path))?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .filter(|s| !s.disposition().contains(Disposition::ATTACHED_PIC))
            .ok_or_else(|| unsupported("no video"))?;
        let (index, tb, params) = (stream.index(), stream.time_base(), stream.parameters());
        let start = stream_start(&stream);
        let to_us = |t: i64| (t as f64 * f64::from(tb) * 1e6).round() as Us;
        // In AV_TIME_BASE (µs), as seeks are: where the video starts, measured as the container's start is.
        let start_at = to_us(start);
        // The video's own length; without one, the container's end less where the video starts.
        let duration_us = if stream.duration() > 0 {
            to_us(stream.duration())
        } else {
            start_us(&input) + input.duration().max(0) - start_at
        };
        if duration_us <= 0 {
            return Err(unsupported("no length"));
        }
        let mut decoder = software(&params, tb)?;
        let hw = Hardware::attach(&mut decoder);
        set_discards(&mut input, Some(index), None);
        if from_us > 0 {
            let ts = from_us + start_at;
            if input.seek(ts, ..ts).is_err() {
                // It can't seek there: read from the start (the frames before `from_us` are passed over).
                input = crate::media::open::input(&path).map_err(|e| input_error(e, path))?;
                set_discards(&mut input, Some(index), None);
            }
        }
        Ok(Self {
            input,
            index,
            decoder,
            hw,
            params,
            packet_time_base: tb,
            time_base: f64::from(tb),
            start,
            duration_us,
            cancel,
            tick: from_us,
            held: ffmpeg::frame::Video::empty(),
            held_at: None,
            ahead: ffmpeg::frame::Video::empty(),
            ahead_at: None,
            frame: ffmpeg::frame::Video::empty(),
            picture: Picture::default(),
            flushed: false,
            ended: false,
        })
    }

    /// A pts of the stream on the file's clock (`stream_start`).
    fn us(&self, pts: i64) -> Us {
        ((pts - self.start) as f64 * self.time_base * 1e6).round() as Us
    }

    /// The frame past `tick` becomes the one on screen (the one before is let go).
    fn advance(&mut self) {
        std::mem::swap(&mut self.held, &mut self.ahead);
        unref(&mut self.ahead);
        self.held_at = self.ahead_at.take();
        self.picture.stale();
    }

    /// One step of decoding: a frame received (on screen at `tick`, or kept as `ahead`), a packet sent, or the end.
    fn step(&mut self) -> Result<(), ActivityError> {
        match self.decoder.receive_frame(&mut self.frame) {
            Ok(()) => {
                let Some(pts) = self.frame.timestamp().or_else(|| self.frame.pts()) else {
                    return Ok(());
                };
                let at = self.us(pts);
                if at <= self.tick {
                    std::mem::swap(&mut self.held, &mut self.frame);
                    self.held_at = Some(at);
                    self.picture.stale();
                } else {
                    std::mem::swap(&mut self.ahead, &mut self.frame);
                    self.ahead_at = Some(at);
                }
                // Two frames held at most: the one on screen and the next.
                unref(&mut self.frame);
                Ok(())
            }
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => self.feed(),
            Err(ffmpeg::Error::Eof) => {
                self.ended = true;
                Ok(())
            }
            Err(e) if self.flushed => {
                // A decoder that fails while draining has nothing more to give.
                tracing::debug!(%e, "activity: the decoder's last frames");
                self.ended = true;
                Ok(())
            }
            Err(_) if self.hw.is_some() => {
                // The hardware decoder failed: software from here (it picks up at the next keyframe).
                self.hw = None;
                self.decoder = software(&self.params, self.packet_time_base)?;
                Ok(())
            }
            Err(e) => Err(ActivityError::Decode(e.to_string())),
        }
    }

    /// Sends the next packet of the video (damaged ones are passed over), or the end of the stream.
    fn feed(&mut self) -> Result<(), ActivityError> {
        if self.flushed {
            self.ended = true;
            return Ok(());
        }
        match next_packet(&mut self.input) {
            Ok(Some((stream, packet))) if stream == self.index => self.send(&packet),
            Ok(Some(_)) => Ok(()),
            Ok(None) => {
                self.flushed = true;
                let _ = self.decoder.send_eof();
                Ok(())
            }
            Err(e) => Err(ActivityError::Decode(e.to_string())),
        }
    }

    /// A hardware decoder that refuses a packet (a profile its engine lacks) is replaced by a software one.
    fn send(&mut self, packet: &ffmpeg::Packet) -> Result<(), ActivityError> {
        match self.decoder.send_packet(packet) {
            Err(_) if self.hw.is_some() => {
                self.hw = None;
                self.decoder = software(&self.params, self.packet_time_base)?;
                let _ = self.decoder.send_packet(packet);
                Ok(())
            }
            // A damaged packet: the decoder resyncs at a later one.
            _ => Ok(()),
        }
    }
}

/// Lets a frame's buffers go (back to the decoder's pool); no allocation.
fn unref(frame: &mut ffmpeg::frame::Video) {
    // SAFETY: a live frame of ours; unreferencing leaves it empty and reusable.
    unsafe { ffmpeg::ffi::av_frame_unref(frame.as_mut_ptr()) };
}

/// A software decoder for the stream.
fn software(
    params: &ffmpeg::codec::Parameters,
    tb: ffmpeg::Rational,
) -> Result<ffmpeg::decoder::Video, ActivityError> {
    let mut decoder = ffmpeg::codec::Context::from_parameters(params.clone())
        .and_then(|c| c.decoder().video())
        .map_err(open_error)?;
    decoder.set_packet_time_base(tb);
    Ok(decoder)
}

impl ActivityFrames for FfmpegFrames {
    fn next(&mut self, out: &mut [u8; LUMA_BYTES]) -> Result<Option<Us>, ActivityError> {
        if self.tick >= self.duration_us {
            return Ok(None);
        }
        // Until a frame past the tick (or the end) says which frame is on screen at it.
        while self.ahead_at.is_none() && !self.ended {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(ActivityError::Cancelled);
            }
            self.step()?;
        }
        if self.held_at.is_none() {
            if self.ahead_at.is_none() {
                // No frame at all.
                return Ok(None);
            }
            // A tick before the first frame shows the first frame.
            self.advance();
        }
        self.picture.of(&self.held).map_err(ActivityError::Decode)?;
        out.copy_from_slice(self.picture.luma());
        let at = self.tick;
        self.tick += TICK_US;
        if self.ahead_at.is_some_and(|a| a <= self.tick) {
            self.advance();
        }
        Ok(Some(at))
    }

    fn duration_us(&self) -> Us {
        self.duration_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EINVAL on a file the OS can't read now (Windows' C runtime says it for a network
    /// name gone or an online-only cloud file offline) may pass; the same code on a file the OS reads is a refusal.
    #[test]
    fn an_einval_lasts_only_while_the_os_reads_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let einval = ffmpeg::Error::Other {
            errno: ffmpeg::error::EINVAL,
        };
        // Nothing of a directory reads as a file's head (its open or its read fails), nor of a file that is gone.
        for path in [dir.path().to_path_buf(), dir.path().join("gone.mov")] {
            let opened = input_error(einval, &path);
            assert!(
                matches!(opened, ActivityError::Unavailable(_)),
                "{}: {opened:?}",
                path.display()
            );
        }
        let list = dir.path().join("list.mov");
        std::fs::write(&list, "ffconcat version 1.0\nfile screen.mov\n").unwrap();
        let opened = input_error(einval, &list);
        assert!(
            matches!(opened, ActivityError::Unsupported(_)),
            "{opened:?}"
        );
    }
}
