//! Quitting during a take (Command Q, or Klipolo's last window closed, while a take is the recorder's): the exit is
//! held, the take is stopped as the person's own Stop would (one that recorded is saved and lands; one that hadn't
//! started ends with nothing), and Klipolo exits once the take has landed, or after `QUIT_WAIT_MS`, whichever comes
//! first. A take still saving then finishes in its own worker, in its folder.
//!
//! The wait belongs to that take: a quit something else holds on to (an export's "Keep exporting") leaves the next take
//! to hold the next quit. A second quit while it waits is held too; the one exit follows the landing.

use std::sync::{Arc, PoisonError};
use std::time::Duration;

use super::take::Recorder;

/// How long quitting waits for a take to land.
pub(super) const QUIT_WAIT_MS: u64 = 30_000;
/// How often the wait looks.
const LOOK: Duration = Duration::from_millis(50);

/// Quitting's hold on a take.
#[derive(Default)]
pub(super) struct Quit {
    /// The take quitting waits for.
    waiting: Option<String>,
    /// The take the wait gave up on: its quit goes ahead.
    let_go: Option<String>,
}

impl Recorder {
    /// Klipolo is asked to exit: true when the exit is held for the take (module doc), `exit` then called once, when
    /// it has landed or after `QUIT_WAIT_MS`. False with no take, or for the take the wait gave up on.
    pub(super) fn hold_quit(self: &Arc<Self>, exit: Box<dyn FnOnce() + Send>) -> bool {
        if !self.active() {
            return false;
        }
        let Some(id) = self.take_id() else {
            return false;
        };
        {
            let mut quit = self.quit.lock().unwrap_or_else(PoisonError::into_inner);
            if quit.let_go.as_ref() == Some(&id) {
                return false;
            }
            if quit.waiting.as_ref() == Some(&id) {
                return true;
            }
            quit.waiting = Some(id.clone());
        }
        tracing::info!(
            take = id,
            "recorder: Klipolo quits during a take, so it is stopped and lands first"
        );
        self.stop_running();
        let deadline = self.now_ms().saturating_add(QUIT_WAIT_MS);
        let me = self.clone();
        let waiting = id.clone();
        let spawned = std::thread::Builder::new()
            .name("klipolo-record-quit".into())
            .spawn(move || {
                while me.take_id().as_ref() == Some(&waiting) && me.now_ms() < deadline {
                    std::thread::sleep(LOOK);
                }
                me.end_wait(&waiting);
                exit();
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "recorder: quitting can't wait for the take, so it goes ahead");
            self.end_wait(&id);
            return false;
        }
        true
    }

    /// The wait for take `id` is over: landed, or let go (its quit goes ahead while it saves on, in its worker).
    fn end_wait(&self, id: &str) {
        let still = self.take_id().is_some_and(|take| take == id);
        let mut quit = self.quit.lock().unwrap_or_else(PoisonError::into_inner);
        quit.waiting = None;
        if still {
            tracing::warn!(
                take = id,
                "recorder: the take didn't land in time, so Klipolo quits while it saves"
            );
            quit.let_go = Some(id.to_owned());
        }
    }
}
