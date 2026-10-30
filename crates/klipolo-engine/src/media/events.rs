//! "This file's derived data changed": the timeline redraws its waveform or filmstrip (`EngineEvent::Media`). And
//! "this file's sound is damaged" (`EngineEvent::Notice`, `engine.audioDamaged`), said once per opening.

use std::sync::Arc;

use klipolo_model::time::Us;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which of a file's derived data is ready (or grew).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MediaKind {
    /// The waveform peaks (`.peaks`, written with the sound cache).
    Peaks,
    /// One or more thumbnail sprite chunks.
    Thumbs,
    /// An editing copy.
    Proxy,
    /// The screen activity track grew by a chunk or was read from the cache: what plans with it (Auto zooms,
    /// the follow-crop, chat's facts) looks again. The timeline draws nothing from it.
    Activity,
}

/// What happened to a source file (`path`, as the project names it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaEvent {
    /// Its derived data is ready (or grew).
    Changed { path: String, kind: MediaKind },
    /// Its sound can't be read past `at_us` (its own time): a bad sector, a failing drive. It plays up to there and
    /// is silent after it until the project is opened again.
    AudioDamaged { path: String, at_us: Us },
}

/// Where media events go (the engine forwards them to the UI).
pub type MediaSink = Arc<dyn Fn(MediaEvent) + Send + Sync>;
