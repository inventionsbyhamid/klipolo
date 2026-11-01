//! The supervisor's clock: twice a second while a take is the recorder's, a silent worker is asked to stop, then
//! killed (`supervise`). The app's recorder runs it on a thread; tests tick it themselves.

use std::sync::Arc;
use std::time::Duration;

use super::supervise::SupervisorAction;
use super::take::{Recorder, Slot};

/// How often the ticker ticks the supervisor.
const TICK: Duration = Duration::from_millis(500);

impl Recorder {
    /// The supervisor's clock: a silent worker is asked to stop, then killed. False once there is no take (the ticker
    /// then ends).
    pub fn tick(&self) -> bool {
        let now = self.now_ms();
        let mut slot = self.lock();
        let Some(take) = slot.take.as_mut() else {
            slot.ticking = false;
            return false;
        };
        if let Some(SupervisorAction::Kill { hard }) = take.sup.on_tick(now)
            && let Some(worker) = &take.worker
        {
            (worker.kill)(hard);
        }
        true
    }

    /// The ticker, while a take is the recorder's (the app's recorder; tests tick themselves).
    pub(super) fn start_ticker(self: &Arc<Self>, slot: &mut Slot) {
        if !self.env.ticker || slot.ticking {
            return;
        }
        slot.ticking = true;
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("klipolo-record-tick".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(TICK);
                    if !me.tick() {
                        break;
                    }
                }
            });
        if let Err(error) = started {
            slot.ticking = false;
            tracing::error!(%error, "recorder: the supervisor's ticker didn't start");
        }
    }
}
