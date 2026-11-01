//! The scan thread: one want at a time, at utility QoS, ending after a quiet spell.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};

use super::tidy::{STRIP_FILE, tidy, touch};
use super::{DISK_BUDGET, IDLE, Queue, READ_TRIES, ThumbStore, Want, lock};
use crate::log_name;
use crate::media::events::MediaKind;
use crate::media::key::{CacheKind, stem};
use crate::media::keyframes;
use crate::media::scan::{ScanEnd, scan_thumbs};
use crate::media::sprite::{chunk_of, chunks_for, slot_of};
use crate::media::thumbs::strip;

/// How a job left its file.
enum Outcome {
    /// Every chunk and the keyframe index are on disk (`strip.json` is where).
    Complete(PathBuf),
    /// A scan ran to the end but couldn't finish the file.
    GaveUp,
    /// Reading failed part-way (a bad sector, a drive going): looked at again with the next want, `READ_TRIES` times
    /// per opening.
    Unreadable,
    /// Cancelled, the disk is full, or the file is missing for now: looked at again with the next want.
    Again,
}

/// How a scan that ended with `end` leaves its file: `gone` (it went or changed while scanned), `complete` (every
/// chunk is on disk now), `strip_file` (where they are listed).
fn outcome(
    end: &ScanEnd,
    gone: bool,
    complete: impl FnOnce() -> bool,
    strip_file: PathBuf,
) -> Outcome {
    match end {
        _ if gone => Outcome::Again,
        ScanEnd::Done if complete() => Outcome::Complete(strip_file),
        ScanEnd::Done | ScanEnd::Failed(_) => Outcome::GaveUp,
        ScanEnd::Unreadable(_) => Outcome::Unreadable,
        ScanEnd::Stopped | ScanEnd::DiskFull => Outcome::Again,
    }
}

impl Queue {
    /// Records how a scan of `path` left it.
    fn ended(&mut self, path: String, outcome: Outcome) {
        match outcome {
            Outcome::Complete(strip) => {
                if self.touched.insert(path) {
                    touch(&strip);
                }
            }
            Outcome::GaveUp => {
                self.gave_up.insert(path);
            }
            Outcome::Unreadable => {
                let tries = self.unreadable.entry(path.clone()).or_default();
                *tries += 1;
                if *tries >= READ_TRIES {
                    self.gave_up.insert(path);
                }
            }
            Outcome::Again => {}
        }
    }
}

impl ThumbStore {
    /// The scan thread: one want at a time, then ends after a quiet spell.
    pub(super) fn work(self: Arc<Self>) {
        crate::qos::set_current(crate::qos::Qos::Utility);
        loop {
            let (want, cancel) = {
                let mut q = lock(&self.queue);
                loop {
                    if let Some(want) = q.wants.pop_front() {
                        q.cancel = Arc::new(AtomicBool::new(false));
                        q.current = Some(want.path.clone());
                        break (want, q.cancel.clone());
                    }
                    // Nothing more to scan: the activity pass's turn (under the queue's lock, as `want` sets it).
                    self.thumbs_busy(false);
                    let (next, wait) = self
                        .changed
                        .wait_timeout(q, IDLE)
                        .unwrap_or_else(PoisonError::into_inner);
                    q = next;
                    if wait.timed_out() && q.wants.is_empty() {
                        q.worker = false;
                        return;
                    }
                }
            };
            let outcome = self.run(&want, &cancel);
            let mut q = lock(&self.queue);
            q.current = None;
            let idle = q.wants.is_empty();
            if !cancel.load(Ordering::SeqCst) {
                q.ended(want.path, outcome);
            }
            drop(q);
            if idle {
                self.set_waiting(false);
            }
        }
    }

    /// Scans what `want` still misses. A file already done is told so at once, held or not: only a scan that needs
    /// work waits for holds, and says it does (`set_waiting`: transcription lets thumbnails go first).
    fn run(&self, want: &Want, cancel: &AtomicBool) -> Outcome {
        if self.disk_full() {
            return Outcome::Again;
        }
        let root = &self.shared.root;
        let Some(key) = stem(&want.path, &CacheKind::Thumbs) else {
            // Missing for now (an unplugged drive): looked at again with the next want.
            return Outcome::Again;
        };
        let have: HashSet<u32> = strip(root, &want.path)
            .map(|s| s.chunks.iter().map(|c| c.index).collect())
            .unwrap_or_default();
        let at = chunk_of(slot_of(want.at_us));
        let mut missing: Vec<u32> = (0..chunks_for(want.duration_us))
            .filter(|c| !have.contains(c))
            .collect();
        // The chunk under the playhead first, then on to the end, then the start.
        missing.sort_by_key(|&c| (c < at, c));
        let strip_file = root.join(&key).join(STRIP_FILE);
        if missing.is_empty() && keyframes::has(root, &key) {
            return Outcome::Complete(strip_file);
        }
        self.set_waiting(true);
        if !self.wait_unheld(cancel) {
            return Outcome::Again;
        }
        self.running.store(true, Ordering::SeqCst);
        let scanned = scan_thumbs(&self.shared, &want.path, &missing, &|| {
            self.wait_unheld(cancel)
        });
        self.running.store(false, Ordering::SeqCst);
        // After `running` is cleared: a listener reading `scanning()` on this event sees the scan ended.
        self.shared.emit(&want.path, MediaKind::Thumbs);
        // The file went (an unplugged drive) or changed while it was scanned: looked at again later.
        let gone = stem(&want.path, &CacheKind::Thumbs).as_ref() != Some(&key);
        if let (ScanEnd::Failed(error), false) = (&scanned.end, gone) {
            tracing::warn!(%error, file = %log_name(&want.path), "thumbnails not made");
        }
        tracing::debug!(
            file = %log_name(&want.path),
            bytes_read = scanned.bytes_read,
            end = ?scanned.end,
            "thumbnail scan"
        );
        if scanned.wrote {
            self.tidy();
        }
        let complete = || {
            strip(root, &want.path)
                .is_some_and(|s| s.chunks.len() >= chunks_for(want.duration_us) as usize)
        };
        outcome(&scanned.end, gone, complete, strip_file)
    }

    /// Keeps the folder under the budget, never removing the open project's files, the running scan's or an
    /// unfinished export's.
    fn tidy(&self) {
        let (project, current) = {
            let q = lock(&self.queue);
            (q.project.clone(), q.current.clone())
        };
        let jobs = lock(&self.jobs_root).clone();
        tidy(&self.shared.root, DISK_BUDGET, false, || {
            let mut paths = project;
            paths.extend(current);
            if let Some(jobs) = &jobs {
                paths.extend(crate::media::export_job_sources(jobs));
            }
            paths
                .iter()
                .filter_map(|p| stem(p, &CacheKind::Thumbs))
                .collect()
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use klipolo_platform::durable::Os;

    use super::*;

    /// A file whose reading keeps failing part-way (a bad sector) is scanned again on the next few edits, then not
    /// until the project is opened again (each edit asks for the project's files).
    #[test]
    fn a_file_that_keeps_failing_reads_waits_for_the_next_opening() {
        let root = tempfile::tempdir().unwrap();
        let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
        let path = "/media/bad-sector.mov".to_string();
        let held = |store: &ThumbStore| lock(&store.queue).gave_up.contains(&path);
        // A scan whose reading failed part-way, as `run` records it.
        let unreadable = || {
            let end = ScanEnd::Unreadable("Input/output error".into());
            outcome(&end, false, || false, PathBuf::new())
        };
        for _ in 1..READ_TRIES {
            lock(&store.queue).ended(path.clone(), unreadable());
            assert!(!held(&store), "tried again with the next want");
        }
        lock(&store.queue).ended(path.clone(), unreadable());
        assert!(held(&store));
        store.want(vec![Want {
            path: path.clone(),
            duration_us: 1_000_000,
            at_us: 0,
        }]);
        assert!(!store.pending(), "not scanned again in this opening");
        store.cancel_all();
        assert!(!held(&store), "the next opening tries it again");
    }
}
