//! One event into its lines (`CursorWriter::event`'s rules: the module notes of `writer`).

use klipolo_cursor::ClickSource;
use klipolo_cursor::format::Line;
use klipolo_model::time::Us;

use super::writer::{NO_SHAPE, area_ok};
use super::{CursorEvent, CursorNote, CursorWriter};
use crate::run::core::Effect;

impl CursorWriter {
    pub(super) fn line(
        &mut self,
        ev: &CursorEvent,
        t: Us,
        fx: &mut Vec<Effect>,
    ) -> Option<CursorNote> {
        match *ev {
            CursorEvent::Move { x, y, .. } => self.position(t, x, y, false),
            CursorEvent::Beat { x, y, .. } => self.position(t, x, y, true),
            CursorEvent::Button {
                btn, down, x, y, ..
            } => {
                let (x, y) = (place(x)?, place(y)?);
                self.place = (x, y);
                // A timed place is newer than any held one.
                self.kept.place = None;
                if btn < 32 {
                    self.held = if down {
                        self.held | 1 << btn
                    } else {
                        self.held & !(1 << btn)
                    };
                }
                let d = u8::from(down);
                self.reorder.push(t, Line::Button { t, btn, d, x, y });
            }
            CursorEvent::Shape { key, .. } => {
                let id = self.shapes.id_of(key).unwrap_or(NO_SHAPE);
                self.kept.shape = Some(key);
                self.show_shape(t, id);
            }
            CursorEvent::NewShape { key, ref image, .. } => {
                let id = self.define(key, image, fx);
                self.kept.shape = Some(key);
                self.show_shape(t, id.unwrap_or(NO_SHAPE));
                if id.is_none() && self.shapes.is_full() && !self.shapes_full {
                    self.shapes_full = true;
                    return Some(CursorNote::ShapesFull);
                }
            }
            CursorEvent::Visible { on, .. } => {
                self.kept.visible = Some(on);
                self.show_visible(t, on);
            }
            CursorEvent::Area { w_pts, h_pts, .. } => {
                if area_ok(w_pts, h_pts) && self.area != Some((w_pts, h_pts)) {
                    self.area = Some((w_pts, h_pts));
                    let (w, h) = (w_pts, h_pts);
                    self.reorder.push(t, Line::Area { t, w, h });
                }
            }
            CursorEvent::ClicksSampled { .. } if self.clicks == ClickSource::Events => {
                self.clicks = ClickSource::Sampled;
                let clicks = ClickSource::Sampled;
                self.reorder.push(t, Line::Clicks { t, clicks });
                return Some(CursorNote::Sampled);
            }
            CursorEvent::ShapesFull { .. } => return self.shapes_full(),
            CursorEvent::ClicksSampled { .. } | CursorEvent::Failed => {}
        }
        None
    }

    /// The source's shapes are full (it sends no new one, the last stays): `CursorNote::ShapesFull`, once.
    pub(super) fn shapes_full(&mut self) -> Option<CursorNote> {
        (!std::mem::replace(&mut self.shapes_full, true)).then_some(CursorNote::ShapesFull)
    }

    /// A move or a beat, thinned (see the module notes). A position older than the newest fed is stale: dropped. A
    /// timed one ends any place held from before the gate or a pause, so a later pause's end can't bring that back
    /// (`restate` takes the held place before it writes it here).
    pub(super) fn position(&mut self, t: Us, x: f32, y: f32, beat: bool) {
        let (Some(x), Some(y)) = (place(x), place(y)) else {
            return;
        };
        if self.last_fed.is_some_and(|last| t < last) {
            return;
        }
        self.last_fed = Some(t);
        self.kept.place = None;
        self.place = (x, y);
        if beat {
            if let Some(rest) = self.thin.rest_due(t) {
                self.put_move(rest);
            }
            if self.last_move.is_some_and(|m| t - m.0 < Self::STILL_US) {
                return;
            }
        }
        for kept in self.thin.next((t, x, y)).into_iter().flatten() {
            self.put_move(kept);
        }
    }

    /// A kept position into the straight-line thinning (`straight`), which hands the reorder window what it writes.
    pub(super) fn put_move(&mut self, (t, x, y): (Us, f32, f32)) {
        let reorder = &mut self.reorder;
        let mut write = |(t, x, y): (Us, f32, f32)| reorder.push(t, Line::Move { t, x, y });
        if let Some((lt, lx, ly)) = self.last_move
            && t - lt >= Self::STILL_US
            && (lx, ly) != (x, y)
        {
            self.straight
                .push((t - Self::MIN_MOVE_GAP_US, lx, ly), &mut write);
        }
        self.last_move = Some((t, x, y));
        self.straight.push((t, x, y), &mut write);
    }

    /// The thinning's newest point into the window (every gather, tick and flush: nothing waits longer).
    pub(super) fn flush_moves(&mut self) {
        let reorder = &mut self.reorder;
        self.straight
            .flush(&mut |(t, x, y)| reorder.push(t, Line::Move { t, x, y }));
    }
}

/// A place as written: finite, within ±10 of the picture, to 5 decimals (finer than the reader's 2 / 65,535); None for
/// a non-finite one.
fn place(v: f32) -> Option<f32> {
    v.is_finite()
        .then(|| ((f64::from(v.clamp(-10.0, 10.0)) * 1e5).round() / 1e5) as f32)
}
