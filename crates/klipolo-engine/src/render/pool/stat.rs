//! Whether a source's file is there, asked off the engine's task. Only a definite answer counts: not found
//! (or not a file) is missing, another error is unreadable, and a disk that hasn't answered yet (spinning up, a network
//! volume) is just not answering: nothing is said, and when the answer comes the caller's redraw signal fires, so a
//! paused still is drawn again with it. One stat per file is in flight at a time, whoever asks, so a
//! volume that hangs holds one blocking thread per file, never one per frame.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, watch};

use crate::media::key::SourceId;

/// What asking about a file found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileState {
    There,
    /// Not found, or not a file: the preview says it is missing.
    Missing,
    /// It answered with an error (no permission, a failing drive): it couldn't be read.
    Unreadable,
    /// No answer within the limit: the preview says nothing and is woken when it comes; export stops and names it.
    NotAnswering,
}

/// How a caller asks: how long it waits for the answer, and the signal fired when an answer it gave up on comes (the
/// preview's redraw).
#[derive(Clone, Debug, Default)]
pub(crate) struct Ask {
    /// None: as long as it takes.
    pub limit: Option<Duration>,
    pub wake: Option<Arc<Notify>>,
}

/// What a stat found, and the file's identity when it is there (size, time and number: an editing copy's record is
/// checked against it).
type Answer = (FileState, Option<SourceId>);

/// An answer that came after a caller gave up on it is kept this long for the frame its signal draws: asking again
/// could take as long again (a slow network share), and the frame would never hear it.
const LATE_KEPT: Duration = Duration::from_secs(5);

enum Entry {
    /// A stat in flight: when it was asked, its answer once it comes, and the signals to fire then (callers that gave
    /// up on it; none when nobody did).
    Asking {
        at: Instant,
        answer: watch::Receiver<Option<Answer>>,
        wake: Vec<Arc<Notify>>,
    },
    /// The answer of a stat a caller gave up on, and when it came.
    Late { answer: Answer, at: Instant },
}

/// The stats in flight, and the late answers kept, by path.
fn entries() -> &'static Mutex<HashMap<Arc<str>, Entry>> {
    static ENTRIES: OnceLock<Mutex<HashMap<Arc<str>, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(Mutex::default)
}

/// `path`'s state, waiting for its stat up to `ask.limit` after the stat was asked. A stat already in flight for `path`
/// is waited for, not asked again, so one that has hung past the limit answers `NotAnswering` at once; when it does
/// answer, `ask.wake` fires and the next caller with a limit takes that answer (for `LATE_KEPT`).
pub(crate) async fn file_state(path: &Arc<str>, ask: &Ask) -> FileState {
    file_id(path, ask).await.0
}

/// `file_state`, with the file's identity when it is there (the same stat: one per path in flight, whoever asks).
pub(crate) async fn file_id(path: &Arc<str>, ask: &Ask) -> Answer {
    let (asked_at, mut answer) = {
        let mut entries = entries().lock().unwrap_or_else(PoisonError::into_inner);
        match entries.get(path) {
            Some(Entry::Late { answer, at }) if ask.limit.is_some() && at.elapsed() < LATE_KEPT => {
                return *answer;
            }
            Some(Entry::Asking { at, answer, .. }) => (*at, answer.clone()),
            _ => {
                // Late answers nobody came back for go when the next stat starts (the map stays small).
                entries.retain(
                    |_, e| !matches!(e, Entry::Late { at, .. } if at.elapsed() >= LATE_KEPT),
                );
                let (tx, rx) = watch::channel(None);
                let at = Instant::now();
                let entry = Entry::Asking {
                    at,
                    answer: rx.clone(),
                    wake: Vec::new(),
                };
                entries.insert(path.clone(), entry);
                let path = path.clone();
                tokio::task::spawn_blocking(move || {
                    let answer = std::panic::catch_unwind(|| stat(&path))
                        .unwrap_or((FileState::Unreadable, None));
                    answered(path, answer, &tx);
                });
                (at, rx)
            }
        }
    };
    let answered = async { answer.wait_for(Option::is_some).await.ok().and_then(|s| *s) };
    let Some(limit) = ask.limit else {
        return answered.await.unwrap_or((FileState::NotAnswering, None));
    };
    // An answer already in is taken even when the time left is none (`timeout` polls before its deadline).
    let left = limit.saturating_sub(asked_at.elapsed());
    if let Ok(Some(answer)) = tokio::time::timeout(left, answered).await {
        return answer;
    }
    // Given up: fire `wake` when the answer comes, unless it just came (answers are sent under the lock).
    let mut entries = entries().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(answer) = *answer.borrow() {
        return answer;
    }
    if let (Some(Entry::Asking { wake, .. }), Some(signal)) = (entries.get_mut(path), &ask.wake)
        && !wake.iter().any(|w| Arc::ptr_eq(w, signal))
    {
        wake.push(signal.clone());
    }
    (FileState::NotAnswering, None)
}

/// The stat of `path` answered `answer`: its callers hear it, and if one gave up on it, the answer is kept for the
/// frame its signal draws.
fn answered(path: Arc<str>, answer: Answer, tx: &watch::Sender<Option<Answer>>) {
    let wake = {
        let mut entries = entries().lock().unwrap_or_else(PoisonError::into_inner);
        let _ = tx.send(Some(answer));
        match entries.remove(&path) {
            Some(Entry::Asking { wake, .. }) if !wake.is_empty() => {
                let at = Instant::now();
                entries.insert(path, Entry::Late { answer, at });
                wake
            }
            _ => Vec::new(),
        }
    };
    for signal in wake {
        signal.notify_one();
    }
}

fn stat(path: &str) -> Answer {
    #[cfg(any(test, feature = "trial"))]
    slow_down(path);
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => (FileState::There, Some(SourceId::of(&meta))),
        Ok(_) => (FileState::Missing, None),
        Err(error) if error.kind() == ErrorKind::NotFound => (FileState::Missing, None),
        Err(_) => (FileState::Unreadable, None),
    }
}

/// A test's slow disk: stats of these paths take this long (tests and the `trial` build only).
#[cfg(any(test, feature = "trial"))]
static SLOW: Mutex<Option<HashMap<Arc<str>, Duration>>> = Mutex::new(None);

#[cfg(any(test, feature = "trial"))]
fn slow_down(path: &str) {
    let delay = SLOW
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|slow| slow.get(path).copied());
    if let Some(delay) = delay {
        std::thread::sleep(delay);
    }
}

/// Test hook (tests and the `trial` build only): every stat of `path` takes `delay` (a disk spinning up), or as long as
/// it takes with `None`.
#[cfg(any(test, feature = "trial"))]
#[doc(hidden)]
pub fn slow_stats(path: Arc<str>, delay: Option<Duration>) {
    let mut slow = SLOW.lock().unwrap_or_else(PoisonError::into_inner);
    let slow = slow.get_or_insert_with(HashMap::new);
    match delay {
        Some(delay) => slow.insert(path, delay),
        None => slow.remove(&path),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ask(path: &Path) -> Arc<str> {
        path.display().to_string().into()
    }

    /// Only "not found" (or not a file) is missing; an error such as no permission is unreadable.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_a_file_not_found_is_missing() {
        use std::os::unix::fs::PermissionsExt;
        let patient = Ask::default();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("intro.mp4");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(file_state(&ask(&file), &patient).await, FileState::There);
        let gone = dir.path().join("gone.mp4");
        assert_eq!(file_state(&ask(&gone), &patient).await, FileState::Missing);
        assert_eq!(
            file_state(&ask(dir.path()), &patient).await,
            FileState::Missing
        );

        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let inside = locked.join("intro.mp4");
        std::fs::write(&inside, b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway: then there is nothing to check.
        if std::fs::metadata(&inside).is_err() {
            let ask_ = Ask {
                limit: Some(Duration::from_secs(5)),
                wake: None,
            };
            assert_eq!(
                file_state(&ask(&inside), &ask_).await,
                FileState::Unreadable
            );
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// An answer that comes after the caller gave up fires its signal once, and the frame that signal
    /// draws takes that answer instead of asking again (which would take as long again); later callers wait for the
    /// one stat in flight instead of starting another.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_answer_given_up_on_wakes_the_caller_and_is_kept_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let gone = ask(&dir.path().join("gone.mp4"));
        let slow = Duration::from_millis(400);
        slow_stats(gone.clone(), Some(slow));
        let signal = Arc::new(Notify::new());
        let preview = Ask {
            limit: Some(Duration::from_millis(100)),
            wake: Some(signal.clone()),
        };
        let started = Instant::now();
        assert_eq!(file_state(&gone, &preview).await, FileState::NotAnswering);
        // Asked again while it is out: no second stat, no second signal.
        assert_eq!(file_state(&gone, &preview).await, FileState::NotAnswering);
        tokio::time::timeout(Duration::from_secs(5), signal.notified())
            .await
            .expect("the signal fires once the stat answers");
        assert!(started.elapsed() >= slow, "{:?}", started.elapsed());
        // The stats are still slow: an answer in time can only be the one kept.
        assert_eq!(file_state(&gone, &preview).await, FileState::Missing);
        assert!(
            tokio::time::timeout(Duration::from_millis(600), signal.notified())
                .await
                .is_err(),
            "one signal per answer"
        );
        slow_stats(gone, None);
    }
}
