//! A source's decoder slot: reused while it reads what the plan names, opened (or taken from its warm-up) otherwise.

use std::sync::Arc;
use std::time::Instant;

use super::open::{CopyCheck, copy_check, open};
use super::{DecoderPool, Failure, FileState, Opened, RETRY_AFTER, Slot, State, file_state};
use crate::media::{MediaEvent, MediaKind};
use crate::plan::{SourceRef, VideoRole};

impl DecoderPool {
    pub(super) async fn slot(&mut self, s: &SourceRef, lane: u8, role: VideoRole) -> Opened {
        let key = (s.asset_id.clone(), lane);
        let tick = self.tick;
        if let Some(slot) = self.slots.get_mut(&key)
            && slot.serves(&s.path)
        {
            if *slot.path != *s.path {
                // The app dropped a copy that fell back: this decoder already reads the original, which the plan
                // names now. Later, a copy made again at that path opens afresh and is reported again if it fails.
                self.unusable.remove(&slot.path);
                slot.path = s.path.clone();
            }
            slot.used = tick;
            match &mut slot.state {
                // Its disk didn't answer after a frame failed: that stat is asked first (still out, it answers at once;
                // answered since, its kept answer is taken), so no frame waits for the reader again while the disk is
                // silent. A file gone meanwhile is opened again below (an editing copy's original, or missing).
                State::Ready(decoder) if slot.silent => {
                    match file_state(&decoder.file, &self.ask).await {
                        FileState::NotAnswering => return Err(Failure::NotAnswering),
                        FileState::Missing => {}
                        FileState::There | FileState::Unreadable => {
                            slot.silent = false;
                            return Ok(decoder.clone());
                        }
                    }
                }
                // An editing copy opened while its original's disk was silent: checked against the original until it
                // answers (still out, it answers at once: the copy is read meanwhile). A copy that is no longer the
                // original's is opened again below, which reads the original.
                State::Ready(decoder) if decoder.unchecked => {
                    match copy_check(&decoder.file, &s.original, &self.ask).await {
                        CopyCheck::Current => {
                            decoder.unchecked = false;
                            return Ok(decoder.clone());
                        }
                        CopyCheck::Unchecked | CopyCheck::Silent => return Ok(decoder.clone()),
                        CopyCheck::Stale => {}
                    }
                }
                State::Ready(decoder) => return Ok(decoder.clone()),
                // A disk that wasn't answering is asked again at once (a stat still out answers at once; one that
                // answered since is taken from its kept answer).
                State::Failed { failure, at }
                    if *failure != Failure::NotAnswering && at.elapsed() < RETRY_AFTER =>
                {
                    return Err(*failure);
                }
                State::Failed { .. } => {}
            }
        }
        let result = match self.warming.remove(&key) {
            Some(w) if w.path == s.path => w.task.await.unwrap_or(Err(Failure::Unreadable)),
            other => {
                // A warm-up for the file the plan no longer names (a copy swapped in or dropped meanwhile).
                if let Some(w) = other {
                    w.task.abort();
                }
                let opening = open(
                    s,
                    lane,
                    self.tuning_for(role, lane),
                    self.force_ffmpeg,
                    &self.ask,
                );
                // Export's first open of a source is a chain of bounded steps (the stats, the reader's start, an FFmpeg
                // start after it fails): the heartbeat hears it, so it can't fall silent past the parent's watchdog
                //.
                if self.patient {
                    self.beat.alive_while(opening).await
                } else {
                    opening.await
                }
            }
        };
        if let Ok(decoder) = &result
            && *decoder.file != *s.path
        {
            self.copy_unusable(s);
        }
        let budget = if result.is_ok() {
            self.tuning_for(role, lane).max_cache_bytes
        } else {
            0
        };
        let state = match &result {
            Ok(decoder) => State::Ready(decoder.clone()),
            Err(failure) => State::Failed {
                failure: *failure,
                at: Instant::now(),
            },
        };
        self.slots.insert(
            key,
            Slot {
                state,
                path: s.path.clone(),
                used: tick,
                at_us: s.time_us,
                budget,
                silent: false,
            },
        );
        result
    }

    /// `s`'s editing copy couldn't be opened and its original is read instead: the app hears it once, naming the
    /// original, and drops the mapping.
    fn copy_unusable(&mut self, s: &SourceRef) {
        if !self.unusable.insert(s.path.clone()) {
            return;
        }
        tracing::warn!(
            file = %crate::log_name(&*s.original),
            "an editing copy couldn't be opened; the original is read"
        );
        if let Some(sink) = &self.sink {
            sink(MediaEvent::Changed {
                path: s.original.to_string(),
                kind: MediaKind::Proxy,
            });
        }
    }
}

impl Slot {
    /// Whether this slot reads what the plan names now: opened for `path`, or (a copy that fell back) reading it.
    pub(super) fn serves(&self, path: &Arc<str>) -> bool {
        *self.path == **path
            || matches!(&self.state, State::Ready(decoder) if *decoder.file == **path)
    }
}
