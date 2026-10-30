//! The screen activity pass: each screen file's activity track, made in the background after
//! the sound and the thumbnails, in 60-second chunks a stop never loses, waiting while anything holds background work.
//! The FFmpeg decoder runs on the committed fixture (`scripts/gen-capture-fixtures.sh`); chunks, stops, holds and
//! cancels run on a synthetic screen (`synthetic`), so they are counted exactly.

mod support;

#[path = "activity_pass/failures.rs"]
mod failures;
#[path = "activity_pass/fixture.rs"]
mod fixture;
#[path = "activity_pass/holds.rs"]
mod holds;
#[path = "activity_pass/quiet.rs"]
mod quiet;
#[path = "activity_pass/synthetic.rs"]
mod synthetic;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_cursor::activity::{TICK_BYTES, decode};
use klipolo_engine::media::activity::{ActivityStore, wanted};
use klipolo_engine::media::key::{CacheKind, stem};
use klipolo_engine::media::store::ThumbStore;
use klipolo_engine::media::{MediaEvent, MediaKind};
use klipolo_engine::platform::activity::{ActivityError, Opener};
use klipolo_model::project::SourceRole;
use klipolo_model::time::Us;
use klipolo_platform::durable::Os;
use synthetic::{Log, Screen, blank_opener, opener};

const S: Us = 1_000_000;
const HEADER_BYTES: u64 = 20;
const CHUNK_BYTES: u64 = 4 + 2 + 600 * TICK_BYTES as u64 + 4;

/// The thumbnail store and the activity store after it in line, on `root`; the platform's decoder unless `opener`.
pub fn stores(root: &Path, opener: Option<Opener>) -> (Arc<ThumbStore>, Arc<ActivityStore>) {
    let thumbs = ThumbStore::new(root.join("thumbs"), Arc::new(Os));
    let activity = match opener {
        Some(o) => ActivityStore::attach_with(&thumbs, root.join("activity"), o),
        None => ActivityStore::attach(&thumbs, root.join("activity")),
    };
    (thumbs, activity)
}

/// A file for the synthetic decoder to "read": its cache name needs one on disk.
fn screen_file(dir: &Path) -> String {
    let path = dir.join("screen.mov");
    std::fs::write(&path, b"synthetic").unwrap();
    path.display().to_string()
}

/// The activity file of `video` under `root`: finished (`.activity`) or being written (`.activity.part`).
fn file(root: &Path, video: &str, part: bool) -> PathBuf {
    let stem = stem(video, &CacheKind::Activity).unwrap();
    let ext = if part { "activity.part" } else { "activity" };
    root.join("activity").join(format!("{stem}.{ext}"))
}

/// A cancel flag nobody sets.
pub fn never() -> Arc<AtomicBool> {
    Arc::default()
}

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Activity events, as the engine forwards them.
fn activity_events(thumbs: &ThumbStore) -> Arc<Mutex<usize>> {
    let seen = Arc::new(Mutex::new(0));
    let count = seen.clone();
    thumbs.set_sink(Arc::new(move |e| {
        if let MediaEvent::Changed {
            kind: MediaKind::Activity,
            ..
        } = e
        {
            *count.lock().unwrap() += 1;
        }
    }));
    seen
}

#[test]
fn it_writes_a_chunk_per_minute_and_resumes_after_a_stop() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let screen = Screen {
        duration_us: 120 * S,
        per_frame: Duration::ZERO,
    };
    let (_thumbs, store) = stores(dir.path(), Some(opener(screen, log.clone())));
    // Stopped once chunk 0 is written (its progress cancels).
    let cancel = never();
    let stopped = store.ensure(&video, &cancel, &|done| {
        if done >= 0.5 {
            cancel.store(true, Ordering::SeqCst);
        }
    });
    assert!(
        matches!(stopped, Err(ActivityError::Cancelled)),
        "{stopped:?}"
    );
    assert_eq!(
        len(&file(dir.path(), &video, true)),
        HEADER_BYTES + CHUNK_BYTES
    );
    let partial = store.get(&video).unwrap();
    assert_eq!(partial.complete_through(), 60 * S);
    let before = log.frames();
    // Started again: only chunk 1 is decoded, from its first picture (60 s) to the last (119.9 s).
    let track = store.ensure(&video, &never(), &|_| {}).unwrap();
    assert_eq!(log.opened_at(), vec![0, 60 * S]);
    assert_eq!(log.frames() - before, 600);
    assert!(track.is_complete() && track.tick_at(119_900_000).is_none());
    // Finished: renamed, and the same bytes as one pass that never stopped.
    let done = file(dir.path(), &video, false);
    assert!(!file(dir.path(), &video, true).exists());
    let last = 4 + 2 + 599 * TICK_BYTES as u64 + 4;
    assert_eq!(len(&done), HEADER_BYTES + CHUNK_BYTES + last);
    let other = tempfile::tempdir().unwrap();
    let again = screen_file(other.path());
    let (_t, fresh) = stores(other.path(), Some(opener(screen, Arc::default())));
    fresh.ensure(&again, &never(), &|_| {}).unwrap();
    let read = |p: &Path| std::fs::read(p).unwrap();
    assert!(read(&done) == read(&file(other.path(), &again, false)));
}

#[test]
fn a_cancel_stops_within_100_ms_and_keeps_written_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let log = Arc::new(Log::default());
    let screen = Screen {
        duration_us: 180 * S,
        per_frame: Duration::from_millis(1),
    };
    let (thumbs, store) = stores(dir.path(), Some(opener(screen, log.clone())));
    let events = activity_events(&thumbs);
    store.want(std::slice::from_ref(&video));
    wait_until("chunk 0", || *events.lock().unwrap() >= 1);
    let asked = Instant::now();
    store.cancel_all();
    wait_until("the pass stops", || !store.pending() && log.open_now() == 0);
    let took = asked.elapsed();
    assert!(took < Duration::from_millis(100), "{took:?}");
    let kept = decode(
        &std::fs::read(file(dir.path(), &video, true)).unwrap(),
        180 * S,
    );
    assert!(kept.complete_through() >= 60 * S, "chunk 0 is kept");
    assert!(!kept.is_complete());
}

#[test]
fn a_damaged_chunk_is_analysed_again() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let screen = Screen {
        duration_us: 150 * S,
        per_frame: Duration::ZERO,
    };
    let (_t, first) = stores(dir.path(), Some(opener(screen, Arc::default())));
    let whole = first.ensure(&video, &never(), &|_| {}).unwrap();
    let done = file(dir.path(), &video, false);
    let mut bytes = std::fs::read(&done).unwrap();
    // A tick of chunk 1 (its checksum no longer matches).
    bytes[(HEADER_BYTES + CHUNK_BYTES) as usize + 6 + 100 * TICK_BYTES] ^= 0x10;
    std::fs::write(&done, &bytes).unwrap();
    // The next opening reads the file, finds chunk 1 missing and makes only that one again.
    let log = Arc::new(Log::default());
    let (_t, next) = stores(dir.path(), Some(opener(screen, log.clone())));
    next.want(std::slice::from_ref(&video));
    wait_until("analysed again", || {
        next.get(&video).is_some_and(|t| t.is_complete()) && !next.pending()
    });
    assert_eq!(log.opened_at(), vec![60 * S]);
    assert_eq!(log.frames(), 601);
    let again = next.get(&video).unwrap();
    for i in 0..1_500 {
        assert_eq!(
            again.tick_at(i * 100_000),
            whole.tick_at(i * 100_000),
            "tick {i}"
        );
    }
    assert!(done.exists() && !file(dir.path(), &video, true).exists());
    // A finished file whose header can't be read (bit-rot, or a format that changed under the same cache name) is
    // made again from its start, once: never "busy" for ever.
    let mut bytes = std::fs::read(&done).unwrap();
    bytes[9] ^= 0x01;
    std::fs::write(&done, &bytes).unwrap();
    let log = Arc::new(Log::default());
    let (_t, third) = stores(dir.path(), Some(opener(screen, log.clone())));
    third.want(std::slice::from_ref(&video));
    wait_until("made again from its start", || {
        third.get(&video).is_some_and(|t| t.is_complete()) && !third.pending()
    });
    assert_eq!(log.opened_at(), vec![0]);
    assert!(done.exists() && !file(dir.path(), &video, true).exists());
}

/// A file that opens but never gives a picture can't be analysed: it fails after one open, not one a chunk.
#[test]
fn a_file_that_never_gives_a_picture_fails_after_one_open() {
    let dir = tempfile::tempdir().unwrap();
    let video = screen_file(dir.path());
    let opens = Arc::new(AtomicUsize::new(0));
    let (_t, store) = stores(dir.path(), Some(blank_opener(180 * S, opens.clone())));
    let ended = store.ensure(&video, &never(), &|_| {});
    assert!(
        matches!(ended, Err(ActivityError::Unsupported(_))),
        "{ended:?}"
    );
    assert_eq!(opens.load(Ordering::SeqCst), 1);
}

/// The feed asks for each screen file the main track plays, once; never a camera's.
#[test]
fn the_editor_asks_for_each_screen_file_the_main_track_plays() {
    let p = support::recording::recording_project(&[("camera", SourceRole::Camera, 0, 60.0, 0.0)]);
    assert_eq!(wanted(&p), vec!["/media/screen".to_string()]);
}
