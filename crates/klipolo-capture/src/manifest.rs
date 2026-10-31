//! The manifest (`manifest.json`): what the take holds, derived from the config, the journal, the anchors and the
//! files' measured lengths, never the only record. Stop and recovery derive it the same way, so it can
//! always be rebuilt; import turns it into format 4's `Recording`.
//!
//! - **Runs** come from `RunStarted` (each source's runs in run order). A run's duration is its file's measured length;
//!   a run whose file has no measured length holds nothing and is left out, and so is a source with no runs.
//! - **Placement.** Video carries its own times: a video run starts at its `RunStarted` offset, with no drift. A sound
//!   run with at least 10 anchors is placed by its own drift line (`drift::fit`, one line per run: the session ends a run
//!   at an outage of 1 s or more). With fewer, or a line that fails the checks below, it starts at its first sample's
//!   time with no drift and is `approximate`.
//! - **A line that isn't one.** If a run's anchors lie on two lines (an outage the run wasn't split at), one line
//!   places part of the run off by the outage. The fit is refused when its drift is over 1,000 ppm (ten times a real
//!   device's 100 ppm) or when at least two anchors, and 10 % or more of them, sit 40 ms or more off it. The margins
//!   clear what a good line shows: the fit's own cut drops about 4 % of a clean, jittery run's anchors, the anchors'
//!   device stamps jitter by milliseconds, and one stray anchor in a short run isn't a second line. A step
//!   under 40 ms still passes as one line; the session ends a run at any real frame loss, so that is a backstop, tighter
//!   than the 250 ms it was. `Fit::anchors_used` and `max_residual_us` aren't used as alarms.
//! - **Duration**: `Finalized`'s, or the latest run end without it, and never shorter than a run.
//! - **The cursor** (`cursor`): the screen's plan, a burn-in's time and the weakest click mode, from the journal.
//! - **Notes**: the critical `Health` entries in order, then each gap between two runs of a source, from the end of the
//!   earlier run's media to the next run's first sample ("Mic stopped for 3 s at 1:00"; gaps under 50 ms aren't
//!   noted), then `capture.recovered.approximate` once if a line was refused.
//!
//! Cost: O(entries + anchors), one pass each; one fit per sound run.

mod cursor;
mod derive;

pub use derive::derive;

use std::io;

use klipolo_cursor::ClickSource;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::{GameInfo, Marker, RecordingKind, SourceRole};
use klipolo_model::time::Us;
use klipolo_platform::durable::{Durability, write_atomic};
use serde::{Deserialize, Serialize};

use crate::media::SourceId;
use crate::session::{FileKey, ImportTarget, SessionDir};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub session: String,
    pub kind: RecordingKind,
    pub duration_us: Us,
    pub sources: Vec<ManifestSource>,
    pub markers: Vec<Marker>,
    /// The cursor when a screen source recorded one (its file may still be empty).
    pub cursor: Option<ManifestCursor>,
    pub game: Option<GameInfo>,
    pub notes: Vec<CodedError>,
    pub recovered: bool,
    pub target: ImportTarget,
}

/// The take's cursor file and how it was recorded (`manifest::cursor`), as import's `RecordingCursor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestCursor {
    pub file: FileKey,
    pub capture: CursorCapture,
    pub clicks: ClickSource,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSource {
    pub id: SourceId,
    pub role: SourceRole,
    pub label: String,
    pub runs: Vec<ManifestRun>,
}

/// A run on the recording clock, as format 4's `SourceRun` takes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRun {
    pub file: FileKey,
    pub offset_us: Us,
    /// The file's own length.
    pub duration_us: Us,
    pub drift_ppm: f64,
    /// Placed by its first sample's time, without a drift line.
    pub approximate: bool,
    /// A sound run's loudest sample on each channel, linear 0-1 (`Entry::Levels`); None for video, or when unknown.
    /// Import plays a stereo run whose second channel stayed near silent centred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_peaks: Option<Vec<f32>>,
}

impl ManifestRun {
    /// Where the run ends on the recording clock (its own length shrinks by the drift, as `SourceRun::rec_span`).
    pub(super) fn end_us(&self) -> Us {
        let len = self.duration_us as f64 / (1.0 + self.drift_ppm * 1e-6);
        self.offset_us.saturating_add(len.round() as Us)
    }
}

/// Writes `manifest.json` atomically and fully to the disk.
pub fn write_manifest(dir: &SessionDir, m: &Manifest) -> io::Result<()> {
    write_atomic(
        &dir.manifest(),
        &serde_json::to_vec_pretty(m)?,
        Durability::Full,
    )
}

/// `manifest.json`; NotFound without one, InvalidData when it isn't a manifest.
pub fn read_manifest(dir: &SessionDir) -> io::Result<Manifest> {
    serde_json::from_slice(&std::fs::read(dir.manifest())?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// A source's label as a message's `source`: a nested `capture.source.*` code, or the person's own name as text.
pub(crate) fn source_param(label: &str) -> ErrorParam {
    if label.starts_with("capture.source.") {
        ErrorParam::Coded(Box::new(CodedError::new(label)))
    } else {
        ErrorParam::Text(label.to_owned())
    }
}
