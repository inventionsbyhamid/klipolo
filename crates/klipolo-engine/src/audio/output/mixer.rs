//! The output thread's mixer: keeps the ring ahead of the callback with the timeline mix, repositions it (flush and
//! map onto the clock) and resamples when the device isn't at 48 kHz.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use klipolo_model::time::Us;
use ringbuf::traits::{Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use super::{AHEAD, BLOCK, FLUSH_TIMEOUT};
use crate::audio::clock::AudioClock;
use crate::audio::mix::{Mixdown, RAMP, RATE, finish, to_frame};
use crate::audio::pcm::{PcmKey, PcmStore};
use crate::audio::plan::{AudioPlan, AudioSource};
use crate::audio::stream::Shared;

pub(super) struct Mixer {
    prod: HeapProd<f32>,
    store: Arc<PcmStore>,
    mixdown: Mixdown,
    pub(super) shared: Arc<Shared>,
    clock: Arc<AudioClock>,
    gain: Arc<AtomicU32>,
    last_gain: f32,
    /// Timeline frames (48 kHz) per device frame.
    ratio: f64,
    /// Samples to keep queued.
    ahead: usize,
    flush: u64,
    pause: u64,
    /// Frames still fading in after a reposition (the sound never starts at full level).
    fade_in: usize,
    request: u64,
    /// The stream frame of the latest flush, and the timeline frame (48 kHz) heard there.
    pub(super) mark: u64,
    mark_pos: f64,
    /// Device frames pushed since the mark.
    written: u64,
    block: Vec<f32>,
    /// A device not at 48 kHz: the 48 kHz mix from timeline frame `mixed_at`, resampled from.
    mixed: Vec<f32>,
    mixed_at: i64,
    chunk: Vec<f32>,
}

impl Mixer {
    /// A mixer for a device at `rate`, and the callback's ends of its ring and flags.
    pub(super) fn new(
        store: Arc<PcmStore>,
        rate: u32,
        clock: Arc<AudioClock>,
        gain: Arc<AtomicU32>,
    ) -> (Mixer, HeapCons<f32>, Arc<Shared>) {
        let ahead = (AHEAD.as_secs_f64() * f64::from(rate)) as usize;
        let (prod, cons) = HeapRb::<f32>::new((ahead + 2 * BLOCK) * 2).split();
        let shared = Arc::new(Shared::default());
        let last_gain = f32::from_bits(gain.load(Ordering::Relaxed));
        let mixer = Mixer {
            prod,
            store,
            mixdown: Mixdown::new(Arc::new(AudioPlan {
                sources: Vec::new(),
                loudness: klipolo_model::project::Loudness::Off,
            })),
            shared: shared.clone(),
            clock,
            gain,
            last_gain,
            ratio: f64::from(RATE) / f64::from(rate),
            ahead: ahead * 2,
            flush: 0,
            pause: 0,
            fade_in: 0,
            request: 0,
            mark: 0,
            mark_pos: 0.0,
            written: 0,
            block: Vec::new(),
            mixed: Vec::new(),
            mixed_at: 0,
            chunk: vec![0.0; BLOCK * 2],
        };
        (mixer, cons, shared)
    }

    pub(super) fn set_plan(&mut self, plan: Arc<AudioPlan>) {
        self.mixdown.set_plan(plan);
    }

    /// Drops what is queued; returns the stream frame where sound pushed from now on starts.
    pub(super) fn flush(&mut self, wait: Duration) -> Result<u64, String> {
        self.flush += 1;
        self.shared.flush.store(self.flush, Ordering::Release);
        let deadline = Instant::now() + wait;
        while self.shared.acked.load(Ordering::Acquire) != self.flush {
            if Instant::now() >= deadline {
                return Err("the output device didn't answer".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(self.shared.ack_mark.load(Ordering::Relaxed))
    }

    /// Before the stream stops: the callback fades out what is playing and drops what is queued (so a resume never
    /// plays sound from before the pause). Waits up to `wait` for that; a callback that doesn't run in time does it
    /// on the first buffer after the resume instead.
    pub(super) fn pause(&mut self, wait: Duration) {
        self.pause += 1;
        self.shared.pause.store(self.pause, Ordering::Release);
        let deadline = Instant::now() + wait;
        while self.shared.paused.load(Ordering::Acquire) != self.pause && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Plays from `from_us` for `request`: flushes, maps it onto the clock and fills the ring.
    pub(super) fn seek(&mut self, from_us: Us, request: u64, wait: Duration) -> Result<(), String> {
        self.mark = self.flush(wait)?;
        self.mark_pos = to_frame(from_us.max(0)) as f64;
        self.written = 0;
        self.fade_in = RAMP as usize;
        self.request = request;
        self.clock
            .map(request, self.mark, self.mark_pos, self.ratio);
        self.top_up();
        Ok(())
    }

    /// The same position, re-mixed with the new plan from the next buffer on.
    pub(super) fn replan(&mut self) -> Result<(), String> {
        let mark = self.flush(FLUSH_TIMEOUT)?;
        self.mark_pos += (mark - self.mark) as f64 * self.ratio;
        self.mark = mark;
        self.written = 0;
        self.fade_in = RAMP as usize;
        self.clock
            .map(self.request, self.mark, self.mark_pos, self.ratio);
        self.top_up();
        Ok(())
    }

    pub(super) fn top_up(&mut self) {
        while self.prod.occupied_len() < self.ahead && self.prod.vacant_len() >= BLOCK * 2 {
            self.mix_block(BLOCK);
            let pushed = self.prod.push_slice(&self.block);
            self.written += (pushed / 2) as u64;
        }
    }

    /// `frames` device frames from `written` on, resampled linearly from the 48 kHz mix when the device runs at
    /// another rate.
    fn mix_block(&mut self, frames: usize) {
        let store = &self.store;
        let pcm = |s: &AudioSource| store.ready(&PcmKey::of(s));
        let pos = self.mark_pos + self.written as f64 * self.ratio;
        let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
        self.block.resize(frames * 2, 0.0);
        if self.ratio == 1.0 {
            self.mixdown.render(&pcm, pos as u64, &mut self.block);
        } else {
            // Keep a continuous 48 kHz mix covering this block, so the voices stream instead of re-seating.
            let first = pos.floor() as i64;
            let last = (pos + frames as f64 * self.ratio).floor() as i64 + 2;
            let have = (self.mixed.len() / 2) as i64;
            if first < self.mixed_at || first > self.mixed_at + have {
                self.mixed.clear();
                self.mixed_at = first;
            }
            self.mixed
                .drain(..(((first - self.mixed_at) * 2) as usize).min(self.mixed.len()));
            self.mixed_at = first;
            while self.mixed_at + ((self.mixed.len() / 2) as i64) < last {
                let at = self.mixed_at + (self.mixed.len() / 2) as i64;
                self.mixdown.render(&pcm, at.max(0) as u64, &mut self.chunk);
                self.mixed.extend_from_slice(&self.chunk);
            }
            for j in 0..frames {
                let x = pos + j as f64 * self.ratio - first as f64;
                let i = x.floor() as usize;
                let t = (x - i as f64) as f32;
                for c in 0..2 {
                    let (a, b) = (self.mixed[i * 2 + c], self.mixed[(i + 1) * 2 + c]);
                    self.block[j * 2 + c] = a + (b - a) * t;
                }
            }
        }
        finish(&mut self.block, self.last_gain, gain);
        self.last_gain = gain;
        let ramp = RAMP as usize;
        let fading = self.fade_in.min(frames);
        for (j, lr) in self.block[..fading * 2].chunks_exact_mut(2).enumerate() {
            let g = (ramp - self.fade_in + j + 1) as f32 / ramp as f32;
            lr[0] *= g;
            lr[1] *= g;
        }
        self.fade_in -= fading;
    }
}
