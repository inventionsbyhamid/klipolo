//! Sound from ScreenCaptureKit: an app's or a game's own sound, which comes on its screen pipe's stream
//! (`MacSoundPipe::sharing`), and the whole computer's sound without Klipolo's (`MacSoundPipe::computer`), from a display
//! stream of its own at 2×2 pixels and 1 fps with no cursor (OBS's audio-only lesson).
//!
//! The stream's audio arrives as 48 kHz Float32, one buffer per channel, on the stream's queue; each buffer becomes an
//! interleaved stereo `PcmBlock` stamped with its own presentation time on the host clock and is `try_send`ed (a full
//! channel drops it, counted). A sound pipe that shares a screen pipe's stream starts once that stream runs, or fails
//! at once with the screen's error (`SharedStream`), and sends nothing of its own: the screen pipe reports the stream's
//! stops, and its restart brings the sound back.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cidre::cm;

use super::filter;
use super::host_time::host_ns_of;
use crate::guard::assert_real_capture_allowed;
use crate::media::{AudioFormat, PcmBlock};
use crate::pipe::{AudioOut, AudioPipe, AudioStats, PipeError};
use crate::platform::common::sck::{interleave, pipe_event_for};
use crate::platform::common::shared_stream::SharedStream;
use crate::vendor::cap::sck::{Capturer, Frame, StreamCfgBuilder};

/// What every ScreenCaptureKit sound stream is set to.
pub(super) const FORMAT: AudioFormat = AudioFormat {
    rate: 48_000,
    channels: 2,
};
/// A backstop for a sound pipe waiting on the stream it shares: the screen pipe reports every outcome of its start,
/// which may wait as long as the person takes over macOS's prompt.
const SHARED_BACKSTOP: Duration = Duration::from_secs(120);
const STOP_WAIT: Duration = Duration::from_secs(1);
const START_WAIT: Duration = Duration::from_secs(10);

/// A stream's sound and the sound pipe that takes it.
#[derive(Default)]
pub(super) struct SoundLink {
    out: Mutex<Option<AudioOut>>,
    /// The stream's start, as the screen pipe reports it.
    pub(super) stream: SharedStream,
    callbacks: AtomicU64,
    frames: AtomicU64,
    dropped: AtomicU64,
    /// A buffer that wasn't Float32 was logged once.
    odd_logged: std::sync::atomic::AtomicBool,
}

impl SoundLink {
    /// One audio buffer from the stream's queue, to the sound pipe if one is started (else it is dropped uncounted).
    pub(super) fn audio(&self, sample: &cm::SampleBuf) {
        let out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(out) = out.as_ref() else {
            return;
        };
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        let Some(samples) = self.stereo(sample) else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        self.frames
            .fetch_add((samples.len() / 2) as u64, Ordering::Relaxed);
        let block = PcmBlock {
            host_ns: host_ns_of(sample.pts()),
            samples,
        };
        if out.blocks.try_send(block).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The buffer as interleaved stereo: two planar channels interleaved, one channel doubled, or already interleaved.
    fn stereo(&self, sample: &cm::SampleBuf) -> Option<Vec<f32>> {
        let float = sample
            .format_desc()
            .and_then(|d| d.stream_basic_desc())
            .is_some_and(|a| a.bits_per_channel == 32 && a.format_flags.0 & 1 != 0);
        if !float {
            if !self.odd_logged.swap(true, Ordering::Relaxed) {
                tracing::warn!("sound: a buffer that isn't Float32 is dropped");
            }
            return None;
        }
        let list = sample.audio_buf_list::<2>().ok()?;
        let list = list.list();
        let plane = |i: usize| -> &[f32] {
            let b = &list.buffers[i];
            // SAFETY: the block buffer holds `data_bytes_size` bytes of Float32 at `data` while `list` lives.
            unsafe {
                std::slice::from_raw_parts(b.data.cast::<f32>(), b.data_bytes_size as usize / 4)
            }
        };
        match (list.number_buffers, list.buffers[0].number_channels) {
            (2, _) => Some(interleave(plane(0), plane(1))),
            (1, 2) => Some(plane(0).to_vec()),
            (1, 1) => Some(interleave(plane(0), plane(0))),
            _ => None,
        }
    }

    fn take(&self, out: Option<AudioOut>) {
        *self.out.lock().unwrap_or_else(PoisonError::into_inner) = out;
    }

    fn stats(&self) -> AudioStats {
        AudioStats {
            callbacks: self.callbacks.load(Ordering::Relaxed),
            frames: self.frames.load(Ordering::Relaxed),
            dropped_channel: self.dropped.load(Ordering::Relaxed),
        }
    }
}

pub struct MacSoundPipe {
    link: Arc<SoundLink>,
    /// The computer's sound: its own stream, made at each start.
    own: Option<Option<Capturer>>,
}

impl MacSoundPipe {
    /// The sound of a screen pipe's stream (`link` is that pipe's). Opens nothing.
    pub(super) fn sharing(link: Arc<SoundLink>) -> Self {
        assert_real_capture_allowed("the app's or game's sound");
        Self { link, own: None }
    }

    /// The whole computer's sound without Klipolo's. Opens nothing until `start`.
    pub(super) fn computer() -> Self {
        assert_real_capture_allowed("the computer's sound");
        Self {
            link: Arc::default(),
            own: Some(None),
        }
    }

    /// The computer's own stream: the main display without Klipolo, 2×2 at 1 fps, its sound only.
    fn own_stream(&self, out: &AudioOut) -> Result<Capturer, PipeError> {
        let content = filter::content()?;
        let picked = filter::main_display_without_klipolo(&content)?;
        let mut cfg = StreamCfgBuilder::default();
        cfg.set_width(2);
        cfg.set_height(2);
        cfg.set_shows_cursor(false);
        cfg.set_frame_interval(cm::Time::new(1, 1));
        cfg.set_captures_audio(true);
        cfg.set_excludes_current_process_audio(true);
        cfg.set_sample_rate(FORMAT.rate.into());
        cfg.set_channel_count(2);
        let (link, events) = (self.link.clone(), out.events.clone());
        let capturer = Capturer::builder(picked.filter, cfg.build())
            .with_output_sample_buf_cb(move |frame| {
                if let Frame::Audio(a) = frame {
                    link.audio(a.sample_buf());
                }
            })
            .with_stop_with_err_cb(move |_, error| {
                let _ = events.try_send(pipe_event_for(error.code()));
            })
            .build()
            .map_err(|e| PipeError::Os(format!("the sound stream: {}", e.code())))?;
        if out.stopping.is_set() {
            return Err(PipeError::Os("stopped before the sound opened".into()));
        }
        match capturer.start_within(START_WAIT) {
            Ok(()) => Ok(capturer),
            Err(Some(e)) => Err(super::screen::start_error(e.code())),
            Err(None) => {
                // It may still start: stopped, never left running unowned.
                capturer.stop_within(STOP_WAIT);
                Err(PipeError::Os("the sound stream didn't start".into()))
            }
        }
    }
}

impl AudioPipe for MacSoundPipe {
    fn start(&mut self, out: AudioOut) -> Result<AudioFormat, PipeError> {
        assert_real_capture_allowed("ScreenCaptureKit's sound");
        self.stop();
        if self.own.is_some() {
            let capturer = self.own_stream(&out)?;
            self.own = Some(Some(capturer));
            self.link.stream.running();
        }
        let stopping = out.stopping.clone();
        self.link.take(Some(out));
        if let Err(e) = self.link.stream.wait(&stopping, SHARED_BACKSTOP) {
            self.link.take(None);
            return Err(e);
        }
        Ok(FORMAT)
    }

    fn stop(&mut self) {
        self.link.take(None);
        if let Some(Some(capturer)) = self.own.as_mut().map(Option::take) {
            if !capturer.stop_within(STOP_WAIT) {
                tracing::warn!("sound: the computer's stream didn't stop within 1 s");
            }
            self.link.stream.idle();
        }
    }

    fn stats(&self) -> AudioStats {
        self.link.stats()
    }
}

impl Drop for MacSoundPipe {
    fn drop(&mut self) {
        self.stop();
    }
}
