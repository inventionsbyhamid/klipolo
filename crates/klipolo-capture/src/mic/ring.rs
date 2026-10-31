//! The real-time hand-off from a sound device's callback to the session (the audio callback must be real-time safe).
//! The callback (`MicFeed::push`) converts its block to `f32`, keeps the mic's channels, stamps it on the host clock and
//! writes it into two bounded lock-free rings, one of samples and one of stamps (ringbuf's SPSC rings, made before the
//! stream starts): no allocation, no lock, no system call, never a wait. When the rings are full the block is dropped
//! whole and counted. A pump thread (`MicPump::tick`, every `PUMP_TICK`) turns them into `PcmBlock`s, the one allocation
//! per block the seam's `Vec` needs, and `try_send`s them oldest first. Another audio pipe can use it the same way.
//!
//! Bounds: the rings hold `RING_MS` of sound (192 KB for stereo at 48 kHz) and `RING_BLOCKS` stamps; the session's
//! channel behind them holds `PCM_CHANNEL` blocks. A full channel holds the oldest block back for the next tick, so a
//! slow session loses the newest sound at the rings, whole blocks, never scattered ones.
//!
//! **A lost device comes back**. After the pump sent `DeviceLost`, `watch` asks the pipe once a
//! second whether the device is listed again (by name: the OS's registry, which opens nothing) and sends `DeviceBack`
//! the first time it is. The session then restarts the pipe on the person's own mic, so a USB blip doesn't move the
//! take to another mic, and a computer with no other mic gets its voice back. (The 3 s presence wait is only for
//! switching back from a fallback, which the pipe's controller looks after: `run::own_mic`.)

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::TrySendError;
use std::time::{Duration, Instant};

use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::hosttime::Rebase;
use crate::media::{AudioFormat, PcmBlock};
use crate::mic::mic_channels;
use crate::pipe::{AudioOut, AudioStats, PipeEvent};

/// The sound the rings hold between the callback and the pump: 50 ticks.
pub const RING_MS: u32 = 500;
/// The blocks the rings hold (callbacks as short as 64 frames at 48 kHz fill 0.5 s with under 400).
pub const RING_BLOCKS: usize = 512;
/// How often the pump hands blocks to the session.
pub const PUMP_TICK: Duration = Duration::from_millis(10);
/// How long the last tick after Stop waits for room in a full channel before what's left is dropped.
pub const FINISH_WAIT: Duration = Duration::from_millis(250);
/// How often a lost device is looked for (`MicPump::watch`).
pub const LOOK_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug)]
struct Stamp {
    host_ns: u64,
    frames: u32,
}

/// What the callback, the device's error callback, the pump and the pipe share, lock-free: the counters `AudioStats`
/// reads, and a fault the pump sends as one `PipeEvent` (the error callback never touches a channel).
#[derive(Debug, Default)]
pub struct Shared {
    callbacks: AtomicU64,
    frames: AtomicU64,
    dropped: AtomicU64,
    /// 0: none; `LOST`; `STOPPED`. The first fault wins.
    fault: AtomicU8,
}

const LOST: u8 = 1;
const STOPPED: u8 = 2;

impl Shared {
    pub fn stats(&self) -> AudioStats {
        AudioStats {
            callbacks: self.callbacks.load(Ordering::Relaxed),
            frames: self.frames.load(Ordering::Relaxed),
            dropped_channel: self.dropped.load(Ordering::Relaxed),
        }
    }

    /// The device went away (`PipeEvent::DeviceLost`: the heal policy waits, then falls back to the built-in mic).
    pub fn device_lost(&self) {
        let _ = self
            .fault
            .compare_exchange(0, LOST, Ordering::AcqRel, Ordering::Acquire);
    }

    /// The stream stopped for another reason (`PipeEvent::StoppedBySystem`: the heal policy restarts it).
    pub fn stream_stopped(&self) {
        let _ = self
            .fault
            .compare_exchange(0, STOPPED, Ordering::AcqRel, Ordering::Acquire);
    }

    fn fault(&self) -> Option<PipeEvent> {
        match self.fault.load(Ordering::Acquire) {
            LOST => Some(PipeEvent::DeviceLost),
            STOPPED => Some(PipeEvent::StoppedBySystem),
            _ => None,
        }
    }
}

/// The two ends of a hand-off for a device of `rate` Hz and `device_channels` channels (at least one), made before the
/// stream starts: the feed moves into the callback, the pump onto its thread.
pub fn handoff(rate: u32, device_channels: u16) -> (MicFeed, MicPump, Arc<Shared>) {
    let device_channels = device_channels.max(1);
    let format = AudioFormat {
        rate,
        channels: mic_channels(device_channels),
    };
    let frames = (u64::from(rate) * u64::from(RING_MS) / 1_000).max(4_096) as usize;
    let (samples_in, samples_out) =
        HeapRb::<f32>::new(frames * usize::from(format.channels)).split();
    let (stamps_in, stamps_out) = HeapRb::<Stamp>::new(RING_BLOCKS).split();
    let shared = Arc::new(Shared::default());
    let feed = MicFeed {
        samples: samples_in,
        stamps: stamps_in,
        device_channels: usize::from(device_channels),
        channels: usize::from(format.channels),
        clock: Rebase::default(),
        shared: shared.clone(),
    };
    let pump = MicPump {
        samples: samples_out,
        stamps: stamps_out,
        format,
        pending: None,
        fault_sent: false,
        watch: None,
        shared: shared.clone(),
    };
    (feed, pump, shared)
}

/// The callback's end.
pub struct MicFeed {
    samples: HeapProd<f32>,
    stamps: HeapProd<Stamp>,
    device_channels: usize,
    channels: usize,
    clock: Rebase,
    shared: Arc<Shared>,
}

impl MicFeed {
    /// One callback's interleaved samples, its instants (`hosttime::from_cpal`) and the host clock now. Real-time safe:
    /// never allocates, locks, blocks or panics. False when the block was dropped (the rings are full).
    pub fn push<T: Copy>(
        &mut self,
        data: &[T],
        to_f32: fn(T) -> f32,
        capture: Duration,
        callback: Duration,
        now_host_ns: u64,
    ) -> bool {
        let frames = data.len() / self.device_channels;
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        self.shared
            .frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        let host_ns = self.clock.stamp(capture, callback, now_host_ns);
        let Ok(frames32) = u32::try_from(frames) else {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if frames == 0 {
            return true;
        }
        if self.stamps.is_full() || self.samples.vacant_len() < frames * self.channels {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let (device, keep) = (self.device_channels, self.channels);
        let kept = data
            .chunks_exact(device)
            .flat_map(|frame| frame.iter().take(keep).map(|&s| to_f32(s)));
        self.samples.push_iter(kept);
        // The samples are in before their stamp, so the pump never sees a stamp without them.
        let _ = self.stamps.try_push(Stamp {
            host_ns,
            frames: frames32,
        });
        true
    }
}

/// Whether the session still listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Open,
    /// The session's channel is gone: stop pumping.
    Closed,
}

/// The pump thread's end.
pub struct MicPump {
    samples: HeapCons<f32>,
    stamps: HeapCons<Stamp>,
    format: AudioFormat,
    /// The oldest block, held back while the session's channel is full.
    pending: Option<PcmBlock>,
    fault_sent: bool,
    /// Looking for a lost device (`watch`).
    watch: Option<Watch>,
    shared: Arc<Shared>,
}

/// The look for a lost device: when next, and whether it was seen (then `DeviceBack` is sent once).
#[derive(Clone, Copy, Debug)]
struct Watch {
    next: Instant,
    seen: bool,
    sent: bool,
}

impl MicPump {
    /// What the pipe records: the device's rate in one or two channels.
    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// Hands the session every block the callback wrote since the last tick, oldest first, then a fault as one event.
    /// Never blocks: a full channel keeps the block for the next tick, and a full event channel the event.
    pub fn tick(&mut self, out: &AudioOut) -> Flow {
        while let Some(block) = self.pending.take().or_else(|| self.next_block()) {
            match out.blocks.try_send(block) {
                Ok(()) => {}
                Err(TrySendError::Full(block)) => {
                    self.pending = Some(block);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return Flow::Closed,
            }
        }
        if !self.fault_sent
            && let Some(event) = self.shared.fault()
        {
            match out.events.try_send(event) {
                Ok(()) => self.fault_sent = true,
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return Flow::Closed,
            }
        }
        Flow::Open
    }

    /// After `DeviceLost` went out: once every `LOOK_EVERY`, `listed()` says whether the device is in the OS's list again
    /// (names only, opening nothing); the first time it is, `DeviceBack` is sent, once (kept for the next call while the
    /// event channel is full). `listed` is called at most once per call, and only when a look is due. Nothing happens
    /// for any other fault, or before a loss.
    pub fn watch(&mut self, out: &AudioOut, now: Instant, listed: impl FnOnce() -> bool) -> Flow {
        if !self.fault_sent || self.shared.fault.load(Ordering::Acquire) != LOST {
            return Flow::Open;
        }
        let w = self.watch.get_or_insert(Watch {
            next: now + LOOK_EVERY,
            seen: false,
            sent: false,
        });
        if !w.seen && now >= w.next {
            w.next = now + LOOK_EVERY;
            w.seen = listed();
        }
        if w.seen && !w.sent {
            match out.events.try_send(PipeEvent::DeviceBack) {
                Ok(()) => w.sent = true,
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => return Flow::Closed,
            }
        }
        Flow::Open
    }

    /// The last ticks, once the stream is gone: up to `FINISH_WAIT` for room in a full channel, so a take's last moments
    /// aren't lost to a busy session; then what's still held is dropped and counted.
    pub fn finish(&mut self, out: &AudioOut) {
        let until = Instant::now() + FINISH_WAIT;
        while self.tick(out) == Flow::Open && self.holding() && Instant::now() < until {
            std::thread::sleep(PUMP_TICK);
        }
        let left = u64::from(self.pending.take().is_some()) + self.stamps.occupied_len() as u64;
        self.shared.dropped.fetch_add(left, Ordering::Relaxed);
    }

    fn holding(&self) -> bool {
        self.pending.is_some() || !self.stamps.is_empty()
    }

    fn next_block(&mut self) -> Option<PcmBlock> {
        let stamp = self.stamps.try_pop()?;
        let mut samples = vec![0.0; stamp.frames as usize * usize::from(self.format.channels)];
        let got = self.samples.pop_slice(&mut samples);
        samples.truncate(got);
        Some(PcmBlock {
            host_ns: stamp.host_ns,
            samples,
        })
    }
}
