//! An export's foreground pass over a file (`ensure`): its missing chunks made now, whatever holds background
//! work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use klipolo_cursor::activity::ActivityTrack;

use super::{ActivityStore, ENSURE_WAIT};
use crate::media::activity::files::{any_made, files, finished};
use crate::media::activity::run::{Ended, Pace};
use crate::media::key::{CacheKind, stem};
use crate::platform::activity::ActivityError;

impl ActivityStore {
    /// Export: `path`'s missing chunks made now on the caller's thread, whatever holds background work, with the share
    /// made so far after each chunk. A background pass over the same file stops first (it comes back after).
    /// `cancel` reaches the decoder itself, so a stop lands between packets. A minute whose pictures can't be read is
    /// passed over: the track comes back with it missing, or an error when nothing of
    /// the file could be made. A file marked as one that can't be analysed (`fail`) is `Unsupported` at once; one another
    /// pass holds past `ENSURE_WAIT`, or that can't be read now, is `Unavailable` or `Decode`: it may pass, never marked.
    pub fn ensure(
        &self,
        path: &str,
        cancel: &Arc<AtomicBool>,
        progress: &dyn Fn(f32),
    ) -> Result<Arc<ActivityTrack>, ActivityError> {
        if let Some(track) = self.get(path).filter(|t| t.is_complete()) {
            return Ok(track);
        }
        self.preempt(path);
        let stem = stem(path, &CacheKind::Activity)
            .ok_or_else(|| ActivityError::Unavailable("the file isn't there".into()))?;
        let pace = || {
            if cancel.load(Ordering::SeqCst) {
                Pace::Stop
            } else {
                Pace::Go
            }
        };
        let made = |track: Arc<ActivityTrack>, share: f32| {
            self.publish(path, track);
            progress(share);
        };
        let pass = self.pass(path, &stem, cancel, &pace, &made);
        let until = Instant::now() + ENSURE_WAIT;
        loop {
            match pass.run() {
                Ended::Complete(track) => {
                    self.publish(path, track.clone());
                    return Ok(track);
                }
                Ended::Partial(track, _) if any_made(&track) => {
                    self.publish(path, track.clone());
                    return Ok(track);
                }
                // A pause while another pass holds the file is a pause, never a read error.
                Ended::Busy if cancel.load(Ordering::SeqCst) => {
                    return Err(ActivityError::Cancelled);
                }
                Ended::Busy if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ended::Busy => {
                    return Err(ActivityError::Unavailable(
                        "another pass is writing it".into(),
                    ));
                }
                Ended::Stopped | Ended::Held => return Err(ActivityError::Cancelled),
                Ended::Unreadable(e) | Ended::Partial(_, e) => {
                    return Err(ActivityError::Decode(e));
                }
                Ended::Failed(e) => return Err(ActivityError::Unsupported(e)),
                Ended::DiskFull => return Err(ActivityError::DiskFull),
            }
        }
    }

    /// Export, for a screen it can't read now that the person said to export without: the
    /// minutes of `path` already made, as the editor plans with them (its track in memory, else its finished file, else
    /// the chunks its part file holds, read without its lock). None when nothing is made, or the screen file isn't
    /// there. O(bytes of one activity file).
    pub fn made_so_far(&self, path: &str) -> Option<Arc<ActivityTrack>> {
        if let Some(track) = self.get(path) {
            return Some(track);
        }
        let stem = stem(path, &CacheKind::Activity)?;
        let (done, part) = files(&self.root, &stem);
        let track = finished(&done)
            .filter(|t| t.is_complete())
            .or_else(|| finished(&part).filter(any_made))?;
        Some(Arc::new(track))
    }
}
