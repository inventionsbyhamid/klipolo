//! What recovery gives and takes: a run's outcome, the take's result, its errors, and the watch a long repair reports
//! to.

use std::fmt;
use std::io::ErrorKind;

use klipolo_model::time::Us;

use crate::manifest::Manifest;

/// A run file after repair: what the manifest takes from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunRepair {
    /// The media it holds: the fMP4 index's total, or the CAF's frames over its rate.
    pub duration_us: Us,
    /// Non-zero bytes cut off, kept beside the file.
    pub damaged_bytes: u64,
    /// The file was written to (a head, a cut, the remux or the CAF's size).
    pub changed: bool,
}

/// What repair made of one run file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// Ready to import, as measured.
    Repaired(RunRepair),
    /// Nothing recorded to import: no file, only its head, or only zeros (pages that never landed).
    Empty,
    /// Recorded media that couldn't be repaired automatically. Its file stays in the take's folder (as the step found
    /// it, with anything cut kept beside it), and the manifest says so (`capture.recovered.partKept`): a person never
    /// loses part of a take silently.
    LeftOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoverError {
    /// A recorder holds the take.
    Live,
    /// No readable `session.json`.
    NoConfig,
    /// A disk step failed (logged with its file); nothing it did is lost, and a later repair goes on from there.
    Io(ErrorKind),
    /// `RepairWatch::cancelled` said so. Every step finished stays; a later repair goes on from there.
    Cancelled,
}

/// What a long repair reports and how it is cancelled (`repair_watched`).
pub trait RepairWatch {
    /// `done` of `total` bytes of the take's run files are checked. Called after each file and while a file's media
    /// past its durable mark is read (a take without a journal reads every sample).
    fn progress(&mut self, done: u64, total: u64);
    /// True once the person cancelled: the repair stops at its next step.
    fn cancelled(&self) -> bool;
}

/// No progress to show and never cancelled.
pub(super) struct Unwatched;

impl RepairWatch for Unwatched {
    fn progress(&mut self, _done: u64, _total: u64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

impl fmt::Display for RecoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Live => write!(f, "a recorder holds the take"),
            Self::NoConfig => write!(f, "the take has no readable session.json"),
            Self::Io(kind) => write!(f, "a disk step failed: {kind}"),
            Self::Cancelled => write!(f, "the repair was cancelled"),
        }
    }
}

impl std::error::Error for RecoverError {}

#[derive(Clone, Debug, PartialEq)]
pub struct Repaired {
    pub manifest: Manifest,
    pub took_ms: u32,
    /// Non-zero bytes cut off this time and kept beside their files.
    pub damaged_bytes: u64,
}
