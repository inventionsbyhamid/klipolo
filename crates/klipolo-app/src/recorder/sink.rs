//! Where the recorder's outcomes go in the app (`AppSink`): the page's events, a finished take's landing, and the
//! background media work (the engine's, editing copies and analysis's decoding), held while a take is the recorder's.
//!
//! Events:
//! - `recorder-event` (`RecorderEvent`): the worker's events and the recorder's own, each with its take's id.
//! - `recorder-imported` (`TakeImport`): a take landed in its project as one undo step, then its clicks' auto zoom as a
//!   second (`zooms` items; none and no step without clicks). Its snapshot is that project, open now, zooms and all.
//!   When the take didn't land, the page hears a critical `Health` note on `recorder-event` instead (a `capture.*`
//!   code); the take's files stay in its folder either way.
//! - `recorder-left` (`LeftTake`): what became of a take an earlier run left behind (`left`) when it didn't land as a
//!   `recorder-imported`: it couldn't be added, its new project couldn't be opened, or its worker records it still.

use std::path::Path;

use klipolo_app::analysis::holds::Holds;
use klipolo_capture::protocol::Event;
use klipolo_capture::shell::Level;
use klipolo_model::coded::CodedError;
use tauri::{AppHandle, Emitter, Manager};

use super::destination::today;
use super::env::{Done, Sink};
use super::import::{Landing, land};
use super::setup::Files;
use super::types::{LeftOutcome, LeftTake, RecorderEvent};
use crate::commands::analysis::AnalysisHandle;
use crate::media_jobs::MediaJobs;
use crate::project::{ProjectSessionState, library_dir};
use crate::state::EngineState;

pub(super) struct AppSink {
    app: AppHandle,
    /// The recorder's own files (`<app data>/recorder`).
    files: Option<Files>,
}

impl AppSink {
    pub(super) fn new(app: AppHandle, files: Option<Files>) -> Self {
        Self { app, files }
    }
}

impl Sink for AppSink {
    fn event(&self, take: &str, ev: &Event) {
        let payload = RecorderEvent {
            take: take.to_owned(),
            event: ev.clone(),
        };
        if let Err(error) = self.app.emit("recorder-event", payload) {
            tracing::warn!(%error, "recorder: an event didn't reach the page");
        }
    }

    fn finished(&self, done: &Done) -> Result<(), CodedError> {
        let Some(sessions) = self.app.try_state::<ProjectSessionState>() else {
            tracing::error!(
                take = done.id,
                "recorder: no project session to land the take in"
            );
            return Err(CodedError::new("capture.import.notAdded"));
        };
        let mut probe = |path: &Path, _key: &str| crate::media::probe_asset(path);
        let landing = Landing {
            library: &library_dir,
            today: today(),
            files: self.files.as_ref(),
            probe: &mut probe,
        };
        match land(&sessions, done, landing) {
            Ok(import) => {
                if let Err(error) = self.app.emit("recorder-imported", import) {
                    tracing::warn!(%error, "recorder: the landed take didn't reach the page");
                }
                Ok(())
            }
            // A take an earlier run left behind: `left` says what became of it.
            Err(note) if done.earlier_run => Err(note),
            Err(note) => {
                let said = Event::Health {
                    note: note.clone(),
                    level: Level::Critical,
                    source: None,
                };
                self.event(&done.id, &said);
                Err(note)
            }
        }
    }

    fn left(&self, take: &str, outcome: LeftOutcome) {
        let payload = LeftTake {
            take: take.to_owned(),
            outcome,
        };
        if let Err(error) = self.app.emit("recorder-left", payload) {
            tracing::warn!(%error, "recorder: a take left behind didn't reach the page");
        }
    }

    fn hold_media(&self, held: bool) {
        let engine = self.app.try_state::<EngineState>();
        let copies = self.app.try_state::<MediaJobs>();
        let analysis = self.app.try_state::<AnalysisHandle>();
        let holds = analysis.as_ref().map(|a| a.holds().as_ref());
        hold_background(engine.as_deref(), copies.as_deref(), holds, held);
    }
}

/// A take holds Klipolo's background media work from arming until it has landed, so nothing competes with it: the
/// engine's (`EngineState::hold_for_recording`), editing copies (`MediaJobs::set_recording`) and analysis's decoding
/// (`Holds::set_recording`). `held` false lets go.
pub(crate) fn hold_background(
    engine: Option<&EngineState>,
    copies: Option<&MediaJobs>,
    analysis: Option<&Holds>,
    held: bool,
) {
    if let Some(engine) = engine {
        engine.hold_for_recording(held);
    }
    if let Some(copies) = copies {
        copies.set_recording(held);
    }
    if let Some(analysis) = analysis {
        analysis.set_recording(held);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use klipolo_app::analysis::holds::{Held, Holds, Work};

    use super::hold_background;

    /// A take holds analysis's decoding from arming until it has landed, as it holds copies.
    #[test]
    fn a_take_holds_analysis_until_it_lets_go() {
        let holds = Holds::default();
        hold_background(None, None, Some(&holds), true);
        for work in [Work::Sound, Work::Picture] {
            assert_eq!(holds.held(work, Instant::now()), Some(Held::Recording));
        }
        hold_background(None, None, Some(&holds), false);
        assert_eq!(holds.held(Work::Picture, Instant::now()), None);
    }
}
