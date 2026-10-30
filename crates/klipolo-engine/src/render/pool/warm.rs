//! Warming decoders ahead of playback, and keeping the pool within its budgets (counting warm-ups).

use std::time::Instant;

use super::{DecoderPool, MAX_SLOTS, NAMES, POOL_CACHE_BYTES, State, WARM_EXPIRY, Warming, open};
use crate::plan::{FramePlan, Layer, VideoLayer};
use crate::render::pressure::{self, Pressure};

impl DecoderPool {
    /// Drops every warm-up in progress (playback stopped or jumped: their starts won't come).
    pub fn cancel_warming(&mut self) {
        for (_, w) in self.warming.drain() {
            w.task.abort();
        }
    }

    /// Gets each source's reader to its time in the background, so the frame that shows it doesn't wait: if no lane
    /// is positioned there already, the lane the playhead isn't using is opened there, or (when open) asked for that
    /// frame, which moves its reader. Never waits; nothing is warmed under memory pressure.
    ///
    /// Of a transition only the incoming side is warmed: the outgoing side is the clip already playing, on the lane
    /// the playhead uses. Warming it too would move the spare lane, which a recording cut in two needs for the
    /// incoming side, to the outgoing clip's time. When the window opens on the outgoing clip's first frame
    /// (`TransitionLayer::outgoing_starts`) nothing plays it yet, so both sides are warmed.
    pub fn warm(&mut self, plan: &FramePlan) {
        if pressure::level() != Pressure::Normal {
            return;
        }
        for layer in &plan.layers {
            let (first, then): (&[Layer], &[Layer]) = match layer {
                Layer::Transition(t) if t.outgoing_starts => (&t.outgoing, &t.incoming),
                Layer::Transition(t) => (&[], &t.incoming),
                other => (std::slice::from_ref(other), &[]),
            };
            for layer in first.iter().chain(then) {
                if let Layer::Video(v) = layer {
                    self.warm_one(v);
                }
            }
        }
        self.evict();
    }

    fn warm_one(&mut self, v: &VideoLayer) {
        let (asset, time_us) = (&v.source.asset_id, v.source.time_us);
        if self
            .positioned(asset, &v.source.path, time_us, &[false; NAMES.len()])
            .is_some()
        {
            return;
        }
        let lane = self.spare_lane(asset);
        let key = (asset.clone(), lane);
        if let Some(slot) = self.slots.get_mut(&key) {
            if !slot.serves(&v.source.path) {
                // Opened for the file the plan no longer names (a copy swapped in or dropped): open it afresh.
                self.slots.remove(&key);
            } else {
                // Not one whose disk is silent: its reader would only queue another request.
                if let State::Ready(decoder) = &slot.state
                    && !slot.silent
                {
                    slot.at_us = time_us;
                    let decoder = decoder.clone();
                    tokio::spawn(async move { drop(decoder.frame_at(time_us).await) });
                }
                return;
            }
        }
        if let Some(w) = self.warming.get(&key) {
            if w.path == v.source.path {
                return;
            }
            if let Some(w) = self.warming.remove(&key) {
                w.task.abort();
            }
        }
        let (source, tuning, force_ffmpeg, ask) = (
            v.source.clone(),
            self.tuning_for(v.role, lane),
            self.force_ffmpeg,
            self.ask.clone(),
        );
        let task = tokio::spawn(async move {
            let opened = open(&source, lane, tuning, force_ffmpeg, &ask).await;
            if let Ok(decoder) = &opened {
                let _ = decoder.frame_at(time_us).await;
            }
            opened
        });
        self.warming.insert(
            key,
            Warming {
                task,
                path: v.source.path.clone(),
                budget: tuning.max_cache_bytes,
                since: Instant::now(),
                at_us: time_us,
            },
        );
    }

    /// Closes the least recently used decoders past `MAX_SLOTS` or `POOL_CACHE_BYTES` (halved under memory pressure;
    /// when critical, everything this frame didn't use), counting warm-ups, which go first. Never one this frame used.
    pub(super) fn evict(&mut self) {
        let level = pressure::level();
        if level != Pressure::Normal {
            self.cancel_warming();
        }
        self.warming.retain(|_, w| {
            let live = w.since.elapsed() < WARM_EXPIRY;
            if !live {
                w.task.abort();
            }
            live
        });
        let (max_slots, budget) = match level {
            Pressure::Normal => (MAX_SLOTS, POOL_CACHE_BYTES),
            Pressure::Warning => (MAX_SLOTS, POOL_CACHE_BYTES / 2),
            Pressure::Critical => (0, 0),
        };
        loop {
            let count = self.slots.len() + self.warming.len();
            let bytes = self.slots.values().map(|s| s.budget).sum::<usize>()
                + self.warming.values().map(|w| w.budget).sum::<usize>();
            if count <= max_slots && bytes <= budget {
                return;
            }
            if let Some(key) = self
                .warming
                .iter()
                .min_by_key(|(_, w)| w.since)
                .map(|(k, _)| k.clone())
            {
                if let Some(w) = self.warming.remove(&key) {
                    w.task.abort();
                }
                continue;
            }
            let Some(oldest) = self
                .slots
                .iter()
                .filter(|(_, slot)| slot.used < self.tick)
                .min_by_key(|(_, slot)| slot.used)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.slots.remove(&oldest);
        }
    }
}
