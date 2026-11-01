//! The journal (`journal.jsonl`): the take's append-only source of truth. After a crash it says which
//! runs each source opened and ended, how much of each file reached the disk, where the pauses and markers were, and
//! what went wrong. The manifest is derived from it; recovery and a resumed recorder read it.
//!
//! One JSON object per line, tagged `t`, camelCase. A power cut can tear the last line, which `read` ignores; an entry
//! appended after that goes through `open_append`, which ends the torn line first, so the new entry isn't glued to it
//! and lost. The audio anchors go to `timing.jsonl` in the same way (`anchor_line`, `read_anchors`).
//!
//! The audio anchors' `timing.jsonl` is `timing`'s.
//!
//! Cost: one line is one small allocation. `read` streams the file a line at a time; a 3-hour take's journal (a
//! `Durable` line a second) is about 2 MB, and its entries about 3 MB in memory while recovery runs.

mod timing;

pub use timing::{anchor_line, read_anchors};

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use klipolo_cursor::ClickSource;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::project::MarkerKind;
use klipolo_model::time::Us;
use klipolo_platform::file::PositionedIo;
use serde::{Deserialize, Serialize};

use crate::clock::SessionClock;
use crate::media::{AudioFormat, SourceId, StreamInfo};
use crate::session::FileKey;

/// One journal line. Times named `*_us` are session µs (the start gate is 0, pauses left out); `host_ns` is the host
/// clock (`klipolo_platform::clock::host_ns`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Entry {
    /// The recorder began, before the countdown.
    Started {
        host_ns: u64,
    },
    /// The start gate opened: session time 0.
    Gate {
        host_ns: u64,
    },
    /// A source's run opened its file. `offset_us` is its first sample's session time. A video run keeps its stream
    /// (the codec config, as hex), so recovery can rebuild a damaged head; a sound run keeps its format.
    RunStarted {
        source: SourceId,
        run: u32,
        file: FileKey,
        offset_us: Us,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        video: Option<StreamInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<AudioFormat>,
    },
    /// A run ended: a heal restarted its source, a resumed recorder salvaged it, or the take stopped.
    RunEnded {
        source: SourceId,
        run: u32,
        end_us: Us,
    },
    /// A sound run's loudest sample on each channel (linear, 0-1), journaled when the run ends (or by recovery and a
    /// resumed recorder for a run a crash ended). A two-input interface with one mic leaves its second channel near
    /// silent for the whole run, and import plays such a run centred.
    ///
    /// `through`: a running level, journaled with each flush tick while the run records, covering the file's first
    /// `through` bytes; recovery and a resumed recorder then read only what came after it.
    /// None: the whole run.
    Levels {
        file: FileKey,
        peaks: Vec<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        through: Option<u64>,
    },
    /// How many bytes of each file are on the disk, after a flush tick. Recovery trusts media only up to here. Stop
    /// journals the final lengths after its last sync and before the remux writes its tail, so that bound never cuts
    /// what Stop made durable.
    Durable {
        through: BTreeMap<FileKey, u64>,
    },
    /// Session time stopped. `host_ns` too, so a resumed recorder rebuilds the clock with the real pause length
    /// (`JournalRead::clock`): `at_us` is the same at a pause and its resume.
    Pause {
        at_us: Us,
        host_ns: u64,
    },
    Resume {
        at_us: Us,
        host_ns: u64,
    },
    Marker {
        id: String,
        at_us: Us,
        kind: MarkerKind,
    },
    /// A health note: its code and its message's values (`Entry::health`). The critical ones become the manifest's
    /// notes. A gap between two runs is derived from the runs themselves, so a gap journaled here isn't critical.
    Health {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SourceId>,
        critical: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        params: BTreeMap<String, ErrorParam>,
    },
    StopRequested {
        at_us: Us,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The soft remux of `file`: started (its tail written) or done (every flip made).
    Remux {
        file: FileKey,
        done: bool,
    },
    /// The take is complete: every run ended, remuxed or sized, and the manifest written.
    Finalized {
        duration_us: Us,
    },
    /// Recovery repaired the take, at ms since 1970.
    Recovered {
        at_ms: u64,
    },
    /// The app is importing the take into `project`: written, and pushed to the disk, before the project changes, so a
    /// crash or a failed `Imported` never makes another project on the next Open.
    Importing {
        project: String,
    },
    Imported {
        project: String,
    },
    Heartbeat {
        at_us: Us,
        queued_ms: u32,
    },
    /// The watchdog decided to burn the cursor into the frames.
    CursorBurnDecided {
        at_us: Us,
    },
    /// The frames show the cursor from here: the stream confirmed it, or the healed run's first frame came.
    CursorBurned {
        at_us: Us,
    },
    /// The cursor's clicks come from `clicks` from here (the take's start, then a switch to sampling).
    CursorClicks {
        at_us: Us,
        clicks: ClickSource,
    },
}

impl Entry {
    /// A health entry carrying `note` whole.
    pub fn health(note: &CodedError, source: Option<SourceId>, critical: bool) -> Self {
        Self::Health {
            code: note.code.clone(),
            source,
            critical,
            params: note.params.clone(),
        }
    }
}

/// `entry` as one journal line: JSON and a newline, written with one `write` so a crash tears at most this line.
pub fn line(entry: &Entry) -> Vec<u8> {
    // Can't fail: every map key is a string, and serde_json writes a non-finite number as null.
    let mut out = serde_json::to_vec(entry).expect("a journal entry serialises");
    out.push(b'\n');
    out
}

/// A journal as read back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JournalRead {
    pub entries: Vec<Entry>,
    /// The last line was cut short (no newline, and it doesn't parse): the crash came mid-write. Normal after a crash.
    pub torn: bool,
    /// Other lines that didn't parse; each is logged.
    pub skipped: usize,
}

impl JournalRead {
    /// How far each file is known to be on the disk: the most any `Durable` entry says.
    pub fn durable_through(&self) -> BTreeMap<FileKey, u64> {
        let mut out = BTreeMap::new();
        for entry in &self.entries {
            if let Entry::Durable { through } = entry {
                for (key, &len) in through {
                    let known = out.entry(key.clone()).or_insert(len);
                    *known = (*known).max(len);
                }
            }
        }
        out
    }

    /// The session clock as the recorder had it: started at `Gate`, paused and resumed at the entries' host times. A
    /// trailing `Pause` stays paused. The host clock must not have restarted since (a resume never crosses a reboot or
    /// an app relaunch).
    pub fn clock(&self) -> SessionClock {
        let mut clock = SessionClock::new();
        for entry in &self.entries {
            match *entry {
                Entry::Gate { host_ns } => clock.start(host_ns),
                Entry::Pause { host_ns, .. } => clock.pause(host_ns),
                Entry::Resume { host_ns, .. } => clock.resume(host_ns),
                _ => {}
            }
        }
        clock
    }
}

/// Reads a journal a line at a time. A last line without a newline that doesn't parse sets `torn`; any other line
/// that doesn't parse is counted in `skipped` and logged. Empty lines are ignored. NotFound without a journal.
pub fn read(path: &Path) -> io::Result<JournalRead> {
    let mut out = JournalRead::default();
    for_each_line(path, |n, line, whole| match serde_json::from_slice(line) {
        Ok(entry) => out.entries.push(entry),
        Err(_) if !whole => out.torn = true,
        Err(e) => {
            out.skipped += 1;
            tracing::warn!(line = n, error = %e, "journal: skipped a line that doesn't parse");
        }
    })?;
    Ok(out)
}

/// Opens a journal (or `timing.jsonl`) to append to, creating it. When a crash left its last line without a newline,
/// a newline ends that line first: the next entry then starts its own line, and the torn one is skipped on reading.
pub fn open_append(path: &Path) -> io::Result<File> {
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)?;
    let len = file.metadata()?.len();
    if len > 0 {
        let mut last = [0_u8];
        file.read_exact_at(&mut last, len - 1)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    Ok(file)
}

/// Calls `each(line number, line without its newline and surrounding blanks, whether it ended in a newline)` for every
/// non-empty line, streaming the file.
fn for_each_line(path: &Path, mut each: impl FnMut(usize, &[u8], bool)) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buf = Vec::new();
    let mut n = 0;
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            return Ok(());
        }
        n += 1;
        let line = buf.trim_ascii();
        if !line.is_empty() {
            each(n, line, buf.ends_with(b"\n"));
        }
    }
}
