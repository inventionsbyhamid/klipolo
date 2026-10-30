//! The bounded cursor track: a take's cursor file read once into compact, time-sorted arrays, so every
//! lookup is a binary search (O(log n), no allocation).
//!
//! **Memory:** a position is 8 bytes: its time as 100 µs ticks from the first (`u32`, about 119 hours) and its place
//! as 16-bit fixed point over −0.5–1.5 (2 / 65,535 of the picture: 0.03 px at 1080p). A press is 32 bytes plus its
//! held spans (16 each); a shape span, a hidden span, an area and a click-mode change 16 each. An hour of continuous
//! motion at 250 Hz is 7.2 MB; a typical hour (moving about a quarter of the time) about 2 MB. `MAX_BYTES` caps a track.
//!
//! **Reading** (`read`) streams lines through one reused buffer (see `read.rs`).

use std::io::BufRead;

use crate::format::Capture;
use crate::press::Press;
use crate::shapes::ShapeDef;
use crate::{ClickSource, Us};

mod decimate;
mod info;
mod lines;
mod memory;
mod order;
mod presses;
mod read;
mod walk;
mod window;

pub use decimate::{Decimate, Sample};
pub(crate) use info::area_ok;
pub use info::{HeaderInfo, LastTimes, Limits, ReadNotes};
pub(crate) use walk::Walk;

/// A position's time step.
pub(crate) const TICK_US: Us = 100;
/// The fixed-point range of a position: −0.5 to 1.5 of the picture.
const LO: f64 = -0.5;
const SPAN: f64 = 2.0;
const FIX: f64 = u16::MAX as f64;
/// One fixed-point step (2 / 65,535 of the picture): a multiply, as positions are read per step and per frame. The
/// precision the reader keeps (to the nearest: half a step), so the recorder thins its moves to it.
pub const QUANTUM: f64 = SPAN / FIX;

/// A place as 16-bit fixed point over −0.5–1.5, to the nearest (the smoothed path stores its points the same way).
/// Clamped first, so adding a half and truncating rounds: `round` is a library call on baseline x86-64.
pub(crate) fn fix(v: f32) -> u16 {
    (((f64::from(v) - LO) / SPAN * FIX).clamp(0.0, FIX) + 0.5) as u16
}

pub(crate) fn unfix(q: f64) -> f64 {
    q * QUANTUM + LO
}

/// One take's cursor (see the module notes).
#[derive(Debug, Default)]
pub struct CursorTrack {
    /// The first kept position's time: tick 0.
    t0: Us,
    t: Vec<u32>,
    xy: Vec<[u16; 2]>,
    /// By down time.
    presses: Vec<Press>,
    /// The primary button's held spans `[down, up)`, and every button's merged: sorted and disjoint.
    primary: Vec<(Us, Us)>,
    held: Vec<(Us, Us)>,
    /// From each time, the shape with that id.
    shapes: Vec<(Us, u16)>,
    /// By id.
    defs: Vec<ShapeDef>,
    /// Spans `[from, to)` an app hid the cursor in, sorted and disjoint.
    hidden: Vec<(Us, Us)>,
    areas: Vec<(Us, f32, f32)>,
    clicks_from: Vec<(Us, ClickSource)>,
    header: Option<HeaderInfo>,
    burned_from: Option<Us>,
    notes: ReadNotes,
    last: LastTimes,
}

/// Whether `t` is inside one of `spans` (sorted, disjoint, `[from, to)`).
fn in_spans(spans: &[(Us, Us)], t: Us) -> bool {
    let i = spans.partition_point(|s| s.0 <= t);
    i > 0 && t < spans[i - 1].1
}

/// The value of the last of `items` (sorted by time) at or before `t`.
fn last_at<T>(items: &[(Us, T)], t: Us) -> Option<&T> {
    let i = items.partition_point(|i| i.0 <= t);
    i.checked_sub(1).map(|i| &items[i].1)
}

impl CursorTrack {
    pub const MAX_BYTES: usize = 16 << 20;

    /// Streams `r`'s lines (one reused buffer, no allocation per line). Positions are kept `min_gap_us` apart and
    /// every other line is kept, until `max_bytes` would be passed (`notes`). Order is checked per kind.
    pub fn read(r: impl BufRead, limits: Limits) -> Self {
        read::read(r, limits)
    }

    pub fn header(&self) -> Option<HeaderInfo> {
        self.header
    }

    pub(crate) fn time(&self, tick: u32) -> Us {
        self.t0 + Us::from(tick) * TICK_US
    }

    /// The kept samples as stored: their ticks (`time` gives each one's µs) and 16-bit places, so a scan can compare
    /// places without converting each (the facts' still runs).
    pub(crate) fn stored(&self) -> (&[u32], &[[u16; 2]]) {
        (&self.t, &self.xy)
    }

    /// The `a` lines' sizes, by time (`area_at` reads them).
    pub(crate) fn area_changes(&self) -> &[(Us, f32, f32)] {
        &self.areas
    }

    /// Where an `x` line says the frames show the cursor from (the recorder's fallback), if one does.
    pub(crate) fn burned_from(&self) -> Option<Us> {
        self.burned_from
    }

    /// The spans an app hid the cursor in (`visible_at`), sorted and disjoint; `Us::MAX` ends one still open.
    pub(crate) fn hidden(&self) -> &[(Us, Us)] {
        &self.hidden
    }

    fn place(&self, i: usize) -> (f64, f64) {
        let [x, y] = self.xy[i];
        (unfix(f64::from(x)), unfix(f64::from(y)))
    }

    /// The raw position: linear between kept samples, held after the last, None before the first. O(log n).
    pub fn position(&self, t: Us) -> Option<(f32, f32)> {
        self.between(self.after(t), t)
    }

    /// Kept samples in [from, to), their times strictly increasing (a later position in the same 100 µs tick replaces
    /// the earlier: `read`).
    pub fn samples(&self, from: Us, to: Us) -> impl Iterator<Item = (Us, f32, f32)> + '_ {
        let a = self.t.partition_point(|&k| self.time(k) < from);
        let b = self.t.partition_point(|&k| self.time(k) < to).max(a);
        (a..b).map(|i| {
            let (x, y) = self.place(i);
            (self.time(self.t[i]), x as f32, y as f32)
        })
    }

    pub fn presses(&self) -> &[Press] {
        &self.presses
    }

    /// Primary-button presses.
    pub fn clicks(&self) -> impl Iterator<Item = &Press> + '_ {
        self.presses.iter().filter(|p| p.btn == 0)
    }

    /// The primary button is down at `t`. O(log n).
    pub fn held_at(&self, t: Us) -> bool {
        in_spans(&self.primary, t)
    }

    /// The primary button's last press that went down at or before `t`, `[down, up)` (`up` is `Us::MAX` while it is
    /// still held at the file's end): the cursor layer's press squish. O(log n).
    pub fn primary_press_at(&self, t: Us) -> Option<(Us, Us)> {
        let i = self.primary.partition_point(|s| s.0 <= t);
        i.checked_sub(1).map(|i| self.primary[i])
    }

    /// Any button is down at `t` (the drag rule). O(log n).
    pub fn any_held_at(&self, t: Us) -> bool {
        in_spans(&self.held, t)
    }

    pub fn shape_at(&self, t: Us) -> Option<&ShapeDef> {
        let id = *last_at(&self.shapes, t)?;
        let i = self.defs.binary_search_by_key(&id, |d| d.id).ok()?;
        Some(&self.defs[i])
    }

    pub fn shapes(&self) -> &[ShapeDef] {
        &self.defs
    }

    /// The ids of the shapes shown over `[from, to]`: the one at `from`, then each change up to `to`, in order (an id
    /// may repeat). What an export preloads. O(log n + changes).
    pub fn shape_ids_in(&self, from: Us, to: Us) -> impl Iterator<Item = u16> + '_ {
        let start = self
            .shapes
            .partition_point(|s| s.0 <= from)
            .saturating_sub(1);
        let end = self.shapes.partition_point(|s| s.0 <= to);
        self.shapes[start..end.max(start)].iter().map(|s| s.1)
    }

    /// Not hidden by an app at `t`.
    pub fn visible_at(&self, t: Us) -> bool {
        !in_spans(&self.hidden, t)
    }

    /// The captured area's size in points at `t`: the last `a` line's, else the header's. 0 × 0 means unknown (no
    /// header, as in format 1, or one whose size wasn't positive).
    pub fn area_at(&self, t: Us) -> (f32, f32) {
        let i = self.areas.partition_point(|a| a.0 <= t);
        match i.checked_sub(1) {
            Some(i) => (self.areas[i].1, self.areas[i].2),
            None => self.header.map_or((0.0, 0.0), |h| h.area_pts),
        }
    }

    /// The take's clicks as it started: the header's; a file without one is format 1, sampled once per frame.
    fn clicks_first(&self) -> ClickSource {
        self.header.map_or(ClickSource::Sampled, |h| h.clicks)
    }

    /// The click source from `t` on; the weakest over the take is `clicks_weakest`.
    pub fn clicks_at(&self, t: Us) -> ClickSource {
        last_at(&self.clicks_from, t)
            .copied()
            .unwrap_or_else(|| self.clicks_first())
    }

    pub fn clicks_weakest(&self) -> ClickSource {
        let later = self.clicks_from.iter().map(|c| c.1);
        later.fold(self.clicks_first(), Ord::max)
    }

    /// The frames don't show the cursor at `t` (the header says separate and `t` is before any `x` line).
    pub fn separate_at(&self, t: Us) -> bool {
        self.header.is_some_and(|h| h.capture == Capture::Separate)
            && self.burned_from.is_none_or(|from| t < from)
    }

    /// Positions kept.
    pub fn len(&self) -> usize {
        self.t.len()
    }

    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    /// Lines skipped for being out of order within their kind (the caller logs `notes` once per take).
    pub fn skipped(&self) -> u32 {
        self.notes.skipped
    }

    pub fn notes(&self) -> ReadNotes {
        self.notes
    }

    /// The first and last kept positions' times.
    pub fn span(&self) -> Option<(Us, Us)> {
        Some((self.time(*self.t.first()?), self.time(*self.t.last()?)))
    }
}
