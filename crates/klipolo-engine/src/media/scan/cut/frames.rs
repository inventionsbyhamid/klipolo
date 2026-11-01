//! The cutter's decoder side: keyframes in, tiles into slots out.

use klipolo_model::time::Us;

use super::Cutter;
use crate::media::sprite::{Sprite, chunk_of, slot_of};

impl Cutter {
    /// Puts the decoded frame at source time `us` in its slot. A frame of a later chunk closes the one being filled
    /// (every frame before it has come out) and the keyframe-less chunks in between.
    fn place(&mut self, us: Us) {
        let chunk = chunk_of(slot_of(us));
        match self.sprite.as_ref().map(|s| s.chunk) {
            Some(current) if chunk < current => return,
            Some(current) if chunk == current => {}
            _ => {
                let mut previous = self.sprite.take();
                if let Some(p) = previous.as_mut() {
                    self.write(p);
                    self.bridge(p, chunk);
                }
                let Some(tile) = self.video.as_ref().map(|v| v.tile()) else {
                    return;
                };
                let mut next = Sprite::new(chunk, tile);
                if let Some(p) = &previous {
                    next.carry_from(p);
                }
                self.sprite = Some(next);
            }
        }
        self.put(slot_of(us));
    }

    /// The decoder's current frame into `slot` of the chunk being filled. The first one chunk 0 gets (what its tile 0
    /// shows) is the poster too, while there is none.
    pub(super) fn put(&mut self, slot: u64) {
        let first = self
            .sprite
            .as_ref()
            .is_some_and(|s| s.chunk == 0 && s.is_empty());
        let mut placed = false;
        if let (Some(video), Some(sprite)) = (self.video.as_mut(), self.sprite.as_mut())
            && let Some((planes, strides)) = video.tile_planes()
        {
            sprite.put(slot, planes, strides);
            placed = true;
        }
        if placed && first && self.want_poster {
            self.poster();
        }
    }

    /// Sends one keyframe; each frame that comes out goes to `slot` (or, None, to its own time's slot).
    pub(super) fn decode(&mut self, packet: &ffmpeg::Packet, slot: Option<u64>) {
        let Some(video) = self.video.as_mut() else {
            return;
        };
        self.decoded += 1;
        if let Err(e) = video.send(packet) {
            self.warn(&format!("thumbnail decode: {e}"));
            return;
        }
        self.receive(slot);
    }

    fn receive(&mut self, slot: Option<u64>) {
        loop {
            let Some(video) = self.video.as_mut() else {
                return;
            };
            if video.decoder.receive_frame(&mut video.frame).is_err() {
                return;
            }
            let pts = video.frame.timestamp().or_else(|| video.frame.pts());
            let us = pts.map(|pts| video.us(pts));
            match (slot, us) {
                (Some(slot), _) => self.put(slot),
                (None, Some(us)) => self.place(us),
                (None, None) => {}
            }
        }
    }

    /// Lets out what the decoder holds, then readies it for the next packet (after a seek).
    pub(super) fn drain(&mut self, slot: Option<u64>) {
        if self
            .video
            .as_mut()
            .is_some_and(|v| v.decoder.send_eof().is_ok())
        {
            self.receive(slot);
        }
        if let Some(video) = self.video.as_mut() {
            video.decoder.flush();
        }
    }
}
