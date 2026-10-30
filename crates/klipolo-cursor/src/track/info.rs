//! What a read is told (`Limits`) and what it tells (`HeaderInfo`, `ReadNotes`, `LastTimes`).

use super::CursorTrack;
use crate::format::{Capture, Line};
use crate::{ClickSource, Us};

/// How much of a file a track keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub max_bytes: usize,
    pub min_gap_us: Us,
}

impl Limits {
    /// The closest two kept positions are (the recorder writes at most 250 a second).
    pub const MIN_GAP_US: Us = 4_000;

    /// 16 MB; 4 ms between kept positions, widened in 4 ms steps for files whose length means more than about two
    /// hours of continuous motion (≈ 40 bytes a line), so any file fits deterministically.
    pub fn for_file_len(len: u64) -> Self {
        const LINE_BYTES: u64 = 40;
        const TWO_HOURS: u64 = 2 * 3_600 * 250;
        let steps = (len / LINE_BYTES).div_ceil(TWO_HOURS).max(1);
        Self {
            max_bytes: CursorTrack::MAX_BYTES,
            min_gap_us: Self::MIN_GAP_US.saturating_mul(Us::try_from(steps).unwrap_or(Us::MAX)),
        }
    }
}

/// The file's first line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderInfo {
    pub capture: Capture,
    pub clicks: ClickSource,
    /// The captured area in points; 0 × 0 when unknown (the header gave a size that isn't positive).
    pub area_pts: (f32, f32),
}

impl HeaderInfo {
    /// A header line's. An area that isn't positive is refused as unknown (0 × 0), but the header is kept: its capture
    /// and clicks say how the take is drawn, and dropping a separate take's header would draw no cursor over frames
    /// that have none.
    pub(super) fn of(line: &Line) -> Option<Self> {
        let Line::Header {
            capture,
            clicks,
            w,
            h,
            ..
        } = *line
        else {
            return None;
        };
        let area_pts = if area_ok(w, h) { (w, h) } else { (0.0, 0.0) };
        Some(Self {
            capture,
            clicks,
            area_pts,
        })
    }
}

/// An area's size in points is positive (an `a` line with another is refused; a header's is unknown).
pub(crate) fn area_ok(w: f32, h: f32) -> bool {
    w > 0.0 && h > 0.0
}

/// What reading a file passed over, for the caller to log once per take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadNotes {
    /// Lines older than their kind's last (`skipped`).
    pub skipped: u32,
    /// Header lines that differ from the first (skipped).
    pub other_headers: u32,
    /// Header lines seen, identical repeats included (a resumed take writes none again: the writer's test counts them).
    pub headers: u32,
    /// Lines with an impossible value: a time beyond `format::TIME_LIMIT_US`, an area whose size isn't positive, a
    /// shape whose height isn't positive or whose hotspot is outside its image. Each was refused; a header's area alone
    /// is (see `HeaderInfo::of`), apart from exactly 0 × 0, the recorder's "not known yet" before its picture started
    /// (an `a` line gives it later), which isn't counted.
    pub refused: u32,
    /// The file passed `max_bytes`: later positions (and, past it, other lines) weren't kept.
    pub limited: bool,
    /// Reading failed part-way; what was read is kept.
    pub read_failed: bool,
}

impl ReadNotes {
    /// Something in the file wasn't kept as written (`headers` is a count, not a loss).
    pub fn lost_any(&self) -> bool {
        self.skipped > 0
            || self.other_headers > 0
            || self.refused > 0
            || self.limited
            || self.read_failed
    }
}

/// Each kind's last time in the file, as read (a line skipped as out of order changes none): what a resumed writer
/// continues from, so it never writes a line older than its kind's last. None: no line of that kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LastTimes {
    pub moves: Option<Us>,
    /// The latest of every button's.
    pub buttons: Option<Us>,
    pub shapes: Option<Us>,
    pub visible: Option<Us>,
    pub areas: Option<Us>,
    pub clicks: Option<Us>,
    pub burned: Option<Us>,
}

impl CursorTrack {
    pub fn last_times(&self) -> LastTimes {
        self.last
    }
}
