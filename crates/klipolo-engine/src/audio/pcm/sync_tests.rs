//! Builds a caller can cancel, that never wait without a timeout; a joined worker build made urgent; what
//! is building; a paused queue. Builds are held block by block through `hooks` (20 ms a block, and gates at given
//! blocks), so nothing races the clock. Unit tests, since the hooks and the QoS seam exist only in test builds.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_model::project::Loudness;

use super::build::Progress;
use super::hooks::{self, BLOCK, Held, wait_until};
use super::tests::write_wav;
use super::{BuildError, PcmKey, PcmStore, lock};
use crate::audio::mix::RATE;
use crate::audio::plan::{AudioPlan, AudioSource};
use crate::qos::Qos;

/// `seconds` of mono WAV at `dir/name`.
pub(super) fn wav(dir: &Path, name: &str, seconds: u64) -> PcmKey {
    let path = dir.join(name);
    write_wav(&path, seconds);
    PcmKey {
        path: path.display().to_string(),
    }
}

/// A plan that plays `keys` from the start, so `prepare` queues them in this order.
pub(super) fn plan(keys: &[&PcmKey]) -> AudioPlan {
    AudioPlan {
        sources: keys
            .iter()
            .map(|k| AudioSource {
                path: k.path.clone(),
                start_us: 0,
                end_us: 4_000_000,
                source_in_us: 0,
                speed: 1.0,
                rate: 1.0,
                gain: 1.0,
                fade_in_us: 0,
                fade_out_us: 0,
                crossfade: Default::default(),
                seam: Default::default(),
                filter: None,
                centre_from: None,
                duck: None,
                line: None,
            })
            .collect(),
        loudness: Loudness::Off,
    }
}

fn running(store: &PcmStore, key: &PcmKey) -> Arc<Progress> {
    lock(&store.inner)
        .building
        .get(key)
        .cloned()
        .expect("a build in flight")
}

pub(super) fn complete(store: &PcmStore, key: &PcmKey) -> bool {
    store.ready(key).is_some_and(|p| p.is_complete())
}

fn qos(progress: &Progress) -> Vec<Qos> {
    lock(&progress.qos).clone()
}

/// The bytes of the one finished cache file in `dir`.
fn cache_bytes(dir: &Path) -> Vec<u8> {
    let file = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "pcm"))
        .expect("a cache file");
    std::fs::read(file).unwrap()
}

#[test]
fn a_build_cancelled_mid_file_stops_within_200_ms_and_the_next_build_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let key = wav(dir.path(), "a.wav", 2);
    let store = PcmStore::new(dir.path().join("cache"));
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled_at = Arc::new(Mutex::new(None::<Instant>));
    let unhook = {
        let (cancel, cancelled_at) = (cancel.clone(), cancelled_at.clone());
        hooks::on_blocks(&key.path, move |n| {
            std::thread::sleep(BLOCK);
            if n == 2 {
                *lock(&cancelled_at) = Some(Instant::now());
                cancel.store(true, Ordering::SeqCst);
            }
        })
    };
    let built = store.build_cancellable(&key, &cancel);
    let took = lock(&cancelled_at).expect("block 2 was reached").elapsed();
    assert!(
        matches!(built, Err(BuildError::Cancelled)),
        "{:?}",
        built.map(|_| ())
    );
    assert!(took < Duration::from_millis(200), "{took:?}");
    drop(unhook);
    // Nothing is left that could be read as a cache, nor kept as playable.
    let cache = dir.path().join("cache");
    let left: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(store.ready(&key).is_none() && store.building().is_empty());
    // The next build starts over and is whole: what a fresh store builds.
    let pcm = store.build(&key).unwrap();
    assert!(pcm.is_complete());
    assert_eq!(pcm.frames(), 2 * RATE as usize);
    PcmStore::new(dir.path().join("fresh")).build(&key).unwrap();
    assert!(cache_bytes(&cache) == cache_bytes(&dir.path().join("fresh")));
}

#[test]
fn cancel_while_the_worker_builds_the_same_file_returns_within_200_ms() {
    let dir = tempfile::tempdir().unwrap();
    let key = wav(dir.path(), "a.wav", 4);
    let store = PcmStore::new(dir.path().join("cache"));
    let (held, _unhook) = Held::new(&key.path, 3);
    store.prepare(&plan(&[&key]), 0);
    held.wait_at(3);
    let progress = running(&store, &key);
    let cancel = Arc::new(AtomicBool::new(false));
    let joiner = {
        let (store, key, cancel) = (store.clone(), key.clone(), cancel.clone());
        std::thread::spawn(move || {
            let built = store.build_cancellable(&key, &cancel);
            (built.map(|_| ()), Instant::now())
        })
    };
    wait_until("the join waits", || {
        progress.urgent.load(Ordering::SeqCst) > 0
    });
    let cancelled_at = Instant::now();
    cancel.store(true, Ordering::SeqCst);
    let (built, returned) = joiner.join().unwrap();
    assert!(matches!(built, Err(BuildError::Cancelled)), "{built:?}");
    let took = returned - cancelled_at;
    assert!(took < Duration::from_millis(200), "{took:?}");
    // The worker's build goes on to its end (playback needs it).
    assert_eq!(store.building(), [key.path.as_str()]);
    held.release();
    wait_until("the worker's build is complete", || {
        complete(&store, &key) && store.building().is_empty()
    });
}

#[test]
fn a_joined_build_is_urgent_until_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let key = wav(dir.path(), "a.wav", 4);
    let store = PcmStore::new(dir.path().join("cache"));
    let (held, _unhook) = Held::new(&key.path, 3);
    store.prepare(&plan(&[&key]), 0);
    held.wait_at(3);
    let progress = running(&store, &key);
    assert!(progress.worker && qos(&progress).is_empty());
    let joiner = {
        let (store, key) = (store.clone(), key.clone());
        std::thread::spawn(move || {
            store
                .build_cancellable(&key, &AtomicBool::new(false))
                .map(|p| p.is_complete())
        })
    };
    wait_until("the join marks the build urgent", || {
        progress.urgent.load(Ordering::SeqCst) > 0
    });
    // The worker raises its thread at its next block.
    held.open_to(6);
    assert_eq!(qos(&progress), [Qos::UserInitiated]);
    held.release();
    assert!(matches!(joiner.join().unwrap(), Ok(true)));
    assert_eq!(progress.urgent.load(Ordering::SeqCst), 0);
    // Back to utility once its build has ended.
    wait_until("the worker settles", || qos(&progress).len() == 2);
    assert_eq!(qos(&progress), [Qos::UserInitiated, Qos::Utility]);
}

/// A join Sync cancels mustn't leave the worker at user-initiated for the rest of a long build.
#[test]
fn a_cancelled_join_clears_urgent() {
    let dir = tempfile::tempdir().unwrap();
    let key = wav(dir.path(), "a.wav", 4);
    let store = PcmStore::new(dir.path().join("cache"));
    let (held, _unhook) = Held::new(&key.path, 3);
    store.prepare(&plan(&[&key]), 0);
    held.wait_at(3);
    let progress = running(&store, &key);
    let cancel = Arc::new(AtomicBool::new(false));
    let joiner = {
        let (store, key, cancel) = (store.clone(), key.clone(), cancel.clone());
        std::thread::spawn(move || store.build_cancellable(&key, &cancel).map(|_| ()))
    };
    wait_until("the join marks the build urgent", || {
        progress.urgent.load(Ordering::SeqCst) > 0
    });
    held.open_to(6);
    assert_eq!(qos(&progress), [Qos::UserInitiated]);
    cancel.store(true, Ordering::SeqCst);
    let built = joiner.join().unwrap();
    assert!(matches!(built, Err(BuildError::Cancelled)), "{built:?}");
    assert_eq!(
        progress.urgent.load(Ordering::SeqCst),
        0,
        "cleared before the join returned"
    );
    // The worker goes back to utility at its next block, while its build goes on.
    held.open_to(9);
    assert_eq!(qos(&progress), [Qos::UserInitiated, Qos::Utility]);
    assert_eq!(store.building(), [key.path.as_str()]);
    held.release();
    wait_until("the worker's build is complete", || {
        complete(&store, &key) && store.building().is_empty()
    });
    assert_eq!(qos(&progress), [Qos::UserInitiated, Qos::Utility]);
}

#[test]
fn building_lists_every_build_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 4), wav(dir.path(), "b.wav", 4));
    let store = PcmStore::new(dir.path().join("cache"));
    let (held_a, _unhook_a) = Held::new(&a.path, 3);
    let (held_b, _unhook_b) = Held::new(&b.path, 3);
    // The worker's build of a, and a caller's of b (transcription, an export on this store).
    store.prepare(&plan(&[&a]), 0);
    let caller = {
        let (store, b) = (store.clone(), b.clone());
        std::thread::spawn(move || store.build(&b).map(|p| p.is_complete()))
    };
    held_a.wait_at(3);
    held_b.wait_at(3);
    let mut listed = store.building();
    listed.sort();
    assert_eq!(listed, [a.path.clone(), b.path.clone()]);
    // What a held build has decoded grows block by block.
    let first = store.decoded(&a).expect("a's build is in flight");
    held_a.open_to(5);
    let later = store.decoded(&a).unwrap();
    assert!(first > 0 && later > first, "{first} then {later}");
    held_a.release();
    held_b.release();
    assert!(matches!(caller.join().unwrap(), Ok(true)));
    wait_until("both builds end", || {
        store.building().is_empty() && complete(&store, &a)
    });
    assert_eq!(store.decoded(&a), None);
}

#[test]
fn while_paused_the_worker_starts_no_new_build() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (wav(dir.path(), "a.wav", 2), wav(dir.path(), "b.wav", 2));
    let store = PcmStore::new(dir.path().join("cache"));
    let (held_a, _unhook_a) = Held::new(&a.path, 3);
    let (held_b, _unhook_b) = Held::new(&b.path, u64::MAX);
    held_b.release();
    store.prepare(&plan(&[&a, &b]), 0);
    held_a.wait_at(3);
    store.pause_queue(true);
    // The build it started goes on to its end…
    held_a.release();
    wait_until("a is complete", || {
        complete(&store, &a) && store.building().is_empty()
    });
    // …and it starts no other while paused; b keeps its place in the queue.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(held_b.entered.load(Ordering::SeqCst), 0);
    assert!(store.building().is_empty() && store.ready(&b).is_none());
    assert!(lock(&store.inner).queue.iter().any(|j| j.key == b));
    store.pause_queue(false);
    wait_until("b is built once the queue goes on", || complete(&store, &b));
}
