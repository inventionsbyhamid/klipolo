//! Cursor events into `cursor.jsonl` format 2, inside the session core: pure besides the effects it
//! pushes. Every line goes through the reorder window; a gather (at least 100 ms apart) appends what has left it.
//!
//! - **Positions** are thinned as a read keeps them (`klipolo_cursor::Decimate`: at least 4 ms apart, plus where the
//!   cursor came to rest), with places to 5 decimals (finer than the reader's 16-bit grid). A beat is written only
//!   250 ms or more after the last position line. Before the first move after a still of 250 ms or more, the held
//!   position is written again 4 ms before it, so the read path doesn't drift across the still.
//! - **Buttons** at their own times; the ones still down are released at a burn-in and at Stop, so no press stays open
//!   to the end of the take.
//! - **Shapes**: a new one's PNG is written whole (`Effect::WriteAtomic`) before its `S` line, which isn't timed and
//!   goes straight out; its `s` line goes through the window. Past `MAX_SHAPES`, or for an image the file can't hold,
//!   an `s` line names no definition, so the shape draws as the Arrow look.
//! - **The area**: the header's, once known (0 × 0 before, then an `a` line: the reader takes that as unknown).
//!
//! Cost: O(1) per event, plus a gather's sort of the window. Memory: the window, the shape keys and one gather's bytes.

use klipolo_cursor::format::{Capture, Line, VERSION, write_line};
use klipolo_cursor::{ClickSource, Decimate, Limits};
use klipolo_model::time::Us;

use super::state::Kept;
use super::straight::Straight;
use super::{CursorEvent, Reorder, ShapeStore};
use crate::preflight::cursor::CursorPlan;
use crate::run::core::Effect;
use crate::session::CURSOR;

/// The shape id an `s` line names when the shape has no definition (the Arrow look).
pub(super) const NO_SHAPE: u16 = u16::MAX;

/// What the core tells the person about the cursor (once each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorNote {
    /// `capture.note.cursorSampled`: clicks are sampled from here.
    Sampled,
    /// `capture.note.cursorShapes`: the store is full: new shapes draw as the Arrow look, or (a source that stops
    /// sending them: `CursorEvent::ShapesFull`) the last one stays.
    ShapesFull,
}

/// See the module notes.
#[derive(Clone, Debug)]
pub struct CursorWriter {
    pub(super) reorder: Reorder,
    pub(super) out: Vec<u8>,
    pub(super) thin: Decimate,
    pub(super) straight: Straight,
    /// The newest position fed to `thin` (an older one is stale and dropped).
    pub(super) last_fed: Option<Us>,
    /// The last position line, and where the cursor was last seen (a button's place counts).
    pub(super) last_move: Option<(Us, f32, f32)>,
    pub(super) place: (f32, f32),
    pub(super) shapes: ShapeStore,
    pub(super) shapes_full: bool,
    pub(super) open: bool,
    /// The file has its header (a resumed one may not).
    pub(super) header: bool,
    pub(super) clicks: ClickSource,
    pub(super) area: Option<(f32, f32)>,
    /// Buttons down, as bits (0-31).
    pub(super) held: u32,
    /// What the source said while lines couldn't be written, and what the file shows (`state`).
    pub(super) kept: Kept,
    pub(super) gathered_at: Option<Us>,
    pub(super) bytes: u64,
}

impl Default for CursorWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorWriter {
    pub const GATHER_US: Us = 100_000;
    pub const MIN_MOVE_GAP_US: Us = Limits::MIN_GAP_US;
    /// A beat is written this long after the last position line, and a move this long after one is a still's end.
    pub const STILL_US: Us = 250_000;

    pub fn new() -> Self {
        Self {
            reorder: Reorder::new(),
            out: Vec::new(),
            thin: Decimate::new(Self::MIN_MOVE_GAP_US),
            straight: Straight::default(),
            last_fed: None,
            last_move: None,
            place: (0.5, 0.5),
            shapes: ShapeStore::new(),
            shapes_full: false,
            open: false,
            header: false,
            clicks: ClickSource::Sampled,
            area: None,
            held: 0,
            kept: Kept::default(),
            gathered_at: None,
            bytes: 0,
        }
    }

    /// The file opened to append to, its header first unless it has one. `area_pts` is used when no `Area` came yet;
    /// 0 × 0 means not known.
    pub fn open(
        &mut self,
        plan: CursorPlan,
        clicks: ClickSource,
        area_pts: (f32, f32),
        fx: &mut Vec<Effect>,
    ) {
        if self.open {
            return;
        }
        self.open = true;
        fx.push(Effect::OpenLines { key: CURSOR.into() });
        if !self.header {
            self.header = true;
            self.clicks = clicks;
            let (w, h) = self.area.unwrap_or(area_pts);
            let capture = match plan {
                CursorPlan::Separate => Capture::Separate,
                CursorPlan::Burned | CursorPlan::None => Capture::Burned,
            };
            let header = Line::Header {
                v: VERSION,
                capture,
                clicks,
                w,
                h,
            };
            write_line(&header, &mut self.out);
            self.area = (w > 0.0 && h > 0.0).then_some((w, h));
            self.append(fx);
        }
        self.define_early(fx);
    }

    /// `t`: the event's time on the session clock. None (before the gate, while paused): its timed lines are dropped,
    /// but what it says is kept (`state`: a shape is defined, the latest place, shape, visibility and area written at
    /// the gate and when the pause ends).
    pub fn event(
        &mut self,
        ev: &CursorEvent,
        t: Option<Us>,
        fx: &mut Vec<Effect>,
    ) -> Option<CursorNote> {
        let Some(t) = t.filter(|_| self.open) else {
            return self.hold(ev, fx);
        };
        let note = self.line(ev, t, fx);
        if self.gathered_at.is_none_or(|at| t - at >= Self::GATHER_US) {
            self.flush_moves();
            self.reorder.drain_ready(&mut self.out);
            self.append(fx);
            self.gathered_at = Some(t);
        }
        note
    }

    /// The frames show the system cursor from `t` (the `x` line).
    pub fn burned(&mut self, t: Us, fx: &mut Vec<Effect>) {
        if self.open {
            self.reorder.push(t, Line::Burned { t });
            self.flush(fx);
        }
    }

    /// The buttons still down are released at `t` (a pause, a burn-in, Stop): no press stays open after it.
    pub fn release(&mut self, t: Us) {
        let (x, y) = self.place;
        for btn in (0..32u8).filter(|b| self.held >> b & 1 == 1) {
            self.reorder.push(t, Line::Button { t, btn, d: 0, x, y });
        }
        self.held = 0;
    }

    /// Everything waiting, appended now (a pause, Stop, a burn-in): the window and a rest point passed over.
    pub fn flush(&mut self, fx: &mut Vec<Effect>) {
        if !self.open {
            return;
        }
        if let Some(rest) = self.thin.last() {
            self.put_move(rest);
        }
        self.flush_moves();
        self.reorder.drain_all(&mut self.out);
        self.append(fx);
    }

    /// The clock at `now` (session µs): a rest point due is kept, and what left the window is appended once a gather
    /// is due (the core's poll, ten times a second), or now (`gather`: the core's tick, just before the writer's flush
    /// tick, so a crash loses at most a tick and the window).
    pub fn tick(&mut self, now: Us, gather: bool, fx: &mut Vec<Effect>) {
        if !self.open {
            return;
        }
        if let Some(rest) = self.thin.rest_due(now) {
            self.put_move(rest);
        }
        self.reorder.advance(now);
        if gather
            || self
                .gathered_at
                .is_none_or(|at| now - at >= Self::GATHER_US)
        {
            self.flush_moves();
            self.reorder.drain_ready(&mut self.out);
            self.append(fx);
            self.gathered_at = Some(now);
        }
    }

    /// The file has its header (written, or a resumed file's own).
    pub fn has_header(&self) -> bool {
        self.header
    }

    /// The bytes written so far (the disk tests).
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Lines dropped for coming after a later line of their kind was written.
    pub fn late(&self) -> u64 {
        self.reorder.late()
    }

    fn append(&mut self, fx: &mut Vec<Effect>) {
        if self.out.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.out);
        self.bytes += bytes.len() as u64;
        fx.push(Effect::Append {
            key: CURSOR.into(),
            bytes,
            media_us: 0,
        });
    }
}

/// A size in points the file can hold: finite and positive.
pub(super) fn area_ok(w: f32, h: f32) -> bool {
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}
