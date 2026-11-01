//! A sound run's file in recovery: every whole frame, less a power cut's zeros past the durable length, sized.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::Path;

use klipolo_platform::file::PositionedIo;
use klipolo_platform::paths::tilde;

use super::damage::holds_data;
use super::{RunOutcome, RunRepair};
use crate::caf::{self, HEADER_LEN};
use crate::io::Io;
use crate::media::AudioFormat;

/// Repairs the sound run at `path`. Steps:
/// 1. **Scan** it. A header of all zeros (written, but the power went before it reached the disk) is rebuilt from the
///    run's `format` (the journal's `RunStarted.audio`): `caf::header` is the bytes our writer wrote. Any other header
///    that isn't ours leaves the file alone.
/// 2. **Keep** every whole frame, except a run of all-zero frames at the end past the durable length `durable`
///    (`caf::keep_frames`): silence before that mark is real; zeros after it are pages that never landed. Non-zero
///    sound past the mark reached the disk, so it is kept.
/// 3. **Size** it (`caf::finalize`: cuts only those zeros and a torn last frame, never pads), unless it already is.
///
/// The duration is the frames over the rate (never FFmpeg's estimate). `Empty`: no file, or no frame that
/// isn't zeros. `LeftOut` (logged; the manifest notes it): a header that isn't ours or can't be rebuilt, or `finalize`
/// refused; the file is left as it was. Idempotent. Every write and sync goes through `io`; no journal line vouches for
/// a sound run (its size is written after the frames are pushed, `caf::finalize`).
pub fn repair_sound(
    io: &dyn Io,
    path: &Path,
    format: Option<AudioFormat>,
    durable: Option<u64>,
) -> io::Result<RunOutcome> {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::warn!(file = %tilde(&path.to_string_lossy()), "recovery: a run's file is missing");
            return Ok(RunOutcome::Empty);
        }
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let mut changed = false;
    let scan = match caf::scan(&mut &file, len) {
        Ok(scan) => scan,
        Err(e) if matches!(e.kind(), ErrorKind::InvalidData | ErrorKind::UnexpectedEof) => {
            let mut header = [0u8; HEADER_LEN as usize];
            let zero_header = len > HEADER_LEN && {
                file.read_exact_at(&mut header, 0)?;
                header.iter().all(|&b| b == 0)
            };
            let Some(format) = format.filter(|_| zero_header) else {
                tracing::warn!(file = %tilde(&path.to_string_lossy()), error = %e, "recovery: a sound run that isn't ours or can't be rebuilt");
                return Ok(if holds_data(&file, 0, len)? {
                    RunOutcome::LeftOut
                } else {
                    RunOutcome::Empty
                });
            };
            io.write_at(&file, path, 0, &caf::header(format))?;
            changed = true;
            caf::scan(&mut &file, len)?
        }
        Err(e) => return Err(e),
    };
    let durable_frames = scan.frames_within(durable.unwrap_or(0));
    let keep = caf::keep_frames(&mut &file, &scan, durable_frames)?;
    if keep == 0 {
        return Ok(RunOutcome::Empty);
    }
    let sized =
        scan.sized && keep == scan.frames && len == scan.data_at + keep * scan.frame_bytes();
    if !sized {
        match caf::finalize(io, &file, path, &scan, keep) {
            Ok(()) => changed = true,
            Err(e) if e.kind() == ErrorKind::InvalidData => {
                tracing::error!(file = %tilde(&path.to_string_lossy()), error = %e, "recovery: finalize refused a sound run");
                return Ok(RunOutcome::LeftOut);
            }
            Err(e) => return Err(e),
        }
    }
    let duration_us = (u128::from(keep) * 1_000_000 / u128::from(scan.format.rate)) as i64;
    Ok(RunOutcome::Repaired(RunRepair {
        duration_us,
        damaged_bytes: 0,
        changed,
    }))
}

/// How much `channel_peaks` reads at a time.
const PEAK_STEP_BYTES: u64 = 64 * 1024;

/// The loudest sample on each channel over frames `from..to`, linear 0-1: for `Entry::Levels`, a crashed run's frames
/// after its last running level (the live recorder measures its own as it writes). Reads them once, in 64 KiB pieces of
/// whole frames; `step(bytes read)` after each piece stops the read (`Interrupted`) when it returns false.
pub(crate) fn channel_peaks(
    file: &File,
    scan: &caf::CafScan,
    (from, to): (u64, u64),
    step: &mut dyn FnMut(u64) -> bool,
) -> io::Result<Vec<f32>> {
    let channels = usize::from(scan.format.channels.max(1));
    let bpf = scan.frame_bytes();
    let mut peaks = vec![0u32; channels];
    let to = to.min(scan.frames);
    let per_read = (PEAK_STEP_BYTES / bpf).max(1);
    let mut buf = vec![0u8; (per_read * bpf) as usize];
    let mut at = from.min(to);
    while at < to {
        let n = per_read.min(to - at);
        let piece = &mut buf[..(n * bpf) as usize];
        file.read_exact_at(piece, scan.data_at + at * bpf)?;
        for frame in piece.chunks_exact(bpf as usize) {
            for (peak, s) in peaks.iter_mut().zip(frame.chunks_exact(3)) {
                let v = i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8;
                *peak = (*peak).max(v.unsigned_abs());
            }
        }
        at += n;
        if !step((at - from) * bpf) {
            return Err(io::Error::new(
                ErrorKind::Interrupted,
                "the levels read was cancelled",
            ));
        }
    }
    Ok(peaks.iter().map(|&p| p as f32 / 8_388_608.0).collect())
}
