//! A video pipe's encoder thread (the screen's, shared with the camera): pooled frames from the capture queue,
//! over a bounded channel (`handoff`), into the pinned VideoToolbox session, so VideoToolbox holding a call never holds
//! the capture queue. It keeps a still screen's file going on its own clock, whatever ScreenCaptureKit sends, by the
//! pure schedule `platform::common::still` (re-feeds on the picture's clock, nudges for a quiet stream, nothing while
//! paused), and:
//! - forces a keyframe once a second of picture time passed without one (`keyframe_due`): VideoToolbox spaces them by
//!   frame count, so a still screen got one every 3.6 s at 60 fps (the smoke run);
//! - gives `start` the stream's description once the first frame came out, and the counters and the p99 latency (once
//!   a second) to `VideoStats`.
//!
//! At `Stop` every frame queued before it is encoded, then the session finishes, so its last packets are sent before
//! `stop` returns. The thread sleeps until the schedule's next deadline, or blocks on the channel; a resume wakes it.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::encode::VtEncoder;
use super::handoff::{Pooled, Shared, ToEncoder};
use crate::media::StreamInfo;
use crate::platform::common::sck::keyframe_due;
use crate::platform::common::still::{Stamp, StillSchedule};

/// Frames between the stream's queue and the encoder: a full channel drops at the queue, never waits there.
pub(super) const FRAMES_IN_FLIGHT: usize = 4;
/// How often the thread looks for the stream's description before the first frame came out.
const FIRST_LOOK: Duration = Duration::from_millis(5);
const LATENCY_EVERY: Duration = Duration::from_secs(1);
/// The shortest sleep: a deadline already passed is acted on at once, so this never spins.
const MIN_SLEEP: Duration = Duration::from_millis(1);

pub(super) struct EncoderThread {
    /// The thread's name ("klipolo-screen-encode").
    pub(super) name: &'static str,
    /// Re-feed a still picture's last frame (a screen); a camera's file holds only the device's frames.
    pub(super) refeeds: bool,
    pub(super) encoder: VtEncoder,
    pub(super) frames: Receiver<ToEncoder>,
    pub(super) shared: Arc<Shared>,
    pub(super) info: SyncSender<StreamInfo>,
}

/// What the thread keeps between frames.
struct Kept {
    schedule: StillSchedule,
    /// The last frame encoded, for the re-feeds.
    last: Option<Pooled>,
    /// The last frame asked to be a keyframe (its packet may not be out yet).
    forced_at: u64,
}

impl EncoderThread {
    pub(super) fn spawn(self) -> std::io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name(self.name.into())
            .stack_size(512 * 1024)
            .spawn(move || self.run())
    }

    fn run(mut self) {
        qos::set_current(Qos::UserInteractive);
        let mut info = Some(self.info.clone());
        let mut latency_at = Instant::now();
        // The cursor is its own source: no heartbeat here.
        let schedule = StillSchedule::new(false);
        let mut kept = Kept {
            schedule: if self.refeeds {
                schedule
            } else {
                schedule.without_refeeds()
            },
            last: None,
            forced_at: 0,
        };
        loop {
            let now = host_ns();
            kept.schedule.paused(self.paused(), now);
            let wait = match &info {
                Some(_) => Some(FIRST_LOOK),
                None => (kept.schedule.next(now, self.alive()))
                    .map(|at| Duration::from_nanos(at.saturating_sub(now)).max(MIN_SLEEP)),
            };
            let got = match wait {
                Some(wait) => self.frames.recv_timeout(wait),
                None => self
                    .frames
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match got {
                Ok(ToEncoder::Frame { buf, host_ns: at }) => {
                    let stamp = kept.schedule.frame(at);
                    if !self.encode(&buf, stamp, &mut kept) {
                        break;
                    }
                    kept.last = Some(buf);
                }
                Ok(ToEncoder::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Ok(ToEncoder::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            }
            if !self.due(&mut kept) {
                break;
            }
            if let Some(tx) = &info
                && let Some(described) = self.encoder.info()
            {
                let _ = tx.try_send(described);
                info = None;
            }
            self.publish(&mut latency_at);
        }
        self.encoder.finish();
        if let (Some(tx), Some(described)) = (&info, self.encoder.info()) {
            let _ = tx.try_send(described);
        }
        latency_at = Instant::now() - LATENCY_EVERY;
        self.publish(&mut latency_at);
    }

    fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    fn alive(&self) -> u64 {
        self.shared.alive.load(Ordering::Acquire)
    }

    /// One frame into the session, a keyframe when asked for, the schedule's or due; false once the session's receiver
    /// is gone.
    fn encode(&mut self, buf: &Pooled, stamp: Stamp, kept: &mut Kept) -> bool {
        let wanted = self.shared.key_wanted.swap(false, Ordering::AcqRel);
        let last_key = (self.encoder.last_keyframe_ns().unwrap_or(stamp.at)).max(kept.forced_at);
        let key = wanted || stamp.key || keyframe_due(last_key, stamp.at);
        if key {
            kept.forced_at = stamp.at;
        }
        // A frame VideoToolbox refused is counted and reported by the encoder; the next one is tried.
        !(self.encoder.encode(&buf.0, stamp.at, key).is_err() && self.encoder.closed())
    }

    /// What the schedule says is due: a re-feed of the last frame, a nudge.
    fn due(&mut self, kept: &mut Kept) -> bool {
        let now = host_ns();
        kept.schedule.paused(self.paused(), now);
        let due = kept.schedule.due(now, self.alive());
        if let (Some(stamp), Some(buf)) = (due.refeed, kept.last.clone())
            && !self.encode(&buf, stamp, kept)
        {
            return false;
        }
        if due.nudge && self.shared.nudge() {
            self.shared.nudges.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    /// The encoder's counters into `VideoStats`; its latency percentile once a second (a select over 1,000 values).
    fn publish(&self, latency_at: &mut Instant) {
        let (encoder, channel) = self.encoder.dropped();
        self.shared.encoder_counts.store(encoder, Ordering::Relaxed);
        self.shared
            .dropped_channel
            .store(channel, Ordering::Relaxed);
        if latency_at.elapsed() >= LATENCY_EVERY {
            *latency_at = Instant::now();
            let p99 = self.encoder.latency_p99_ms();
            self.shared
                .encode_p99_ms
                .store(p99.to_bits(), Ordering::Relaxed);
        }
    }
}
