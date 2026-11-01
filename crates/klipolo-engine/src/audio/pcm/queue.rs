//! The store's build calls: a build on the calling thread (which joins a build of the same file in flight, and which
//! its caller can cancel), the plan's queue for the build thread (which Sync can pause), what is building, and waiting
//! for a queued file.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use klipolo_model::time::Us;

use super::build::{Progress, RETARGET_AHEAD};
use super::{BuildError, CHUNK, Inner, Job, Pcm, PcmKey, PcmStore, RETRY_AFTER, lock};
use crate::audio::mix::to_frame;
use crate::audio::plan::{AudioPlan, AudioSource};

impl PcmStore {
    /// Decodes (or opens the cached copy of) one file on the calling thread, after any build of it that is already
    /// running: `build_cancellable` with a flag that is never set. Blocking: export and tests.
    pub fn build(&self, key: &PcmKey) -> Result<Arc<Pcm>, BuildError> {
        self.build_cancellable(key, &AtomicBool::new(false))
    }

    /// `build`, cancellable and never waiting without a timeout:
    /// - when a build of this file is in flight (the store's worker, or another caller), it waits for that build in
    ///   50 ms slices, re-checking `cancel`, and marks it urgent (the worker raises its thread to user-initiated until
    ///   the build ends). A cancel returns `Cancelled` at once, its mark cleared, and leaves that build running
    ///   (playback needs it). A close that cancels that build returns `Cancelled` too;
    /// - otherwise it decodes on the calling thread, checking `cancel` (and a close) every block (≤ 50 ms of work).
    ///
    /// A cancelled build of its own discards its temporary files; the next build starts over.
    pub fn build_cancellable(
        &self,
        key: &PcmKey,
        cancel: &AtomicBool,
    ) -> Result<Arc<Pcm>, BuildError> {
        let progress = {
            let mut inner = lock(&self.inner);
            loop {
                if let Some(pcm) = inner.ready.get(key).filter(|p| p.is_complete()) {
                    return Ok(pcm.clone());
                }
                if cancel.load(Ordering::Acquire) {
                    return Err(BuildError::Cancelled);
                }
                let Some(running) = inner.building.get(key).cloned() else {
                    break;
                };
                inner = self.join(inner, key, &running, cancel)?;
            }
            // A queued job for the file stays queued: the worker skips it while this build runs, and it goes once the
            // build ends unless its caller cancelled it (playback still needs the file then).
            let progress = Arc::new(Progress::default());
            inner.building.insert(key.clone(), progress.clone());
            progress
        };
        let job = Job {
            key: key.clone(),
            start_at: 0,
        };
        self.run_job_cancellable(&job, progress, cancel)
    }

    /// Waits, marked urgent, for `running` (the build of `key` in flight) to end, in 50 ms slices; `Cancelled` when
    /// `cancel` is set or a close cancelled that build. The mark is cleared before it returns.
    fn join<'a>(
        &'a self,
        mut inner: MutexGuard<'a, Inner>,
        key: &PcmKey,
        running: &Arc<Progress>,
        cancel: &AtomicBool,
    ) -> Result<MutexGuard<'a, Inner>, BuildError> {
        let _urgent = Urgent::mark(running);
        loop {
            if cancel.load(Ordering::Acquire) || running.cancel.load(Ordering::Acquire) {
                return Err(BuildError::Cancelled);
            }
            if !inner
                .building
                .get(key)
                .is_some_and(|p| Arc::ptr_eq(p, running))
            {
                return Ok(inner);
            }
            inner = self
                .changed
                .wait_timeout(inner, JOIN_SLICE)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// While true, the worker finishes the build it has started and starts no other; queued builds keep their place
    /// (Sync holds it while it listens).
    pub fn pause_queue(&self, paused: bool) {
        lock(&self.inner).paused = paused;
        self.changed.notify_all();
    }

    /// The files with a build in flight on this store, whoever runs it: its worker, a caller of `build`
    /// (transcription, an export on this store) or Sync. Sync counts the ones that aren't its own against its two
    /// decodes. One small allocation per call; Sync calls it every 50 ms at most.
    pub fn building(&self) -> Vec<String> {
        lock(&self.inner)
            .building
            .keys()
            .map(|k| k.path.clone())
            .collect()
    }

    /// Frames decoded so far by the build of `key` in flight (its `Progress.at`: where its current run writes next);
    /// None when none is.
    pub fn decoded(&self, key: &PcmKey) -> Option<u64> {
        lock(&self.inner)
            .building
            .get(key)
            .map(|p| p.at.load(Ordering::Relaxed))
    }

    /// Queues the plan's missing audio for the build thread: sources under `at_us` first (decoded from there),
    /// then the rest in timeline order. A build already running jumps to `at_us` when that is far from where it
    /// is decoding. A source that can't be decoded stays silent (and is logged).
    pub fn prepare(self: &Arc<Self>, plan: &AudioPlan, at_us: Us) {
        let mut order: Vec<Job> = Vec::new();
        let mut push = |s: &AudioSource, start_at: u64| {
            let key = PcmKey::of(s);
            if !order.iter().any(|j| j.key == key) {
                order.push(Job { key, start_at });
            }
        };
        for s in plan
            .sources
            .iter()
            .filter(|s| s.start_us <= at_us && at_us < s.end_us)
        {
            let at =
                to_frame(s.source_in_us) as f64 + to_frame(at_us - s.start_us) as f64 * s.step();
            // Decoding from the start reaches anything in the first 10 s at once anyway.
            let at = if at < 480_000.0 {
                0
            } else {
                at as u64 / CHUNK * CHUNK
            };
            push(s, at);
        }
        for s in &plan.sources {
            push(s, 0);
        }
        let now = Instant::now();
        let mut inner = lock(&self.inner);
        // Playhead-first holds for builds already running too: one decoding far from the playhead jumps there.
        for job in order.iter().filter(|j| j.start_at > 0) {
            let (Some(progress), Some(pcm)) =
                (inner.building.get(&job.key), inner.ready.get(&job.key))
            else {
                continue;
            };
            let at = progress.at.load(Ordering::Relaxed);
            if !pcm.is_ready(job.start_at)
                && (job.start_at < at || job.start_at > at + RETARGET_AHEAD)
            {
                progress.retarget.store(job.start_at + 1, Ordering::Release);
            }
        }
        // A file a caller (Sync) is building, its copy still growing, is queued all the same, so a cancel there leaves
        // it to the worker; the worker skips it while that build runs.
        let mut queue: Vec<Job> = order
            .into_iter()
            .filter(|j| {
                !inner.ready.get(&j.key).is_some_and(|p| p.is_complete())
                    && !inner.building.get(&j.key).is_some_and(|p| p.worker)
                    && !inner.damaged.contains(&j.key)
                    && inner
                        .failed
                        .get(&j.key)
                        .is_none_or(|at| now - *at >= RETRY_AFTER)
            })
            .collect();
        // Jobs queued earlier for files this plan doesn't use keep their turn after it.
        let old = std::mem::take(&mut inner.queue);
        let later: Vec<Job> = old
            .into_iter()
            .filter(|j| !queue.iter().any(|q| q.key == j.key))
            .collect();
        queue.extend(later);
        inner.queue = queue;
        if !inner.queue.is_empty() && !inner.worker {
            inner.worker = true;
            let store = self.clone();
            let spawned = std::thread::Builder::new()
                .name("klipolo-audio-pcm".into())
                .spawn(move || store.work());
            if let Err(error) = spawned {
                inner.worker = false;
                tracing::warn!(%error, "audio decoder thread");
            }
        }
        drop(inner);
        self.changed.notify_all();
    }

    /// The file's finished audio, waiting while it is queued or decoding; None if it failed, isn't queued, or
    /// `cancel` is set (checked a few times a second).
    pub fn wait_complete(&self, key: &PcmKey, cancel: &AtomicBool) -> Option<Arc<Pcm>> {
        let mut inner = lock(&self.inner);
        loop {
            if cancel.load(Ordering::Acquire) {
                return None;
            }
            match inner.ready.get(key) {
                Some(pcm) if pcm.is_complete() => return Some(pcm.clone()),
                _ if inner.building.contains_key(key)
                    || inner.queue.iter().any(|j| j.key == *key) => {}
                _ => return None,
            }
            inner = self
                .changed
                .wait_timeout(inner, Duration::from_millis(250))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A join re-checks its caller's flag this often.
const JOIN_SLICE: Duration = Duration::from_millis(50);

/// A caller waiting on a build: the build stays urgent while this lives (also when the wait unwinds).
struct Urgent<'a>(&'a Progress);

impl<'a> Urgent<'a> {
    fn mark(progress: &'a Progress) -> Self {
        progress.urgent.fetch_add(1, Ordering::AcqRel);
        Self(progress)
    }
}

impl Drop for Urgent<'_> {
    fn drop(&mut self) {
        self.0.urgent.fetch_sub(1, Ordering::AcqRel);
    }
}
