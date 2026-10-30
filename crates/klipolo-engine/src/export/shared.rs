//! What the worker's threads share: its errors, the stop control (pause, parent gone) and the job's manifest, which
//! the worker alone writes while it runs (saved after every finished part). Also the file helpers both sides use.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use std::path::Path;

use klipolo_platform::durable::{self, DiskReason, Durability};

use super::job::{ExportJob, PartStatus};
use super::protocol::ErrorCode;
use super::segment::SegmentLedger;

#[derive(Clone, Debug)]
pub struct WorkerError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

pub(crate) fn internal(message: impl ToString) -> WorkerError {
    WorkerError {
        code: ErrorCode::Internal,
        message: message.to_string(),
        retryable: true,
    }
}

pub(crate) fn disk_full() -> WorkerError {
    WorkerError {
        code: ErrorCode::DiskFull,
        message: "disk full".into(),
        retryable: false,
    }
}

/// A file error: a full disk pauses the job (not retryable); anything else is retried.
pub(crate) fn io_error(e: std::io::Error, what: &str) -> WorkerError {
    if is_disk_full(&e) {
        return disk_full();
    }
    WorkerError {
        code: ErrorCode::Io,
        message: format!("{what}: {e}"),
        retryable: true,
    }
}

/// An FFmpeg error while encoding or writing (ENOSPC from a packet, a trailer or a faststart rewrite is a full
/// disk).
pub(crate) fn ff_error(e: ffmpeg::Error, what: &str) -> WorkerError {
    match e {
        ffmpeg::Error::Other { errno } if durable::classify_errno(errno) == DiskReason::Full => {
            disk_full()
        }
        e => WorkerError {
            code: ErrorCode::Encode,
            message: format!("{what}: {e}"),
            retryable: true,
        },
    }
}

/// An error creating, writing or closing an output file through FFmpeg (a segment, a joined file): a full disk pauses;
/// anything else is an I/O error (`Io`). Never `Encode`: a read-only or failing volume isn't the encoder being busy.
pub(crate) fn file_error(e: ffmpeg::Error, what: &str) -> WorkerError {
    match e {
        ffmpeg::Error::Other { errno } if durable::classify_errno(errno) == DiskReason::Full => {
            disk_full()
        }
        e => WorkerError {
            code: ErrorCode::Io,
            message: format!("{what}: {e}"),
            retryable: true,
        },
    }
}

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const STOP: u8 = 2;

/// How the worker should stop, if at all: `pause` (asked by the parent: finish the current segment when less than
/// half of it is left, else drop it) or `stop` (the parent is gone: stop now). Either way finished segments are kept.
#[derive(Default)]
pub struct Control(AtomicU8);

impl Control {
    pub fn pause(&self) {
        let _ = self
            .0
            .compare_exchange(RUN, PAUSE, Ordering::SeqCst, Ordering::SeqCst);
    }

    pub fn stop(&self) {
        self.0.store(STOP, Ordering::SeqCst);
    }

    pub fn running(&self) -> bool {
        self.0.load(Ordering::SeqCst) == RUN
    }

    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst) == STOP
    }
}

/// The job behind a lock; every change is saved before it counts.
#[derive(Clone)]
pub struct JobHandle {
    job: Arc<Mutex<ExportJob>>,
    /// Frames of finished segments set back to Pending since the progress last looked (`take_set_back`).
    set_back: Arc<AtomicU64>,
}

impl JobHandle {
    pub fn new(job: ExportJob) -> Self {
        Self {
            job: Arc::new(Mutex::new(job)),
            set_back: Arc::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ExportJob> {
        self.job.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn read<T>(&self, f: impl FnOnce(&ExportJob) -> T) -> T {
        f(&self.lock())
    }

    /// Changes the manifest and saves it (atomically). A failed save undoes nothing in memory, but the change
    /// doesn't count: the caller fails.
    pub fn update(&self, f: impl FnOnce(&mut ExportJob)) -> Result<(), WorkerError> {
        let mut job = self.lock();
        f(&mut job);
        job.save()
            .map_err(|e| io_error(e, "the export's progress couldn't be saved"))
    }

    /// Records a written, fsynced and verified segment, and the encoder that wrote it (`config`, `software`).
    ///
    /// Finished segments of this format from another encoder can't share a file with it: now (never sooner) that
    /// this encoder has produced verified output, they are marked for rendering again. A run that fails before this
    /// point leaves them finished. Returns their frames (each segment's own length: the last one is shorter), which
    /// the progress takes off at once (`take_set_back`).
    pub fn segment_done(
        &self,
        output: usize,
        index: u32,
        (bytes, checksum): (u64, u32),
        (config, software): (u32, bool),
    ) -> Result<u64, WorkerError> {
        let (mut redone, mut frames) = (0u32, 0u64);
        self.update(|job| {
            for s in job.outputs[output].segments.iter_mut() {
                if s.index == index {
                    s.bytes = bytes;
                    s.checksum = checksum;
                    s.status = PartStatus::Done;
                    s.config = Some(config);
                    s.software = software;
                } else if s.status == PartStatus::Done && s.config != Some(config) {
                    s.status = PartStatus::Pending;
                    redone += 1;
                    frames += s.frames;
                }
            }
        })?;
        if redone > 0 {
            self.set_back.fetch_add(frames, Ordering::SeqCst);
            tracing::warn!(
                output,
                redone,
                "segments from another encoder are rendered again"
            );
        }
        Ok(frames)
    }

    /// Frames set back to Pending by `segment_done` since the last call.
    pub(super) fn take_set_back(&self) -> u64 {
        self.set_back.swap(0, Ordering::SeqCst)
    }

    pub fn segments_done(&self) -> u32 {
        self.read(ExportJob::segments_done)
    }
}

/// The export's manifest records each format's segments (`segment_done` above).
impl SegmentLedger for JobHandle {
    fn segment_path(&self, output: usize, index: u32) -> std::path::PathBuf {
        self.read(|j| {
            let file = j.outputs[output]
                .segments
                .iter()
                .find(|s| s.index == index)
                .map_or_else(String::new, |s| s.file.clone());
            j.path(&file)
        })
    }

    fn segment_done(
        &self,
        output: usize,
        index: u32,
        file: (u64, u32),
        encoder: (u32, bool),
    ) -> Result<u64, WorkerError> {
        JobHandle::segment_done(self, output, index, file, encoder)
    }
}

/// CRC-32 of a whole file (read in 1 MiB blocks; just-written files come from the page cache).
pub fn checksum(path: &std::path::Path) -> std::io::Result<u32> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buf[..n]);
    }
}

/// Writes `bytes` to `path` so a crash leaves the old file or the new one, never half: a temp file in the same
/// folder, an ordered sync (`F_BARRIERFSYNC` on macOS: after every earlier write, segments included), then a rename
/// (`klipolo_platform::durable`).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    durable::write_atomic(path, bytes, Durability::Ordered)
}

/// True for the errors a full disk gives (ENOSPC; a quota or a file-size limit stops the same way).
pub fn is_disk_full(e: &std::io::Error) -> bool {
    durable::classify(e) == DiskReason::Full
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_error_is_never_the_encoders() {
        let full = file_error(
            ffmpeg::Error::Other {
                errno: libc::ENOSPC,
            },
            "writing a segment",
        );
        assert_eq!(full.code, ErrorCode::DiskFull);
        for errno in [libc::EROFS, libc::EACCES, libc::EIO] {
            let e = file_error(ffmpeg::Error::Other { errno }, "writing a segment");
            assert_eq!(e.code, ErrorCode::Io, "errno {errno}");
        }
    }
}

/// A frame rate as a fraction: whole for export (30/1), the source's exact rate for an editing copy (24000/1001), so
/// copy frame n is source frame n at the same time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    pub const fn whole(fps: u32) -> Self {
        Self { num: fps, den: 1 }
    }

    pub fn f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den.max(1))
    }

    /// Frames per second as FFmpeg's rational.
    pub fn rational(self) -> ffmpeg::Rational {
        ffmpeg::Rational::new(self.num as i32, self.den.max(1) as i32)
    }

    /// One frame's length: the encoder's and the segments' time base (frame n has pts n).
    pub fn frame_base(self) -> ffmpeg::Rational {
        ffmpeg::Rational::new(self.den.max(1) as i32, self.num.max(1) as i32)
    }

    /// Seconds `frames` frames last.
    pub fn seconds(self, frames: u64) -> f64 {
        frames as f64 * f64::from(self.den.max(1)) / f64::from(self.num.max(1))
    }

    /// Frame `n`'s start in µs, rounded to the nearest (as the timeline's frame starts are).
    pub fn frame_start_us(self, n: u64) -> i64 {
        let (num, den) = (u128::from(self.num.max(1)), u128::from(self.den.max(1)));
        ((u128::from(n) * 1_000_000 * den + num / 2) / num) as i64
    }

    /// Frames of a `duration_us` video (a partial last frame counts; 0 for an unknown length).
    pub fn frames_in(self, duration_us: i64) -> u64 {
        let (num, den) = (u128::from(self.num.max(1)), u128::from(self.den.max(1)));
        let d = u128::try_from(duration_us.max(0)).unwrap_or(0);
        ((d * num).div_ceil(1_000_000 * den)) as u64
    }
}
