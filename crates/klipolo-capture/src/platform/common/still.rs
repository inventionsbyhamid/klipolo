//! The screen encoder thread's schedule, pure so the Linux CI runs test it: when a still
//! screen's last frame is encoded again and at what picture time, when a quiet stream is nudged, when a still cursor is
//! sampled, and when the thread next has anything to do. The thread sleeps until then, or blocks on its channel when
//! nothing is due; nothing is ever due without being acted on, so it never spins.
//! - **Re-feeds:** 0.5 s of picture time after the last frame, the last frame is encoded again, stamped on the
//!   picture's own clock (`last + REFEED_NS`, never past now and never before a resume). It is handed out
//!   `REFEED_MARGIN_NS` after that stamp, past ScreenCaptureKit's delivery latency, so a real frame shown just before
//!   the stamp arrives first and keeps its own time. A thread more than a re-feed late stamps
//!   it now (no burst of catch-ups).
//! - **Pause:** nothing is re-fed or sampled. The thread still nudges a quiet stream once a second, so the stall rule
//!   finds its answers when the take resumes. A resume wakes the thread; the next frame, re-fed at once, is a keyframe.
//! - **Nudges:** a stream quiet for 1 s (no buffer and no answer) is nudged, then again each second it stays quiet. An
//!   answer is a sign of life: the pipe counts it (`VideoStats::answered`), which the stall rule reads with the
//!   delivered buffers, so a quiet stream that answers is never a stall. One that doesn't answer, or answers with an
//!   error, stalls after 2 s as before.

use super::sck::{CURSOR_BEAT_NS, REFEED_NS};

/// A stream quiet this long is nudged (and again each time this passes without a sign of life).
pub const NUDGE_AFTER_NS: u64 = 1_000_000_000;
/// A re-feed is handed out this long after its stamp: above ScreenCaptureKit's delivery latency (about a frame), so
/// a real frame displayed before the stamp is delivered first.
pub const REFEED_MARGIN_NS: u64 = 50_000_000;
/// The least time apart two frames' picture times are.
const MIN_STEP_NS: u64 = 1_000;

#[derive(Clone, Debug, Default)]
pub struct StillSchedule {
    /// The last frame's picture time (real or re-fed).
    last: Option<u64>,
    paused: bool,
    /// No re-feed is stamped before this: the last resume.
    floor: u64,
    /// The next frame is a keyframe: the first after a resume.
    key_next: bool,
    nudged_at: u64,
    cursor: bool,
    /// A camera's file holds only the device's own frames: no re-feeds (Cap's stretched-camera lesson).
    no_refeeds: bool,
}

/// What is due now (`StillSchedule::due`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Due {
    /// Encode the last frame again.
    pub refeed: Option<Stamp>,
    /// Ask the stream for a buffer (its own configuration again).
    pub nudge: bool,
    /// Sample a still cursor (the cursor track keeps its own 250 ms).
    pub beat: bool,
}

/// A frame's picture time (host ns), and whether it must be a keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub at: u64,
    pub key: bool,
}

impl StillSchedule {
    /// `cursor`: the pipe samples the cursor itself (a pipe-sourced cursor: portal metadata, DDA; never macOS's, which
    /// is its own source).
    pub fn new(cursor: bool) -> Self {
        Self {
            cursor,
            ..Self::default()
        }
    }

    /// The same schedule without re-feeds: a camera's frames come only from the device, and one that stops sending is
    /// the stall rule's (a suspended camera counts as blank pictures).
    pub fn without_refeeds(self) -> Self {
        Self {
            no_refeeds: true,
            ..self
        }
    }

    /// A real frame displayed at `at`: its picture time, never earlier than the last frame's, and a keyframe when it is
    /// the first after a resume.
    pub fn frame(&mut self, at: u64) -> Stamp {
        let at = self.last.map_or(at, |last| at.max(last + MIN_STEP_NS));
        self.last = Some(at);
        Stamp {
            at,
            key: std::mem::take(&mut self.key_next),
        }
    }

    /// Whether the take (or the source) is paused, as seen at `now`.
    pub fn paused(&mut self, paused: bool, now: u64) {
        if self.paused && !paused {
            (self.floor, self.key_next) = (now, true);
        }
        self.paused = paused;
    }

    /// What is due at `now`, the stream's last sign of life being `alive` (0: none yet). What it hands out is done:
    /// a re-feed becomes the last frame, a nudge is marked.
    pub fn due(&mut self, now: u64, alive: u64) -> Due {
        let mut due = Due {
            beat: self.cursor && !self.paused,
            ..Due::default()
        };
        if let Some(on_time) = self.refeed_at()
            && now >= on_time + REFEED_MARGIN_NS
        {
            let at = if now >= on_time + REFEED_NS {
                now
            } else {
                on_time.max(self.floor).min(now)
            };
            due.refeed = Some(self.frame(at));
        }
        if let Some(at) = self.nudge_at(alive)
            && now >= at
        {
            self.nudged_at = now;
            due.nudge = true;
        }
        due
    }

    /// When there is next something to do, or None: nothing to watch until a frame or a message comes.
    pub fn next(&self, now: u64, alive: u64) -> Option<u64> {
        let beat = (self.cursor && !self.paused).then(|| now + CURSOR_BEAT_NS);
        let refeed = self.refeed_at().map(|at| at + REFEED_MARGIN_NS);
        [refeed, self.nudge_at(alive), beat]
            .into_iter()
            .flatten()
            .min()
    }

    fn refeed_at(&self) -> Option<u64> {
        self.last
            .filter(|_| !self.paused && !self.no_refeeds)
            .map(|last| last + REFEED_NS)
    }

    fn nudge_at(&self, alive: u64) -> Option<u64> {
        (alive > 0).then(|| alive.max(self.nudged_at) + NUDGE_AFTER_NS)
    }
}
