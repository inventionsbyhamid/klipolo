//! The build thread: runs queued builds one at a time (none while the queue is paused) and records how each went. A
//! build someone waits on (`PcmStore::build_cancellable`) raises the thread to user-initiated until it ends.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use super::build::Progress;
use super::{BuildError, DISK_BUDGET, Job, Pcm, PcmStore, build, lock, protected_stems, tidy};
use crate::audio::mix::RATE;
use crate::log_name;
use crate::media::events::{MediaEvent, MediaKind};
use crate::media::store::READ_TRIES;
use crate::qos::Qos;

impl Progress {
    /// A build the store's build thread runs: that thread follows `urgent`.
    pub(super) fn worker() -> Self {
        Self {
            worker: true,
            ..Self::default()
        }
    }

    /// The build's own thread, at each block: a worker build runs at user-initiated while someone waits on it, and
    /// at utility again once no one does. Two atomic loads when nothing changes.
    pub(super) fn follow_urgency(&self) {
        if !self.worker {
            return;
        }
        let urgent = self.urgent.load(Ordering::Acquire) > 0;
        if self.raised.swap(urgent, Ordering::AcqRel) != urgent {
            self.set_qos(if urgent {
                Qos::UserInitiated
            } else {
                Qos::Utility
            });
        }
    }

    /// The build ended: its thread goes back to utility if it was raised.
    fn settle(&self) {
        if self.raised.swap(false, Ordering::AcqRel) {
            self.set_qos(Qos::Utility);
        }
    }

    fn set_qos(&self, qos: Qos) {
        #[cfg(test)]
        lock(&self.qos).push(qos);
        crate::qos::set_current(qos);
    }
}

impl PcmStore {
    /// The build thread: runs queued jobs one at a time, then ends after a quiet spell. Thumbnail scans wait while it
    /// decodes (sound first, one background decode at a time; the scans cut the thumbnails after it).
    pub(super) fn work(self: Arc<Self>) {
        crate::qos::set_current(crate::qos::Qos::Utility);
        let thumbs = lock(&self.thumbs).clone();
        loop {
            let job = {
                let mut inner = lock(&self.inner);
                loop {
                    // Paused (Sync listens): the queue keeps its order until it resumes. A file a caller is building
                    // waits its turn until that build ends.
                    let next = (!inner.paused)
                        .then(|| {
                            let building = &inner.building;
                            inner
                                .queue
                                .iter()
                                .position(|j| !building.contains_key(&j.key))
                        })
                        .flatten();
                    if let Some(next) = next {
                        let job = inner.queue.remove(next);
                        let progress = Arc::new(Progress::worker());
                        inner.building.insert(job.key.clone(), progress.clone());
                        if let Some(thumbs) = &thumbs {
                            thumbs.sound_busy(true);
                        }
                        break Some((job, progress));
                    }
                    if let Some(thumbs) = &thumbs {
                        thumbs.sound_busy(false);
                    }
                    let (next, wait) = self
                        .changed
                        .wait_timeout(inner, Duration::from_secs(30))
                        .unwrap_or_else(PoisonError::into_inner);
                    inner = next;
                    if wait.timed_out() && inner.queue.is_empty() {
                        inner.worker = false;
                        break None;
                    }
                }
            };
            let Some((job, progress)) = job else { return };
            match self.run_job(&job, progress.clone()) {
                Ok(_) | Err(BuildError::Cancelled) => {}
                Err(error) => {
                    tracing::warn!(%error, file = %log_name(&job.key.path), "audio not decoded");
                }
            }
            progress.settle();
        }
    }

    /// Builds one job on this thread and records the outcome. `progress` is the build's entry in `building` (a close
    /// may have removed it since, and the next project may have started another build of the same file).
    pub(super) fn run_job(
        &self,
        job: &Job,
        progress: Arc<Progress>,
    ) -> Result<Arc<Pcm>, BuildError> {
        self.run_job_cancellable(job, progress, &AtomicBool::new(false))
    }

    /// `run_job`, which `cancel` stops at its next block like a close does.
    pub(super) fn run_job_cancellable(
        &self,
        job: &Job,
        progress: Arc<Progress>,
        cancel: &AtomicBool,
    ) -> Result<Arc<Pcm>, BuildError> {
        let result = build::build(self, job, &progress, cancel);
        let mut inner = lock(&self.inner);
        if inner
            .building
            .get(&job.key)
            .is_some_and(|p| Arc::ptr_eq(p, &progress))
        {
            inner.building.remove(&job.key);
        }
        // Another build of the same file took over: its copy is the one to keep.
        let replaced = inner.building.contains_key(&job.key);
        // A build that ended serves the file's queued job too; one cancelled leaves it for the worker.
        if !matches!(result, Err(BuildError::Cancelled)) && !replaced {
            inner.queue.retain(|j| j.key != job.key);
        }
        // Reading failed part-way again and again (a bad sector): what decoded stays playable and the file waits for
        // the next opening. A build a close cancelled counts toward nothing.
        let mut damaged_at = None;
        if let Err(BuildError::Unreadable { at, .. }) = &result
            && !progress.cancel.load(Ordering::Acquire)
        {
            let tries = inner.unreadable.entry(job.key.clone()).or_default();
            *tries += 1;
            if *tries >= READ_TRIES && !replaced {
                inner.damaged.insert(job.key.clone());
                damaged_at = Some(*at);
            }
        }
        match &result {
            Ok(_) => {
                inner.failed.remove(&job.key);
            }
            Err(_) if damaged_at.is_some() => {}
            Err(error) => {
                // A cancelled build (a closed project) isn't a failure: it may be built again.
                if !matches!(error, BuildError::Cancelled) {
                    inner.failed.insert(job.key.clone(), Instant::now());
                }
                if !replaced && let Some(pcm) = inner.ready.remove(&job.key) {
                    pcm.superseded.store(true, Ordering::Release);
                    self.generation.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
        // What the open project uses: every file this opening built or opened, and the ones still to come.
        let in_use: HashSet<String> = inner
            .used
            .values()
            .cloned()
            .chain(inner.ready.keys().map(|k| k.path.clone()))
            .chain(inner.building.keys().map(|k| k.path.clone()))
            .chain(inner.queue.iter().map(|j| j.key.path.clone()))
            .collect();
        drop(inner);
        self.changed.notify_all();
        if let Some(at) = damaged_at {
            self.emit(MediaEvent::AudioDamaged {
                path: job.key.path.clone(),
                at_us: (at as i128 * 1_000_000 / i128::from(RATE)) as i64,
            });
        }
        if result.is_ok() {
            self.emit(MediaEvent::Changed {
                path: job.key.path.clone(),
                kind: MediaKind::Peaks,
            });
            if let Some(jobs) = self.evicts.get() {
                tidy::tidy(&self.dir, false, DISK_BUDGET, || {
                    protected_stems(&in_use, jobs.as_deref())
                });
            }
        }
        result
    }
}
