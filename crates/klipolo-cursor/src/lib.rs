//! Klipolo's cursor layer, pure: the cursor file a screen take records (format 2), the
//! bounded track it is read into, presses, shapes, the smoothed path drawn from it, the take's facts (still runs,
//! dwells, idle stretches), their one mapping onto the timeline, the plain-words summary, and the screen's activity
//! track (where the picture changed: typing, playing media, quiet), and the smart camera and the vertical follow-crop
//! planned from them. It does no IO beyond reading an `impl BufRead` or bytes, starts no threads and names no OS, so
//! all of it runs in tests anywhere. The engine, the recorder, chat and the app depend on it, so they read a take's
//! cursor the same way.

pub mod activity;
pub mod camera;
pub mod ease;
pub mod facts;
pub mod follow;
pub mod format;
pub mod press;
pub mod rect;
pub mod shapes;
pub mod smooth;
pub mod summary;
pub mod timeline;
pub mod track;
pub mod zooms;

pub use format::{Capture, Line, SystemShape};
pub use klipolo_model::cursor_style::ClickSource;
pub use press::Press;
pub use rect::{Rect01, RegionSpan};
pub use shapes::{MAX_SHAPES, ShapeDef};
pub use track::{CursorTrack, Decimate, HeaderInfo, LastTimes, Limits, ReadNotes};

/// Microseconds, on the recording clock (the session clock, pauses left out).
pub type Us = klipolo_model::time::Us;
