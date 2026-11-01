//! Pipe events through the heal policy, and its actions: a restart (one per outage, with a first-frame grace; never
//! while the display sleeps), a wait for the app or the device, the mic's fallback, a source stopped for the take, a
//! stop that saves. The module doc of `rules` lists the finer rules.

use klipolo_model::coded::{CodedError, ErrorParam};

use super::core::SessionCore;
use super::effect::{Effect, PipeCmd};
use super::rules::FIRST_FRAME_GRACE_NS;
use super::source::{Status, fps_of, main_picture, mic_rules, video_rules};
use crate::english::msg;
use crate::heal::{HealAction, SourceKind};
use crate::health::HealthNote;
use crate::manifest::source_param;
use crate::pipe::{PipeError, PipeEvent};
use crate::preset::lighter;
use crate::protocol::Event;
use crate::shell::{Flash, Level, WorkerState};

impl SessionCore {
    /// A pipe's event, through the heal policy.
    pub fn pipe_event(&mut self, source: &str, event: PipeEvent, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        let Some(i) = self.index(source) else {
            return fx;
        };
        match (&event, self.state) {
            (PipeEvent::RestoreToken(_), _) => {
                // The app saves it to the game's profile; the core has nothing to do with it.
                tracing::info!(source, "core: the portal gave a new restore token");
            }
            (PipeEvent::Blank(blank), _) => {
                self.srcs[i].blank = *blank;
                let _ = self.heal.decide(source, self.srcs[i].kind, &event, now_ns);
            }
            // "Stop sharing" before the gate is a start refused: nothing was recorded, so it never says "was saved".
            (PipeEvent::StoppedByUser, WorkerState::Starting | WorkerState::Countdown) => {
                fx.push(Effect::Emit(Event::Failed {
                    error: msg!("capture.stop.sharingBeforeStart"),
                }));
                self.stop(now_ns, None, &mut fx);
            }
            // Permission pulled before the gate is a start it refused: the main picture ends
            // the take with nothing saved and its permission's step; any other source is noted and left out.
            (PipeEvent::PermissionRevoked, WorkerState::Starting | WorkerState::Countdown) => {
                if main_picture(&self.srcs) != Some(i) {
                    fx.push(self.pipe(i, PipeCmd::Stop));
                }
                fx.extend(self.failed_to_start(source, PipeError::PermissionDenied, now_ns));
            }
            (_, WorkerState::Recording | WorkerState::Paused) => {
                self.heal_event(i, &event, now_ns, &mut fx)
            }
            // A pipe's own note as it starts (an encoder fallback, Presenter Overlay) is kept for after the take too.
            (PipeEvent::Note(_), WorkerState::Starting | WorkerState::Countdown) => {
                self.heal_event(i, &event, now_ns, &mut fx)
            }
            _ => {}
        }
        fx
    }

    pub(super) fn heal_event(
        &mut self,
        i: usize,
        event: &PipeEvent,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        let src = &mut self.srcs[i];
        // One stall spends one restart: a video source's restart under way absorbs the next stall report (the pipe's
        // and the stall rule's of one outage) until its first frame or the grace. A stop is always acted on, and a
        // sound source has no stall rule to catch a restart that failed, so nothing else is absorbed.
        let restarting = src
            .restart_at
            .is_some_and(|at| now.saturating_sub(at) < FIRST_FRAME_GRACE_NS);
        if *event == PipeEvent::Stalled && src.is_video() && restarting {
            return;
        }
        if matches!(event, PipeEvent::Stalled | PipeEvent::StoppedBySystem)
            && src.waits_for_wake
            && !self.display_awake
        {
            src.wake_restart = true;
            src.status = Status::Starting;
            return self.end_run(i, fx);
        }
        let action = self.heal.decide(&src.id, src.kind, event, now);
        self.act(i, action, now, fx);
    }

    /// Carries out the heal policy's `action` for source `i`. `Ignore`, the answer to the wait's poll on every core
    /// input while a mic waits for its own device, costs nothing: the source's id is cloned only in the arms that note
    /// it.
    pub(super) fn act(&mut self, i: usize, action: HealAction, now: u64, fx: &mut Vec<Effect>) {
        match action {
            HealAction::Restart | HealAction::RebuildEncoder => {
                self.restart(i, PipeCmd::Restart, now, fx)
            }
            HealAction::LowerPreset => {
                // The pipe steps down by the same rule (`VideoPipe::lower_preset`), so the next run's muxer and drop
                // rule follow the lighter rate; at the lightest preset this is one more rebuild.
                let src = &mut self.srcs[i];
                if let Some(p) = src.preset.and_then(lighter) {
                    (src.preset, src.fps) = (Some(p), fps_of(Some(p)));
                }
                self.restart(i, PipeCmd::Lower, now, fx)
            }
            HealAction::WaitForTarget => {
                let id = self.srcs[i].id.clone();
                self.end_run(i, fx);
                self.srcs[i].status = Status::WaitingTarget;
                fx.push(self.pipe(i, PipeCmd::Pause));
                let app = self
                    .cfg
                    .game
                    .as_ref()
                    .map_or_else(|| id.clone(), |g| g.name.clone());
                let note = msg!("capture.health.appQuit", app = app.as_str());
                self.note(
                    note,
                    Level::Warning,
                    Some(&id),
                    false,
                    Some(Flash::Problem),
                    fx,
                );
            }
            HealAction::WaitForDevice => {
                let id = self.srcs[i].id.clone();
                self.end_run(i, fx);
                self.srcs[i].status = Status::WaitingDevice(now);
                let note = msg!(
                    "capture.health.reconnecting",
                    source = source_param(&self.srcs[i].label)
                );
                self.note(note, Level::Healing, Some(&id), false, None, fx);
            }
            HealAction::FallbackMic => self.fall_back(i, now, fx),
            HealAction::SwitchBack => self.switch_back(i, now, fx),
            HealAction::Flapping => self.flapping(i, now, fx),
            HealAction::PauseSource(note) => {
                self.end_run(i, fx);
                self.srcs[i].status = Status::Stopped;
                fx.push(self.pipe(i, PipeCmd::Stop));
                self.health(note, true, Some(Flash::Problem), fx);
                self.end_its_sound(i, fx);
                let live =
                    |s: &&super::source::Src| !matches!(s.status, Status::Stopped | Status::Left);
                if !self.srcs.iter().any(|s| live(&s)) {
                    self.stop_and_save("capture.stop.noSources", now, fx);
                }
            }
            HealAction::StopAndSave(code) => self.stop_and_save(code, now, fx),
            HealAction::Note(note) => {
                // A device missing for 10 s shows as a gap in the manifest already; a blank picture or a pipe's own
                // warning doesn't, so those are kept for after the take.
                let missing = note.params.iter().any(|(k, v)| {
                    *k == "reason" && matches!(v, ErrorParam::Text(t) if t == "missing")
                });
                let flash = (note.level != Level::Ok).then_some(Flash::Problem);
                self.health(note, !missing, flash, fx);
            }
            HealAction::Ignore => {}
        }
    }

    /// Source `i` was given up: the app's or game's sound that comes on its stream ends with it, with a note, never
    /// silently.
    fn end_its_sound(&mut self, i: usize, fx: &mut Vec<Effect>) {
        let of = self.srcs[i].id.clone();
        for j in 0..self.srcs.len() {
            let src = &self.srcs[j];
            if src.comes_with.as_deref() == Some(of.as_str())
                && !matches!(src.status, Status::Stopped | Status::Left)
            {
                self.end_run(j, fx);
                self.srcs[j].status = Status::Stopped;
                fx.push(self.pipe(j, PipeCmd::Stop));
                let note = crate::heal::ended_with_its_video(&self.srcs[j].id);
                self.health(note, true, None, fx);
            }
        }
    }

    /// Ends source `i`'s run and restarts its pipe with `cmd` (`Restart` or `Lower`); the restart's first frame gets a
    /// grace before it counts as stalled.
    pub(super) fn restart(&mut self, i: usize, cmd: PipeCmd, now: u64, fx: &mut Vec<Effect>) {
        self.end_run(i, fx);
        let src = &mut self.srcs[i];
        (src.status, src.restart_at, src.key_asked) = (Status::Starting, Some(now), false);
        fx.push(self.pipe(i, cmd));
    }

    /// A (re)started pipe while the take runs: a keyframe asked, new stall rules, and paused if the take is.
    pub(super) fn live_again(&mut self, i: usize, now: u64, fx: &mut Vec<Effect>) {
        let src = &mut self.srcs[i];
        src.restart_at.get_or_insert(now);
        if src.is_video() {
            src.video_rules = Some(video_rules(&src.id, src.fps));
            src.key_asked = true;
            fx.push(self.pipe(i, PipeCmd::Keyframe));
        } else if src.kind == SourceKind::Mic && self.srcs[i].mic_rules.is_none() {
            self.srcs[i].mic_rules = Some(mic_rules(
                &self.srcs[i].id,
                self.srcs[i].device.as_deref(),
                &self.mics,
            ));
        }
        if self.state == WorkerState::Paused {
            fx.push(self.pipe(i, PipeCmd::Pause));
        }
    }

    /// A rule's note: weighted by its level, journaled `critical` (it reaches the manifest's notes).
    pub(super) fn health(
        &self,
        note: HealthNote,
        critical: bool,
        flash: Option<Flash>,
        fx: &mut Vec<Effect>,
    ) {
        let source = note.source.clone();
        self.note(
            note.coded(),
            note.level,
            source.as_deref(),
            critical,
            flash,
            fx,
        );
    }

    /// A note about the take as a whole, shown now, not kept for after it.
    pub(super) fn health_coded(&self, note: CodedError, level: Level, fx: &mut Vec<Effect>) {
        fx.push(Effect::Emit(Event::Health {
            note,
            level,
            source: None,
        }));
    }
}
