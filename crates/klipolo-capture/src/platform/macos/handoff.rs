//! The hand-off between a capture queue and its encoder thread (`encode_thread`), shared by the screen and
//! the camera: a picture is copied into our pool at once on the capture queue (the capture buffer goes back
//! in microseconds) and sent over a bounded channel, never waited for; one start's switches and counters are shared
//! by the capture queue, the encoder thread and the pipe.
//!
//! The copy's thread CPU and wall time are summed (two clock reads a frame), so the stop log says whether the pool's
//! copy ran on the GPU.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Mutex, PoisonError};

use cidre::{arc, cv};

use super::pool::SurfacePool;
use crate::pipe::VideoStats;

/// A pooled frame on its way to the encoder.
pub(super) struct Pooled(pub(super) arc::R<cv::PixelBuf>);

// SAFETY: a CVPixelBuffer may be retained, released and read on any thread; each goes to one encoder thread.
unsafe impl Send for Pooled {}

impl Clone for Pooled {
    fn clone(&self) -> Self {
        Self(self.0.retained())
    }
}

/// Asks a stream for a buffer: its own configuration again (`Capturer::nudger`).
pub(super) type Nudge = Box<dyn Fn() + Send + Sync>;

pub(super) enum ToEncoder {
    Frame {
        buf: Pooled,
        host_ns: u64,
    },
    /// Look at the schedule again now (the take resumed).
    Wake,
    /// Everything before it is encoded, then the session finishes.
    Stop,
}

/// One start's counters and switches, shared by the capture queue, the encoder thread and the pipe.
#[derive(Default)]
pub(super) struct Shared {
    /// Set first by `stop`: nothing is copied, sampled or sent from then on.
    pub(super) stopped: AtomicBool,
    pub(super) paused: AtomicBool,
    /// The encoder's next frame is a keyframe (asked for, or the first after a start or a resume).
    pub(super) key_wanted: AtomicBool,
    /// The stream draws the cursor in its frames: burned in from the start, or since the cursor's live switch,
    /// so `screen::damage` counts the cursor's moves as real changes.
    pub(super) cursor_in_frames: AtomicBool,
    pub(super) delivered: AtomicU64,
    pub(super) complete: AtomicU64,
    pub(super) idle: AtomicU64,
    pub(super) blank: AtomicU64,
    pub(super) dropped_pool: AtomicU64,
    pub(super) dropped_encoder: AtomicU64,
    pub(super) encoder_counts: AtomicU64,
    pub(super) dropped_channel: AtomicU64,
    pub(super) encode_p99_ms: AtomicU32,
    /// When the stream last showed it is live (host ns): a buffer of any status, or an answered nudge. The encoder
    /// thread nudges a stream that went quiet, to tell a still screen from a stuck stream.
    pub(super) alive: AtomicU64,
    /// Asks the stream for a buffer (its own config again); set once the stream exists, taken out when it finishes so
    /// a stopped stream isn't kept.
    nudge: Mutex<Option<Nudge>>,
    pub(super) nudges: AtomicU64,
    /// Nudges the stream answered without an error (`VideoStats::answered`).
    answered: AtomicU64,
    /// The pool copies: count, wall ns, thread CPU ns.
    copies: AtomicU64,
    copy_wall_ns: AtomicU64,
    copy_cpu_ns: AtomicU64,
}

impl Shared {
    pub(super) fn stats(&self) -> VideoStats {
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        VideoStats {
            delivered: n(&self.delivered),
            complete: n(&self.complete),
            idle: n(&self.idle),
            blank: n(&self.blank),
            dropped_pool: n(&self.dropped_pool),
            dropped_encoder: n(&self.dropped_encoder) + n(&self.encoder_counts),
            dropped_channel: n(&self.dropped_channel),
            answered: n(&self.answered),
            encode_p99_ms: f32::from_bits(self.encode_p99_ms.load(Ordering::Relaxed)),
            cursor_dropped: 0,
        }
    }

    /// The stream's nudge, once it exists; None takes it out.
    pub(super) fn set_nudge(&self, nudge: Option<Nudge>) {
        *self.nudge.lock().unwrap_or_else(PoisonError::into_inner) = nudge;
    }

    /// Nudges the stream; false when there is none (not made yet, or finished).
    pub(super) fn nudge(&self) -> bool {
        let nudge = self.nudge.lock().unwrap_or_else(PoisonError::into_inner);
        nudge.as_ref().map(|nudge| nudge()).is_some()
    }

    /// The stream answered a nudge: without an error it is live, though quiet; an error, logged
    /// with its code, leaves it to the stall rule.
    pub(super) fn answered(&self, error: Option<isize>) {
        match error {
            None => {
                self.answered.fetch_add(1, Ordering::Relaxed);
                self.alive
                    .store(klipolo_platform::clock::host_ns(), Ordering::Release);
            }
            Some(code) => tracing::warn!(code, "screen: the stream answered a nudge with an error"),
        }
    }

    /// The pool copies so far: (count, mean wall µs, mean thread CPU µs).
    pub(super) fn copy_cost(&self) -> (u64, f64, f64) {
        let n = self.copies.load(Ordering::Relaxed);
        let mean = |ns: &AtomicU64| ns.load(Ordering::Relaxed) as f64 / n.max(1) as f64 / 1e3;
        (n, mean(&self.copy_wall_ns), mean(&self.copy_cpu_ns))
    }

    /// `image` copied into `pool` on the GPU, the copy's cost summed; None when the pool is full (counted in
    /// `dropped_pool`).
    pub(super) fn copy(&self, pool: &mut SurfacePool, image: &cv::PixelBuf) -> Option<Pooled> {
        let (wall, cpu) = (
            now_ns(libc::CLOCK_UPTIME_RAW),
            now_ns(libc::CLOCK_THREAD_CPUTIME_ID),
        );
        let copied = pool.copy(image);
        let (wall, cpu) = (
            now_ns(libc::CLOCK_UPTIME_RAW).saturating_sub(wall),
            now_ns(libc::CLOCK_THREAD_CPUTIME_ID).saturating_sub(cpu),
        );
        self.copies.fetch_add(1, Ordering::Relaxed);
        self.copy_wall_ns.fetch_add(wall, Ordering::Relaxed);
        self.copy_cpu_ns.fetch_add(cpu, Ordering::Relaxed);
        if copied.is_none() {
            self.dropped_pool.store(pool.dropped(), Ordering::Relaxed);
        }
        copied.map(Pooled)
    }

    /// `frame` (at host ns `at`) to the encoder thread: true when it was queued. A full channel drops it, counted as the
    /// encoder's (it was behind), which breaks no GOP since the frame was never encoded.
    pub(super) fn send(&self, encoder: &SyncSender<ToEncoder>, frame: Pooled, at: u64) -> bool {
        let frame = ToEncoder::Frame {
            buf: frame,
            host_ns: at,
        };
        match encoder.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.dropped_encoder.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

fn now_ns(clock: libc::clockid_t) -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: fills a local timespec.
    unsafe { libc::clock_gettime(clock, &mut t) };
    (t.tv_sec as u64).saturating_mul(1_000_000_000) + t.tv_nsec as u64
}
