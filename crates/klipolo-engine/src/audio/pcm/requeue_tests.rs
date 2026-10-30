//! A build its caller (Sync) cancels leaves the file queued for the worker, so playback
//! that needed it isn't left silent; a build that ends serves the file's queued job; a close lets go of a pause
//! the opening took. Builds are held block by block through `hooks`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::hooks::{self, Held, wait_until};
use super::sync_tests::{complete, plan, wav};
use super::{BuildError, PcmStore, lock};

#[test]
fn a_build_its_caller_cancels_leaves_the_file_queued_for_playback() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 4), wav(dir.path(), "b.wav", 4));
    let store = PcmStore::new(dir.path().join("cache"));
    // Sync holds the queue and decodes a and b itself; playback wanted a before Sync began, and b while Sync decodes it.
    store.pause_queue(true);
    store.prepare(&plan(&[&a]), 0);
    let (held_a, _unhook_a) = Held::new(&a.path, 3);
    let (held_b, _unhook_b) = Held::new(&b.path, 3);
    let cancel = Arc::new(AtomicBool::new(false));
    let sync = |key| {
        let (store, cancel) = (store.clone(), cancel.clone());
        std::thread::spawn(move || store.build_cancellable(&key, &cancel).map(|_| ()))
    };
    let (sync_a, sync_b) = (sync(a.clone()), sync(b.clone()));
    held_a.wait_at(3);
    held_b.wait_at(3);
    store.prepare(&plan(&[&a, &b]), 0);
    cancel.store(true, Ordering::SeqCst);
    for built in [sync_a.join().unwrap(), sync_b.join().unwrap()] {
        assert!(matches!(built, Err(BuildError::Cancelled)), "{built:?}");
    }
    let queued = |key| lock(&store.inner).queue.iter().any(|j| j.key == key);
    assert!(
        queued(a.clone()) && queued(b.clone()),
        "both stay queued for playback"
    );
    held_a.release();
    held_b.release();
    store.pause_queue(false);
    wait_until("the worker builds both", || {
        complete(&store, &a) && complete(&store, &b)
    });
    assert!(lock(&store.inner).queue.is_empty());
}

#[test]
fn a_build_that_ends_serves_the_queued_job_and_the_worker_waits_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav(dir.path(), "a.wav", 2);
    let store = PcmStore::new(dir.path().join("cache"));
    // Every build of a counts its start and waits at block 3 until let go.
    let (starts, at_gate, gate) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
    );
    let _unhook = {
        let (starts, at_gate, gate) = (starts.clone(), at_gate.clone(), gate.clone());
        hooks::on_blocks(&a.path, move |n| {
            if n == 0 {
                starts.fetch_add(1, Ordering::SeqCst);
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            while n >= 3 && gate.load(Ordering::SeqCst) && Instant::now() < deadline {
                at_gate.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    };
    let caller = {
        let (store, a) = (store.clone(), a.clone());
        std::thread::spawn(move || store.build(&a).map(|p| p.is_complete()))
    };
    wait_until("the caller's build is held", || {
        at_gate.load(Ordering::SeqCst)
    });
    // Playback queues a while the caller builds it: the worker leaves it to that build (one decode, never two).
    store.prepare(&plan(&[&a]), 0);
    std::thread::sleep(Duration::from_millis(100));
    assert!(lock(&store.inner).queue.iter().any(|j| j.key == a));
    gate.store(false, Ordering::SeqCst);
    assert!(matches!(caller.join().unwrap(), Ok(true)));
    wait_until("the queued job is served", || {
        lock(&store.inner).queue.is_empty() && store.building().is_empty()
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(starts.load(Ordering::SeqCst), 1, "no second build ran");
}

#[test]
fn a_close_lets_go_of_the_pause() {
    let dir = tempfile::tempdir().unwrap();
    let a = wav(dir.path(), "a.wav", 2);
    let store = PcmStore::new(dir.path().join("cache"));
    store.pause_queue(true);
    store.close_project();
    // The next opening's playback builds at once: a pause that leaked from the last one doesn't hold it.
    store.prepare(&plan(&[&a]), 0);
    wait_until("the worker builds it", || complete(&store, &a));
}
