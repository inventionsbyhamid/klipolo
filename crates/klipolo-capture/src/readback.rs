//! The read-back, 10 s into a take: what reached the disk is read back once, off the capture path, so a
//! take that is being written wrong (a bad encoder output, a filesystem that drops writes) is caught while the person
//! can still act. The take goes on either way; the core gives one critical `capture.health.readBack` note.
//!
//! - **Each video run** (the journal's `RunStarted`): the file's head reads and describes the journal's stream; its
//!   first complete fragment starts on a keyframe; and that keyframe decodes, where the backend can say
//!   (`Backend::decodes`). A run with no complete fragment yet (started a moment ago) isn't judged.
//! - **Each mic run**: a CAF of ours, with at least a second of frames that aren't exactly zero. That one is `silent`:
//!   the core leaves it out when the mic rules already warned. Game and computer sound are only checked to be a CAF of
//!   ours: silence is normal there.
//!
//! Cost: one walk per video file (headers only) and one read of the first sample; the mic's file so far (about 1.4 MB
//! at 10 s) in 64 KiB pieces. Runs once per take, on a utility thread.

use std::fs::File;
use std::io;

use klipolo_model::project::SourceRole;
use klipolo_model::time::Us;
use klipolo_platform::file::PositionedIo;

use crate::backend::Backend;
use crate::caf;
use crate::fmp4::read;
use crate::journal::{self, Entry};
use crate::media::{SourceId, StreamInfo};
use crate::session::{self, FileKey, SessionDir};

/// The session time the read-back runs at.
pub const AFTER_US: Us = 10_000_000;
/// The sound a mic run must hold by then: a second of frames.
const SOUND_US: u64 = 1_000_000;
const PIECE: u64 = 64 * 1024;

/// A run file that didn't read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub source: SourceId,
    pub file: FileKey,
    /// For the log: "head", "keyframe", "decode", "sound" or "unreadable".
    pub what: &'static str,
    /// A mic run with less than a second of sound: the mic rules may already have said so.
    pub silent: bool,
}

/// Reads back every run the journal names so far.
pub fn check(dir: &SessionDir, backend: &dyn Backend) -> Vec<Problem> {
    let (Ok(cfg), Ok(journal)) = (session::read_config(dir), journal::read(&dir.journal())) else {
        tracing::warn!("read-back: the take's config or journal didn't read");
        return vec![];
    };
    let mut problems = Vec::new();
    for entry in &journal.entries {
        let Entry::RunStarted {
            source,
            file,
            video,
            ..
        } = entry
        else {
            continue;
        };
        let role = cfg.sources.iter().find(|s| &s.id == source).map(|s| s.role);
        let found = match dir.key_path(file).and_then(File::open) {
            Err(e) => Err(e),
            Ok(f) if file.ends_with(".mp4") => video_problem(&f, video.as_ref(), backend),
            Ok(f) => sound_problem(&f, role == Some(SourceRole::Mic)),
        };
        let what = match found {
            Ok(None) => continue,
            Ok(Some(what)) => what,
            Err(e) => {
                tracing::warn!(file, error = %e, "read-back: a run's file didn't read");
                "unreadable"
            }
        };
        tracing::error!(file, what, "read-back: a run's file isn't as written");
        problems.push(Problem {
            source: source.clone(),
            file: file.clone(),
            what,
            silent: what == "sound",
        });
    }
    problems
}

fn video_problem(
    file: &File,
    info: Option<&StreamInfo>,
    backend: &dyn Backend,
) -> io::Result<Option<&'static str>> {
    let walk = read::walk(&mut &*file, file.metadata()?.len())?;
    let Some(head) = &walk.head else {
        return Ok(Some("head"));
    };
    if info.is_some_and(|info| head.info != *info) {
        return Ok(Some("head"));
    }
    let Some(first) = walk.index.fragments.first() else {
        return Ok(None);
    };
    let Some(sample) = first.samples.first() else {
        return Ok(Some("keyframe"));
    };
    if !sample.key {
        return Ok(Some("keyframe"));
    }
    let mut bytes = vec![0u8; sample.size as usize];
    file.read_exact_at(&mut bytes, first.data_at)?;
    Ok((backend.decodes(&head.info, &bytes) == Some(false)).then_some("decode"))
}

fn sound_problem(file: &File, mic: bool) -> io::Result<Option<&'static str>> {
    let Ok(scan) = caf::scan(&mut &*file, file.metadata()?.len()) else {
        return Ok(Some("head"));
    };
    if !mic {
        return Ok(None);
    }
    let bpf = scan.frame_bytes();
    let wanted = u64::from(scan.format.rate) * SOUND_US / 1_000_000;
    let mut buf = vec![0u8; ((PIECE / bpf).max(1) * bpf) as usize];
    let (mut at, mut sounding) = (0, 0);
    while at < scan.frames && sounding < wanted {
        let n = (buf.len() as u64 / bpf).min(scan.frames - at);
        let piece = &mut buf[..(n * bpf) as usize];
        file.read_exact_at(piece, scan.data_at + at * bpf)?;
        sounding += piece
            .chunks_exact(bpf as usize)
            .filter(|f| f.iter().any(|&b| b != 0))
            .count() as u64;
        at += n;
    }
    Ok((sounding < wanted).then_some("sound"))
}
