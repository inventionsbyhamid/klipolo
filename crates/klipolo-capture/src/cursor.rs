//! The cursor as its own layer: what a screen take records of the cursor, besides its
//! frames, and the seam each OS's source implements.
//!
//! - **The seam** (`CursorSource`, `CursorEvent`): a source sends mouse positions, button events at their own hardware
//!   times, shapes as they change, the captured area's size and a heartbeat while still, all stamped on the host clock.
//!   Where the video pipe itself knows the cursor (portal metadata, DDA), it sends the same events on
//!   `VideoOut::cursor`. Never keys, key codes, modifiers, text or window titles.
//! - **The writer** (`writer`, `order`, `shapes`): events into `cursor.jsonl` format 2 (`klipolo_cursor::format`),
//!   thinned as a read would keep them, held 50 ms and appended sorted, so a button the hardware stamped before a later
//!   heartbeat still lands in order; each shape once, as `cursors/<id>.png`, written whole before its `S` line.
//! - **The watchdog** (`watch`): a silent or failed source burns the cursor into the frames rather than leave a video
//!   without one.
//! - **Resume** (`resume`): a crashed take's writer rebuilt from its file, so a resumed take writes no second header
//!   and continues its shape ids.
//!
//! Everything here is pure (no threads, no IO): the session core owns it and turns it into effects, so it is tested on
//! the synchronous harness with a fake clock.

mod lines;
mod order;
mod resume;
mod shapes;
mod state;
mod straight;
mod watch;
mod writer;

use std::sync::mpsc::SyncSender;

use klipolo_cursor::{ClickSource, SystemShape};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use order::Reorder;
pub use resume::resumed_writer;
pub use shapes::{ShapeStore, encode_png};
pub use watch::{BurnReason, Watch, WatchAction};
pub use writer::{CursorNote, CursorWriter};

use crate::pipe::PipeError;

/// What a cursor source sends. Times are the events' own, on the host clock; places are 0-1 across the
/// captured area from its top left (outside it is outside 0-1).
#[derive(Clone, Debug, PartialEq)]
pub enum CursorEvent {
    Move {
        host_ns: u64,
        x: f32,
        y: f32,
    },
    Button {
        host_ns: u64,
        btn: u8,
        down: bool,
        x: f32,
        y: f32,
    },
    /// A shape this source already sent, by its key.
    Shape {
        host_ns: u64,
        key: u64,
    },
    NewShape {
        host_ns: u64,
        key: u64,
        image: ShapeImage,
    },
    Visible {
        host_ns: u64,
        on: bool,
    },
    Area {
        host_ns: u64,
        w_pts: f32,
        h_pts: f32,
    },
    /// Every 500 ms while nothing else came (every source, pipe-sourced ones too): alive, and the cursor is here.
    Beat {
        host_ns: u64,
        x: f32,
        y: f32,
    },
    /// From here clicks are sampled, not events (the monitor stopped delivering buttons).
    ClicksSampled {
        host_ns: u64,
    },
    /// The source has sent `klipolo_cursor::MAX_SHAPES` shapes and sends no new one (an animated cursor's frames): the
    /// cursor keeps its last shape, and the take is told once.
    ShapesFull {
        host_ns: u64,
    },
    /// The source can't go on (its thread failed, the OS stopped it).
    Failed,
}

impl CursorEvent {
    /// The event's own time; None for `Failed`.
    pub fn host_ns(&self) -> Option<u64> {
        match *self {
            Self::Move { host_ns, .. }
            | Self::Button { host_ns, .. }
            | Self::Shape { host_ns, .. }
            | Self::NewShape { host_ns, .. }
            | Self::Visible { host_ns, .. }
            | Self::Area { host_ns, .. }
            | Self::Beat { host_ns, .. }
            | Self::ClicksSampled { host_ns }
            | Self::ShapesFull { host_ns } => Some(host_ns),
            Self::Failed => None,
        }
    }
}

/// A cursor shape's image as the source read it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeImage {
    /// At most 256 × 256 (the source scales a larger one down).
    pub px: (u16, u16),
    /// RGBA8, straight alpha, `px.0 × px.1 × 4` bytes.
    pub rgba: Vec<u8>,
    pub hotspot_px: (f32, f32),
    /// Its height in points on the screen it was read on.
    pub pts: f32,
    pub sys: Option<SystemShape>,
}

/// A screen's cursor where it doesn't come with the frames (macOS, Windows' WGC). Its constructor asks the consent guard
/// (`guard::assert_real_capture_allowed("cursor")`), and it starts only inside a take the person started. It sends a
/// shape's image only for a key it hasn't sent before, and none past `klipolo_cursor::MAX_SHAPES` (the queue is counted
/// in events, and an image can be 256 KB).
pub trait CursorSource: Send {
    /// Starts sending (after Record only). Never blocks: a full `out` drops events, counted by the source.
    fn start(&mut self, out: SyncSender<CursorEvent>) -> Result<(), PipeError>;
    fn set_paused(&mut self, paused: bool);
    /// Idempotent.
    fn stop(&mut self);
    fn clicks(&self) -> ClickSource;
}

/// The events a source may have in flight before it drops.
pub const CURSOR_QUEUE: usize = 4096;

/// What this computer can do with a screen's cursor (`CaptureCaps::cursor`): preflight takes a separate cursor only when
/// it can hide it, follow it and read its shapes (`preflight::cursor::cursor_plan`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorCaps {
    /// The frames can leave the system cursor out (and put it back mid-take).
    pub hide: bool,
    pub positions: bool,
    pub shapes: bool,
    pub clicks: ClickSource,
}

impl CursorCaps {
    /// Nothing: the cursor stays in the frames, with no positions or clicks.
    pub fn none() -> Self {
        Self {
            hide: false,
            positions: false,
            shapes: false,
            clicks: ClickSource::None,
        }
    }
}
