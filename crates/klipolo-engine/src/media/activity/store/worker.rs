//! The activity thread: one file at a time at utility QoS, a finished file read, any other analysed once nothing holds
//! background work; it ends after a quiet spell.

use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};

use klipolo_cursor::activity::ActivityTrack;

use super::{ActivityStore, BUSY_AGAIN, IDLE, lock};
use crate::log_name;
use crate::media::activity::files::{any_made, files, finished};
use crate::media::activity::run::{Ended, MARKED, Pace};
use crate::media::key::{CacheKind, stem};
use crate::media::store::READ_TRIES;

impl ActivityStore {
    /// The thread: one file at a time, ending after a quiet spell.
    pub(super) fn work(self: Arc<Self>) {
        crate::qos::set_current(crate::qos::Qos::Utility);
        loop {
            let (path, cancel) = {
                let mut s = lock(&self.state);
                loop {
                    if let Some(path) = s.queue.pop_front() {
                        let cancel = Arc::new(AtomicBool::new(false));
                        s.current = Some((path.clone(), cancel.clone()));
                        break (path, cancel);
                    }
                    let (next, wait) = self
                        .changed
                        .wait_timeout(s, IDLE)
                        .unwrap_or_else(PoisonError::into_inner);
                    s = next;
                    if wait.timed_out() && s.queue.is_empty() {
                        s.worker = false;
                        return;
                    }
                }
            };
            let ended = self.job(&path, &cancel);
            let mut s = lock(&self.state);
            s.current = None;
            if cancel.load(Ordering::SeqCst) {
                continue;
            }
            match ended {
                Ended::Complete(_) | Ended::Stopped => {}
                Ended::Failed(error) => {
                    tracing::warn!(%error, file = %log_name(&path), "screen activity: not analysed");
                    s.gave_up.insert(path.clone());
                    drop(s);
                    // Marked so in the folder (an export plans without it too), and its track goes.
                    self.fail(&path);
                }
                Ended::Held => s.queue.push_front(path),
                Ended::Busy => {
                    // Another process writes it: looked at again a little later (a want or a cancel wakes this).
                    s.queue.push_back(path);
                    drop(self.changed.wait_timeout(s, BUSY_AGAIN));
                }
                Ended::Unreadable(error) | Ended::Partial(_, error) => {
                    tracing::warn!(%error, file = %log_name(&path), "screen activity: reading failed");
                    let tries = s.unreadable.entry(path.clone()).or_default();
                    *tries += 1;
                    if *tries < READ_TRIES {
                        s.queue.push_back(path);
                    } else {
                        s.gave_up.insert(path);
                    }
                }
                Ended::DiskFull => {
                    s.disk_full = true;
                    s.queue.clear();
                }
            }
        }
    }

    /// Looks at one file: a finished one is read; otherwise its pass runs once nothing holds background work.
    fn job(&self, path: &str, cancel: &Arc<AtomicBool>) -> Ended {
        let Some(stem) = stem(path, &CacheKind::Activity) else {
            // Not there now (an unplugged drive): looked at again with the next want.
            return Ended::Stopped;
        };
        if lock(&self.state).disk_full {
            return Ended::Stopped;
        }
        if self.marked(&stem) {
            return Ended::Failed(MARKED.into());
        }
        let (done, part) = files(&self.root, &stem);
        if let Some(track) = finished(&done).filter(|t| t.is_complete()) {
            let track = Arc::new(track);
            self.publish(path, track.clone());
            return Ended::Complete(track);
        }
        // A part file from an earlier opening shows the chunks it has now (read only, no lock), not once the pass may
        // run again.
        if lock(&self.state).tracks.peek(path).is_none()
            && let Some(track) = finished(&part).filter(any_made)
        {
            self.publish(path, Arc::new(track));
        }
        let Some(thumbs) = self.thumbs.upgrade() else {
            return Ended::Stopped;
        };
        if !thumbs.wait_quiet(cancel) {
            return Ended::Stopped;
        }
        let pace = || {
            if cancel.load(Ordering::SeqCst) {
                Pace::Stop
            } else if thumbs.quiet() || thumbs.wait_quiet_keeping(cancel) {
                Pace::Go
            } else if cancel.load(Ordering::SeqCst) {
                Pace::Stop
            } else {
                Pace::Held
            }
        };
        let made = |track: Arc<ActivityTrack>, _share: f32| self.publish(path, track);
        let ended = self.pass(path, &stem, cancel, &pace, &made).run();
        // A pass that passed over a minute holds what it made past it, as export plans with.
        if let Ended::Complete(track) | Ended::Partial(track, _) = &ended {
            self.publish(path, track.clone());
        }
        ended
    }
}
