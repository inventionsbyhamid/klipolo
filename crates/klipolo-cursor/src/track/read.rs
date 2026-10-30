//! A cursor file read into a track: its lines through one reused buffer (`lines`), each line parsed and checked (`format::parse`),
//! then kept by its kind's rules.
//! - **Positions** are decimated (`decimate`).
//! - **Buttons** are paired into presses (`presses`).
//! - **Format-1 lines** become a position and, where their held bits change, button events.
//! - **The budget:** positions stop at 15/16 of `max_bytes`, so the other lines (presses above all) keep room; they
//!   stop at `max_bytes`.
//!
//! Cost: O(lines), O(1) work and no allocation per line (a shape definition's strings aside), then a sort of the
//! presses. The buffers double as they grow, so a full 16 MB track peaks near twice that while it is read; it is
//! trimmed to fit at the end.

use std::io::BufRead;

use super::decimate::{Decimate, Sample};
use super::lines::each_line;
use super::order::{Kind, Order};
use super::presses::{PRESS_BYTES, Pairing, order_and_spans};
use super::{CursorTrack, HeaderInfo, Limits, TICK_US, area_ok, fix};
use crate::format::{self, Line, Parsed, V1};
use crate::shapes::{MAX_SHAPES, ShapeDef};
use crate::{Press, Us};

/// A position's cost: its tick and its place.
const POSITION_BYTES: usize = size_of::<u32>() + size_of::<[u16; 2]>();
/// A shape span, a hidden span, an area or a click-mode change.
const SPAN_BYTES: usize = 16;

pub(super) fn read(r: impl BufRead, limits: Limits) -> CursorTrack {
    let mut b = Builder {
        track: CursorTrack::default(),
        limits,
        order: Order::default(),
        used: 0,
        positions: Decimate::new(limits.min_gap_us),
        pairing: Pairing::default(),
        v1_held: 0,
    };
    let read = each_line(r, |line| {
        if let Some(parsed) = format::parse(line) {
            b.add(parsed);
        }
    });
    b.track.last = b.order.last_times();
    b.track.notes.read_failed = read.is_err();
    b.finish()
}

struct Builder {
    track: CursorTrack,
    limits: Limits,
    order: Order,
    /// The bytes kept so far.
    used: usize,
    positions: Decimate,
    pairing: Pairing,
    /// A format-1 file's buttons held, as bits.
    v1_held: u8,
}

impl Builder {
    fn add(&mut self, parsed: Parsed) {
        match parsed {
            Parsed::Line(line) => self.line(line),
            Parsed::V1(sample) => self.v1(sample),
            Parsed::Refused => {
                self.track.notes.refused = self.track.notes.refused.saturating_add(1)
            }
        }
    }

    /// A format-1 line: its position, then an event for each held bit that changed (the first line counts as after a
    /// release, as format 1 was always read).
    fn v1(&mut self, V1 { t, x, y, b }: V1) {
        self.line(Line::Move { t, x, y });
        let changed = b ^ self.v1_held;
        self.v1_held = b;
        for btn in (0..8u8).filter(|i| changed >> i & 1 == 1) {
            let d = b >> btn & 1;
            self.line(Line::Button { t, btn, d, x, y });
        }
    }

    fn line(&mut self, line: Line) {
        if let Some((kind, t)) = Kind::of(&line) {
            if !self.order.admit(kind, t) {
                self.track.notes.skipped = self.track.notes.skipped.saturating_add(1);
                return;
            }
            // A span's room is taken first, so each arm below is one step (a repeated shape, or a visibility that
            // changes nothing, takes it too: never more than the file holds).
            if kind.is_span() && !self.room(SPAN_BYTES) {
                return;
            }
        }
        match line {
            Line::Header { w, h, .. } => {
                let notes = &mut self.track.notes;
                notes.headers = notes.headers.saturating_add(1);
                // Exactly 0 × 0 is the recorder's "not known yet" (an `a` line follows): unknown, not refused.
                if !area_ok(w, h) && (w, h) != (0.0, 0.0) {
                    notes.refused = notes.refused.saturating_add(1);
                }
                if let Some(info) = HeaderInfo::of(&line) {
                    self.header(info);
                }
            }
            Line::ShapeDef { .. } => {
                if let Some(def) = ShapeDef::from_line(line) {
                    self.def(def);
                }
            }
            Line::Move { t, x, y } => self.position(t, x, y),
            Line::Button { t, btn, d, x, y } => {
                if let Some(press) = self.pairing.event(t, btn, d == 1, x, y) {
                    self.press(press);
                }
            }
            Line::Shape { t, id } => {
                if self.track.shapes.last().is_none_or(|s| s.1 != id) {
                    self.track.shapes.push((t, id));
                }
            }
            Line::Visible { t, on } => self.visible(t, on),
            Line::Area { t, w, h } => self.track.areas.push((t, w, h)),
            Line::Clicks { t, clicks } => self.track.clicks_from.push((t, clicks)),
            Line::Burned { t } => {
                self.track.burned_from.get_or_insert(t);
            }
        }
    }

    /// Whether `cost` more bytes fit the budget (taken if so).
    fn room(&mut self, cost: usize) -> bool {
        let fits = self.used + cost <= self.limits.max_bytes;
        if fits {
            self.used += cost;
        } else {
            self.track.notes.limited = true;
        }
        fits
    }

    fn header(&mut self, info: HeaderInfo) {
        match self.track.header {
            None => self.track.header = Some(info),
            Some(first) if first == info => {}
            Some(_) => {
                let notes = &mut self.track.notes;
                notes.other_headers = notes.other_headers.saturating_add(1);
            }
        }
    }

    fn def(&mut self, def: ShapeDef) {
        let defs = &self.track.defs;
        let new = defs.len() < MAX_SHAPES && !defs.iter().any(|d| d.id == def.id);
        if new && self.room(def.bytes()) {
            self.track.defs.push(def);
        }
    }

    fn position(&mut self, t: Us, x: f32, y: f32) {
        for kept in self.positions.next((t, x, y)).into_iter().flatten() {
            self.keep(kept);
        }
    }

    /// Keeps a position. One in the same 100 µs tick as the last kept replaces it, so kept times strictly increase (no
    /// two samples at one time: a consumer may divide by their gap). Kept positions come in time order.
    fn keep(&mut self, (t, x, y): Sample) {
        if self.track.t.is_empty() {
            self.track.t0 = t;
        }
        let tick = u32::try_from(t.saturating_sub(self.track.t0) / TICK_US);
        if let (Ok(tick), Some(&last), Some(xy)) =
            (tick, self.track.t.last(), self.track.xy.last_mut())
            && tick == last
        {
            *xy = [fix(x), fix(y)];
            return;
        }
        let positions_max = self.limits.max_bytes - self.limits.max_bytes / 16;
        let (Ok(tick), true) = (tick, self.used + POSITION_BYTES <= positions_max) else {
            self.track.notes.limited = true;
            return;
        };
        self.used += POSITION_BYTES;
        self.track.t.push(tick);
        self.track.xy.push([fix(x), fix(y)]);
    }

    fn press(&mut self, press: Press) {
        if self.room(PRESS_BYTES) {
            self.track.presses.push(press);
        }
    }

    fn visible(&mut self, t: Us, on: bool) {
        let open = self.track.hidden.last().is_some_and(|h| h.1 == Us::MAX);
        if open && on {
            if let Some(last) = self.track.hidden.last_mut() {
                last.1 = t;
            }
        } else if !open && !on {
            self.track.hidden.push((t, Us::MAX));
        }
    }

    fn finish(mut self) -> CursorTrack {
        if let Some(last) = self.positions.last() {
            self.keep(last);
        }
        for press in std::mem::take(&mut self.pairing).still_held() {
            self.press(press);
        }
        let mut track = self.track;
        (track.primary, track.held) = order_and_spans(&mut track.presses);
        track.defs.sort_by_key(|d| d.id);
        track.trim();
        track
    }
}
