//! What scene text counts about itself: every layout (shaping) in the process and on
//! this thread, and every build that ran anywhere but the text build thread. Diagnostics, like `gpu_objects_made`: a
//! warm frame lays out nothing, and the render thread builds nothing.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// The scene text build thread's name (`text_worker.rs`).
pub(super) const BUILD_THREAD: &str = "klipolo-scene-text";

static LAYOUTS: AtomicU64 = AtomicU64::new(0);
static ELSEWHERE: AtomicU64 = AtomicU64::new(0);
thread_local! {
    static HERE: Cell<u64> = const { Cell::new(0) };
}

/// How many times scene texts have been laid out in this process.
pub fn text_layout_calls() -> u64 {
    LAYOUTS.load(Ordering::Relaxed)
}

/// How many times scene texts have been laid out on the calling thread (a test's own, whatever its siblings do).
pub fn text_layout_calls_on_this_thread() -> u64 {
    HERE.with(Cell::get)
}

/// How many scene texts were built on a thread other than the text build thread (the render thread should build
/// none; `TextProbe`, a test's CPU copy, builds on its own).
pub fn text_builds_on_other_threads() -> u64 {
    ELSEWHERE.load(Ordering::Relaxed)
}

pub(super) fn count_layout() {
    LAYOUTS.fetch_add(1, Ordering::Relaxed);
    HERE.with(|c| c.set(c.get() + 1));
}

pub(super) fn count_build() {
    if std::thread::current().name() != Some(BUILD_THREAD) {
        ELSEWHERE.fetch_add(1, Ordering::Relaxed);
    }
}
