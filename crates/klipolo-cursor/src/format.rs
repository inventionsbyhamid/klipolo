//! The cursor file, format 2: a screen take's `cursor.jsonl`, one JSON object per line tagged by `k`.
//! The recorder appends it; the engine, chat and the recorder's resume read it (`CursorTrack::read`).
//!
//! Times are µs on the recording clock, from each event's own timestamp. Positions are 0–1 across the captured
//! picture from its top left (outside it is outside 0–1). The file never holds key events or window titles.
//!
//! **Reading** (`decode`) parses a line into a borrowed flat record and checks it, so it allocates nothing per line
//! (only a shape definition's two strings). A line is refused (None) when it is torn, foreign, longer than `MAX_LINE`,
//! not finite, or a shape definition whose file isn't exactly `cursors/<digits>.png` or whose image is larger than
//! 256 × 256.
//!
//! **Format 1** (the first recorder builds: `{"t":…,"x":…,"y":…,"b":1}` once per frame, no `k`) is never a `Line`. The
//! track still reads it as positions and presses, so takes recorded before format 2 keep their cursor.

use serde::{Deserialize, Serialize};

use crate::{ClickSource, Us};

mod decode;

pub(crate) use decode::{Parsed, V1, parse};

pub const VERSION: u32 = 2;
/// A longer line isn't a cursor line (power-cut zeros, garbage): skipped without keeping it.
pub const MAX_LINE: usize = 512;
/// A line whose time is beyond ±this (2^50 µs, about 35 years) is refused, so no sum on a kept time can overflow
/// (a crafted file's times near `i64::MAX` would overflow the smoothing's).
pub const TIME_LIMIT_US: Us = 1 << 50;

/// How the take's screen file shows the cursor, as the recorder started it (the `x` line marks a later burn-in).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capture {
    Separate,
    Burned,
}

/// A system cursor shape the recorder recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemShape {
    Arrow,
    IBeam,
    VerticalIBeam,
    PointingHand,
    OpenHand,
    ClosedHand,
    Crosshair,
    NotAllowed,
    ResizeLeftRight,
    ResizeUpDown,
    ResizeNwse,
    ResizeNesw,
    Move,
    Wait,
    Progress,
    Help,
    ContextMenu,
    DragCopy,
    DragLink,
    ZoomIn,
    ZoomOut,
}

/// One line of the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Line {
    /// The first line; a resumed take writes none again, and a repeated identical one is accepted. `w`, `h`: the
    /// captured area in points.
    #[serde(rename = "h")]
    Header {
        v: u32,
        capture: Capture,
        clicks: ClickSource,
        w: f32,
        h: f32,
    },
    /// A position: on movement (at least 4 ms apart) and a heartbeat every 500 ms while still.
    #[serde(rename = "m")]
    Move { t: Us, x: f32, y: f32 },
    /// A button event at its own hardware time: `btn` 0 primary, 1 secondary, 2 middle; `d` 1 down, 0 up.
    #[serde(rename = "b")]
    Button {
        t: Us,
        btn: u8,
        d: u8,
        x: f32,
        y: f32,
    },
    /// A shape, before its first use. `key`: the recorder's hash of the image (16 hex digits), so a resumed take
    /// continues the store. `pw`, `ph`, `hx`, `hy` in pixels; `pts` its height in points.
    #[serde(rename = "S")]
    ShapeDef {
        id: u16,
        key: String,
        file: String,
        pw: u16,
        ph: u16,
        hx: f32,
        hy: f32,
        pts: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sys: Option<SystemShape>,
    },
    /// The shape changed.
    #[serde(rename = "s")]
    Shape { t: Us, id: u16 },
    /// The system hid or showed the cursor.
    #[serde(rename = "v")]
    Visible { t: Us, on: bool },
    /// The captured area's size in points changed (a resized window).
    #[serde(rename = "a")]
    Area { t: Us, w: f32, h: f32 },
    /// From `t`, clicks come from this source (a switch to sampling mid-take).
    #[serde(rename = "c")]
    Clicks { t: Us, clicks: ClickSource },
    /// From `t`, the frames show the system cursor (the recorder's fallback).
    #[serde(rename = "x")]
    Burned { t: Us },
}

/// One line without its newline; None for a torn, foreign, over-long or non-finite one, for a `ShapeDef` whose `file`
/// isn't `cursors/<digits>.png` or whose size is over 256 × 256 (a crafted file can't point elsewhere), and for an
/// impossible value: a time beyond `TIME_LIMIT_US`, an `Area` whose size isn't positive, a `ShapeDef` whose height isn't
/// positive or whose hotspot is outside its image.
pub fn parse_line(line: &[u8]) -> Option<Line> {
    match parse(line)? {
        Parsed::Line(line) => Some(line),
        Parsed::V1(_) | Parsed::Refused => None,
    }
}

/// `line` as JSON plus "\n", appended to `out`.
pub fn write_line(line: &Line, out: &mut Vec<u8>) {
    // Can't fail: numbers, plain strings and unit enums into a Vec.
    let _ = serde_json::to_writer(&mut *out, line);
    out.push(b'\n');
}
