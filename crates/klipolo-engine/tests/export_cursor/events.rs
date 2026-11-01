//! What the runner tells the UI while export analyses the screens: "Analysing the screen…"
//! (`ExportEvent::Analysing`), each whole percent once and all before the first frame's progress; a Cancel then leaves
//! no job; and the question about a screen that can't be read reaches the UI coded, once, waiting for the answer.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_engine::api::{CodedError, ExportEvent, Said};
use klipolo_engine::export::prepare::unreadable;
use klipolo_engine::export::runner::{Attempt, Launcher, Outcome, run_batch_with};
use tokio::sync::{mpsc, oneshot, watch};

use crate::fixture;
use crate::worker::batch;

/// A worker that says `lines`, then exits (unsuccessfully), or waits until it is killed (`hang`).
struct Scripted {
    lines: Vec<String>,
    hang: bool,
    launches: AtomicU32,
}

impl Launcher for Scripted {
    fn launch(&self, _job: &Path) -> Result<Attempt, String> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let (tx, lines) = mpsc::unbounded_channel();
        for line in &self.lines {
            tx.send(line.clone()).unwrap();
        }
        let (exit_tx, exit) = oneshot::channel();
        let held = Arc::new(Mutex::new(Some((tx, exit_tx))));
        let end =
            move |held: &Mutex<Option<(mpsc::UnboundedSender<String>, oneshot::Sender<bool>)>>| {
                if let Some((tx, exit_tx)) = held.lock().unwrap().take() {
                    drop(tx);
                    let _ = exit_tx.send(false);
                }
            };
        if !self.hang {
            end(&held);
        }
        Ok(Attempt {
            lines,
            exit,
            kill: Box::new(move || end(&held)),
            pause: Box::new(|| {}),
        })
    }
}

fn analysing(done: f32) -> String {
    format!(r#"{{"type":"analysing","done":{done}}}"#)
}

/// Runs the scripted worker on a planning-only project in `dir`; `stop_at` cancels when it says so.
async fn run(
    worker: &Scripted,
    dir: &Path,
    stop_at: impl Fn(&ExportEvent) -> bool + Send + Sync,
) -> (Outcome, Vec<ExportEvent>) {
    let p = fixture(&dir.join("takes"), false);
    let (stop, cancel) = watch::channel(false);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let emit = move |e: ExportEvent| {
        if stop_at(&e) {
            let _ = stop.send(true);
        }
        sink.lock().unwrap().push(e);
    };
    let outcome = run_batch_with(worker, batch(&p, dir, false), 1, cancel, &emit).await;
    let events = events.lock().unwrap().clone();
    (outcome, events)
}

#[tokio::test(flavor = "multi_thread")]
async fn analysing_reaches_the_ui_before_the_first_frame_and_a_cancel_then_leaves_no_job() {
    let dir = tempfile::tempdir().unwrap();
    let progress = r#"{"type":"progress","frame":3,"totalFrames":60,"fps":30.0,"segmentsDone":0,"segmentsTotal":2}"#;
    let worker = Scripted {
        lines: [
            analysing(0.0),
            analysing(0.004),
            analysing(0.5),
            analysing(1.0),
            progress.to_string(),
        ]
        .into(),
        hang: true,
        launches: AtomicU32::new(0),
    };
    let (outcome, events) = run(&worker, dir.path(), |e| {
        matches!(e, ExportEvent::Progress { .. })
    })
    .await;
    assert!(matches!(outcome, Outcome::Cancelled), "{outcome:?}");
    let said: Vec<f32> = (events.iter())
        .filter_map(|e| match e {
            ExportEvent::Analysing { done } => Some(*done),
            _ => None,
        })
        .collect();
    assert_eq!(said, [0.0, 0.5, 1.0], "each whole percent once");
    let last = events
        .iter()
        .rposition(|e| matches!(e, ExportEvent::Analysing { .. }));
    let frame = events
        .iter()
        .position(|e| matches!(e, ExportEvent::Progress { .. }));
    assert!(last < frame, "{events:?}");
    // Cancelled while it analyses: the job is gone (the minutes made stay in the editor's folder, not the job's).
    let dir = tempfile::tempdir().unwrap();
    let worker = Scripted {
        lines: vec![analysing(0.1)],
        hang: true,
        launches: AtomicU32::new(0),
    };
    let (outcome, events) = run(&worker, dir.path(), |e| {
        matches!(e, ExportEvent::Analysing { .. })
    })
    .await;
    assert!(matches!(outcome, Outcome::Cancelled), "{outcome:?}");
    assert!(events.contains(&ExportEvent::Analysing { done: 0.1 }));
    // A cancelled job leaves the list at once and its folder is deleted in the background (as a discard is: set aside,
    // then removed), so the folder is waited for.
    let root = dir.path().join(".jobs");
    assert!(
        klipolo_engine::export::jobs::list_jobs(&root).is_empty(),
        "no job listed"
    );
    let left = || std::fs::read_dir(&root).map_or(0, |d| d.count());
    for _ in 0..500 {
        if left() == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(left(), 0, "no job left");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_question_about_a_screen_that_cant_be_read_reaches_the_ui_coded_and_waits() {
    let dir = tempfile::tempdir().unwrap();
    let asked = unreadable("/Volumes/Gone/screen.mov");
    let line = serde_json::json!({
        "type": "error",
        "code": "activityUnreadable",
        "message": asked.message,
        "retryable": false,
    });
    let worker = Scripted {
        lines: vec![line.to_string()],
        hang: false,
        launches: AtomicU32::new(0),
    };
    let (outcome, _) = run(&worker, dir.path(), |_| false).await;
    let said: CodedError = serde_json::from_str(&asked.message).unwrap();
    assert_eq!(said.code, "exports.activityUnreadable");
    assert!(
        matches!(&outcome, Outcome::Failed(Said::Coded(c)) if *c == said),
        "{outcome:?}"
    );
    assert_eq!(
        worker.launches.load(Ordering::SeqCst),
        1,
        "not retried: it waits for the answer"
    );
}
