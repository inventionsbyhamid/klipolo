//! Export analyses the screens its plans read before its first frame: it says so
//! (`WorkerLine::Analysing`) and writes into the editor's folder; a pause stops it within a tick, keeping only whole
//! minutes, and the next run goes on from there; a screen analysed already is read, never decoded again.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use klipolo_cursor::activity::{CHUNK_TICKS, Tick, decode, encode_chunk, header, stored_duration};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::prepare::{Analysis, analyse, analyse_screens};
use klipolo_engine::export::protocol::{Lines, WorkerLine, parse_line};
use klipolo_engine::export::shared::{Control, WorkerError};
use klipolo_engine::export::worker::{self, Ended};
use klipolo_engine::media::activity::ActivityStore;
use klipolo_engine::media::key::{CacheKind, stem};
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::Project;
use klipolo_model::time::Us;
use klipolo_model::timeline::{ItemContent, ZoomTarget};

use crate::support::{bx, item, secs, zoom_track};
use crate::synthetic::{Log, Screen, blank_opener, opener};
use crate::worker::batch;
use crate::{NOW, add_take, click, glide, rest, write_take};

const S: Us = 1_000_000;
const HEADER_BYTES: u64 = 20;
const CHUNK_BYTES: u64 = 4 + 2 + 600 * 14 + 4;

/// The committed activity clip (`scripts/gen-capture-fixtures.sh`): 12 s of a 640 × 360 screen with a line typed.
fn clip() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/activity-12s-360p.mp4");
    assert!(path.exists(), "run scripts/gen-capture-fixtures.sh");
    path.display().to_string()
}

/// `video`'s activity file in `root`: finished, part-made or marked as one that can't be analysed.
fn activity_file(root: &Path, video: &str, ext: &str) -> PathBuf {
    let stem = stem(video, &CacheKind::Activity).unwrap();
    root.join(format!("{stem}.{ext}"))
}

/// One burned-in take of the clip under an Auto zoom: the cursor clicks, then rests while the line is typed.
fn clip_project(dir: &Path) -> Project {
    let mut p = Project::new("Analysing");
    p.id = "project".into();
    p.settings.fps = crate::FPS;
    let mut lines: Vec<_> = glide((0.5, 0.5), (0.3, 0.6), 0.0, 1.0).collect();
    lines.extend(click((0.3, 0.6), 1.0, 1.1));
    lines.extend(rest(0.3, 0.6, 1.0, 12.0));
    let take = write_take(dir, 1, &[], lines);
    add_take(
        &mut p,
        1,
        &clip(),
        12.0,
        (&take, CursorCapture::Burned),
        (0.0, 12.0),
    );
    let auto = ItemContent::Zoom {
        target: ZoomTarget::Auto { using: vec![] },
        scale: 2.0,
    };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "auto",
        0.0,
        12.0,
        bx(0.0, 0.0, 1.0, 1.0),
        auto,
    )])];
    p
}

/// Exports `p` into `dir` with the worker in this process, the editor's activity folder `root`: how it ended and what
/// it said. (Off a Mac it can't encode: it stops there, after the screens are analysed.)
async fn export_with(
    p: &Project,
    dir: &Path,
    root: &Path,
    control: Arc<Control>,
) -> (Result<Ended, WorkerError>, Vec<WorkerLine>) {
    let mut b = batch(p, dir, false);
    b.activity_dir = Some(root.to_path_buf());
    let job = ExportJob::create(&b, 1).unwrap();
    let mut out = Vec::new();
    let ended = worker::export(job, &mut Lines::new(&mut out), control).await;
    let lines = String::from_utf8(out)
        .unwrap()
        .lines()
        .filter_map(parse_line)
        .collect();
    (ended, lines)
}

fn analysing(lines: &[WorkerLine]) -> Vec<f32> {
    (lines.iter())
        .filter_map(|l| match l {
            WorkerLine::Analysing { done } => Some(*done),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn export_waits_for_missing_activity_chunks_and_reports_analysing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let p = clip_project(dir.path());
    let (ended, lines) = export_with(&p, &dir.path().join("first"), &root, Arc::default()).await;
    let said = analysing(&lines);
    assert!(!said.is_empty(), "{ended:?} {lines:?}");
    assert!(said.windows(2).all(|w| w[0] <= w[1]), "{said:?}");
    assert_eq!(said.last(), Some(&1.0));
    // After `Started`, and before the first frame.
    let started = lines
        .iter()
        .position(|l| matches!(l, WorkerLine::Started { .. }));
    let first = lines
        .iter()
        .position(|l| matches!(l, WorkerLine::Analysing { .. }));
    assert!(started.is_some() && started < first, "{lines:?}");
    let last = lines
        .iter()
        .rposition(|l| matches!(l, WorkerLine::Analysing { .. }));
    if let Some(frame) = lines
        .iter()
        .position(|l| matches!(l, WorkerLine::Progress { .. }))
    {
        assert!(last < Some(frame), "{lines:?}");
    }
    // The whole clip's activity is in the editor's folder.
    let bytes = std::fs::read(activity_file(&root, &clip(), "activity")).unwrap();
    let made = decode(&bytes, stored_duration(&bytes).unwrap());
    assert!(made.is_complete() && made.duration_us() > 11 * S);
    // Exported again: read from the folder, never analysed again.
    let (ended, lines) = export_with(&p, &dir.path().join("again"), &root, Arc::default()).await;
    assert!(analysing(&lines).is_empty(), "{ended:?} {lines:?}");
}

/// A run paused while it analyses the screens stops there and says it paused (the job pauses), keeping no
/// half-made minute; nothing of the export is written.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_paused_while_analysing_ends_paused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let p = clip_project(dir.path());
    let control = Arc::new(Control::default());
    control.pause();
    let (ended, lines) = export_with(&p, &dir.path().join("paused"), &root, control).await;
    assert!(matches!(ended, Ok(Ended::Paused)), "{ended:?} {lines:?}");
    assert!(!activity_file(&root, &clip(), "activity").exists());
    let part = activity_file(&root, &clip(), "activity.part");
    let len = std::fs::metadata(&part).map_or(0, |m| m.len());
    assert!(len <= HEADER_BYTES, "no half-made minute: {len}");
    assert!(
        !lines
            .iter()
            .any(|l| matches!(l, WorkerLine::Progress { .. } | WorkerLine::Error { .. }))
    );
}

/// The worker's stdout as lines, pausing `control` once the first minute is made.
struct PauseAtFirstMinute {
    control: Arc<Control>,
    at: Arc<Mutex<Option<Instant>>>,
    pending: Vec<u8>,
}

impl Write for PauseAtFirstMinute {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let minute = matches!(parse_line(&String::from_utf8_lossy(&line)), Some(WorkerLine::Analysing { done }) if done > 0.0);
            let mut at = self.at.lock().unwrap();
            if minute && at.is_none() {
                *at = Some(Instant::now());
                self.control.pause();
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pause_while_analysing_stops_within_a_tick_and_keeps_whole_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let video = dir.path().join("screen.mov");
    std::fs::write(&video, b"synthetic").unwrap();
    let video = video.display().to_string();
    // Three minutes at a millisecond a picture: about 0.6 s a minute.
    let screen = Screen {
        duration_us: 180 * S,
        per_frame: Duration::from_millis(1),
    };
    let log = Arc::new(Log::default());
    let store = ActivityStore::detached(root.clone(), opener(screen, log.clone()));
    let files = vec![(video.clone(), 180 * S)];
    let control = Arc::new(Control::default());
    let at = Arc::new(Mutex::new(None));
    let mut out = Lines::new(PauseAtFirstMinute {
        control: control.clone(),
        at: at.clone(),
        pending: Vec::new(),
    });
    let ended = analyse_screens(
        Some(store.clone()),
        files.clone(),
        &mut out,
        &control,
        NOW,
        &|_| {},
    )
    .await;
    let took = at.lock().unwrap().map(|at| at.elapsed());
    assert!(matches!(ended, Ok(None)), "stopped: {}", ended.is_ok());
    assert!(
        took.is_some_and(|t| t < Duration::from_millis(500)),
        "{took:?}"
    );
    assert_eq!(log.open_now(), 0, "its decoder let go");
    // Whole minutes only, nothing half-written, and nothing marked.
    let len = std::fs::metadata(activity_file(&root, &video, "activity.part"))
        .unwrap()
        .len();
    assert!(
        len > HEADER_BYTES && (len - HEADER_BYTES).is_multiple_of(CHUNK_BYTES),
        "{len}"
    );
    let kept = ((len - HEADER_BYTES) / CHUNK_BYTES) as Us;
    assert!(!activity_file(&root, &video, "activity.failed").exists());
    // Nothing is stuck: the next run goes on from there, decoding only the minutes left.
    let again = analyse_screens(
        Some(store),
        files,
        &mut Lines::new(Vec::new()),
        &Control::default(),
        NOW,
        &|_| {},
    )
    .await;
    let again = again.ok().flatten().expect("analysed");
    assert!(again.get(&video).is_some_and(|t| t.is_complete()));
    assert_eq!(log.opened_at(), vec![0, kept * 60 * S]);
}

#[test]
#[ignore = "real scale: a two-hour screen analysed already; run in release for the figure"]
fn a_two_hour_screen_analysed_already_is_read_not_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("activity");
    let video = dir.path().join("screen.mov");
    std::fs::write(&video, b"synthetic").unwrap();
    let video = video.display().to_string();
    // Its activity file as the editor's pass left it: 120 whole minutes, a few ticks of each changing.
    let mut bytes = Vec::new();
    header(secs(7_200.0), &mut bytes);
    let minute: Vec<Tick> = (0..CHUNK_TICKS)
        .map(|k| Tick {
            changed: (k % 7) as u16,
            ..Tick::default()
        })
        .collect();
    for i in 0..120 {
        encode_chunk(i, &minute, &mut bytes);
    }
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(activity_file(&root, &video, "activity"), &bytes).unwrap();
    let opens = Arc::new(AtomicUsize::new(0));
    let store = ActivityStore::detached(root, blank_opener(secs(7_200.0), opens.clone()));
    let never = Arc::new(AtomicBool::new(false));
    let said = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let progress = |_| {
        said.fetch_add(1, Ordering::SeqCst);
    };
    let ended = analyse(
        &store,
        &[(video.clone(), secs(7_200.0))],
        &never,
        &progress,
        NOW,
    );
    let took = started.elapsed();
    println!(
        "a two-hour screen analysed already: ready in {took:?} ({} KB read)",
        bytes.len() / 1024
    );
    let Ok(Analysis::Done(analysed)) = ended else {
        panic!("analysed");
    };
    assert!(analysed.get(&video).is_some_and(|t| t.is_complete()));
    assert_eq!(opens.load(Ordering::SeqCst), 0, "nothing decoded");
    assert_eq!(said.load(Ordering::SeqCst), 0, "nothing to say");
    assert!(took < Duration::from_millis(250), "{took:?}");
}
