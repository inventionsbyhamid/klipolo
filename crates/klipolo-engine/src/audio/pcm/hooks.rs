//! Tests only: a hook every build of one file calls with each decoded block, before taking it, so a test can hold a
//! build block by block or cancel it at a given block, without timing races; and `Held`, a holder built on it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::lock;

/// Called with the block's number in its build (0, 1, …) on the build's own thread.
pub(super) type Hook = Arc<dyn Fn(u64) + Send + Sync>;

static HOOKS: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());

/// Builds of `path` call `hook` with each block until the returned guard drops.
pub(super) fn on_blocks(path: &str, hook: impl Fn(u64) + Send + Sync + 'static) -> Unhook {
    let mut hooks = lock(&HOOKS);
    hooks.retain(|(p, _)| p != path);
    hooks.push((path.to_owned(), Arc::new(hook)));
    Unhook(path.to_owned())
}

/// The hook builds of `path` call, if a test set one.
pub(super) fn find(path: &str) -> Option<Hook> {
    lock(&HOOKS)
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, h)| h.clone())
}

/// Removes its file's hook when dropped.
pub(super) struct Unhook(String);

impl Drop for Unhook {
    fn drop(&mut self) {
        lock(&HOOKS).retain(|(p, _)| *p != self.0);
    }
}

/// How long a held block takes.
pub(super) const BLOCK: Duration = Duration::from_millis(20);

/// Polls `ok` every 2 ms, for 30 s at most (a failing test never hangs).
pub(super) fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// One file's builds, held: each block takes 20 ms while `slow`, and blocks from `gate` on wait for it to be raised
/// (30 s at most each).
pub(super) struct Held {
    /// Blocks entered so far (the highest block's number + 1).
    pub(super) entered: AtomicU64,
    gate: AtomicU64,
    slow: AtomicBool,
}

impl Held {
    pub(super) fn new(path: &str, gate: u64) -> (Arc<Held>, Unhook) {
        let held = Arc::new(Held {
            entered: AtomicU64::new(0),
            gate: AtomicU64::new(gate),
            slow: AtomicBool::new(true),
        });
        let h = held.clone();
        let unhook = on_blocks(path, move |n| {
            h.entered.fetch_max(n + 1, Ordering::SeqCst);
            if h.slow.load(Ordering::SeqCst) {
                std::thread::sleep(BLOCK);
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            while n >= h.gate.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        (held, unhook)
    }

    /// Waits until the build is held at block `gate`.
    pub(super) fn wait_at(&self, gate: u64) {
        wait_until(&format!("block {gate}"), || {
            self.entered.load(Ordering::SeqCst) > gate
        });
    }

    /// Lets the build on to block `gate`, and waits until it is held there.
    pub(super) fn open_to(&self, gate: u64) {
        self.gate.store(gate, Ordering::SeqCst);
        self.wait_at(gate);
    }

    /// Lets the build run at full speed to its end.
    pub(super) fn release(&self) {
        self.slow.store(false, Ordering::SeqCst);
        self.gate.store(u64::MAX, Ordering::SeqCst);
    }
}
