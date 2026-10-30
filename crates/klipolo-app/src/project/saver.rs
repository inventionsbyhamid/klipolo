//! Saves the open project in the background: 2 s after the last change (a burst of edits is one save), at once when
//! asked (`flush`: closing, switching, quitting), and again after a failure (with a growing pause). The person hears
//! when saves start failing and when they work again (`SaveStatus`, the `project-save` event), once each.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use klipolo_model::project::Project;
use klipolo_platform::durable::{Durability, Os, Steps};
use serde::Serialize;

use super::disk::{SaveError, save_with};

/// How long the edits pause before they are saved.
pub(crate) const PAUSE: Duration = Duration::from_secs(2);
/// The first wait before a failed save is tried again; it doubles up to `MAX_BACKOFF`.
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Where saves are written: the real disk, or a test's.
pub(crate) type Disk = Arc<dyn Steps + Send + Sync>;
/// Where save failures, and the save that works again after them, are told.
pub(crate) type Report = Arc<dyn Fn(SaveStatus) + Send + Sync>;

/// Saves keep failing (`ok: false`, with what went wrong and what to do), or work again after that (`ok: true`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveStatus {
    pub(crate) ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

struct Queue {
    /// The newest project not saved yet, its file and its number.
    pending: Option<(PathBuf, Arc<Project>, u64)>,
    /// The number of the newest project handed in, and of the newest one written.
    submitted: u64,
    saved: u64,
    last_change: Instant,
    /// Saves asked for now (`flush`), and every save tried so far.
    wanted: u64,
    attempts: u64,
    /// Why the last try failed, while saves keep failing; and when to try again.
    failing: Option<SaveError>,
    retry_at: Option<Instant>,
    backoff: Duration,
    stop: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
    pause: Duration,
    disk: Disk,
    report: Mutex<Option<Report>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tell(&self, status: SaveStatus) {
        let report = self
            .report
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(report) = report {
            report(status);
        }
    }
}

/// The background saver of the open project (one thread, until it is dropped).
pub(crate) struct Saver {
    shared: Arc<Shared>,
}

impl Default for Saver {
    fn default() -> Self {
        Self::new(PAUSE, Arc::new(Os), Arc::new(|_| {}))
    }
}

impl Saver {
    pub(crate) fn new(pause: Duration, disk: Disk, report: Report) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                pending: None,
                submitted: 0,
                saved: 0,
                last_change: Instant::now(),
                wanted: 0,
                attempts: 0,
                failing: None,
                retry_at: None,
                backoff: FIRST_BACKOFF,
                stop: false,
            }),
            changed: Condvar::new(),
            pause,
            disk,
            report: Mutex::new(Some(report)),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("klipolo-saver".into())
            .spawn(move || run(&worker))
            .expect("the saver's thread starts");
        Self { shared }
    }

    /// Where save failures and recoveries are told from now on.
    pub(crate) fn set_report(&self, report: Report) {
        *self
            .shared
            .report
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(report);
    }

    /// `project` is the newest to save, into `file`: written once the edits pause.
    pub(crate) fn submit(&self, file: PathBuf, project: Arc<Project>) {
        let mut queue = self.shared.lock();
        queue.submitted += 1;
        queue.pending = Some((file, project, queue.submitted));
        queue.last_change = Instant::now();
        self.shared.changed.notify_all();
    }

    /// Saves what is waiting now and waits until it is on the disk, at most `timeout`. Fails with why it couldn't be
    /// saved (it stays waiting, and is tried again later).
    pub(crate) fn flush(&self, timeout: Duration) -> Result<(), SaveError> {
        let deadline = Instant::now() + timeout;
        let mut queue = self.shared.lock();
        let target = queue.submitted;
        if queue.saved >= target {
            return Ok(());
        }
        let tried = queue.attempts;
        queue.wanted += 1;
        self.shared.changed.notify_all();
        loop {
            if queue.saved >= target {
                return Ok(());
            }
            if queue.attempts > tried
                && let Some(error) = &queue.failing
            {
                return Err(error.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(SaveError::timed_out());
            }
            queue = self
                .shared
                .changed
                .wait_timeout(queue, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
    }
}

/// The saver's thread: waits for a project to save and its moment (the pause, a flush, a retry), saves it outside the
/// lock, and tells when saving starts failing or works again.
fn run(shared: &Shared) {
    let mut queue = shared.lock();
    loop {
        if queue.stop {
            return;
        }
        let due = match (&queue.pending, queue.wanted > 0) {
            (None, _) => None,
            (Some(_), true) => Some(Instant::now()),
            (Some(_), false) => Some(match queue.retry_at {
                Some(retry) => retry,
                None => queue.last_change + shared.pause,
            }),
        };
        let now = Instant::now();
        match due {
            None => {
                queue = shared
                    .changed
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            Some(at) if at > now => {
                queue = shared
                    .changed
                    .wait_timeout(queue, at - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            Some(_) => {}
        }
        let Some((file, project, number)) = queue.pending.take() else {
            continue;
        };
        queue.wanted = 0;
        drop(queue);
        let saved = save_with(shared.disk.as_ref(), &file, &project, Durability::Ordered);
        queue = shared.lock();
        queue.attempts += 1;
        let told = match saved {
            Ok(()) => {
                queue.saved = queue.saved.max(number);
                queue.retry_at = None;
                queue.backoff = FIRST_BACKOFF;
                queue.failing.take().map(|_| SaveStatus {
                    ok: true,
                    message: None,
                })
            }
            Err(error) => {
                tracing::warn!(detail = error.detail(), "the project couldn't be saved");
                // A newer project waiting goes next; else this one is tried again after the back-off.
                if queue.pending.is_none() {
                    queue.pending = Some((file, project, number));
                }
                queue.retry_at = Some(Instant::now() + queue.backoff);
                queue.backoff = (queue.backoff * 2).min(MAX_BACKOFF);
                let first = queue.failing.is_none();
                let message = error.to_string();
                queue.failing = Some(error);
                first.then_some(SaveStatus {
                    ok: false,
                    message: Some(message),
                })
            }
        };
        shared.changed.notify_all();
        if let Some(status) = told {
            drop(queue);
            shared.tell(status);
            queue = shared.lock();
        }
    }
}
