//! What a source says while no timed line can be written (before the gate, during a pause): it isn't
//! lost. A shape is defined whenever it arrives (its PNG and `S` line need no time; before the file opens it waits, at
//! most `MAX_SHAPES`, and is defined at the opening). The latest shape, visibility and area are kept, and written at
//! the gate and when a pause ends, wherever the file doesn't already show them. So a source that sends its current
//! shape as it starts (during the countdown) draws as that shape, never as the Arrow look. The latest place is kept
//! too, and written as a move then, so the cursor is where it is from the gate, not from the first move or beat
//! and so is a switch to sampled clicks (a monitor that couldn't start switches in the
//! countdown). A resumed take's writer knows what its file shows from the file (`Kept::shown_in`).

use klipolo_cursor::format::{Line, write_line};
use klipolo_cursor::{ClickSource, CursorTrack, MAX_SHAPES};
use klipolo_model::time::Us;

use super::writer::{NO_SHAPE, area_ok};
use super::{CursorEvent, CursorNote, CursorWriter, ShapeImage};
use crate::run::core::Effect;

/// See the module notes.
#[derive(Clone, Debug)]
pub(super) struct Kept {
    /// Shapes sent before the file opened, by key, defined at the opening.
    early: Vec<(u64, ShapeImage)>,
    /// The latest shape (by key), visibility and area (while paused) the source gave.
    pub(super) shape: Option<u64>,
    pub(super) visible: Option<bool>,
    area: Option<(f32, f32)>,
    pub(super) place: Option<(f32, f32)>,
    /// The source switched to sampled clicks (before the gate: a monitor that couldn't start; in a pause).
    sampled: bool,
    /// What the file shows from its last lines (None: not known, so the next one is written).
    shown_shape: Option<u16>,
    shown_visible: Option<bool>,
}

impl Default for Kept {
    fn default() -> Self {
        Self {
            early: Vec::new(),
            shape: None,
            visible: None,
            area: None,
            place: None,
            sampled: false,
            shown_shape: None,
            shown_visible: Some(true),
        }
    }
}

impl Kept {
    /// What a resumed take's file shows from its last lines; not known if it wasn't read whole.
    pub(super) fn shown_in(track: &CursorTrack) -> Self {
        let whole = !track.notes().limited && !track.notes().read_failed;
        let last = Us::MAX - 1;
        Self {
            shown_shape: whole.then(|| track.shape_at(last).map(|d| d.id)).flatten(),
            shown_visible: whole.then(|| track.visible_at(last)),
            ..Self::default()
        }
    }
}

impl CursorWriter {
    /// An event whose timed lines can't be written now: what it says is kept (see the module notes).
    pub(super) fn hold(&mut self, ev: &CursorEvent, fx: &mut Vec<Effect>) -> Option<CursorNote> {
        match *ev {
            CursorEvent::NewShape { key, ref image, .. } => {
                self.kept.shape = Some(key);
                if !self.open {
                    let early = &mut self.kept.early;
                    early.retain(|(k, _)| *k != key);
                    if early.len() < MAX_SHAPES {
                        early.push((key, image.clone()));
                    }
                } else if self.define(key, image, fx).is_none()
                    && self.shapes.is_full()
                    && !self.shapes_full
                {
                    self.shapes_full = true;
                    return Some(CursorNote::ShapesFull);
                }
            }
            CursorEvent::Shape { key, .. } => self.kept.shape = Some(key),
            CursorEvent::Visible { on, .. } => self.kept.visible = Some(on),
            CursorEvent::Move { x, y, .. }
            | CursorEvent::Beat { x, y, .. }
            | CursorEvent::Button { x, y, .. } => self.kept.place = Some((x, y)),
            CursorEvent::ClicksSampled { .. } => self.kept.sampled = true,
            CursorEvent::ShapesFull { .. } => return self.shapes_full(),
            CursorEvent::Area { w_pts, h_pts, .. } if area_ok(w_pts, h_pts) => {
                if self.open {
                    self.kept.area = Some((w_pts, h_pts));
                } else {
                    self.area = Some((w_pts, h_pts));
                }
            }
            _ => {}
        }
        None
    }

    /// The kept place, shape, visibility and area written at `t` (the gate, the end of a pause), where the file
    /// doesn't show them already, and a switch to sampled clicks (`CursorNote::Sampled` the first time, as for one
    /// that came in time).
    pub fn restate(&mut self, t: Us) -> Option<CursorNote> {
        if !self.open {
            return None;
        }
        let mut note = None;
        if std::mem::take(&mut self.kept.sampled) && self.clicks == ClickSource::Events {
            self.clicks = ClickSource::Sampled;
            let clicks = ClickSource::Sampled;
            self.reorder.push(t, Line::Clicks { t, clicks });
            note = Some(CursorNote::Sampled);
        }
        if let Some((x, y)) = self.kept.place.take() {
            self.position(t, x, y, false);
        }
        if let Some(key) = self.kept.shape {
            let id = self.shapes.id_of(key).unwrap_or(NO_SHAPE);
            self.show_shape(t, id);
        }
        if let Some(on) = self.kept.visible {
            self.show_visible(t, on);
        }
        if let Some((w, h)) = self.kept.area.take()
            && self.area != Some((w, h))
        {
            self.area = Some((w, h));
            self.reorder.push(t, Line::Area { t, w, h });
        }
        note
    }

    /// A shape's id: its own if the store has its key, else a new one, its PNG written whole first and its untimed
    /// `S` line straight out. None past `MAX_SHAPES`, or for an image the file can't hold.
    pub(super) fn define(
        &mut self,
        key: u64,
        image: &ShapeImage,
        fx: &mut Vec<Effect>,
    ) -> Option<u16> {
        if let Some(id) = self.shapes.id_of(key) {
            return Some(id);
        }
        let (id, file, png, def) = self.shapes.add(key, image)?;
        fx.push(Effect::WriteAtomic {
            key: file,
            bytes: png,
        });
        write_line(&def, &mut self.out);
        Some(id)
    }

    /// The shapes sent before the file opened, defined now.
    pub(super) fn define_early(&mut self, fx: &mut Vec<Effect>) {
        for (key, image) in std::mem::take(&mut self.kept.early) {
            let _ = self.define(key, &image, fx);
        }
    }

    pub(super) fn show_shape(&mut self, t: Us, id: u16) {
        if self.kept.shown_shape != Some(id) {
            self.kept.shown_shape = Some(id);
            self.reorder.push(t, Line::Shape { t, id });
        }
    }

    pub(super) fn show_visible(&mut self, t: Us, on: bool) {
        if self.kept.shown_visible != Some(on) {
            self.kept.shown_visible = Some(on);
            self.reorder.push(t, Line::Visible { t, on });
        }
    }
}
