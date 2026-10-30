//! Audio is the master clock: the picture follows the samples the device has really consumed, so it never
//! drifts from the sound (device crystals run ±20–50 ppm off the host clock, 70–180 ms an hour) and a route change
//! (AirPods mid-play) only moves the latency term.
//!
//! - The output callback publishes, per buffer, the stream frame at the buffer's start and the host time it is due
//!   (`stamp`), plus the buffer's length.
//! - The mixer publishes, per repositioning, which timeline frame (48 kHz) plays at which stream frame (`map`).
//! - Whoever needs the time (the actor) extrapolates from the newest stamp: the frame heard now is
//!   `frame + (now − host − latency) × rate`, and the timeline time follows from the mapping.
//!
//! Each side writes its own seqlock, so readers never block a writer (the callback is real-time).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use std::time::Instant;

use klipolo_model::time::Us;

use super::mix::RATE;

/// Nanoseconds on one monotonic clock for the whole process (the callback and the actor both read it).
pub fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// A single-writer seqlock over `N` words: the writer never waits; a reader retries while a write is under way.
struct Seqlock<const N: usize> {
    seq: AtomicU64,
    words: [AtomicU64; N],
}

impl<const N: usize> Seqlock<N> {
    fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// One writer only.
    fn write(&self, values: [u64; N]) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (w, v) in self.words.iter().zip(values) {
            w.store(v, Ordering::Relaxed);
        }
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    fn read(&self) -> [u64; N] {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let values = std::array::from_fn(|i| self.words[i].load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return values;
            }
        }
    }
}

/// Shared by the callback (stamps), the mixer (mapping, latency) and the actor (reads).
pub struct AudioClock {
    /// [stream frame at the buffer's start, host ns it is due + 1 (0: no buffer yet)].
    stamp: Seqlock<2>,
    /// [request, stream mark, timeline frame at the mark (f64 bits), timeline frames per stream frame (f64 bits)].
    map: Seqlock<4>,
    /// The device's stream rate (frames per second).
    rate: AtomicU32,
    /// One buffer, in ns (the callback runs about this far ahead of the buffer leaving).
    buffer_ns: AtomicU64,
    /// Device, stream and safety-offset latency, in ns (`hardware_latency`, refreshed on route changes).
    hardware_ns: AtomicU64,
}

impl AudioClock {
    pub fn new(rate: u32) -> Self {
        Self {
            stamp: Seqlock::new(),
            map: Seqlock::new(),
            rate: AtomicU32::new(rate.max(1)),
            buffer_ns: AtomicU64::new(0),
            hardware_ns: AtomicU64::new(0),
        }
    }

    /// The callback: the buffer starting at stream frame `frame` (of `frames`) is due at `host_ns`.
    pub fn stamp(&self, frame: u64, host_ns: u64, frames: usize) {
        let rate = u64::from(self.rate.load(Ordering::Relaxed));
        self.buffer_ns
            .store(frames as u64 * 1_000_000_000 / rate, Ordering::Relaxed);
        self.stamp.write([frame, host_ns + 1]);
    }

    /// The mixer: for `request`, timeline frame `mark_pos` plays at stream frame `mark`.
    pub fn map(&self, request: u64, mark: u64, mark_pos: f64, ratio: f64) {
        self.map
            .write([request, mark, mark_pos.to_bits(), ratio.to_bits()]);
    }

    /// The device's stream rate, once it is open.
    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
    }

    pub fn set_hardware_ns(&self, ns: u64) {
        self.hardware_ns.store(ns, Ordering::Relaxed);
    }

    /// Ns from a frame's due time until it is heard.
    pub fn latency_ns(&self) -> u64 {
        self.buffer_ns.load(Ordering::Relaxed) + self.hardware_ns.load(Ordering::Relaxed)
    }

    /// Timeline time heard at `now_ns`, once the mixer has positioned `request` and the device has asked for sound.
    /// Before that sound is heard it is earlier than the position asked for (the caller holds the picture there).
    pub fn timeline_us(&self, request: u64, now_ns: u64) -> Option<Us> {
        let [frame, host] = self.stamp.read();
        let [req, mark, pos, ratio] = self.map.read();
        if host == 0 || req != request {
            return None;
        }
        let rate = f64::from(self.rate.load(Ordering::Relaxed));
        let since = now_ns as f64 - (host - 1) as f64 - self.latency_ns() as f64;
        let heard = frame as f64 + since * rate / 1e9;
        let t = f64::from_bits(pos) + (heard - mark as f64) * f64::from_bits(ratio);
        Some((t * 1e6 / f64::from(RATE)).round() as Us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device whose crystal runs 50 ppm fast plays an hour: the clock still names the frame being heard (a host
    /// clock started at the same moment would be 180 ms behind by the end).
    #[test]
    fn audio_clock_stays_on_the_heard_sample_for_an_hour() {
        let clock = AudioClock::new(RATE);
        clock.set_hardware_ns(12_000_000);
        clock.map(1, 0, 0.0, 1.0);
        let true_rate = f64::from(RATE) * (1.0 + 50e-6);
        let buffer = 4_096u64;
        let latency = |c: &AudioClock| c.latency_ns() as f64;
        let mut worst: f64 = 0.0;
        let mut host_drift: f64 = 0.0;
        for k in 0..(3_600.0 * true_rate / buffer as f64) as u64 {
            let frame = k * buffer;
            let due = frame as f64 / true_rate * 1e9;
            clock.stamp(frame, due as u64, buffer as usize);
            // Read half-way to the next buffer.
            let now = due + buffer as f64 / true_rate * 0.5e9;
            let heard_frame = (now - latency(&clock)) * true_rate / 1e9;
            let truth = heard_frame * 1e6 / f64::from(RATE);
            let got = clock.timeline_us(1, now as u64).unwrap() as f64;
            worst = worst.max((got - truth).abs());
            host_drift = (now - latency(&clock)) / 1e3 - truth;
        }
        println!(
            "over an hour: audio clock within {worst:.1} µs; a host clock drifts {host_drift:.0} µs"
        );
        assert!(worst < 1_000.0, "audio clock off by {worst} µs");
        assert!(host_drift.abs() > 150_000.0, "{host_drift}");
        assert_eq!(
            clock.timeline_us(2, 0),
            None,
            "another request isn't mapped yet"
        );
    }
}
