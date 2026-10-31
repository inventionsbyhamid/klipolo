//! Where the recorder's outcomes go in the app (`AppSink`): the page's events, a finished take's landing, and the
//! engine's background media work, held while a take is the recorder's.
//!
//! Events:
//! - `recorder-event` (`RecorderEvent`): the worker's events and the recorder's own, each with its take's id.
//! - `recorder-imported` (`TakeImport`): a take landed in its project as one undo step. Its snapshot is that project,
//!   open now. When the take didn't land, the page hears a critical `Health` note on `recorder-event` instead (a
//!   `capture.*` code); the take's files stay in its folder either way.

use std::path::Path;

use klipolo_capture::protocol::Event;
use klipolo_capture::shell::Level;
use tauri::{AppHandle, Emitter, Manager};

use super::destination::today;
use super::env::{Done, Sink};
use super::import::{Landing, land};
use super::setup::Files;
use super::types::RecorderEvent;
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

    fn finished(&self, done: &Done) {
        let Some(sessions) = self.app.try_state::<ProjectSessionState>() else {
            tracing::error!(
                take = done.id,
                "recorder: no project session to land the take in"
            );
            return;
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
            }
            Err(note) => {
                let said = Event::Health {
                    note,
                    level: Level::Critical,
                    source: None,
                };
                self.event(&done.id, &said);
            }
        }
    }

    fn hold_media(&self, held: bool) {
        if let Some(engine) = self.app.try_state::<EngineState>() {
            engine.hold_for_recording(held);
        }
    }
}
