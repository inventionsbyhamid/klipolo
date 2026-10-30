//! A screen export can't read now: it is tried again, then the export asks, naming it (Retry,
//! or export without it); it is never marked as one that can't be analysed, so the editor and later exports analyse it
//! once it reads. A pause while another pass holds the screen is a pause, never a failure.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use klipolo_engine::export::prepare::{Analysis, Retry, analyse, analyse_screens, unreadable};
use klipolo_engine::export::protocol::{ErrorCode, Lines};
use klipolo_engine::export::shared::Control;
use klipolo_engine::media::activity::ActivityStore;
use klipolo_engine::media::key::{CacheKind, stem};
use klipolo_model::time::Us;

use crate::NOW;
use crate::synthetic::{Screen, opener, unavailable_opener};

const S: Us = 1_000_000;
const SCREEN: Screen = Screen {
    duration_us: 120 * S,
    per_frame: Duration::ZERO,
};

/// A stand-in screen file in `dir` (the decoders here are synthetic).
fn screen(dir: &Path) -> String {
    let path = dir.join("screen.mov");
    std::fs::write(&path, b"synthetic").unwrap();
    path.display().to_string()
}

/// `video`'s activity file of kind `ext` in `root`.
fn activity_file(root: &Path, video: &str, ext: &str) -> PathBuf {
    root.join(format!(
        "{}.{ext}",
        stem(video, &CacheKind::Activity).unwrap()
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_screen_that_cant_be_read_now_is_tried_again_then_asked_about_never_marked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let video = screen(dir.path());
    let files = vec![(video.clone(), 120 * S)];
    let opens = Arc::new(AtomicUsize::new(0));
    let store = ActivityStore::detached(root.clone(), unavailable_opener(opens.clone()));
    let (control, mut out) = (Control::default(), Lines::new(Vec::new()));
    let asked = analyse_screens(
        Some(store.clone()),
        files.clone(),
        &mut out,
        &control,
        NOW,
        &|_| {},
    )
    .await;
    let question = asked.err().expect("it asks");
    assert_eq!(question.code, ErrorCode::ActivityUnreadable);
    assert_eq!(question.message, unreadable(&video).message);
    assert!(!question.retryable, "it waits for the answer");
    assert!(
        question.message.contains("screen.mov"),
        "{}",
        question.message
    );
    assert_eq!(
        opens.load(Ordering::SeqCst),
        4,
        "tried, then three more times"
    );
    assert!(!activity_file(&root, &video, "activity.failed").exists());
    // The person said to export without it: it is planned without, still never marked.
    let without = Retry {
        without: vec![video.clone()],
        ..NOW
    };
    let analysed = analyse_screens(
        Some(store),
        files.clone(),
        &mut out,
        &control,
        without,
        &|_| {},
    )
    .await;
    let analysed = analysed.ok().flatten().expect("planned without it");
    assert!(analysed.get(&video).is_none());
    assert!(!activity_file(&root, &video, "activity.failed").exists());
    // Once it reads, the next export analyses it.
    let store = ActivityStore::detached(root, opener(SCREEN, Arc::default()));
    let analysed = analyse_screens(Some(store), files, &mut out, &control, NOW, &|_| {}).await;
    assert!(
        analysed
            .ok()
            .flatten()
            .unwrap()
            .get(&video)
            .is_some_and(|t| t.is_complete())
    );
}

/// Another pass (another process) holds the screen's part file the whole time: asked about, never marked.
#[test]
fn a_screen_another_pass_holds_is_asked_about_not_marked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let video = screen(dir.path());
    std::fs::create_dir_all(&root).unwrap();
    let held = std::fs::File::create(activity_file(&root, &video, "activity.part")).unwrap();
    klipolo_platform::process::lock_exclusive(&held).unwrap();
    let store = ActivityStore::detached(root.clone(), opener(SCREEN, Arc::default()));
    let never = Arc::new(AtomicBool::new(false));
    let ended = analyse(&store, &[(video.clone(), 120 * S)], &never, &|_| {}, NOW);
    assert!(
        matches!(&ended, Ok(Analysis::Unreadable(p)) if *p == video),
        "asks"
    );
    assert!(!activity_file(&root, &video, "activity.failed").exists());
}

/// A pause while another pass holds the screen stops the analysis as a pause; nothing is marked.
#[test]
fn a_pause_while_another_pass_holds_the_screen_is_a_pause() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let video = screen(dir.path());
    std::fs::create_dir_all(&root).unwrap();
    let held = std::fs::File::create(activity_file(&root, &video, "activity.part")).unwrap();
    klipolo_platform::process::lock_exclusive(&held).unwrap();
    let store = ActivityStore::detached(root.clone(), opener(SCREEN, Arc::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    let pause = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        pause.store(true, Ordering::SeqCst);
    });
    let ended = analyse(&store, &[(video.clone(), 120 * S)], &cancel, &|_| {}, NOW);
    assert!(matches!(ended, Ok(Analysis::Stopped)), "a pause");
    assert!(!activity_file(&root, &video, "activity.failed").exists());
}

/// The activity folder itself can't be written (here: a file stands where it should be): asked about, not planned
/// without silently, never marked.
#[test]
fn an_activity_folder_that_cant_be_written_is_asked_about() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    std::fs::write(&root, b"not a folder").unwrap();
    let video = screen(dir.path());
    let store = ActivityStore::detached(root, opener(SCREEN, Arc::default()));
    let never = Arc::new(AtomicBool::new(false));
    let ended = analyse(&store, &[(video.clone(), 120 * S)], &never, &|_| {}, NOW);
    assert!(
        matches!(&ended, Ok(Analysis::Unreadable(p)) if *p == video),
        "asks"
    );
}

/// The answer is per screen. A second screen that can't be read asks too, naming it,
/// and a screen the person said to export without is tried once, never again three times.
#[tokio::test(flavor = "multi_thread")]
async fn each_screen_that_cant_be_read_asks_and_one_said_to_go_without_is_tried_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let [a, b] = ["screen-a.mov", "screen-b.mov"].map(|name| {
        let path = dir.path().join(name);
        std::fs::write(&path, b"synthetic").unwrap();
        path.display().to_string()
    });
    let files = vec![(a.clone(), 120 * S), (b.clone(), 120 * S)];
    let opens = Arc::new(AtomicUsize::new(0));
    let store = ActivityStore::detached(root.clone(), unavailable_opener(opens.clone()));
    let asked = std::sync::Mutex::new(Vec::new());
    let note = |path: &str| asked.lock().unwrap().push(path.to_owned());
    let run = |without: Vec<String>| {
        let (store, files, note) = (store.clone(), files.clone(), &note);
        async move {
            let retry = Retry { without, ..NOW };
            let mut out = Lines::new(Vec::new());
            analyse_screens(
                Some(store),
                files,
                &mut out,
                &Control::default(),
                retry,
                note,
            )
            .await
        }
    };
    let first = run(Vec::new()).await;
    assert!(
        first
            .err()
            .is_some_and(|q| q.message == unreadable(&a).message),
        "asks about A"
    );
    assert_eq!(opens.swap(0, Ordering::SeqCst), 4);
    // "Export without it" for A: A once, then B is asked about, named.
    let second = run(vec![a.clone()]).await;
    assert!(
        second
            .err()
            .is_some_and(|q| q.message == unreadable(&b).message),
        "asks about B"
    );
    assert_eq!(
        opens.swap(0, Ordering::SeqCst),
        1 + 4,
        "A once, B with its tries"
    );
    // Both answered: each tried once, both planned without, nothing marked.
    let third = run(vec![a.clone(), b.clone()]).await;
    let analysed = third.ok().flatten().expect("planned without both");
    assert!(analysed.is_empty());
    assert_eq!(opens.load(Ordering::SeqCst), 2);
    assert_eq!(
        *asked.lock().unwrap(),
        [a.clone(), b.clone()],
        "each question's screen, for its answer"
    );
    for video in [&a, &b] {
        assert!(!activity_file(&root, video, "activity.failed").exists());
    }
}

/// A screen said to go without is planned with the minutes already in its part file, as the
/// editor plans with them (here another pass holds the file after its first minute).
#[test]
fn a_screen_said_to_go_without_is_planned_with_the_minutes_already_made() {
    use klipolo_cursor::activity::{CHUNK_TICKS, Tick, encode_chunk, header};
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let video = screen(dir.path());
    std::fs::create_dir_all(&root).unwrap();
    let mut part = std::fs::File::create(activity_file(&root, &video, "activity.part")).unwrap();
    let mut bytes = Vec::new();
    header(120 * S, &mut bytes);
    encode_chunk(0, &vec![Tick::default(); CHUNK_TICKS as usize], &mut bytes);
    part.write_all(&bytes).unwrap();
    part.sync_all().unwrap();
    klipolo_platform::process::lock_exclusive(&part).unwrap();
    let store = ActivityStore::detached(root.clone(), opener(SCREEN, Arc::default()));
    let never = Arc::new(AtomicBool::new(false));
    let without = Retry {
        without: vec![video.clone()],
        ..NOW
    };
    let ended = analyse(
        &store,
        &[(video.clone(), 120 * S)],
        &never,
        &|_| {},
        without,
    );
    let Ok(Analysis::Done(analysed)) = ended else {
        panic!("planned");
    };
    let track = analysed.get(&video).expect("the minute already made");
    assert_eq!(track.complete_through(), 60 * S);
    assert!(!track.is_complete());
    assert!(!activity_file(&root, &video, "activity.failed").exists());
}
