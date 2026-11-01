//! A screen pipe's stream as the sound pipe that shares it sees it (macOS's app and game sound
//! come on the screen's ScreenCaptureKit stream). The screen pipe reports every outcome of its start, and the sound
//! pipe's start waits for that outcome, not for a time of its own: a screen that fails before its stream runs (no
//! screen-recording permission on a first game take, the app gone, every encoder refused) fails its sound at once, so
//! the take refuses or goes on within a moment instead of waiting in "Starting". A screen waiting on macOS's monthly
//! prompt keeps its sound waiting with it, however long the person takes. The person's Stop ends the wait.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::pipe::{PipeError, StopFlag};

/// How often a waiting sound pipe looks at the person's Stop.
const STOP_LOOK: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
pub struct SharedStream {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum State {
    /// Not started, being started, or stopped.
    #[default]
    Idle,
    Running,
    Failed(PipeError),
}

impl SharedStream {
    /// The screen's start began (an earlier outcome no longer counts), or the screen stopped.
    pub fn idle(&self) {
        self.set(State::Idle);
    }

    pub fn running(&self) {
        self.set(State::Running);
    }

    /// The screen's start failed with `error`.
    pub fn failed(&self, error: PipeError) {
        self.set(State::Failed(error));
    }

    fn set(&self, to: State) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = to;
        self.changed.notify_all();
    }

    /// Waits for the screen's start: Ok once the stream runs, the screen's error once it failed, or an error once the
    /// person stopped. `backstop` only guards against a start that never reports (every one does).
    pub fn wait(&self, stopping: &StopFlag, backstop: Duration) -> Result<(), PipeError> {
        let until = Instant::now() + backstop;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match &*state {
                State::Running => return Ok(()),
                State::Failed(e) => return Err(e.clone()),
                State::Idle => {}
            }
            if stopping.is_set() {
                return Err(PipeError::Os(
                    "stopped before the screen's stream ran".into(),
                ));
            }
            let Some(left) = until.checked_duration_since(Instant::now()) else {
                return Err(PipeError::Os("the screen's stream never started".into()));
            };
            state = (self.changed.wait_timeout(state, left.min(STOP_LOOK)))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn the_sound_waits_for_the_screen_s_outcome_and_the_person_s_stop() {
        let flag = Arc::new(AtomicBool::new(false));
        let stopping = StopFlag::new(flag.clone());
        // The screen fails before its stream: the sound's wait ends at once with the screen's error.
        let stream = Arc::new(SharedStream::default());
        stream.failed(PipeError::PermissionDenied);
        let at = Instant::now();
        assert_eq!(
            stream.wait(&stopping, Duration::from_secs(60)),
            Err(PipeError::PermissionDenied)
        );
        assert!(at.elapsed() < Duration::from_millis(50));
        // A slow start (the screen on macOS's prompt) keeps the sound waiting until it runs.
        stream.idle();
        let screen = stream.clone();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            screen.running();
        });
        assert_eq!(stream.wait(&stopping, Duration::from_secs(60)), Ok(()));
        late.join().unwrap();
        // The person's Stop ends a wait within a look.
        stream.idle();
        let stop = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            stop.store(true, std::sync::atomic::Ordering::Release);
        });
        let at = Instant::now();
        assert!(stream.wait(&stopping, Duration::from_secs(60)).is_err());
        assert!(
            at.elapsed() < Duration::from_millis(400),
            "{:?}",
            at.elapsed()
        );
    }
}
