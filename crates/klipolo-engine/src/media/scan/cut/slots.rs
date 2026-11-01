//! Seek mode for intra-only video (one decoded frame per slot), and storing the keyframe index.

use super::{Cutter, Run};
use crate::media::keyframes;
use crate::media::sprite::{Sprite, chunk_of};

impl Cutter {
    /// Seek mode (intra-only video): `packet` is the keyframe at or before `slot`'s start, decoded on its own into
    /// that slot. The same keyframe as the slot before isn't decoded again (the slot shows the tile before it). Slots
    /// come in order; a slot of a new chunk writes the one before.
    pub(in crate::media::scan) fn slot(
        &mut self,
        packet: &ffmpeg::Packet,
        slot: u64,
        same_as_before: bool,
    ) {
        if self.stopped {
            return;
        }
        let chunk = chunk_of(slot);
        if self.sprite.as_ref().is_none_or(|s| s.chunk != chunk) {
            if let Some(mut done) = self.sprite.take() {
                self.write(&mut done);
            }
            let Some(tile) = self.video.as_ref().map(|v| v.tile()) else {
                return;
            };
            self.sprite = Some(Sprite::new(chunk, tile));
        }
        if !same_as_before {
            self.decode(packet, Some(slot));
            self.drain(Some(slot));
        }
    }

    /// Seek mode: every slot was visited from its start, so the chunk being filled is complete.
    pub(in crate::media::scan) fn begin_slots(&mut self) {
        self.run = Run {
            from_start: true,
            ..Run::default()
        };
    }

    pub(in crate::media::scan) fn end_slots(&mut self) {
        if let Some(mut sprite) = self.sprite.take() {
            self.write(&mut sprite);
        }
    }

    /// Stores the keyframe index once the scan has seen every keyframe.
    pub(super) fn store_keys(&mut self) {
        if !self.want_keys || self.stopped || !self.keys.complete() {
            return;
        }
        let pts = self.keys.pts();
        self.store_index(Some(pts));
    }

    /// Stores `pts` as the file's keyframe index (None: it can't be had without reading every packet, so decoders
    /// build theirs as before and no scan tries again).
    pub(in crate::media::scan) fn store_index(&mut self, pts: Option<Vec<i64>>) {
        let (Some(mut stored), false) = (self.meta.clone(), self.stopped) else {
            return;
        };
        let (steps, root) = (&*self.shared.steps, &self.shared.root);
        let written = match pts {
            Some(pts) => {
                stored.pts = pts;
                keyframes::write(steps, root, &self.stem, &stored)
            }
            None => keyframes::write_none(steps, root, &self.stem),
        };
        match written {
            Ok(()) => {
                self.want_keys = false;
                self.wrote = true;
            }
            Err(e) => self.failed(&e),
        }
    }
}
