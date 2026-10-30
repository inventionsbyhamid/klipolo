//! Building titles ahead: the playback lookahead's requests
//! to the title build thread (`ahead`), keeping the builds that land (`drain`, `install`), a frame waiting for a build
//! asked ahead rather than building it twice (`wait_for`), and room under the ceiling made by the textures kept for
//! titles not in the frame, the least recently drawn first (`make_room`).

use std::sync::Arc;
use std::time::Instant;

use super::geometry::{Geometry, Try};
use super::worker::{Built, Request, Worker};
use super::{Asked, Drawn, RETRY_AFTER, Slot, TextDrawer, slot};
use crate::plan::TextLayer;
use crate::render::device::RenderDevice;

impl TextDrawer {
    /// The playback lookahead: `layer` shows soon, in an output of `out` pixels (the plan's canvas is
    /// `canvas`). Its layout and texture are asked of the title build thread (started now if it isn't), within the
    /// room the ceiling has now, less the builds already asked (never asked past the ceiling), at the
    /// resolution a cold build would take (full first while its box fits the ceiling, its ink then
    /// checked against the room), unless it is held or asked already (its whole key: text, style and language, review
    /// the thread has `MAX_IN_FLIGHT` builds to do (its first frame builds it then).
    pub fn ahead(
        &mut self,
        dev: &Arc<RenderDevice>,
        layer: &TextLayer,
        canvas: (u32, u32),
        out: (u32, u32),
    ) {
        if layer.text.is_empty() || self.gone || self.in_flight.is_full() {
            return;
        }
        let slot = slot(layer);
        if self.refused.contains_key(&slot) {
            return;
        }
        let geometry = Geometry::of(layer, canvas, out);
        let whole = geometry.sizes(geometry.up);
        let held = self
            .drawn
            .get(&slot)
            .is_some_and(|d| d.whole == whole && d.key.same_text(layer) && d.drawn.is_some());
        if held || self.asked(&slot, whole, layer) {
            return;
        }
        // The room the last frame's texts and the titles built ahead and not shown yet leave (those
        // it kept for titles that left give theirs up), less every build in flight, dropped ones included.
        let last = self.frame.wrapping_sub(1);
        let others: u64 = self
            .drawn
            .iter()
            .filter(|(id, d)| **id != slot && (d.seen == last || d.ahead))
            .map(|(_, d)| d.bytes)
            .sum();
        let room = self.ceiling.saturating_sub(
            others
                .saturating_add(self.in_flight_bytes())
                .saturating_add(self.reserved),
        );
        let open = self.ceiling.saturating_sub(self.reserved);
        let Some(first) = Try::first(&geometry, room, open) else {
            return;
        };
        // Counted at its estimate; a full-resolution try's texture is held to the room by the thread.
        let bytes = if first.whole_first {
            geometry.estimate(1.0).min(room)
        } else {
            geometry.estimate(first.down)
        };
        let (font_px, box_px) = geometry.sizes(geometry.up * first.down);
        let key = geometry.key(layer, font_px, box_px);
        let ticket = self.tickets;
        self.tickets += 1;
        let request = Request {
            ticket,
            slot: slot.clone(),
            key: key.clone(),
            geometry,
            room,
            first,
            whole,
            epoch: self.epoch,
        };
        let worker = self.worker.get_or_insert_with(|| Worker::start(dev));
        if worker.send(request).is_err() {
            self.thread_gone();
            return;
        }
        self.in_flight.push(Asked {
            ticket,
            slot,
            key,
            whole,
            bytes,
            dropped: false,
        });
    }

    /// Whether a build of `layer` (in `slot`, at `whole`) is asked, still wanted and not landed: its whole key, at
    /// whatever size.
    fn asked(&self, slot: &Slot, whole: (u32, (u32, u32)), layer: &TextLayer) -> bool {
        self.in_flight
            .iter()
            .any(|a| !a.dropped && a.slot == *slot && a.whole == whole && a.key.same_text(layer))
    }

    /// The estimated bytes of every build asked and not landed, dropped ones included.
    pub(super) fn in_flight_bytes(&self) -> u64 {
        self.in_flight.iter().map(|a| a.bytes).sum()
    }

    fn thread_gone(&mut self) {
        self.worker = None;
        self.gone = true;
        self.in_flight.clear();
    }

    /// Keeps the builds that landed.
    pub(super) fn drain(&mut self) {
        loop {
            let reply = match &self.worker {
                Some(worker) => worker.try_reply(),
                None => return,
            };
            match reply {
                Ok(Some(built)) => self.install(built, false),
                Ok(None) => return,
                Err(_) => return self.thread_gone(),
            }
        }
    }

    /// Waits for the build of `layer` asked ahead (the frame needs it now), keeping every build that lands meanwhile;
    /// `WAIT_AT_MOST` in all (one deadline, however many builds land first), then the frame builds it
    /// itself.
    pub(super) fn wait_for(&mut self, slot: &Slot, whole: (u32, (u32, u32)), layer: &TextLayer) {
        let deadline = Instant::now() + self.wait;
        while self.asked(slot, whole, layer) {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply = match &self.worker {
                Some(_) if left.is_zero() => Ok(None),
                Some(worker) => worker.reply_within(left),
                None => return self.in_flight.clear(),
            };
            match reply {
                // The frame's own title counts no build still in flight when it lands, as it wouldn't if built here
                // the others land as `drain` keeps them.
                Ok(Some(built)) if built.slot == *slot => self.install(built, true),
                Ok(Some(built)) => self.install(built, false),
                Ok(None) => {
                    // Late: built here. The late build is no longer wanted; it counts until it lands, and is let go
                    // then.
                    for a in &mut self.in_flight {
                        if a.slot == *slot && a.whole == whole && a.key.same_text(layer) {
                            a.dropped = true;
                        }
                    }
                }
                Err(_) => return self.thread_gone(),
            }
        }
    }

    /// Keeps a build that landed, unless it was dropped (no longer wanted, or the fonts changed since it was asked),
    /// the text is held already (drawn here meanwhile, or this frame's under another layout), or it was refused. It
    /// needs room under the ceiling: textures kept for texts not in this frame give theirs up, oldest first; without
    /// enough it is dropped. A build let go releases its bytes. `on_screen`: it is the title a frame is
    /// waiting for, which, like any text of the frame, counts no build still in flight.
    fn install(&mut self, built: Built, on_screen: bool) {
        let dropped = match self.in_flight.iter().position(|a| a.ticket == built.ticket) {
            Some(i) => self.in_flight.remove(i).dropped,
            None => true,
        };
        let frame = self.frame;
        // One that didn't fit its room even at its softest: its first frame refuses it.
        let Some(title) = built.title else {
            return;
        };
        if dropped
            || built.epoch != self.epoch
            || self.refused.contains_key(&built.slot)
            || self
                .drawn
                .get(&built.slot)
                .is_some_and(|d| d.key == title.key || d.seen == frame)
        {
            return;
        }
        let bytes = title
            .drawn
            .as_ref()
            .map_or(0, |(window, _)| window.bytes(title.mipmapped));
        self.drawn.remove(&built.slot);
        let in_flight = if on_screen { 0 } else { self.in_flight_bytes() };
        if !self.make_room(&built.slot, bytes, in_flight) {
            return;
        }
        // Touched on this frame; at most one landing per build in flight, so the list never fills.
        if let Some((_, texture)) = &title.drawn
            && !self.touches.is_full()
        {
            self.touches.push(texture.clone());
        }
        self.drawn.insert(
            built.slot,
            Drawn {
                key: title.key,
                layout: title.layout,
                drawn: title.drawn,
                frame,
                bytes,
                down: title.down,
                whole: built.whole,
                retry_at: (title.down < 1.0).then(|| std::time::Instant::now() + RETRY_AFTER),
                too_big: None,
                seen: u64::MAX,
                ahead: true,
            },
        );
        self.note_peak();
    }

    /// Room for `bytes` more in `slot` under the ceiling, with `in_flight` bytes of builds still to land counted
    /// (a landing build counts the others; a text of the frame counts none): textures
    /// kept for texts not in this frame go, the least recently drawn first, until it fits. Whether it fits.
    pub(super) fn make_room(&mut self, slot: &Slot, bytes: u64, in_flight: u64) -> bool {
        let frame = self.frame;
        loop {
            let held: u64 = self
                .drawn
                .iter()
                .filter(|(id, _)| *id != slot)
                .map(|(_, d)| d.bytes)
                .sum();
            if held
                .saturating_add(self.reserved)
                .saturating_add(in_flight)
                .saturating_add(bytes)
                <= self.ceiling
            {
                return true;
            }
            let oldest = self
                .drawn
                .iter()
                .filter(|(id, d)| *id != slot && d.seen != frame && d.bytes > 0)
                .min_by_key(|(_, d)| d.frame)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.drawn.remove(&id);
                }
                None => return false,
            }
        }
    }
}
