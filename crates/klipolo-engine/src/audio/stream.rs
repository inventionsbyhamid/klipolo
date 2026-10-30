//! The cpal side of preview audio: the default OUTPUT device (never an input device) and its real-time callback
//! (how late it plays is `klipolo_platform::audio_route::hardware_latency`). Patterns from Cap
//! `crates/editor/src/audio_output.rs` (stream build); not vendored.
//!
//! The callback only pops the ring, converts and stores atomics: no locks, allocation or syscalls.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{FromSample, OutputCallbackInfo, SampleFormat, SizedSample, StreamInstant};
use ringbuf::HeapCons;
use ringbuf::traits::{Consumer, Observer};

use super::clock::{AudioClock, now_ns};
use super::mix::RATE;

/// Frames converted per pass, so the callback's scratch never grows (device buffers are rarely over 4096 frames).
const MAX_BUFFER: usize = 8_192;

/// Mixer ↔ callback flags, all atomics.
#[derive(Default)]
pub struct Shared {
    /// The newest flush the mixer asked for (0: none yet).
    pub flush: AtomicU64,
    /// The newest flush the callback carried out, and the stream frame where sound pushed after it starts.
    pub acked: AtomicU64,
    pub ack_mark: AtomicU64,
    /// The newest pause the mixer asked for, and the newest the callback carried out: it fades out what is playing,
    /// then plays silence (and drops anything queued) until the next flush.
    pub pause: AtomicU64,
    pub paused: AtomicU64,
    /// The stream failed (device gone); set from cpal's error callback.
    pub failed: AtomicBool,
}

pub struct Callback {
    pub cons: HeapCons<f32>,
    pub shared: Arc<Shared>,
    pub clock: Arc<AudioClock>,
    pub channels: usize,
    scratch: Vec<f32>,
    /// Stereo frames of sound owed after an underrun: skipped when it arrives, so it isn't heard late.
    debt: usize,
    flushed: u64,
    paused: u64,
    /// Silent until the next flush (after a pause: what is queued is from before it).
    muted: bool,
    played: u64,
}

impl Callback {
    pub fn new(
        cons: HeapCons<f32>,
        shared: Arc<Shared>,
        clock: Arc<AudioClock>,
        channels: usize,
    ) -> Self {
        Self {
            cons,
            shared,
            clock,
            channels: channels.max(1),
            scratch: vec![0.0; MAX_BUFFER * 2],
            debt: 0,
            flushed: 0,
            paused: 0,
            muted: false,
            played: 0,
        }
    }

    /// One device buffer, due at `host_ns` (on `now_ns`'s clock).
    pub(super) fn fill<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], host_ns: u64) {
        let frames = out.len() / self.channels;
        self.clock.stamp(self.played, host_ns, frames);
        let flush = self.shared.flush.load(Ordering::Acquire);
        let pause = self.shared.pause.load(Ordering::Acquire);
        let (flushing, pausing) = (flush != self.flushed, pause != self.paused);
        if self.muted || (pausing && flushing) {
            // Paused, or resuming after a pause this callback never saw: anything queued is from before the pause.
            self.cons.clear();
            self.debt = 0;
            out.fill(T::EQUILIBRIUM);
            self.played += frames as u64;
        } else {
            // Sound that is about to be cut (a pause, a seek, a re-plan) fades out over this buffer instead.
            let fade = (flushing || pausing).then_some(frames);
            for (k, part) in out.chunks_mut(MAX_BUFFER * self.channels).enumerate() {
                self.fill_part(part, fade.map(|total| (k * MAX_BUFFER, total)));
            }
        }
        if pausing {
            self.cons.clear();
            self.debt = 0;
            self.muted = true;
            self.paused = pause;
            self.shared.paused.store(pause, Ordering::Release);
        }
        // A flush happens after this buffer, so the old sound plays (fading) to its end without a gap.
        if flushing {
            self.cons.clear();
            self.debt = 0;
            self.muted = false;
            self.flushed = flush;
            self.shared.ack_mark.store(self.played, Ordering::Relaxed);
            self.shared.acked.store(flush, Ordering::Release);
        }
    }

    /// Fills `out` from the ring; `fade` = (frames before this part, frames in the buffer) ramps it down to silence.
    fn fill_part<T: SizedSample + FromSample<f32>>(
        &mut self,
        out: &mut [T],
        fade: Option<(usize, usize)>,
    ) {
        let frames = out.len() / self.channels;
        let need = frames * 2;
        take(
            &mut self.cons,
            &mut self.debt,
            self.flushed > 0,
            &mut self.scratch[..need],
        );
        if let Some((before, total)) = fade {
            for (j, lr) in self.scratch[..need].chunks_exact_mut(2).enumerate() {
                let g = 1.0 - (before + j + 1) as f32 / total.max(1) as f32;
                lr[0] *= g;
                lr[1] *= g;
            }
        }
        for (frame, lr) in out
            .chunks_exact_mut(self.channels)
            .zip(self.scratch.chunks_exact(2))
        {
            if self.channels == 1 {
                frame[0] = T::from_sample((lr[0] + lr[1]) * 0.5);
            } else {
                frame[0] = T::from_sample(lr[0]);
                frame[1] = T::from_sample(lr[1]);
                frame[2..].fill(T::EQUILIBRIUM);
            }
        }
        self.played += frames as u64;
    }
}

/// The ring as the callback reads it. The mixer pushes from its own thread at any moment, also between two of these
/// calls; a test stands in for that push.
trait Ring {
    fn queued(&self) -> usize;
    fn skip(&mut self, samples: usize) -> usize;
    fn pop(&mut self, out: &mut [f32]) -> usize;
}

impl Ring for HeapCons<f32> {
    fn queued(&self) -> usize {
        self.occupied_len()
    }
    fn skip(&mut self, samples: usize) -> usize {
        Consumer::skip(self, samples)
    }
    fn pop(&mut self, out: &mut [f32]) -> usize {
        self.pop_slice(out)
    }
}

/// Fills `out` (stereo samples) from `ring`, silence where it runs dry. `debt` is the stereo frames owed after an
/// underrun (counted once `owing`, after the first flush): that much sound is skipped when it arrives, so nothing is
/// heard late. Sound is taken only once the debt is paid: what the mixer pushes after the skip is still owed, and
/// taking it would play it `debt` frames behind the clock.
fn take(ring: &mut impl Ring, debt: &mut usize, owing: bool, out: &mut [f32]) {
    let need = out.len();
    if *debt > 0 {
        let late = (*debt * 2).min(ring.queued()) & !1;
        *debt -= ring.skip(late) / 2;
    }
    let got = if *debt == 0 { ring.pop(out) } else { 0 };
    out[got..].fill(0.0);
    if owing {
        *debt += (need - got) / 2;
    }
}

/// 48 kHz f32 stereo when the device offers it (CoreAudio converts to the device's own rate), else its default.
pub fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    if let Ok(configs) = device.supported_output_configs() {
        for c in configs {
            let rates = c.min_sample_rate().0..=c.max_sample_rate().0;
            if c.channels() == 2 && c.sample_format() == SampleFormat::F32 && rates.contains(&RATE)
            {
                return Ok(c.with_sample_rate(cpal::SampleRate(RATE)));
            }
        }
    }
    device
        .default_output_config()
        .map_err(|e| format!("output config: {e}"))
}

pub fn default_output() -> Result<cpal::Device, String> {
    cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "no output device".to_string())
}

/// An output stream on `device` that plays what `cb` pops. Not started.
pub fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    cb: Callback,
) -> Result<cpal::Stream, String> {
    fn typed<T: SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        mut cb: Callback,
    ) -> Result<cpal::Stream, cpal::BuildStreamError> {
        let shared = cb.shared.clone();
        // HAL host times have an origin cpal hides; the first buffer anchors them to `now_ns`, so later stamps keep
        // the HAL's own spacing (the device's real rate) and never drift.
        let mut anchor: Option<(StreamInstant, u64)> = None;
        device.build_output_stream(
            config,
            move |out: &mut [T], info: &OutputCallbackInfo| {
                let due = info.timestamp().callback;
                let (first, at) = *anchor.get_or_insert_with(|| (due, now_ns()));
                let since = due.duration_since(&first).unwrap_or_default();
                cb.fill(out, at + since.as_nanos() as u64);
            },
            move |error| {
                tracing::warn!(%error, "audio output");
                if matches!(error, cpal::StreamError::DeviceNotAvailable) {
                    shared.failed.store(true, Ordering::Release);
                }
            },
            None,
        )
    }
    let stream_config = config.config();
    match config.sample_format() {
        SampleFormat::F32 => typed::<f32>(device, &stream_config, cb),
        SampleFormat::I16 => typed::<i16>(device, &stream_config, cb),
        SampleFormat::I32 => typed::<i32>(device, &stream_config, cb),
        SampleFormat::U16 => typed::<u16>(device, &stream_config, cb),
        other => return Err(format!("unsupported output format {other:?}")),
    }
    .map_err(|e| format!("output stream: {e}"))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{Ring, take};

    /// A ring the mixer pushes to right after the callback skips owed sound: the window between the callback's two
    /// reads that the mixer's thread can hit.
    struct PushedAfterSkip {
        queued: VecDeque<f32>,
        pushed: Vec<f32>,
    }

    impl Ring for PushedAfterSkip {
        fn queued(&self) -> usize {
            self.queued.len()
        }
        fn skip(&mut self, samples: usize) -> usize {
            let n = samples.min(self.queued.len());
            self.queued.drain(..n);
            self.queued.extend(self.pushed.drain(..));
            n
        }
        fn pop(&mut self, out: &mut [f32]) -> usize {
            let n = out.len().min(self.queued.len());
            for (o, s) in out.iter_mut().zip(self.queued.drain(..n)) {
                *o = s;
            }
            n
        }
    }

    /// Timeline frames `range` of a ramp: the left channel is the frame's number.
    fn frames(range: std::ops::Range<usize>) -> impl Iterator<Item = f32> {
        range.flat_map(|i| [i as f32, 0.5])
    }

    #[test]
    fn sound_pushed_while_owed_sound_is_skipped_is_never_heard_late() {
        // After an underrun the stream is 1,024 frames past the sound: frame 1,024 is due now. 512 frames are queued
        // (all owed), and the mixer pushes the next 512 (still owed) just after the callback skips the first.
        let mut ring = PushedAfterSkip {
            queued: frames(0..512).collect(),
            pushed: frames(512..1_024).collect(),
        };
        let mut debt = 1_024;
        let mut out = vec![1.0; 1_024];
        take(&mut ring, &mut debt, true, &mut out);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "frame {} heard where frame 1024 is due",
            out[0]
        );
        assert_eq!(
            debt, 1_024,
            "512 owed frames skipped; this silent buffer owes 512 more"
        );
        // The next buffer pays what is owed and plays the frame due then.
        ring.queued.extend(frames(1_024..2_048));
        take(&mut ring, &mut debt, true, &mut out);
        assert_eq!((out[0], debt), (1_536.0, 0));
    }
}
