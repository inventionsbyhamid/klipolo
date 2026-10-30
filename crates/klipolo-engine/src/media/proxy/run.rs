//! The editing copy's segment loop: every pending segment in order, each saved to the manifest as soon as
//! it verifies. Between segments it stops for a pause or stop request, a computer running hot, too little free space
//! or a changed source. Finished segments are never touched again.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::job::{ProxyJob, ProxyState, Retry, SourceState};
use crate::export::shared::{Control, is_disk_full};
use klipolo_platform::power::Thermal;

/// Free space below this, plus the next segment's expected size, pauses the copy.
pub const FREE_FLOOR: u64 = 2 << 30;

/// Why a copy paused. Each resumes from its finished segments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PauseReason {
    /// Asked to (pause, stop, or the app quit).
    User,
    /// "Your disk is almost full."
    DiskFull,
    /// The computer runs hot (`Thermal::Serious` or worse).
    Thermal,
    /// The video encoder is busy (an export, a screen recorder) or refused: never a software fallback.
    EncoderBusy,
}

/// Why a copy failed, coded: the words, and whether it is tried again by itself (`retry`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailCode {
    /// The source isn't there now ("intro.mp4 is missing"): an unplugged drive, a volume not mounted, a moved file.
    /// Its finished segments are kept, and it continues when the file is there again.
    Missing,
    /// The source was edited or replaced while it was copied: its new version gets a job of its own (a new stem).
    Changed,
    /// The cache folder can't be written (read-only, no permission, a failing drive): trying again can't help until
    /// that is fixed. The message carries the OS's reason.
    CacheUnwritable,
    /// Anything else (a decode, GPU or check failure, an unreadable job).
    Other,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// Every segment is written and verified.
    Finished,
    Paused(PauseReason),
    /// The copy can't be made: why (`code`), in words (`message`); `missing` is `code == FailCode::Missing`.
    Failed {
        message: String,
        missing: bool,
        code: FailCode,
    },
}

impl FailCode {
    /// A missing source is tried again when it is back (or at the next listing of the project's files); the other
    /// failures wait for the user.
    pub fn retry(self) -> Retry {
        match self {
            Self::Missing => Retry::WhenPresent,
            Self::Changed | Self::CacheUnwritable | Self::Other => Retry::Never,
        }
    }
}

impl RunOutcome {
    /// A failure with `code`.
    pub fn failed(code: FailCode, message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            missing: code == FailCode::Missing,
            code,
        }
    }
}

/// Why one segment wasn't made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// A pause or stop arrived mid-segment: the partial segment is dropped.
    Stopped,
    /// The encoder couldn't start (or stopped) because it is busy.
    EncoderBusy,
    /// The disk filled up while writing.
    DiskFull,
    /// The source is gone (the message names it).
    Missing(String),
    /// The segment's file couldn't be written or checked for a reason other than space (read-only, no permission, a
    /// failing drive): the OS's reason.
    CacheUnwritable(String),
    /// Anything else: the copy fails with this message.
    Failed(String),
}

impl RenderError {
    /// A file error: a full disk, or a cache that can't be written (with the OS's reason).
    pub fn from_io(e: &std::io::Error) -> Self {
        if is_disk_full(e) {
            Self::DiskFull
        } else {
            Self::CacheUnwritable(e.to_string())
        }
    }
}

/// Renders and encodes one segment (the worker's GPU pipeline; a fake in tests). Returns the verified file's size
/// and checksum.
pub trait Segments {
    fn render(&mut self, job: &ProxyJob, index: u32) -> Result<(u64, u32), RenderError>;

    /// The CRC-32 of the parameter sets of the encoder that wrote the last segment, when known (segments join only
    /// with ones that share them).
    fn encoder_config(&self) -> Option<u32> {
        None
    }
}

/// Free bytes on the disk holding a folder (None: unknown, which doesn't pause), given the segments done so far.
pub type FreeSpace = Box<dyn Fn(&Path, usize) -> Option<u64> + Send>;

/// What the loop asks of the computer between segments.
pub struct ProxyEnv {
    pub thermal: Box<dyn Fn() -> Thermal + Send>,
    pub free_space: FreeSpace,
}

impl ProxyEnv {
    /// The real computer: `power::thermal` and `durable::free_space`.
    pub fn system() -> Self {
        Self {
            thermal: Box::new(klipolo_platform::power::thermal),
            free_space: Box::new(|path, _| klipolo_platform::durable::free_space(path)),
        }
    }

    /// A fixed thermal state and a free space that depends on the segments done (tests).
    pub fn fixed(thermal: Thermal, free_after: impl Fn(usize) -> u64 + Send + 'static) -> Self {
        Self {
            thermal: Box::new(move || thermal),
            free_space: Box::new(move |_, done| Some(free_after(done))),
        }
    }
}

fn file_name(path: &str) -> std::borrow::Cow<'_, str> {
    crate::log_name(path)
}

/// "intro.mp4 is missing" (the scheduler's sentence).
pub fn missing_message(source: &str) -> String {
    format!("{} is missing", file_name(source))
}

/// Why the cache can't take an editing copy, with the OS's `reason`.
pub fn unwritable_message(reason: &str) -> String {
    format!("Editing copies can't be saved on this disk: {reason}")
}

/// Records how a run ended in the job's state and message, and saves it when they changed (a save that fails changes
/// nothing: the job resumes as it was).
pub fn record_outcome(job: &mut ProxyJob, outcome: &RunOutcome) {
    let (state, message) = match outcome {
        RunOutcome::Finished => (ProxyState::Running, None),
        RunOutcome::Paused(reason) => (ProxyState::Paused, Some(format!("{reason:?}"))),
        RunOutcome::Failed { message, .. } => (ProxyState::Failed, Some(message.clone())),
    };
    if (job.state, &job.message) == (state, &message) {
        return;
    }
    (job.state, job.message) = (state, message);
    if let Err(e) = job.save() {
        tracing::warn!(%e, "editing copy: the job's state couldn't be saved");
    }
}

/// Why the job's source isn't the file it copies any more (gone, or edited or replaced), or None. Checked before every
/// segment and once more before the copy is delivered (a copy is only `Done` for the file as it is).
pub fn source_problem(job: &ProxyJob) -> Option<RunOutcome> {
    match job.source_state() {
        SourceState::Same => None,
        SourceState::Missing => Some(RunOutcome::failed(
            FailCode::Missing,
            missing_message(&job.source),
        )),
        SourceState::Changed => Some(RunOutcome::failed(
            FailCode::Changed,
            format!(
                "{} changed while its editing copy was being made.",
                file_name(&job.source)
            ),
        )),
    }
}

fn end(job: &mut ProxyJob, outcome: RunOutcome) -> RunOutcome {
    record_outcome(job, &outcome);
    outcome
}

/// Every pending segment in order, saving each to the manifest as it verifies. Between segments: a stop request, the
/// computer running hot (Serious or worse) or too little free space pauses with the reason; a missing or changed
/// source fails the job by name. Finished segments are never touched.
pub fn run_segments(
    job: &mut ProxyJob,
    env: &ProxyEnv,
    control: &Control,
    segments: &mut dyn Segments,
) -> RunOutcome {
    // A segment set back (another encoder's parameter sets) is rendered again; each pass over the job sets back at
    // most once per encoder change, so twice the segments bounds the loop.
    let mut budget = job.segments.len() * 2 + 1;
    while let Some(index) = job.next_pending() {
        if budget == 0 {
            let outcome = RunOutcome::failed(FailCode::Other, "The video encoder kept changing.");
            return end(job, outcome);
        }
        budget -= 1;
        if !control.running() {
            return end(job, RunOutcome::Paused(PauseReason::User));
        }
        if (env.thermal)() >= Thermal::Serious {
            return end(job, RunOutcome::Paused(PauseReason::Thermal));
        }
        let free = (env.free_space)(&job.dir, job.done_segments());
        if free.is_some_and(|free| free < FREE_FLOOR + job.expected_bytes(index)) {
            return end(job, RunOutcome::Paused(PauseReason::DiskFull));
        }
        if let Some(outcome) = source_problem(job) {
            return end(job, outcome);
        }
        if job.state != ProxyState::Running {
            job.state = ProxyState::Running;
            job.message = None;
        }
        match segments.render(job, index) {
            Ok(file) => {
                let set_back = job.segment_done(index, file, segments.encoder_config());
                if set_back > 0 {
                    tracing::warn!(
                        set_back,
                        "editing copy: segments from another encoder are made again"
                    );
                }
                if let Err(e) = job.save() {
                    // The segment isn't recorded: the next run makes it again.
                    let outcome = if is_disk_full(&e) {
                        RunOutcome::Paused(PauseReason::DiskFull)
                    } else {
                        let message = unwritable_message(&e.to_string());
                        RunOutcome::failed(FailCode::CacheUnwritable, message)
                    };
                    return outcome;
                }
            }
            Err(RenderError::Stopped) => return end(job, RunOutcome::Paused(PauseReason::User)),
            Err(RenderError::EncoderBusy) => {
                return end(job, RunOutcome::Paused(PauseReason::EncoderBusy));
            }
            Err(RenderError::DiskFull) => {
                return end(job, RunOutcome::Paused(PauseReason::DiskFull));
            }
            Err(RenderError::Missing(message)) => {
                return end(job, RunOutcome::failed(FailCode::Missing, message));
            }
            Err(RenderError::CacheUnwritable(reason)) => {
                let message = unwritable_message(&reason);
                return end(job, RunOutcome::failed(FailCode::CacheUnwritable, message));
            }
            Err(RenderError::Failed(message)) => {
                return end(job, RunOutcome::failed(FailCode::Other, message));
            }
        }
    }
    end(job, RunOutcome::Finished)
}
