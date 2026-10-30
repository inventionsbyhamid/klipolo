//! The cursor's shapes: each image the recorder saw is stored once as `cursors/<n>.png` in the take's
//! folder and defined by an `S` line before its first use.

use crate::format::{Line, SystemShape};

/// The most shapes a take keeps (the recorder's store holds no more; later definitions are ignored).
pub const MAX_SHAPES: usize = 64;
/// The largest shape image, in pixels a side.
pub const MAX_SHAPE_PX: u16 = 256;

/// A shape's definition.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeDef {
    pub id: u16,
    /// The recorder's hash of the image, so a resumed take continues the store.
    pub key: u64,
    /// Its image, relative to the take's folder: exactly `cursors/<1–5 digits>.png` (checked as it is read).
    pub file: String,
    /// The image's size in pixels (each at most `MAX_SHAPE_PX`).
    pub px: (u16, u16),
    /// The hotspot, in the image's pixels.
    pub hotspot_px: (f32, f32),
    /// The shape's height in points on the screen it was recorded on.
    pub pts: f32,
    /// The system shape it is, when the recorder recognised it.
    pub sys: Option<SystemShape>,
}

impl ShapeDef {
    /// The memory it holds, its file name included.
    pub(crate) fn bytes(&self) -> usize {
        size_of::<Self>() + self.file.capacity()
    }
}

/// A key as the file writes it: exactly 16 hex digits.
pub(crate) fn parse_key(key: &str) -> Option<u64> {
    (key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(key, 16).ok())
        .flatten()
}

/// A shape's file is exactly `cursors/<1–5 digits>.png`, so a crafted file can't point anywhere else.
pub(crate) fn file_ok(file: &str) -> bool {
    file.strip_prefix("cursors/")
        .and_then(|f| f.strip_suffix(".png"))
        .is_some_and(|n| (1..=5).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A shape's size: 1 to `MAX_SHAPE_PX` pixels a side.
pub(crate) fn size_ok(w: u16, h: u16) -> bool {
    (1..=MAX_SHAPE_PX).contains(&w) && (1..=MAX_SHAPE_PX).contains(&h)
}

/// A hotspot is inside its image (its edges included), in pixels.
pub(crate) fn hotspot_ok((x, y): (f32, f32), (w, h): (u16, u16)) -> bool {
    (0.0..=f32::from(w)).contains(&x) && (0.0..=f32::from(h)).contains(&y)
}

impl ShapeDef {
    /// An `S` line's shape (`format::parse` checked it).
    pub(crate) fn from_line(line: Line) -> Option<Self> {
        let Line::ShapeDef {
            id,
            key,
            file,
            pw,
            ph,
            hx,
            hy,
            pts,
            sys,
        } = line
        else {
            return None;
        };
        let key = parse_key(&key)?;
        Some(Self {
            id,
            key,
            file,
            px: (pw, ph),
            hotspot_px: (hx, hy),
            pts,
            sys,
        })
    }
}
