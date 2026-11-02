//! What the core asks of the threads layer (`Effect`), what a pipe's start gives it (`Started`), and the runs Stop
//! finishes (`RunDone`).

use klipolo_model::time::Us;

use crate::fmp4::finalize::RemuxPlan;
use crate::journal::Entry;
use crate::media::{AudioFormat, SourceId, StreamInfo};
use crate::protocol::Event;
use crate::session::FileKey;
use crate::shell::ShellOutput;

/// What a pipe's `start` gave.
#[derive(Clone, Debug, PartialEq)]
pub enum Started {
    Video(StreamInfo),
    Audio(AudioFormat),
}

/// What the threads layer does for the core.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    StartPipes(Vec<SourceId>),
    StopPipes,
    Pipe {
        source: SourceId,
        cmd: PipeCmd,
    },
    /// Replace the mic's pipe with one on `device` (an id from the device list); its samples open a new run.
    SwapMic {
        source: SourceId,
        device: String,
    },
    /// A run file, new only, starting with `head`.
    Open {
        key: FileKey,
        head: Vec<u8>,
    },
    /// A line file (`timing.jsonl`, `cursor.jsonl`) to append to, made if missing.
    OpenLines {
        key: FileKey,
    },
    Append {
        key: FileKey,
        bytes: Vec<u8>,
        media_us: Us,
    },
    Close {
        key: FileKey,
    },
    /// A small file written whole: a temp file, synced, renamed, on the writer's thread in order with the rest (a cursor
    /// shape's PNG before its `S` line).
    WriteAtomic {
        key: FileKey,
        bytes: Vec<u8>,
    },
    Journal(Entry),
    /// The writer's flush tick.
    Tick,
    Emit(Event),
    Shell(ShellOutput),
    /// Run `readback::check` once, off the core's thread, and hand its result to `read_back`.
    ReadBack,
    /// Stop asked: finish the writer and the files (`run::finish`), then call `finished`.
    Finalize,
    /// The worker ends with this code (`protocol`'s exit codes).
    Exit(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeCmd {
    Pause,
    Resume,
    Keyframe,
    /// Stop and start the same pipe again; its next sample opens a new run.
    Restart,
    /// Stop the pipe and start it again one preset lighter (`VideoPipe::lower_preset`); its next sample opens a new run.
    Lower,
    Stop,
    /// From now on the frames show the system cursor (`VideoPipe::set_cursor_in_frames`); the run controller answers
    /// with the core's `cursor_in_frames`.
    CursorInFrames,
}

/// A run of the take as Stop finishes it (`run::finish`).
#[derive(Clone, Debug, PartialEq)]
pub struct RunDone {
    pub source: SourceId,
    pub run: u32,
    pub key: FileKey,
    pub kind: RunKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunKind {
    /// The run's own remux plan and length, good when the writer wrote it exactly (`WriterSnapshot::wrote_exactly`);
    /// otherwise Stop walks the file.
    Video {
        info: StreamInfo,
        plan: Option<RemuxPlan>,
        bytes_written: u64,
        duration_us: Us,
    },
    Sound {
        format: AudioFormat,
    },
}
