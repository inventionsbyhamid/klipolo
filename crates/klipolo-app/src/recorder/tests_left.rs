//! Takes an earlier run left behind land as Klipolo opens again: one saved, one cut off (finished from its files before
//! it is handed on), one still recording once its worker lets it go (the person told it is still going); each in a new
//! project, told as from the last session, and never twice. One added already, one of this run and an old one are left
//! alone. One that can't land is told once: a damaged one is never tried again, with a log line; one refused for now
//! is tried again at the next launch. Real fixture media through the app's own probe; nothing captures.

use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use klipolo_capture::journal::Entry;
use klipolo_capture::recover;
use klipolo_capture::session::{ImportTarget, SessionDir, SessionLock};
use klipolo_model::coded::CodedError;
use klipolo_model::timeline::ClipSource;

use super::env::Done;
use super::left::{self, AT_MOST, NOT_RECOVERED, WITHIN_MS};
use super::test_support::*;
use super::test_takes::*;
use super::types::{LeftOutcome, TakeImport};

/// When this run began, in the tests' own time.
const LAUNCHED_MS: u64 = 1_800_000_000_000;
/// An hour before it.
const HOUR_AGO_MS: u64 = LAUNCHED_MS - 3_600_000;
/// How often the tests look again at a take an earlier run's worker still records.
const WATCH_EVERY: Duration = Duration::from_millis(5);

/// The start screen, a recordings folder, and a sink that lands each take there as the app's does.
struct Launch {
    sessions: Arc<Sessions>,
    sink: FakeSink,
    landed: Arc<Mutex<Vec<Result<TakeImport, CodedError>>>>,
    /// Whether each take handed on to land was finished from its files already (its journal's `Recovered`).
    repaired: Arc<Mutex<Vec<bool>>>,
    root: tempfile::TempDir,
}

impl Launch {
    fn new() -> Self {
        let sessions = Arc::new(Sessions::start_screen());
        let landed = Arc::new(Mutex::new(Vec::new()));
        let repaired = Arc::new(Mutex::new(Vec::new()));
        let sink = FakeSink::default();
        let land = landing_into(sessions.clone(), landed.clone());
        let seen = repaired.clone();
        *sink.lands.lock().unwrap() = Some(Box::new(move |done: &Done| {
            seen.lock()
                .unwrap()
                .push(recover::ending(&done.dir).recovered);
            land(done)
        }));
        Self {
            sessions,
            sink,
            landed,
            repaired,
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    /// A take an earlier run saved, set up at `created_ms`.
    fn saved(&self, id: &str, recorded: Recorded, created_ms: u64) -> SessionDir {
        let (dir, _) = take_in(self.root(), id, recorded, ImportTarget::New, created_ms);
        write_saved(&dir, recorded);
        dir
    }

    /// A take an earlier run left cut off (its worker stopped while saving), set up at `created_ms`.
    fn cut_off(&self, id: &str, created_ms: u64) -> SessionDir {
        let (dir, _) = take_in(
            self.root(),
            id,
            Recorded::default(),
            ImportTarget::New,
            created_ms,
        );
        write_cut_off(&dir);
        dir
    }

    /// This run looks for takes left behind, as it opens: how many landed.
    fn recover(&self) -> usize {
        left::recover_left(
            self.root(),
            &self.sink,
            LAUNCHED_MS,
            LAUNCHED_MS,
            WATCH_EVERY,
        )
    }

    /// The takes handed on to land, by id.
    fn handed(&self) -> Vec<String> {
        let finished = self.sink.finished.lock().unwrap();
        finished.iter().map(|done| done.id.clone()).collect()
    }

    fn landed(&self) -> Vec<TakeImport> {
        let landed = self.landed.lock().unwrap();
        landed.iter().filter_map(|l| l.clone().ok()).collect()
    }

    /// What the person was told about takes that didn't land as `recorder-imported`, by take.
    fn told(&self) -> Vec<(String, LeftOutcome)> {
        self.sink.left.lock().unwrap().clone()
    }
}

fn told(take: &str, outcome: LeftOutcome) -> (String, LeftOutcome) {
    (take.to_owned(), outcome)
}

/// What `run` logs, as the app's log says it.
fn logged(run: impl FnOnce()) -> String {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || Log(writer.clone()))
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    String::from_utf8(buf.lock().unwrap().clone()).unwrap()
}

struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_take_left_saved_lands_in_a_new_project_from_the_last_session() {
    let launch = Launch::new();
    let dir = launch.saved("left-1", Recorded::default(), HOUR_AGO_MS);
    assert_eq!(launch.recover(), 1);
    let finished = launch.sink.finished.lock().unwrap().clone();
    assert_eq!(finished.len(), 1);
    let done = &finished[0];
    assert_eq!(done.target, ImportTarget::New);
    assert_eq!(done.project_file, None);
    assert!(!done.recover, "saved: nothing to finish from its files");
    assert!(done.earlier_run);
    let landed = launch.landed();
    assert_eq!(landed.len(), 1);
    assert!(
        landed[0].earlier_run,
        "the page says it is from the last session"
    );
    assert_eq!(
        launch.sessions.library_files(),
        ["Recording, Oct 31.klipolo"]
    );
    let project = launch.sessions.project();
    assert_eq!(
        project.timeline.main[0].source,
        ClipSource::Recording {
            id: "left-1".into()
        }
    );
    assert!(dir.imported_mark().is_file());
    assert!(launch.told().is_empty(), "the landing says it all");
    // Never twice: the next launch finds it added.
    assert_eq!(launch.recover(), 0);
    assert_eq!(launch.handed(), ["left-1"]);
}

#[test]
fn a_take_left_cut_off_is_finished_from_its_files_and_lands() {
    let launch = Launch::new();
    let dir = launch.cut_off("left-1", HOUR_AGO_MS);
    assert_eq!(launch.recover(), 1);
    let finished = launch.sink.finished.lock().unwrap().clone();
    assert!(finished.len() == 1 && finished[0].recover, "{finished:?}");
    // Finished from its files before it was handed on, so the landing itself does no repair.
    assert_eq!(*launch.repaired.lock().unwrap(), [true]);
    let landed = launch.landed();
    assert_eq!(landed.len(), 1, "{:?}", launch.landed.lock().unwrap());
    assert!(landed[0].earlier_run);
    assert_eq!(launch.sessions.project().recordings.len(), 1);
    assert!(dir.imported_mark().is_file());
    assert_eq!(launch.recover(), 0);
    assert_eq!(launch.handed(), ["left-1"]);
}

#[test]
fn a_take_added_already_is_never_added_again() {
    let launch = Launch::new();
    // Added when it was saved, as any take is.
    let dir = launch.saved("added", Recorded::default(), HOUR_AGO_MS);
    let done = super::env::Done {
        id: "added".into(),
        dir: dir.clone(),
        target: ImportTarget::New,
        project_file: None,
        recover: false,
        note: None,
        earlier_run: false,
    };
    launch.sessions.land(&done, None).unwrap();
    assert!(dir.imported_mark().is_file());
    // Added too, its mark lost: its journal's `Imported` line says so.
    let journaled = launch.saved("journaled", Recorded::default(), HOUR_AGO_MS);
    append(
        &journaled,
        &Entry::Imported {
            project: "elsewhere".into(),
        },
    );
    let projects = launch.sessions.library_files();
    assert_eq!(launch.recover(), 0);
    assert!(launch.handed().is_empty());
    assert_eq!(launch.sessions.library_files(), projects, "no project made");
}

#[test]
fn a_damaged_take_is_skipped_with_a_log_line() {
    let launch = Launch::new();
    // A take whose session.json can't be read.
    let unreadable = SessionDir::create(launch.root(), "unreadable").unwrap();
    std::fs::write(unreadable.config(), b"{ not a take").unwrap();
    // A take with nothing to edit: no picture.
    let empty = Recorded {
        picture: false,
        ..Recorded::default()
    };
    let nothing = launch.saved("nothing", empty, HOUR_AGO_MS);
    // And a good one beside them.
    launch.saved("good", Recorded::default(), HOUR_AGO_MS - 1);
    let mut landed = 0;
    let log = logged(|| landed = launch.recover());
    assert_eq!(landed, 1);
    assert!(
        log.contains("a take without a readable session.json is skipped"),
        "{log}"
    );
    assert!(
        log.contains("a take from an earlier run couldn't be added"),
        "{log}"
    );
    assert_eq!(launch.handed(), ["good", "nothing"], "oldest first");
    assert_eq!(launch.landed().len(), 1);
    assert_eq!(
        launch.sessions.library_files(),
        ["Recording, Oct 31.klipolo"],
        "no project for the one that didn't land"
    );
    // The person is told, its files stay where they are, and no launch tries it again.
    assert_eq!(launch.told(), [told("nothing", LeftOutcome::NotAdded)]);
    assert!(nothing.root.join(NOT_RECOVERED).is_file());
    assert!(!nothing.imported_mark().exists());
    assert!(nothing.root.join("mic").is_dir());
    assert_eq!(launch.recover(), 0);
    assert_eq!(launch.handed(), ["good", "nothing"]);
    assert_eq!(launch.told().len(), 1, "told once");
}

/// A cut-off take with nothing recorded in it is told once and never tried again either.
#[test]
fn a_cut_off_take_with_nothing_in_it_is_told_once_and_never_tried_again() {
    let launch = Launch::new();
    let (dir, _) = take_in(
        launch.root(),
        "empty",
        Recorded::default(),
        ImportTarget::New,
        HOUR_AGO_MS,
    );
    assert_eq!(launch.recover(), 0);
    assert_eq!(launch.told(), [told("empty", LeftOutcome::NotAdded)]);
    assert!(dir.root.join(NOT_RECOVERED).is_file());
    assert!(launch.sessions.library_files().is_empty());
    assert_eq!(launch.recover(), 0);
    assert_eq!(launch.told().len(), 1, "told once");
}

/// A take refused for now (the library takes no new project) is told, left unmarked, and lands at the next launch.
#[test]
fn a_take_refused_for_now_is_tried_again_at_the_next_launch() {
    let launch = Launch::new();
    let dir = launch.saved("left-1", Recorded::default(), HOUR_AGO_MS);
    launch.sessions.library_refuses(true);
    assert_eq!(launch.recover(), 0);
    assert_eq!(launch.told(), [told("left-1", LeftOutcome::NotAdded)]);
    assert!(!dir.root.join(NOT_RECOVERED).exists(), "not marked");
    assert!(!dir.imported_mark().exists());
    launch.sessions.library_refuses(false);
    assert_eq!(launch.recover(), 1);
    assert_eq!(launch.handed(), ["left-1", "left-1"]);
    assert!(dir.imported_mark().is_file());
    assert_eq!(launch.told().len(), 1);
}

/// What the person is told about a take that didn't land, and whether that lasts (the take is marked).
#[test]
fn a_take_that_did_not_land_is_told_and_marked_only_when_that_lasts() {
    let outcome = |code: &str| left::outcome_of(&CodedError::new(code));
    for lasting in [
        "capture.import.noVideo",
        "capture.import.videoUnreadable",
        "capture.recovered.failed",
    ] {
        assert_eq!(outcome(lasting), (LeftOutcome::NotAdded, true), "{lasting}");
    }
    for passing in [
        "capture.import.notAdded",
        "project.moved",
        "capture.error.os",
    ] {
        assert_eq!(
            outcome(passing),
            (LeftOutcome::NotAdded, false),
            "{passing}"
        );
    }
    // Added, its project not opened: said, never marked (its `imported` mark says it is added).
    assert_eq!(
        outcome("capture.import.addedNotOpened"),
        (LeftOutcome::AddedNotOpened, false)
    );
}

#[test]
fn only_an_earlier_runs_recent_takes_land_oldest_first() {
    let launch = Launch::new();
    // This run's own take (armed as Klipolo opened), and one from over a week ago.
    launch.saved("this-run", Recorded::default(), LAUNCHED_MS + 1);
    launch.saved("old", Recorded::default(), LAUNCHED_MS - WITHIN_MS - 1);
    for i in 0..AT_MOST + 1 {
        launch.saved(
            &format!("left-{i}"),
            Recorded::default(),
            HOUR_AGO_MS + i as u64,
        );
    }
    assert_eq!(launch.recover(), AT_MOST);
    // The newest `AT_MOST`, oldest first, so the newest shows last.
    let expected: Vec<String> = (1..=AT_MOST).map(|i| format!("left-{i}")).collect();
    assert_eq!(launch.handed(), expected);
    // The next launch takes the one left over.
    assert_eq!(launch.recover(), 1);
    assert_eq!(launch.handed().last().map(String::as_str), Some("left-0"));
}

#[test]
fn a_take_an_earlier_runs_worker_still_records_lands_once_it_is_saved() {
    let launch = Arc::new(Launch::new());
    let (dir, _) = take_in(
        launch.root(),
        "live",
        Recorded::default(),
        ImportTarget::New,
        HOUR_AGO_MS,
    );
    // An earlier run's worker records on.
    let worker = SessionLock::try_acquire(&dir).unwrap().expect("the lock");
    let watching = {
        let launch = launch.clone();
        std::thread::spawn(move || launch.recover())
    };
    std::thread::sleep(WATCH_EVERY * 10);
    assert!(launch.handed().is_empty(), "left to its worker");
    // The person hears it is still going, once.
    assert_eq!(launch.told(), [told("live", LeftOutcome::StillRecording)]);
    // The person stops it from the menu bar: it is saved, and its worker goes.
    write_saved(&dir, Recorded::default());
    drop(worker);
    assert_eq!(watching.join().unwrap(), 1);
    assert_eq!(launch.handed(), ["live"]);
    assert_eq!(launch.told().len(), 1);
    assert!(launch.landed()[0].earlier_run);
    assert!(dir.imported_mark().is_file());
}

/// The app's recorder looks once per run, on a thread of its own; a test run never looks.
#[test]
fn the_recorder_looks_for_takes_left_behind_once_per_run() {
    let rec = fixture();
    let sessions = Arc::new(Sessions::start_screen());
    let landed = Arc::new(Mutex::new(Vec::new()));
    *rec.sink.lands.lock().unwrap() = Some(landing_into(sessions.clone(), landed.clone()));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let (dir, _) = take_in(
        &rec.root,
        "left-1",
        Recorded::default(),
        ImportTarget::New,
        now - 60_000,
    );
    write_saved(&dir, Recorded::default());
    let looking = rec.state.recover_left().expect("a look");
    looking.join().unwrap();
    assert_eq!(landed.lock().unwrap().len(), 1);
    assert!(dir.imported_mark().is_file());
    assert!(rec.state.recover_left().is_none(), "once per run");

    let off = super::state::RecorderState::build(true, || unreachable!("no parts in a test run"));
    assert!(off.recover_left().is_none());
}
