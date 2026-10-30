//! What the worker tells the parent: one JSON object per stdout line. Logs go to stderr. The parent's one
//! command, `pause`, arrives as a line on the worker's stdin.

use std::path::PathBuf;

use klipolo_model::coded::CodedError;
use klipolo_model::project::FormatKind;
use serde::{Deserialize, Serialize};

use super::job::ExportMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    InvalidJob,
    MissingMedia,
    Decode,
    Gpu,
    EncoderUnavailable,
    Encode,
    DiskFull,
    /// A file's disk didn't answer within a minute: the job pauses with a message naming the file.
    NotAnswering,
    /// A frame couldn't be drawn whole (an effect or text budget, a protection that couldn't run):
    /// `message` is the coded reason (`exports.frame.*`, a `CodedError` as JSON), which the export shows
    /// coded. Not retried: the same frame stops any run.
    Frame,
    Io,
    Internal,
    /// A take whose frames have no cursor (`Separate`, `BurnedFrom`) has a cursor this export can't read (its file
    /// can't be read, or the takes before it fill the budget): `message` is the coded question (`exports.cursorMissing`
    /// with the recording's name as `recording`, a `CodedError` as JSON). Sent before anything is written; the job runs
    /// again only once the person allows it (`ExportJob::allow_missing_cursor`). Not retried.
    CursorMissing,
    /// A screen whose activity the plans read can't be read now (a drive gone, no access yet), tried again
    /// (`prepare::activity::RETRY_WAITS`): `message` is the coded question (`exports.activityUnreadable` with the file's
    /// name as `file`; the job keeps its path, `ExportJob::activity_asked`). Not retried: the job runs again once the
    /// person says Try again, or to export without that screen (`ExportJob::activity_without`).
    ActivityUnreadable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkerLine {
    /// `total_frames` counts every format's frames; `frames_done` of them were finished by earlier runs.
    Started {
        job_id: String,
        total_frames: u64,
        frames_done: u64,
        formats: Vec<FormatKind>,
        mode: ExportMode,
    },
    /// `frame` of `total_frames` format-frames done (earlier runs' segments included), at this run's measured `fps`
    /// (format-frames a second).
    Progress {
        frame: u64,
        total_frames: u64,
        fps: f64,
        segments_done: u32,
        segments_total: u32,
    },
    /// Not every format fit in one pass: they are exported one after another.
    Fallback {
        reason: String,
    },
    /// Still setting up (checking what earlier runs finished, decoding the sound): sent every few seconds, so the
    /// parent sees the worker is alive.
    Preparing,
    /// Before the first frame, the screens' activity its plans read is being made ("Analysing the
    /// screen…"): `done` of it (0–1, by length), sent as each minute is made and every few seconds meanwhile. Only a
    /// run with a screen left to analyse sends it.
    Analysing {
        done: f32,
    },
    /// The pictures are done and the sound is still rendering: `done_s` of `total_s` seconds of the current pass
    /// (sent every few seconds while it advances; the AAC re-limit pass starts again from 0).
    Sound {
        done_s: f64,
        total_s: f64,
    },
    /// A frame waits on a video decoder that is still making progress (a loaded computer can hold one for seconds):
    /// sent every few seconds while it waits, so the parent's watchdog stops only a worker that has gone silent.
    WaitingForDecoder,
    /// The segments are being joined (repeated while it runs, so the parent sees the worker is alive).
    Finishing,
    /// A format's joined file is verified at `temp_path` (in the job folder).
    FormatDone {
        format: FormatKind,
        temp_path: PathBuf,
        bytes: u64,
    },
    /// Something this export does that the person should know: a coded note (`exports.note.*`), such as a cursor it
    /// exports without (`exports.note.cursorDropped`).
    Note {
        message: CodedError,
    },
    /// Stopped as asked: every finished segment is kept.
    Paused,
    Done {
        elapsed_s: f64,
    },
    Error {
        code: ErrorCode,
        message: String,
        retryable: bool,
    },
}

/// The command the parent writes to the worker's stdin (a line) to pause it.
pub const PAUSE: &str = "pause";

/// One worker line, or None for anything else on stdout (a library printing there is skipped).
pub fn parse_line(line: &str) -> Option<WorkerLine> {
    serde_json::from_str(line.trim()).ok()
}

/// The worker's stdout. A failed write means the parent is gone: the worker stops and cleans up.
pub struct Lines<W: std::io::Write = std::io::Stdout> {
    out: W,
}

impl Lines {
    pub fn stdout() -> Self {
        Self {
            out: std::io::stdout(),
        }
    }
}

impl<W: std::io::Write> Lines<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }

    /// One line: a `WorkerLine` (export) or a `ProxyLine` (editing copies).
    pub fn send<L: Serialize>(&mut self, line: &L) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.out, line)?;
        self.out.write_all(b"\n")?;
        self.out.flush()
    }
}
