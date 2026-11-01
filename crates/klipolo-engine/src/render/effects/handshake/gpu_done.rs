//! The newest stage frame the GPU has finished, and when.
//! Every preview frame's own work-done callback (the backend's `WorkDone`, tagged with the frame's number) writes it, so
//! it is always current: the stage reads it to start its draw marks' 2 s, and a device loss reads which frames finished.
//! A late callback (of a frame whose wait gave up, or of a lost device's queue) carries its own frame number, so it never
//! moves the value back. No lock: the time is published before the frame number (Release) and read after it (Acquire),
//! so a frame is never paired with an earlier frame's time and a mark's 2 s never start early.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct GpuDone {
    /// What `at` counts from.
    base: Instant,
    frame: AtomicU64,
    /// When the newest frame finished: nanoseconds after `base`, plus one (0: none has).
    at: AtomicU64,
    /// A draw mark waits for a frame to finish: the backend wakes the session's actor when one does.
    waiting: AtomicBool,
}

impl Default for GpuDone {
    fn default() -> Self {
        Self {
            base: Instant::now(),
            frame: AtomicU64::new(0),
            at: AtomicU64::new(0),
            waiting: AtomicBool::new(false),
        }
    }
}

impl GpuDone {
    /// Frame `frame` is done on the GPU (a work-done callback, on the poll thread); whether a draw mark was waiting.
    pub fn done(&self, frame: u64) -> bool {
        if self.frame.load(Ordering::Acquire) < frame {
            let since = u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX - 1);
            self.at.fetch_max(since + 1, Ordering::Release);
            self.frame.fetch_max(frame, Ordering::Release);
        }
        self.waiting.load(Ordering::SeqCst)
    }

    /// The newest frame done, and when it was (at that frame's finish or later, never earlier).
    pub fn newest(&self) -> (u64, Option<Instant>) {
        let frame = self.frame.load(Ordering::Acquire);
        let at = self.at.load(Ordering::Acquire);
        let when = at
            .checked_sub(1)
            .map(|nanos| self.base + Duration::from_nanos(nanos));
        (frame, when)
    }

    pub(in crate::render::effects) fn set_waiting(&self, on: bool) {
        self.waiting.store(on, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A late callback of an older frame (a wait that gave up, a lost device's queue) never moves the newest frame or
    /// its time back; read without a lock, the time is never older than its frame's.
    #[test]
    fn the_newest_frame_moves_only_forward_with_its_time() {
        let g = GpuDone::default();
        assert_eq!(g.newest(), (0, None));
        g.done(3);
        let (frame, at) = g.newest();
        assert_eq!(frame, 3);
        assert!(at.is_some_and(|t| t >= g.base));
        g.done(2);
        assert_eq!(g.newest(), (3, at), "an older frame moves nothing");
        g.done(5);
        let (frame, later) = g.newest();
        assert_eq!(frame, 5);
        assert!(later >= at);
    }
}
