//! The activity pass, next in line after the thumbnails: it shares their holds (`Hold`, the sound cache's
//! `SOUND`) and also waits while thumbnails have work (`THUMBS`), so the order is sound → thumbnails → activity, one
//! background decode at a time. The thumbnail store holds it, tells it of a closed project (`cancel_all`) and of the
//! media sink, and wakes it whenever a hold goes (`release`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use super::{Hold, THUMBS, ThumbStore, lock};
use crate::media::activity::ActivityStore;

/// The longest a playback hold keeps a pass's idle decoder.
const PLAY_GRACE: Duration = Duration::from_secs(10);
/// The longest the sound cache or thumbnails keep it.
const GRACE: Duration = Duration::from_secs(1);

/// How long a pass keeps its idle decoder while `holds` (the hold word) hold it: none for an export, a take or an
/// editing copy (they let it go at once: memory, the disk and the file's lock are theirs); up to `PLAY_GRACE` for
/// playback (a tap of Play keeps the chunk in progress, at no CPU); `GRACE` for the order's own holds (the sound cache,
/// thumbnails).
fn keep_for(holds: u8) -> Duration {
    let at_once = Hold::Export.bit() | Hold::Recording.bit() | Hold::Copy.bit();
    if holds & at_once != 0 {
        Duration::ZERO
    } else if holds & Hold::Playback.bit() != 0 {
        PLAY_GRACE
    } else {
        GRACE
    }
}

impl ThumbStore {
    /// The activity pass after these thumbnails, once attached (`ActivityStore::attach`).
    pub fn activity(&self) -> Option<Arc<ActivityStore>> {
        self.next.get().cloned()
    }

    /// Puts `next` after these thumbnails (once): it hears of the media sink and of a closed project from here.
    pub(crate) fn set_next(&self, next: Arc<ActivityStore>) {
        if let Some(sink) = lock(&self.shared.sink).clone() {
            next.set_sink(sink);
        }
        let _ = self.next.set(next);
    }

    /// Thumbnails have work queued or a scan running (`on`), or none. Called under the queue's lock.
    pub(super) fn thumbs_busy(&self, on: bool) {
        if on {
            *lock(&self.holds) |= THUMBS;
        } else {
            self.release(THUMBS);
        }
    }

    /// Nothing holds background work and thumbnails have none: the activity pass may decode.
    pub(crate) fn quiet(&self) -> bool {
        *lock(&self.holds) == 0
    }

    /// Blocks until `quiet` (true) or `cancel` is set (false). No polling: holds going, a closed project and
    /// `wake_waiters` notify.
    pub(crate) fn wait_quiet(&self, cancel: &AtomicBool) -> bool {
        self.wait_quiet_within(cancel, |_| None)
    }

    /// A pass that a hold stopped mid-file waits with its idle decoder as long as the holds allow (`keep_for`): true
    /// once quiet in time, false when it should let its decoder go (or `cancel` is set).
    pub(crate) fn wait_quiet_keeping(&self, cancel: &AtomicBool) -> bool {
        self.wait_quiet_within(cancel, |holds| Some(keep_for(holds)))
    }

    /// Waits for quiet while `limit` (of the holds, re-read at every wake: an export arriving during a tap of Play lets
    /// the decoder go at once) allows; None: as long as it takes.
    fn wait_quiet_within(
        &self,
        cancel: &AtomicBool,
        limit: impl Fn(u8) -> Option<Duration>,
    ) -> bool {
        let since = Instant::now();
        let mut holds = lock(&self.holds);
        loop {
            if cancel.load(Ordering::SeqCst) {
                return false;
            }
            if *holds == 0 {
                return true;
            }
            holds = match limit(*holds).map(|l| l.saturating_sub(since.elapsed())) {
                None => self
                    .released
                    .wait(holds)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(left) if left.is_zero() => return false,
                Some(left) => {
                    let waited = self.released.wait_timeout(holds, left);
                    waited.unwrap_or_else(PoisonError::into_inner).0
                }
            };
        }
    }

    /// Whatever waits for quiet looks again (a cancelled pass sees its cancel).
    pub(crate) fn wake_waiters(&self) {
        let _holds = lock(&self.holds);
        self.released.notify_all();
    }
}
