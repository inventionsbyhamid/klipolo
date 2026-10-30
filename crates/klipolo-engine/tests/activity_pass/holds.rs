//! Holds: a tap of Play keeps the idle decoder, an export or a take lets it go at
//! once, and a reopened part file shows its chunks while the pass waits.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::media::store::Hold;
use klipolo_engine::platform::activity::ActivityError;

use super::synthetic::{Log, Screen, opener};
use super::{S, activity_events, file, never, screen_file, stores, wait_until};

#[test]
fn it_waits_while_playback_or_an_export_runs() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let screen = Screen {
        duration_us: 120 * S,
        per_frame: Duration::from_millis(1),
    };
    let (thumbs, store) = stores(dir.path(), Some(opener(screen, log.clone())));
    let events = activity_events(&thumbs);
    thumbs.pause(Hold::Playback);
    store.want(std::slice::from_ref(&video));
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        log.opened_at().is_empty(),
        "nothing is decoded while the video plays"
    );
    assert!(store.pending());
    thumbs.resume(Hold::Playback);
    wait_until("chunk 0", || *events.lock().unwrap() >= 1);
    // An export starts: decoding stops and the decoder is let go at once.
    thumbs.pause(Hold::Export);
    let (at, asked) = (log.frames(), Instant::now());
    wait_until("the decoder is let go", || log.open_now() == 0);
    let took = asked.elapsed();
    assert!(took < Duration::from_millis(300), "{took:?}");
    assert!(
        log.frames() <= at + 1,
        "{} frames after the hold",
        log.frames() - at
    );
    // A take recording holds it too.
    thumbs.pause(Hold::Recording);
    thumbs.resume(Hold::Export);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(log.open_now(), 0);
    thumbs.resume(Hold::Recording);
    wait_until("complete", || {
        store.get(&video).is_some_and(|t| t.is_complete())
    });
    // Chunk 0 was kept: the pass resumed at chunk 1.
    assert_eq!(log.opened_at(), vec![0, 60 * S]);
    assert!(store.get(&video).unwrap().tick_at(30 * S).is_some());
}

/// Playback holds keep the pass's idle decoder a while (up to 10 s), so a tap of Play doesn't cost the chunk in
/// progress: no decoding meanwhile, and no reopen after.
#[test]
fn a_tap_of_play_keeps_the_idle_decoder() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let screen = Screen {
        duration_us: 120 * S,
        per_frame: Duration::from_millis(1),
    };
    let (thumbs, store) = stores(dir.path(), Some(opener(screen, log.clone())));
    let events = activity_events(&thumbs);
    store.want(std::slice::from_ref(&video));
    wait_until("chunk 0", || *events.lock().unwrap() >= 1);
    thumbs.pause(Hold::Playback);
    let at = log.frames();
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(
        log.open_now(),
        1,
        "the decoder is kept through a short play"
    );
    assert!(
        log.frames() <= at + 1,
        "and idle: {} frames",
        log.frames() - at
    );
    thumbs.resume(Hold::Playback);
    wait_until("complete", || {
        store.get(&video).is_some_and(|t| t.is_complete())
    });
    assert_eq!(log.opened_at(), vec![0], "never reopened");
}

/// An export or an editing copy that starts while a tap of Play holds the pass lets its decoder go at once, not when
/// the playback grace runs out (a new hold wakes the waiting pass).
#[test]
fn a_copy_arriving_during_play_lets_the_decoder_go_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let screen = Screen {
        duration_us: 120 * S,
        per_frame: Duration::from_millis(1),
    };
    let (thumbs, store) = stores(dir.path(), Some(opener(screen, log.clone())));
    let events = activity_events(&thumbs);
    store.want(std::slice::from_ref(&video));
    wait_until("chunk 0", || *events.lock().unwrap() >= 1);
    thumbs.pause(Hold::Playback);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(log.open_now(), 1, "kept through the tap of Play");
    thumbs.pause(Hold::Copy);
    let asked = Instant::now();
    wait_until("the decoder is let go", || log.open_now() == 0);
    let took = asked.elapsed();
    assert!(took < Duration::from_millis(300), "{took:?}");
    store.cancel_all();
}

/// The next opening shows the chunks a part file already has, before the pass may run again.
#[test]
fn a_reopened_part_file_shows_its_chunks_while_the_pass_waits() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let screen = Screen {
        duration_us: 120 * S,
        per_frame: Duration::ZERO,
    };
    let (_t, first) = stores(dir.path(), Some(opener(screen, Arc::default())));
    let cancel = never();
    let stopped = first.ensure(&video, &cancel, &|done| {
        if done >= 0.5 {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    assert!(
        matches!(stopped, Err(ActivityError::Cancelled)),
        "{stopped:?}"
    );
    assert!(file(dir.path(), &video, true).exists());
    let log = Arc::new(Log::default());
    let (thumbs, next) = stores(dir.path(), Some(opener(screen, log.clone())));
    thumbs.pause(Hold::Playback);
    next.want(std::slice::from_ref(&video));
    wait_until("chunk 0 shown", || {
        next.get(&video)
            .is_some_and(|t| t.complete_through() == 60 * S)
    });
    assert!(
        log.opened_at().is_empty(),
        "nothing decoded while the video plays"
    );
    next.cancel_all();
}
