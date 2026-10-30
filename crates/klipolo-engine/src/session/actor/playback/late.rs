//! Whether playback keeps up: a frame drawn more than a frame's time after its tick was due is late
//! (the actor was held up, by its own work or by a busy computer, and frames were skipped). `LATE_FRAMES` late frames
//! within `WITHIN` say playback is falling behind, once per run; nothing counts before the run's first frame presented
//! while its clock runs, nor for `WARMING` after it (decoders open, the first frames of a cut are slow; a Bluetooth
//! device's start holds the picture first). Transcription hears it (`EngineEvent::FramesLate`) and holds until playback
//! stops. O(1) per frame, no allocation.

use std::time::{Duration, Instant};

use super::{Actor, Clock};
use crate::api::EngineEvent;
use crate::session::PreviewBackend;

impl<B: PreviewBackend> Actor<B> {
    /// The tick that runs `now`, against when it was due: once a run of playback falls behind, the app hears it.
    /// Ticks while the sound is on its way are woken early by it, never late.
    pub(in crate::session::actor) fn note_late(&mut self, now: Instant) {
        let (Some(cur), Some(play)) = (&self.current, &mut self.playing) else {
            return;
        };
        if matches!(play.clock, Clock::Waiting { .. }) {
            return;
        }
        let fps = u64::from(cur.ctx.project.settings.fps.max(1));
        if play
            .late
            .tick(play.next, now, Duration::from_micros(1_000_000 / fps))
        {
            self.emit(EngineEvent::FramesLate {
                at_us: self.playhead_us,
            });
        }
    }

    /// A playback frame went to the presenter: the run's warm-up starts at the first one while its clock runs (the
    /// picture held while the sound starts doesn't count).
    pub(in crate::session::actor) fn note_presented(&mut self, now: Instant) {
        if let Some(play) = self
            .playing
            .as_mut()
            .filter(|p| !matches!(p.clock, Clock::Waiting { .. }))
        {
            play.late.presented(now);
        }
    }
}

/// Late frames within `WITHIN` that say playback is falling behind.
pub(crate) const LATE_FRAMES: usize = 3;
pub(crate) const WITHIN: Duration = Duration::from_secs(1);
/// The second after a run's first frame presented: never counted.
pub(crate) const WARMING: Duration = Duration::from_secs(1);

/// One run of playback's late frames.
#[derive(Debug, Default)]
pub(crate) struct Late {
    /// When its first frame was presented while the clock ran.
    started: Option<Instant>,
    /// When the last `LATE_FRAMES` late frames were drawn, as a ring (`next` is the oldest).
    at: [Option<Instant>; LATE_FRAMES],
    next: usize,
    told: bool,
}

impl Late {
    /// A frame was presented at `now`: the first one starts the warm-up.
    pub(crate) fn presented(&mut self, now: Instant) {
        self.started.get_or_insert(now);
    }

    /// A tick due at `due` that ran at `now`, frames being `frame` apart: true once, when this run falls behind.
    pub(crate) fn tick(&mut self, due: Instant, now: Instant, frame: Duration) -> bool {
        let warming = self.started.is_none_or(|started| now < started + WARMING);
        if self.told || warming || now.saturating_duration_since(due) <= frame {
            return false;
        }
        self.at[self.next] = Some(now);
        self.next = (self.next + 1) % LATE_FRAMES;
        // `next` is now the oldest of the last `LATE_FRAMES` late frames.
        self.told = self.at[self.next].is_some_and(|oldest| now.duration_since(oldest) <= WITHIN);
        self.told
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_micros(33_333);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn three_late_frames_within_a_second_say_so_once() {
        let t0 = Instant::now();
        let mut late = Late::default();
        late.presented(t0);
        // Each tick runs 50 ms after it was due (more than a frame at 30 fps), from 1.2 s on.
        let said: Vec<bool> = (0..6)
            .map(|k| {
                let due = t0 + ms(1_200 + 100 * k);
                late.tick(due, due + ms(50), FRAME)
            })
            .collect();
        assert_eq!(said, [false, false, true, false, false, false]);
    }

    #[test]
    fn the_warm_up_starts_at_the_first_frame_presented() {
        // A Bluetooth start holds the picture for up to 2 s. Nothing late counts before the first frame is
        // presented, nor during the second after it; then three late frames within a second say so.
        let t0 = Instant::now();
        let mut late = Late::default();
        for k in 0..6 {
            let due = t0 + ms(1_000 + 300 * k);
            assert!(
                !late.tick(due, due + ms(100), FRAME),
                "before any frame, {k}"
            );
        }
        late.presented(t0 + ms(3_000));
        late.presented(t0 + ms(3_500)); // only the first counts
        for k in 0..5 {
            let due = t0 + ms(3_100 + 150 * k);
            assert!(!late.tick(due, due + ms(100), FRAME), "warming up, {k}");
        }
        let said: Vec<bool> = (0..3)
            .map(|k| {
                let due = t0 + ms(4_100 + 100 * k);
                late.tick(due, due + ms(50), FRAME)
            })
            .collect();
        assert_eq!(said, [false, false, true]);
    }

    #[test]
    fn frames_on_time_the_warm_up_and_late_frames_far_apart_never_say_so() {
        let t0 = Instant::now();
        let mut late = Late::default();
        late.presented(t0);
        // On time, or late by less than a frame.
        for k in 0..100 {
            let due = t0 + ms(1_000 + 33 * k);
            assert!(!late.tick(due, due + ms(20), FRAME));
        }
        // The first second: slow starts don't count.
        let mut warming = Late::default();
        warming.presented(t0);
        for k in 0..9 {
            let due = t0 + ms(100 * k);
            assert!(!warming.tick(due, due + ms(80), FRAME));
        }
        // One late frame every 600 ms: never three within a second.
        let mut sparse = Late::default();
        sparse.presented(t0);
        for k in 0..10 {
            let due = t0 + ms(2_000 + 600 * k);
            assert!(!sparse.tick(due, due + ms(100), FRAME));
        }
    }
}
