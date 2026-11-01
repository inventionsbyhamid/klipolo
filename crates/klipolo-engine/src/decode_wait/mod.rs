//! How long a frame request waits for its decoder.
//!
//! The preview keeps Cap's fixed deadline (2–4 s by picture size): it draws in real time, and the pool recovers a frame
//! that misses it (`render::pool`). Export isn't real time. A loaded computer can hold a decoder for seconds (paging, a
//! media engine shared with the encoder), and a fixed deadline turned that into a failed export. So an export request
//! waits as long as its decoder keeps making progress: it is stalled only after `stall_after` without any, or after
//! `cap` in all, and only then does the pool recover it (reopen the reader, then decode in software).
//!
//! The decoders tick a shared counter (`DecodeProgress`) for every sample read, packet or frame decoded and reader made.
//! A request looks at it every `SLICE` while it waits. While it waits on a decoder that moves, it sets `DecodeBeat`,
//! which export's worker turns into a heartbeat line for its parent (whose watchdog stops a worker silent for 120 s).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::Instant;

#[cfg(any(test, feature = "trial"))]
pub mod hooks;
#[cfg(test)]
mod tests;

/// How long a request waits for its decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeWait {
    /// A fixed deadline per request (Cap's, by picture size): the preview.
    Fixed,
    /// As long as the decoder makes progress: stalled after `stall_after` without any, and after `cap` in all.
    Progress {
        stall_after: Duration,
        cap: Duration,
    },
}

/// Export's wait. A healthy reader vends a 1080p frame in milliseconds and a whole GOP in well under 2 s even when the
/// computer is loaded, so 9 s without progress is a stall, not load. 45 s caps a frame whose decoder moves but never
/// gets there; the pool's recovery steps (each at most this) are bounded, and progress keeps the parent hearing.
pub const EXPORT_WAIT: DecodeWait = DecodeWait::Progress {
    stall_after: Duration::from_secs(9),
    cap: Duration::from_secs(45),
};

/// How often a waiting request looks at its decoder's progress. Requests answered sooner never look.
const SLICE: Duration = Duration::from_millis(500);

/// A decoder's start (opening its file and decoding its first frame) gets at least this long: an open ticks nothing
/// until it is done, and under heavy load it took 1.7 s, the first frame 2.9 s more. This
/// was the fixed start limit before.
pub const START_WINDOW: Duration = Duration::from_secs(30);

/// A decoder's progress: ticked for every sample read, packet or frame decoded and reader made. Cheap to clone; the
/// decoder's thread and every request share one.
#[derive(Clone, Debug, Default)]
pub struct DecodeProgress(Arc<AtomicU64>);

impl DecodeProgress {
    pub fn tick(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub fn ticks(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Set while a frame waits on a decoder that is still making progress; export's heartbeat takes it (`take`).
#[derive(Debug, Default)]
pub struct DecodeBeat(AtomicBool);

impl DecodeBeat {
    fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether a frame waited on a progressing decoder since the last call.
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::Relaxed)
    }

    /// Runs `step`, a bounded step of a frame's recovery (asking whether the file is there, opening a decoder), setting
    /// the beat now and every `SLICE` until it is done: the worker is getting there, not stuck.
    pub async fn alive_while<T>(&self, step: impl std::future::Future<Output = T>) -> T {
        tokio::pin!(step);
        loop {
            self.set();
            if let Ok(value) = tokio::time::timeout(SLICE, &mut step).await {
                return value;
            }
        }
    }
}

/// What a request got.
#[derive(Debug)]
pub enum Waited<T> {
    Got(T),
    /// The decoder let the request go without an answer (its thread is gone, or it couldn't make a reader).
    Dropped,
    /// No answer: the fixed deadline passed, or (progress) the decoder stopped moving or the cap passed. How long it
    /// waited.
    Stalled(Duration),
}

impl<T> Waited<T> {
    pub fn got(self) -> Option<T> {
        match self {
            Self::Got(value) => Some(value),
            Self::Dropped | Self::Stalled(_) => None,
        }
    }
}

impl DecodeWait {
    /// Waits for `answer`: up to `fixed` (`Fixed`), or while `progress` moves (`Progress`), setting `beat` whenever it
    /// sees the decoder move after waiting a slice. O(1) memory; one wake-up per `SLICE` only while it waits.
    pub async fn wait<T>(
        self,
        answer: oneshot::Receiver<T>,
        fixed: Duration,
        progress: &DecodeProgress,
        beat: Option<&DecodeBeat>,
    ) -> Waited<T> {
        self.wait_from(answer, fixed, Duration::ZERO, progress, beat)
            .await
    }

    /// Waits for a decoder's start: `START_WINDOW` (`Fixed`), or (`Progress`) never stalled within `START_WINDOW` and
    /// after it as `wait`, with a cap of at least the window. `beat` is set throughout the window (a bounded step), then
    /// while the decoder moves.
    pub async fn wait_start<T>(
        self,
        answer: oneshot::Receiver<T>,
        progress: &DecodeProgress,
        beat: Option<&DecodeBeat>,
    ) -> Waited<T> {
        self.wait_from(answer, START_WINDOW, START_WINDOW, progress, beat)
            .await
    }

    /// `wait`, never stalled before `floor` has passed (a stall is then judged as usual).
    async fn wait_from<T>(
        self,
        answer: oneshot::Receiver<T>,
        fixed: Duration,
        floor: Duration,
        progress: &DecodeProgress,
        beat: Option<&DecodeBeat>,
    ) -> Waited<T> {
        let started = Instant::now();
        let (stall_after, cap) = match self {
            Self::Fixed => (fixed, fixed),
            Self::Progress { stall_after, cap } => (stall_after, cap.max(floor)),
        };
        let window = started + floor;
        tokio::pin!(answer);
        let (mut seen, mut moved) = (progress.ticks(), started);
        loop {
            let slice = match self {
                Self::Fixed => started + cap,
                Self::Progress { .. } => Instant::now() + SLICE,
            };
            let until = slice
                .min((moved + stall_after).max(window))
                .min(started + cap);
            match tokio::time::timeout_at(until, &mut answer).await {
                Ok(Ok(value)) => return Waited::Got(value),
                Ok(Err(_)) => return Waited::Dropped,
                Err(_) => {}
            }
            let now = Instant::now();
            let ticks = progress.ticks();
            if ticks != seen {
                (seen, moved) = (ticks, now);
                if let Some(beat) = beat {
                    beat.set();
                }
            } else if now < window
                && let Some(beat) = beat
            {
                beat.set();
            }
            if now >= started + cap || now >= (moved + stall_after).max(window) {
                return Waited::Stalled(now - started);
            }
        }
    }
}
