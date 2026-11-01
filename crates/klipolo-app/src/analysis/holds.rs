//! What analysis's decoding on this Mac makes way for, so the person's own work comes first: playback, an export and a
//! take, as editing copies and thumbnail scans make way for them.
//!
//! - **Picture** work (the scene log's small copy: a video decode and encode on half the cores) waits while the video
//!   plays, and for `SETTLE` after it stops (a pause, then play again, is common), as editing copies do.
//! - **Sound** work (the speech copy and the sound levels: a light audio decode) goes on beside playback until frames
//!   come late during it (the engine's `FramesLate`); then it waits for the rest of that playback. The next playback
//!   tries again.
//! - Both wait while an export runs or a take records.
//!
//! A step waits before it starts and, for the speech copy and the small copy, between the blocks or frames it decodes, so
//! a decode under way stops where it is and goes on from there (its decoder kept open: nothing is decoded twice). The
//! sound levels decode into the preview's own sound cache, which playback may be waiting on: they wait only before they
//! start.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use klipolo_engine::api::EngineEvent;

/// How long picture work waits after playback stops (as editing copies do).
pub const SETTLE: Duration = Duration::from_secs(2);
/// How often a held step looks at its cancel flag.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// What a local step decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// A clip's sound: the speech copy, the sound levels.
    Sound,
    /// A clip's picture: the scene log's small copy.
    Picture,
}

/// Why a step waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Playing,
    /// Frames came late during this playback.
    FramesLate,
    /// Playback stopped less than `SETTLE` ago.
    Settling,
    Exporting,
    Recording,
}

/// What the app last said.
#[derive(Default)]
struct Now {
    playing: bool,
    late: bool,
    stopped_at: Option<Instant>,
    exporting: bool,
    recording: bool,
}

/// The holds on analysis's decoding (one per app, in `AnalysisState`). The app tells each change; a step waits in
/// `wait`.
#[derive(Default)]
pub struct Holds {
    now: Mutex<Now>,
    changed: Condvar,
}

impl Holds {
    /// The video started (true) or stopped (false) playing: a stop starts `SETTLE`, and forgets frames late.
    pub fn set_playing(&self, playing: bool) {
        self.change(|now| {
            if now.playing && !playing {
                now.stopped_at = Some(Instant::now());
            }
            // The next playback tries again.
            if !playing {
                now.late = false;
            }
            now.playing = playing;
        });
    }

    /// Frames came late during the playback under way (the engine's `FramesLate`).
    pub fn frames_late(&self) {
        self.change(|now| now.late |= now.playing);
    }

    /// An export took (true) or released (false) the export slot.
    pub fn set_exporting(&self, exporting: bool) {
        self.change(|now| now.exporting = exporting);
    }

    /// A take is recording, from arming until it has landed.
    pub fn set_recording(&self, recording: bool) {
        self.change(|now| now.recording = recording);
    }

    /// The engine's events that concern analysis: whether the video plays, and frames late while it does.
    pub fn on_engine_event(&self, event: &EngineEvent) {
        match event {
            EngineEvent::State { playing, .. } => self.set_playing(*playing),
            EngineEvent::FramesLate { .. } => self.frames_late(),
            _ => {}
        }
    }

    /// Why `work` waits at `at`; None: it may go on.
    pub fn held(&self, work: Work, at: Instant) -> Option<Held> {
        held(&self.lock(), work, at)
    }

    /// Blocks while `work` is held, until `cancel` is set (looked at every `LOOK_EVERY`); whether it waited. Cheap when
    /// nothing holds it: a step calls it for every block or frame it decodes.
    pub fn wait(&self, work: Work, cancel: &AtomicBool) -> bool {
        let mut now = self.lock();
        let mut waited = None;
        while !cancel.load(Ordering::Relaxed) {
            let Some(why) = held(&now, work, Instant::now()) else {
                break;
            };
            if waited.is_none() {
                tracing::info!(?work, ?why, "analysis: decoding makes way");
            }
            waited.get_or_insert_with(Instant::now);
            now = self
                .changed
                .wait_timeout(now, LOOK_EVERY)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if let Some(since) = waited {
            tracing::info!(
                ?work,
                waited_ms = since.elapsed().as_millis(),
                "analysis: decoding goes on"
            );
        }
        waited.is_some()
    }

    fn change(&self, f: impl FnOnce(&mut Now)) {
        f(&mut self.lock());
        self.changed.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, Now> {
        self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Why `work` waits at `at` under `now`: a take, an export, then playback as each kind of work makes way for it.
fn held(now: &Now, work: Work, at: Instant) -> Option<Held> {
    if now.recording {
        return Some(Held::Recording);
    }
    if now.exporting {
        return Some(Held::Exporting);
    }
    match work {
        Work::Picture if now.playing => Some(Held::Playing),
        Work::Picture if now.stopped_at.is_some_and(|t| at < t + SETTLE) => Some(Held::Settling),
        Work::Sound if now.playing && now.late => Some(Held::FramesLate),
        Work::Sound | Work::Picture => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use klipolo_engine::api::StateReason;

    use super::*;

    fn soon() -> Instant {
        Instant::now() + Duration::from_millis(1)
    }

    #[test]
    fn nothing_holds_a_decode_at_first() {
        let holds = Holds::default();
        assert_eq!(holds.held(Work::Sound, Instant::now()), None);
        assert_eq!(holds.held(Work::Picture, Instant::now()), None);
        assert!(!holds.wait(Work::Picture, &AtomicBool::new(false)));
    }

    /// While the video plays the small copy waits, and for a moment after it stops; the sound goes on beside it.
    #[test]
    fn playback_holds_the_picture_and_lets_the_sound_go_on() {
        let holds = Holds::default();
        holds.set_playing(true);
        assert_eq!(holds.held(Work::Picture, soon()), Some(Held::Playing));
        assert_eq!(holds.held(Work::Sound, soon()), None);
        holds.set_playing(false);
        assert_eq!(holds.held(Work::Picture, soon()), Some(Held::Settling));
        assert_eq!(holds.held(Work::Sound, soon()), None);
        let settled = Instant::now() + SETTLE + Duration::from_millis(1);
        assert_eq!(holds.held(Work::Picture, settled), None);
    }

    /// Frames late during playback hold the sound for the rest of it; the next playback tries again. Late frames while
    /// nothing plays mean nothing.
    #[test]
    fn frames_late_hold_the_sound_until_playback_stops() {
        let holds = Holds::default();
        holds.frames_late();
        assert_eq!(holds.held(Work::Sound, soon()), None, "not playing");
        holds.set_playing(true);
        holds.frames_late();
        assert_eq!(holds.held(Work::Sound, soon()), Some(Held::FramesLate));
        assert_eq!(holds.held(Work::Picture, soon()), Some(Held::Playing));
        holds.set_playing(false);
        assert_eq!(holds.held(Work::Sound, soon()), None);
        holds.set_playing(true);
        assert_eq!(
            holds.held(Work::Sound, soon()),
            None,
            "the next playback tries again"
        );
    }

    #[test]
    fn an_export_or_a_take_holds_every_decode() {
        let holds = Holds::default();
        holds.set_exporting(true);
        for work in [Work::Sound, Work::Picture] {
            assert_eq!(holds.held(work, soon()), Some(Held::Exporting));
        }
        holds.set_recording(true);
        assert_eq!(holds.held(Work::Sound, soon()), Some(Held::Recording));
        holds.set_exporting(false);
        holds.set_recording(false);
        for work in [Work::Sound, Work::Picture] {
            assert_eq!(holds.held(work, soon()), None);
        }
    }

    /// The engine's own words: playing or not, and frames late.
    #[test]
    fn the_engine_says_when_it_plays_and_falls_behind() {
        let holds = Holds::default();
        let state = |playing| EngineEvent::State {
            playing,
            at_us: 0,
            reason: if playing {
                StateReason::Play
            } else {
                StateReason::Pause
            },
            rate: 1.0,
        };
        holds.on_engine_event(&state(true));
        assert_eq!(holds.held(Work::Picture, soon()), Some(Held::Playing));
        holds.on_engine_event(&EngineEvent::FramesLate { at_us: 0 });
        assert_eq!(holds.held(Work::Sound, soon()), Some(Held::FramesLate));
        holds.on_engine_event(&state(false));
        assert_eq!(holds.held(Work::Sound, soon()), None);
    }

    /// A held step waits until it is let go, and a cancel ends the wait too.
    #[test]
    fn a_held_step_waits_until_let_go_or_cancelled() {
        let holds = Arc::new(Holds::default());
        holds.set_exporting(true);
        let waiting = {
            let holds = holds.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let waited = holds.wait(Work::Sound, &AtomicBool::new(false));
                (waited, started.elapsed())
            })
        };
        std::thread::sleep(Duration::from_millis(150));
        assert!(!waiting.is_finished(), "held while the export runs");
        holds.set_exporting(false);
        let (waited, took) = waiting.join().unwrap();
        assert!(waited);
        assert!(took >= Duration::from_millis(150), "{took:?}");

        holds.set_recording(true);
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = {
            let (holds, cancel) = (holds.clone(), cancel.clone());
            std::thread::spawn(move || holds.wait(Work::Picture, &cancel))
        };
        std::thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::Relaxed);
        assert!(
            stopping.join().unwrap(),
            "it waited, then the cancel ended it"
        );
    }
}
