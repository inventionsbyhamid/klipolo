//! Synthetic PCM from a device whose clock runs `(1 + ppm·10⁻⁶)` times the host's: frames per host second =
//! `rate × (1 + ppm·10⁻⁶)`, the fraction carried from block to block (exactly: frame counts come from the elapsed host
//! time in integers, never summed). Each sample is a function of its device frame number, so a source continues
//! seamlessly across calls (`first_frame`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use klipolo_platform::clock::host_ns;

use super::{Scripted, Worker, wait_until};
use crate::media::{AudioFormat, PcmBlock};
use crate::pipe::{AudioOut, AudioPipe, AudioStats, PipeError, PipeEvent};

#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticAudioSpec {
    pub rate: u32,
    pub channels: u16,
    /// A block every this many ms of host time.
    pub block_ms: u32,
    /// How much faster than the host clock the device's runs.
    pub drift_ppm: f64,
    pub signal: Signal,
    /// A 10 ms, 1 kHz, −6 dBFS blip every this many ms of host time from the source's first frame (the sync
    /// schedule's sound half: it lines up with the video flashes).
    pub blip_every_ms: Option<u32>,
}

/// Levels are peak dBFS (the tests read the peak).
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// Exact zeros: a muted or blocked mic.
    Zeros,
    /// White noise up to the level.
    Noise {
        dbfs: f32,
    },
    Tone {
        hz: f32,
        dbfs: f32,
    },
    /// A 200 ms, 440 Hz tone every second (exact zeros between): speech-level peaks.
    Bursts {
        dbfs: f32,
    },
    /// Each part plays for its ms of device time, in turn; the last plays on.
    Then(Vec<(u32, Signal)>),
}

/// Blocks from `from_ns` (the host time of device frame `first_frame`) until `to_ns`, one every `block_ms`, the last
/// cut at `to_ns`. Each block is stamped with the host time of its first frame.
pub fn synthetic_blocks(
    spec: &SyntheticAudioSpec,
    from_ns: u64,
    to_ns: u64,
    first_frame: u64,
) -> Vec<PcmBlock> {
    let mut source = AudioSource::new(spec.clone(), from_ns, first_frame);
    std::iter::from_fn(|| source.next_block(to_ns))
        .filter(|b| !b.samples.is_empty())
        .collect()
}

/// A device's sound, block after block, on the host clock.
pub struct AudioSource {
    spec: SyntheticAudioSpec,
    origin_ns: u64,
    first_frame: u64,
    block_ns: u64,
    /// The next block's number.
    block: u64,
    /// Device frames per 10²¹ host ns: `rate × (10¹² + ppm·10⁶)`.
    per_zs: i128,
}

impl AudioSource {
    /// Device frame `first_frame` is captured at host `origin_ns`.
    pub fn new(spec: SyntheticAudioSpec, origin_ns: u64, first_frame: u64) -> Self {
        let ppm_u = (spec.drift_ppm * 1e6).round() as i128;
        let per_zs = i128::from(spec.rate) * (1_000_000_000_000 + ppm_u);
        let block_ns = u64::from(spec.block_ms.max(1)) * 1_000_000;
        Self {
            spec,
            origin_ns,
            first_frame,
            block_ns,
            block: 0,
            per_zs,
        }
    }

    pub fn format(&self) -> AudioFormat {
        AudioFormat {
            rate: self.spec.rate,
            channels: self.spec.channels,
        }
    }

    /// The next block's host span.
    pub fn next_span(&self) -> (u64, u64) {
        let start = self.origin_ns + self.block * self.block_ns;
        (start, start + self.block_ns)
    }

    /// Skips the blocks that start before `ns`; the device's frames run on meanwhile.
    pub fn skip_to(&mut self, ns: u64) {
        self.block = self
            .block
            .max(ns.saturating_sub(self.origin_ns).div_ceil(self.block_ns));
    }

    /// The next block, cut at `until_ns`; None once it would start at or after `until_ns`.
    pub fn next_block(&mut self, until_ns: u64) -> Option<PcmBlock> {
        let (start, end) = self.next_span();
        if start >= until_ns {
            return None;
        }
        self.block += 1;
        let (from, to) = (self.frames_by(start), self.frames_by(end.min(until_ns)));
        let channels = usize::from(self.spec.channels.max(1));
        let mut samples = Vec::with_capacity((to - from) as usize * channels);
        for n in from..to {
            let f = self.first_frame + n;
            let blip = self.blip(f);
            for c in 0..channels {
                samples.push(
                    (sample(&self.spec.signal, self.spec.rate, f, c) + blip).clamp(-1.0, 1.0),
                );
            }
        }
        let host_ns = self.origin_ns + self.host_of(from);
        Some(PcmBlock { host_ns, samples })
    }

    /// Device frames captured from `origin_ns` until host `ns`.
    fn frames_by(&self, ns: u64) -> u64 {
        (i128::from(ns - self.origin_ns) * self.per_zs / 1_000_000_000_000_000_000_000) as u64
    }

    /// Host ns after `origin_ns` at which device frame `n` (counted from `first_frame`) is captured.
    fn host_of(&self, n: u64) -> u64 {
        ((i128::from(n) * 1_000_000_000_000_000_000_000 + self.per_zs / 2) / self.per_zs.max(1))
            as u64
    }

    /// The sync blip at device frame `f`: every `blip_every_ms` of host time, 10 ms of 1 kHz at −6 dBFS.
    fn blip(&self, f: u64) -> f32 {
        let Some(every_ms) = self.spec.blip_every_ms.filter(|ms| *ms > 0) else {
            return 0.0;
        };
        let rate = f64::from(self.spec.rate);
        let period = f64::from(every_ms) / 1000.0 * rate * (1.0 + self.spec.drift_ppm * 1e-6);
        let mut k = ((f as f64 + 0.5) / period).floor();
        if (k * period).round() > f as f64 {
            k -= 1.0;
        }
        let into = f - (k * period).round() as u64;
        if into >= u64::from(self.spec.rate / 100) {
            return 0.0;
        }
        level(-6.0) * sine(1000.0, into, self.spec.rate)
    }
}

/// `signal`'s sample at device frame `f`, channel `c`.
fn sample(signal: &Signal, rate: u32, f: u64, c: usize) -> f32 {
    match signal {
        Signal::Zeros => 0.0,
        Signal::Noise { dbfs } => level(*dbfs) * noise(f, c),
        Signal::Tone { hz, dbfs } => level(*dbfs) * sine(f64::from(*hz), f, rate),
        Signal::Bursts { dbfs } => {
            if f % u64::from(rate.max(1)) < u64::from(rate / 5) {
                level(*dbfs) * sine(440.0, f, rate)
            } else {
                0.0
            }
        }
        Signal::Then(parts) => {
            let ms = f * 1000 / u64::from(rate.max(1));
            let mut until = 0u64;
            for (i, (length, part)) in parts.iter().enumerate() {
                until += u64::from(*length);
                if ms < until || i + 1 == parts.len() {
                    return sample(part, rate, f, c);
                }
            }
            0.0
        }
    }
}

/// A peak level in dBFS as a linear amplitude.
fn level(dbfs: f32) -> f32 {
    10f32.powf(dbfs / 20.0)
}

fn sine(hz: f64, f: u64, rate: u32) -> f32 {
    let cycles = (hz * f as f64 / f64::from(rate.max(1))).fract();
    (cycles * std::f64::consts::TAU).sin() as f32
}

/// Uniform in [-1, 1), a function of the frame and the channel (splitmix64).
fn noise(f: u64, c: usize) -> f32 {
    let mut z = f
        .wrapping_mul(8)
        .wrapping_add(c as u64)
        .wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// The fake backend's audio pipe: blocks on the backend's shared schedule (device frame 0 at `epoch_ns`, so blips
/// line up with the video pipes' flashes), from `live_after_ms` after `start`, sent with `try_send`.
pub(crate) struct FakeAudioPipe {
    pub(crate) spec: SyntheticAudioSpec,
    pub(crate) epoch_ns: u64,
    pub(crate) live_after_ms: u32,
    pub(crate) events: Vec<(u32, PipeEvent)>,
    pub(crate) start_error: Option<PipeError>,
    /// Its device unplugged over these host-ns spans (`Script::unplugged`): a start inside one fails (`NoDevice`), and
    /// one before it loses the device then (`DeviceLost`) and gets it back as it ends (`DeviceBack`, as `CpalMic`'s look
    /// does).
    pub(crate) unplugged: Vec<(u64, u64)>,
    /// What a start inside an unplugged span fails with.
    pub(crate) unplugged_error: PipeError,
    /// Counts each start asked for (`FakeBackend::starts`).
    pub(crate) starts: Option<Arc<AtomicU64>>,
    stats: Arc<Counters>,
    worker: Option<Worker>,
}

#[derive(Default)]
struct Counters {
    callbacks: AtomicU64,
    frames: AtomicU64,
    dropped_channel: AtomicU64,
}

impl FakeAudioPipe {
    pub(crate) fn new(spec: SyntheticAudioSpec, epoch_ns: u64) -> Self {
        Self {
            spec,
            epoch_ns,
            live_after_ms: 0,
            events: vec![],
            start_error: None,
            unplugged: vec![],
            unplugged_error: PipeError::NoDevice,
            starts: None,
            stats: Arc::default(),
            worker: None,
        }
    }
}

impl AudioPipe for FakeAudioPipe {
    fn start(&mut self, out: AudioOut) -> Result<AudioFormat, PipeError> {
        if let Some(starts) = &self.starts {
            starts.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(e) = &self.start_error {
            return Err(e.clone());
        }
        if self.worker.is_some() {
            return Err(PipeError::Busy);
        }
        let start = host_ns();
        if (self.unplugged.iter()).any(|&(from, to)| (from..to).contains(&start)) {
            return Err(self.unplugged_error.clone());
        }
        let mut source = AudioSource::new(self.spec.clone(), self.epoch_ns, 0);
        let format = source.format();
        source.skip_to(start + u64::from(self.live_after_ms) * 1_000_000);
        let mut events = std::mem::take(&mut self.events);
        events.extend(unplug_events(&self.unplugged, start));
        let mut scripted = Scripted::new(start, events);
        let stats = self.stats.clone();
        self.worker = Some(Worker::spawn("fake-audio", move |stop| {
            loop {
                let (_, end) = source.next_span();
                if !wait_until(end.min(scripted.next_ns()), stop) {
                    return;
                }
                let now = host_ns();
                scripted.deliver(now, &out.events);
                if end > now {
                    continue;
                }
                if !scripted.flowing {
                    source.skip_to(end);
                    continue;
                }
                let Some(block) = source.next_block(end).filter(|b| !b.samples.is_empty()) else {
                    continue;
                };
                stats.callbacks.fetch_add(1, Ordering::Relaxed);
                stats.frames.fetch_add(
                    (block.samples.len() / usize::from(format.channels.max(1))) as u64,
                    Ordering::Relaxed,
                );
                if out.blocks.try_send(block).is_err() {
                    stats.dropped_channel.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
        Ok(format)
    }

    fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
    }

    fn stats(&self) -> AudioStats {
        AudioStats {
            callbacks: self.stats.callbacks.load(Ordering::Relaxed),
            frames: self.stats.frames.load(Ordering::Relaxed),
            dropped_channel: self.stats.dropped_channel.load(Ordering::Relaxed),
        }
    }
}

/// The loss and return a pipe started at `start` (host ns) meets (`FakeAudioPipe::unplugged`), as (ms after it, event):
/// `DeviceLost` as the first span after its start begins and `DeviceBack` as it ends, as `CpalMic`'s look sees it
/// listed again; nothing more, since its stream is dead until the session restarts it.
fn unplug_events(spans: &[(u64, u64)], start: u64) -> Vec<(u32, PipeEvent)> {
    // The return rounds up: a restart it prompts must start after the span, never inside it.
    let ms = |ns: u64, up: bool| {
        let gone = ns - start;
        let ms = if up {
            gone.div_ceil(1_000_000)
        } else {
            gone / 1_000_000
        };
        u32::try_from(ms).unwrap_or(u32::MAX)
    };
    let next = (spans.iter().filter(|&&(from, _)| from > start)).min_by_key(|&&(from, _)| from);
    next.map_or_else(Vec::new, |&(from, to)| {
        vec![
            (ms(from, false), PipeEvent::DeviceLost),
            (ms(to, true), PipeEvent::DeviceBack),
        ]
    })
}
