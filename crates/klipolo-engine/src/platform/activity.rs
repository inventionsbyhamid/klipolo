//! The activity pass's decoder seam: a screen file's picture at every 100 ms tick, as 128 × 72
//! luma for `klipolo_cursor::activity::Differ`. FFmpeg on every platform for now (`ffmpeg`: hardware decode where the
//! platform has one, the Y plane only, a fast bilinear scale).
//!
//! **Which picture a tick gets:** the one on screen then, the latest frame at or before the tick (the first frame for a
//! tick before it). A screen recording may hold a still picture for seconds without a frame (a variable frame rate),
//! and the next frame after the tick would date a change that much early. Ticks run from `from_us` while they are
//! before the file's end; the last frame stays on screen until then. Times are on the video stream's own clock: tick 0
//! is its first frame, as every decoder numbers frames (the takes' activity is mapped on it).
//!
//! **Stopping:** a decoder checks `cancel` between packets, so a stop never waits for more than one packet's decode.

// `pub(crate)`: the small copy for clip analysis (`media::small_copy`) decodes on the same hardware path.
pub(crate) mod ffmpeg;

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use klipolo_cursor::activity::FRAME;
use klipolo_model::time::Us;

/// One picture: 128 × 72 luma, row by row.
pub const LUMA_BYTES: usize = FRAME.0 * FRAME.1;

/// A screen file's pictures, tick by tick.
pub trait ActivityFrames: Send {
    /// The picture on screen at the next 100 ms tick, as 128 × 72 luma, and the tick's time (None past the end).
    fn next(&mut self, out: &mut [u8; LUMA_BYTES]) -> Result<Option<Us>, ActivityError>;
    /// The video's own length on its clock (its stream's, else the container's end less where the video starts): the
    /// activity track's length.
    fn duration_us(&self) -> Us;
}

#[derive(Debug)]
pub enum ActivityError {
    /// What the file is can't be analysed, for good: no video stream, no length, no decoder for its codec, a container
    /// FFmpeg rejects, no picture at time 0. It is marked so (`ActivityStore::fail`).
    Unsupported(String),
    /// It can't be read now, for a reason that may pass: any OS error opening it (a drive gone, access not granted
    /// yet, out of handles), or the file isn't there. Never marked; tried again.
    Unavailable(String),
    /// Reading or decoding failed part-way (a drive gone, damaged data): the chunks written are kept.
    Decode(String),
    /// Stopped by its `cancel`.
    Cancelled,
    /// The activity file's disk is full: nothing more is written (an export pauses, as it would on its own files).
    DiskFull,
}

impl fmt::Display for ActivityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(e) => write!(f, "The screen can't be analysed: {e}"),
            Self::Unavailable(e) => write!(f, "The screen can't be read now: {e}"),
            Self::Decode(e) => write!(f, "The screen couldn't be read: {e}"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::DiskFull => f.write_str("The disk is full"),
        }
    }
}

/// How the activity store opens a file's pictures (`open_frames`; tests pass a synthetic screen).
pub type Opener = Arc<
    dyn Fn(&Path, Us, &Arc<AtomicBool>) -> Result<Box<dyn ActivityFrames>, ActivityError>
        + Send
        + Sync,
>;

/// The platform's decoder for `path`, its first picture at tick `from_us`; `cancel` stops it between packets.
pub fn open_frames(
    path: &Path,
    from_us: Us,
    cancel: &Arc<AtomicBool>,
) -> Result<Box<dyn ActivityFrames>, ActivityError> {
    Ok(Box::new(ffmpeg::FfmpegFrames::open(
        path,
        from_us,
        cancel.clone(),
    )?))
}

/// `open_frames`, as the store's opener.
pub fn platform_opener() -> Opener {
    Arc::new(open_frames)
}
