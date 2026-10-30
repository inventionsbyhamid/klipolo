//! Which decoder lane serves a request. A recording cut into many clips jumps its source time at every cut; with
//! one reader per file each jump reseeks (100–200 ms at 1080p). So the playback lookahead primes the lane the
//! playhead isn't using at the next cut's source time (`warm`), and frames take the lane already positioned at their
//! time: cuts ping-pong between two readers and never wait for a seek.

use std::sync::Arc;

use klipolo_model::time::Us;

use super::{DecoderPool, NAMES};

/// A lane whose reader was last asked for `at` serves a time up to this far after it by decoding forward (a few
/// frames); a longer jump is a cut worth priming the other lane for.
const FORWARD_US: Us = 250_000;

impl DecoderPool {
    /// The lane positioned at or just before `time_us` for `asset` (open or still opening) on the file the plan names
    /// (`path`: a lane still on the original after its copy was swapped in isn't positioned), if any.
    pub(super) fn positioned(
        &self,
        asset: &Arc<str>,
        path: &Arc<str>,
        time_us: Us,
        taken: &[bool],
    ) -> Option<u8> {
        (0..NAMES.len() as u8)
            .filter(|&l| !taken[usize::from(l)])
            .find(|&l| {
                let key = (asset.clone(), l);
                let at = self
                    .slots
                    .get(&key)
                    .filter(|s| s.serves(path))
                    .map(|s| s.at_us)
                    .or_else(|| {
                        self.warming
                            .get(&key)
                            .filter(|w| w.path == *path)
                            .map(|w| w.at_us)
                    });
                at.is_some_and(|at| at <= time_us && time_us - at <= FORWARD_US)
            })
    }

    /// The lane for a frame of `asset` at `time_us`: the positioned one, else the one used last (not `taken`).
    pub(super) fn lane_for(
        &self,
        asset: &Arc<str>,
        path: &Arc<str>,
        time_us: Us,
        taken: &[bool],
    ) -> u8 {
        self.positioned(asset, path, time_us, taken)
            .unwrap_or_else(|| {
                (0..NAMES.len() as u8)
                    .filter(|&l| !taken[usize::from(l)])
                    .max_by_key(|&l| {
                        let key = (asset.clone(), l);
                        // Most recently used first; an open lane before a closed one; lane 0 on a tie.
                        (
                            self.slots.get(&key).map_or(0, |s| s.used + 1),
                            std::cmp::Reverse(l),
                        )
                    })
                    .unwrap_or(0)
            })
    }

    /// The lane the playhead isn't using for `asset`: the least recently used one.
    pub(super) fn spare_lane(&self, asset: &Arc<str>) -> u8 {
        (0..NAMES.len() as u8)
            .min_by_key(|&l| {
                let key = (asset.clone(), l);
                (self.slots.get(&key).map_or(0, |s| s.used + 1), l)
            })
            .unwrap_or(0)
    }

    /// Records that `lane` of `asset` was asked for `time_us`.
    pub(super) fn place(&mut self, asset: &Arc<str>, lane: u8, time_us: Us) {
        if let Some(slot) = self.slots.get_mut(&(asset.clone(), lane)) {
            slot.at_us = time_us;
        }
    }
}
