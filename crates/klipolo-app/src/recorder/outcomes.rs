//! What the supervisor's decisions about one take come to (`events` carries them out once the recorder's lock is let
//! go): the page's events, Klipolo's window hidden, and the take handed on to land.
//!
//! **An encoder that gives up mid-take.** The worker heals a failing encoder itself: it rebuilds it (at most 3 times
//! in 10 minutes), then steps its quality down once, and only then gives the source up with `capture.health.lost` and
//! the reason `encoder` (macOS's encoder can fail this way with two H.264 sessions on one engine, the screen's and the
//! camera's).
//! - **The camera's:** the take records on without it. The page hears it once, plainly (`capture.note.cameraDropped`,
//!   a warning: "The camera stopped recording. The screen kept recording."), and Klipolo stays out of the way.
//! - **The screen's:** a take without its picture is no use, and the person, with Klipolo hidden, wouldn't know. So
//!   the app stops the take there (`Stop`, as the person's own): the worker saves everything written so far, the page
//!   hears why (`capture.stop.encoder`, critical: "The video encoder stopped, so the recording stopped and was saved.
//!   Press Record to try again."), and Klipolo comes back with the take once it is saved.
//!
//! A worker that dies while saving is never resumed: the take is finished from its files (`Recover`). A worker the
//! encoder takes down is resumed (3 times in 10 minutes), and after that the take ends with `capture.stop.crashLoop`,
//! finished from its files (`give_up`).

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::recover;
use klipolo_capture::shell::{Level, WorkerState};
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::project::SourceRole;

use super::env::Done;
use super::send::write_line;
use super::supervise::SupervisorAction;
use super::take::Take;

/// What is done once the lock is let go.
pub(super) enum Effect {
    /// To the page, with its take's id.
    Emit(String, Event),
    Hide,
    /// The take is complete: hand it on to land.
    Finish(Done),
}

pub(super) fn take_event(take: &mut Take, ev: &Event, now: u64) -> Vec<Effect> {
    let mut out = Vec::new();
    for action in take.sup.on_event(ev, now) {
        match action {
            SupervisorAction::Forward(ev) => {
                if let Event::Failed { error } = &ev {
                    // In the app's log too: a take that ends with no picture takes its folder, journal and
                    // diagnostics with it. The code only, never what was recorded.
                    tracing::warn!(take = %take.cfg.id, code = %error.code, "recorder: the take failed");
                }
                let gave_up = encoder_gave_up(&ev);
                out.push(Effect::Emit(take.cfg.id.clone(), ev));
                if let Some(source) = gave_up {
                    out.extend(after_encoder(take, source));
                }
            }
            SupervisorAction::Hide => out.push(Effect::Hide),
            SupervisorAction::Import => out.push(Effect::Finish(done(take, false))),
            _ => {}
        }
    }
    out
}

/// The source the worker gave up because its encoder kept failing.
fn encoder_gave_up(ev: &Event) -> Option<String> {
    let Event::Health {
        note,
        source: Some(source),
        ..
    } = ev
    else {
        return None;
    };
    let encoder = matches!(note.params.get("reason"), Some(ErrorParam::Text(r)) if r == "encoder");
    (note.code == "capture.health.lost" && encoder).then(|| source.clone())
}

/// The worker gave `source` up for its encoder: the screen's stops the take, the camera's is said and the take goes on.
fn after_encoder(take: &mut Take, source: String) -> Vec<Effect> {
    let role = (take.cfg.sources.iter())
        .find(|s| s.id == source)
        .map(|s| s.role);
    match role {
        Some(SourceRole::Screen) => stop_for_encoder(take, source),
        Some(SourceRole::Camera) => camera_dropped(take, source),
        _ => Vec::new(),
    }
}

/// The camera's encoder gave up and the screen records on: said once, as a warning. Klipolo stays hidden.
fn camera_dropped(take: &mut Take, source: String) -> Vec<Effect> {
    if std::mem::replace(&mut take.camera_dropped, true) {
        return Vec::new();
    }
    tracing::warn!(take = %take.cfg.id, source, "recorder: the camera's encoder gave up, so the screen records on without it");
    let note = Event::Health {
        note: CodedError::new("capture.note.cameraDropped"),
        level: Level::Warning,
        source: Some(source),
    };
    vec![Effect::Emit(take.cfg.id.clone(), note)]
}

/// Stops `take` for its screen's encoder, once, unless the person or the worker is stopping it already: `Stop` to the
/// worker, and the page's plain words.
fn stop_for_encoder(take: &mut Take, source: String) -> Vec<Effect> {
    if take.stopped_by_app || take.sup.is_stopping() {
        return Vec::new();
    }
    take.stopped_by_app = true;
    tracing::warn!(take = %take.cfg.id, source, "recorder: an encoder gave up, so the take stops and is saved");
    take.sup.stop_requested();
    if let Some(worker) = take.worker.as_mut() {
        write_line(worker, &Command::Stop);
    }
    let note = Event::Health {
        note: CodedError::new("capture.stop.encoder"),
        level: Level::Critical,
        source: Some(source),
    };
    vec![Effect::Emit(take.cfg.id.clone(), note)]
}

/// Crashed too often (or couldn't be resumed): the take ends with `capture.stop.crashLoop` ("The recorder kept
/// stopping, so the recording was saved.") and is finished from its files, as a stopped one is.
pub(super) fn give_up(done: Done) -> Vec<Effect> {
    tracing::error!(
        take = done.id,
        "recorder: the worker kept stopping, so the take is finished from its files"
    );
    let note = Event::Health {
        note: CodedError::new("capture.stop.crashLoop"),
        level: Level::Critical,
        source: None,
    };
    vec![Effect::Emit(done.id.clone(), note), Effect::Finish(done)]
}

/// Nothing records any more, and there is no take to wait for: the page goes back to the dialog's choices.
pub(super) fn idle() -> Event {
    Event::State {
        state: WorkerState::Idle,
        elapsed_us: 0,
    }
}

pub(super) fn done(take: &Take, recover: bool) -> Done {
    Done {
        id: take.cfg.id.clone(),
        dir: take.dir.clone(),
        target: take.cfg.target.clone(),
        project_file: take.project_file.clone(),
        recover,
        note: None,
    }
}

/// The take's journal ends with the person's Stop, `Finalized` or `Recovered`.
pub(super) fn journal_says_stopped(take: &Take) -> bool {
    let end = recover::ending(&take.dir);
    end.stop_requested || end.finalized || end.recovered
}
