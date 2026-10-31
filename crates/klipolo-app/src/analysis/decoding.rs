//! Whether analysis is decoding a clip on this Mac (its sound, its levels, its small copy), for what must not decode
//! video at the same time: editing copies wait while it does, as analysis is the person's own action.

use std::sync::{Arc, Mutex};

use super::lock;

type Listener = Box<dyn Fn(bool) + Send + Sync>;

/// The listeners, and how many local steps run now (one at a time today).
#[derive(Default)]
pub(super) struct Decoding {
    listeners: Mutex<Vec<Listener>>,
    running: Mutex<usize>,
}

impl Decoding {
    pub(super) fn listen(&self, f: Listener) {
        lock(&self.listeners).push(f);
    }

    /// A local step starts: listeners hear `true` if none ran. It ends when the turn is dropped.
    pub(super) fn begin(self: &Arc<Self>) -> Turn {
        let mut running = lock(&self.running);
        *running += 1;
        if *running == 1 {
            self.tell(true);
        }
        Turn(self.clone())
    }

    fn tell(&self, on: bool) {
        for listener in lock(&self.listeners).iter() {
            listener(on);
        }
    }
}

/// One local step's hold: dropped, listeners hear `false` once no step runs.
pub(super) struct Turn(Arc<Decoding>);

impl Drop for Turn {
    fn drop(&mut self) {
        let mut running = lock(&self.0.running);
        *running = running.saturating_sub(1);
        if *running == 0 {
            self.0.tell(false);
        }
    }
}
