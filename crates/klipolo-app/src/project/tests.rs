//! The open project's edits, undo and redo, the engine feed, saving, and files that can't be read or saved.

use std::fs::File;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicI32;

use klipolo_model::Op;
use klipolo_platform::durable::{Durability, Os, Steps};

use super::saver::{Disk, SaveStatus};
use super::*;

pub(super) fn rename(name: &str) -> Transaction {
    Transaction {
        label: "Rename".into(),
        ops: vec![Op::Rename { name: name.into() }],
        origin: None,
        ..Default::default()
    }
}

/// A session with `name` open from `file` (its folder made), saving through `disk` after `pause`, its save reports
/// collected.
fn session_on(
    file: &Path,
    name: &str,
    pause: Duration,
    disk: Disk,
) -> (ProjectSessionState, Arc<StdMutex<Vec<SaveStatus>>>) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let reports = Arc::new(StdMutex::new(Vec::new()));
    let sink = reports.clone();
    let state = ProjectSessionState::with_saver(Saver::new(
        pause,
        disk,
        Arc::new(move |s| sink.lock().unwrap().push(s)),
    ));
    set_open(
        &state,
        Some(OpenProject {
            document: Document::new(Project::new(name)).unwrap(),
            file: file.to_path_buf(),
            read_only: false,
        }),
    )
    .unwrap();
    (state, reports)
}

/// [`session_on`] on the real disk, with a pause that never comes in a test: saves happen on flush.
fn session(file: &Path, name: &str) -> (ProjectSessionState, Arc<StdMutex<Vec<SaveStatus>>>) {
    session_on(file, name, NEVER, Arc::new(Os))
}

/// A pause that never comes in a test.
const NEVER: Duration = Duration::from_secs(600);

fn project_name(state: &ProjectSessionState) -> String {
    lock(state)
        .as_ref()
        .unwrap()
        .document
        .project()
        .name
        .clone()
}

/// Polls `done` for up to 10 s (a loaded machine is slow, never wrong).
fn eventually(done: impl Fn() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

#[test]
fn apply_returns_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = session(&dir.path().join("p.klipolo"), "Before");
    let before = lock(&state).as_ref().unwrap().document.project().clone();
    let change = with_open(&state, |open| apply_and_commit(open, &rename("After"))).unwrap();
    assert_eq!(change.patch.name.as_deref(), Some("After"));
    assert!(change.patch.clips.is_empty() && change.patch.items.is_empty());
    assert!(change.patch.main.is_none() && change.patch.tracks.is_none());
    assert_eq!((change.base_revision, change.history.revision), (0, 1));
    let now = lock(&state).as_ref().unwrap().document.project().clone();
    assert_eq!(
        klipolo_model::patch::apply(&before, &change.patch).unwrap(),
        now
    );
    // An empty batch changes nothing and says so on the current revision.
    let empty = Transaction {
        ops: Vec::new(),
        ..rename("")
    };
    let nothing = with_open(&state, |open| apply_and_commit(open, &empty)).unwrap();
    assert!(nothing.patch.is_empty());
    assert_eq!((nothing.base_revision, nothing.history.revision), (1, 1));
    let whole = with_open(&state, |open| Ok(open.document.snapshot())).unwrap();
    assert_eq!((whole.project, whole.history.revision), (now, 1));
}

/// A label's params reach the undo step, and the redo step after an undo, so the menu can word "Undo Rename to
/// “Intro”".
#[test]
fn an_edits_label_params_reach_its_undo_and_redo_steps() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = session(&dir.path().join("p.klipolo"), "Before");
    let params: klipolo_model::history::LabelParams =
        [("name".to_string(), "Intro".into())].into_iter().collect();
    let tx = Transaction {
        label: "edit.rename".into(),
        params: Some(params.clone()),
        ..rename("Intro")
    };
    let change = with_open(&state, |open| apply_and_commit(open, &tx)).unwrap();
    assert_eq!(change.history.undo_label.as_deref(), Some("edit.rename"));
    assert_eq!(change.history.undo_params.as_ref(), Some(&params));
    let undone = with_open(&state, |open| step(open, false)).unwrap();
    assert_eq!(undone.history.redo_label.as_deref(), Some("edit.rename"));
    assert_eq!(undone.history.redo_params.as_ref(), Some(&params));
}

#[test]
fn edits_reach_the_engine_feed_before_the_command_returns() {
    let dir = tempfile::tempdir().unwrap();
    let state = ProjectSessionState::default();
    let mut feed = state.subscribe();
    let project = Project::new("Before");
    let id = project.id.clone();
    let file = dir.path().join("Before.klipolo");
    set_open(
        &state,
        Some(OpenProject {
            document: Document::new(project).unwrap(),
            file: file.clone(),
            read_only: false,
        }),
    )
    .unwrap();
    assert_eq!(
        feed.borrow_and_update().as_ref().map(|f| f.revision),
        Some(0)
    );
    with_open(&state, |open| apply_and_commit(open, &rename("After"))).unwrap();
    let published = feed.borrow_and_update().clone().unwrap();
    assert_eq!(
        (
            published.project_id.as_str(),
            published.revision,
            published.project.name.as_str()
        ),
        (id.as_str(), 1, "After")
    );
    // The engine gets the document's own revision, not a copy.
    let shared = lock(&state).as_ref().unwrap().document.shared();
    assert!(Arc::ptr_eq(&published.project, &shared));
    drop(shared);
    assert_eq!(published.opened, 1, "edits keep the opening");
    // Opening the same project again restarts revisions at 0, so it must be a new opening (engine session).
    let again = Document::new(published.project.as_ref().clone()).unwrap();
    set_open(
        &state,
        Some(OpenProject {
            document: again,
            file,
            read_only: false,
        }),
    )
    .unwrap();
    let reopened = feed.borrow_and_update().clone().unwrap();
    assert_eq!((reopened.revision, reopened.opened), (0, 2));
    set_open(&state, None).unwrap();
    assert!(feed.borrow_and_update().is_none());
}

#[test]
fn newer_projects_refuse_edits() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = Project::new("From a newer Klipolo");
    project.requires.insert("templates@1".into());
    let file = dir.path().join("Newer.klipolo");
    save(&file, &project, Durability::Full).unwrap();
    let before = std::fs::read(&file).unwrap();
    let state = ProjectSessionState::default();
    let open = open_file(&file).unwrap();
    let notice = open.document.snapshot().read_only;
    assert_eq!(
        notice.as_deref(),
        Some(klipolo_model::contrib::READ_ONLY_NOTICE),
        "the UI says so once"
    );
    set_open(&state, Some(open)).unwrap();
    let refused = with_open(&state, |open| apply_and_commit(open, &rename("Changed"))).unwrap_err();
    assert_eq!(refused, Said::code("model.readOnly"));
    assert!(with_open(&state, |open| step(open, false)).is_err());
    state.flush().unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before,
        "the file is untouched"
    );
}

/// Undo and redo go with the revision the UI's copy had at their turn: a change it hasn't merged yet (a chat edit) on
/// top of the history, or another project, refuses them with `project.moved`, so Undo never takes back a change the
/// person hasn't seen. Without a base they take the top step as it is.
#[test]
fn undo_and_redo_refuse_a_history_the_copy_has_not_seen() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = session(&dir.path().join("p.klipolo"), "Before");
    let id = lock(&state).as_ref().unwrap().document.project().id.clone();
    // The person's rename (revision 1, the copy's), then chat's (revision 2, not merged by the copy yet).
    with_open(&state, |open| apply_and_commit(open, &rename("Mine"))).unwrap();
    with_open(&state, |open| apply_and_commit(open, &rename("Chat's"))).unwrap();
    let moved = Said::code("project.moved");
    assert_eq!(
        step_at(&state, false, Some(1), Some(&id)).unwrap_err(),
        moved
    );
    assert_eq!(project_name(&state), "Chat's", "nothing was undone");
    assert_eq!(
        step_at(&state, false, Some(2), Some("another")).unwrap_err(),
        moved
    );
    // The copy has merged chat's change: its Undo takes it back.
    let undone = step_at(&state, false, Some(2), Some(&id)).unwrap();
    assert_eq!(
        (undone.history.revision, project_name(&state)),
        (3, "Mine".to_string())
    );
    // Redo, likewise.
    assert_eq!(
        step_at(&state, true, Some(2), Some(&id)).unwrap_err(),
        moved
    );
    step_at(&state, true, Some(3), Some(&id)).unwrap();
    assert_eq!(project_name(&state), "Chat's");
    // A caller with no copy: the top step as it is.
    step_at(&state, false, None, None).unwrap();
    assert_eq!(project_name(&state), "Mine");
}

/// A stale edit is refused, never rebased: the UI resyncs and the person tries again.
#[test]
fn an_edit_built_on_an_older_revision_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = session(&dir.path().join("p.klipolo"), "Before");
    let id = lock(&state).as_ref().unwrap().document.project().id.clone();
    apply_transaction(&state, &id, &rename("First"), Some(0)).unwrap();
    assert_eq!(
        apply_transaction(&state, &id, &rename("Stale"), Some(0)).unwrap_err(),
        ApplyError::Moved
    );
    assert_eq!(
        apply_transaction(&state, "another", &rename("Elsewhere"), None).unwrap_err(),
        ApplyError::OtherProject
    );
    assert_eq!(project_name(&state), "First");
}

/// Undo with no history changes nothing and says so on the current revision.
#[test]
fn undo_with_nothing_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = session(&dir.path().join("p.klipolo"), "Untouched");
    let before = lock(&state).as_ref().unwrap().document.project().clone();
    let undone = with_open(&state, |open| step(open, false)).unwrap();
    assert!(undone.patch.is_empty());
    assert_eq!((undone.base_revision, undone.history.revision), (0, 0));
    assert!(!undone.history.can_undo && !undone.history.can_redo);
    let redone = with_open(&state, |open| step(open, true)).unwrap();
    assert!(redone.patch.is_empty());
    assert_eq!(lock(&state).as_ref().unwrap().document.project(), &before);
}

/// An edit after an undo drops what Redo would have brought back.
#[test]
fn new_edit_clears_redo() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _) = session(&dir.path().join("p.klipolo"), "Start");
    with_open(&state, |open| apply_and_commit(open, &rename("One"))).unwrap();
    let undone = with_open(&state, |open| step(open, false)).unwrap();
    assert!(undone.history.can_redo);
    assert_eq!(project_name(&state), "Start");
    let edited = with_open(&state, |open| apply_and_commit(open, &rename("Two"))).unwrap();
    assert!(!edited.history.can_redo);
    let redone = with_open(&state, |open| step(open, true)).unwrap();
    assert!(redone.patch.is_empty(), "nothing to redo");
    assert_eq!(project_name(&state), "Two");
}

#[test]
fn edits_return_at_once_and_reach_the_disk_in_the_background() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("p.klipolo");
    let (state, _) = session(&file, "Before");
    let change = with_open(&state, |open| apply_and_commit(open, &rename("After"))).unwrap();
    assert_eq!(change.patch.name.as_deref(), Some("After"));
    state.flush().unwrap();
    assert_eq!(read(&file).unwrap().name, "After");
    let undone = with_open(&state, |open| step(open, false)).unwrap();
    assert_eq!(undone.patch.name.as_deref(), Some("Before"));
    state.flush().unwrap();
    assert_eq!(read(&file).unwrap().name, "Before");
}

/// Saved once the edits pause (and not while they keep coming), and on close at once.
#[test]
fn edits_are_saved_after_a_pause_and_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("p.klipolo");
    let (state, _) = session_on(&file, "Start", Duration::from_millis(50), Arc::new(Os));
    with_open(&state, |open| apply_and_commit(open, &rename("Paused"))).unwrap();
    assert!(eventually(|| read(&file).is_ok_and(|p| p.name == "Paused")));

    let file = dir.path().join("q.klipolo");
    let (state, _) = session(&file, "Start");
    with_open(&state, |open| apply_and_commit(open, &rename("Closed"))).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(read(&file).is_err(), "nothing is written before the pause");
    set_open(&state, None).unwrap();
    assert_eq!(read(&file).unwrap().name, "Closed", "closing saves first");
}

/// The real disk, except that every write fails with `.0`'s errno while it isn't 0.
#[derive(Default)]
struct WritesFailWith(AtomicI32);

impl Steps for WritesFailWith {
    fn write(&self, path: &Path, bytes: &[u8]) -> std::io::Result<File> {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            0 => Os.write(path, bytes),
            errno => Err(std::io::Error::from_raw_os_error(errno)),
        }
    }

    fn sync(&self, file: &File, durability: Durability) -> std::io::Result<()> {
        Os.sync(file, durability)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        Os.rename(from, to)
    }

    fn fsync_dir(&self, dir: &Path) -> std::io::Result<()> {
        Os.fsync_dir(dir)
    }
}

/// A full disk: saving says so in plain words and keeps the project open (closing would lose the edits); once there
/// is room again the next save works and says so.
#[test]
fn a_full_disk_is_said_and_keeps_the_project_open() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("p.klipolo");
    let disk = Arc::new(WritesFailWith::default());
    let (state, reports) = session_on(&file, "Start", NEVER, disk.clone());
    disk.0
        .store(libc::ENOSPC, std::sync::atomic::Ordering::SeqCst);
    with_open(&state, |open| apply_and_commit(open, &rename("Kept"))).unwrap();
    let error = state.flush().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Your disk is full, so changes aren't being saved. Free up some space."
    );
    let refused = set_open(&state, None).unwrap_err();
    assert_eq!(refused, Said::Text(error.to_string()));
    assert_eq!(project_name(&state), "Kept", "still open");
    assert_eq!(
        reports.lock().unwrap().as_slice(),
        [SaveStatus::failing(error.to_string())]
    );

    disk.0.store(0, std::sync::atomic::Ordering::SeqCst);
    set_open(&state, None).unwrap();
    assert_eq!(read(&file).unwrap().name, "Kept");
    // Told just after the write lands, so it may follow the close by a moment.
    assert!(
        eventually(|| reports.lock().unwrap().last() == Some(&SaveStatus::saved(1, 1))),
        "the save that works again says what it wrote: {:?}",
        reports.lock().unwrap()
    );
}

/// A project file that can't be read, or isn't a project, is refused with a code the UI words; nothing opens.
#[test]
fn damaged_or_missing_project_files_are_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let damaged = dir.path().join("Damaged.klipolo");
    std::fs::write(&damaged, r#"{"name": "Damaged", "#).unwrap();
    let garbled = dir.path().join("Garbled.klipolo");
    std::fs::write(&garbled, [0xff, 0xfe, 0x00, 0x80]).unwrap();
    let missing = dir.path().join("Missing.klipolo");
    let state = ProjectSessionState::default();
    let code = |file: &Path| match open_in(&state, file).unwrap_err() {
        Said::Coded(coded) => coded.code,
        Said::Text(text) => text,
    };
    assert_eq!(code(&damaged), "model.unreadable");
    assert_eq!(
        code(&garbled),
        "model.unreadable",
        "not text: damaged, not missing"
    );
    assert_eq!(code(&missing), "project.cantOpen");
    assert!(lock(&state).is_none(), "nothing opened");
}

/// Projects are saved as `<name>.klipolo` in the library; a name already taken gets a number, and a name with a slash
/// can't reach outside the folder.
#[test]
fn new_projects_get_their_own_file_in_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let state = ProjectSessionState::default();
    let first = create_in(&state, dir.path(), new_project("My video", vec![])).unwrap();
    assert_eq!(first.project.name, "My video");
    assert!(dir.path().join("My video.klipolo").is_file());
    let second = create_in(&state, dir.path(), new_project("My video", vec![])).unwrap();
    assert_eq!(second.opening, Some(2));
    assert!(dir.path().join("My video 2.klipolo").is_file());
    create_in(&state, dir.path(), new_project("a/../b", vec![])).unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        3,
        "three files, all in the library"
    );
    let untitled = new_project("  ", vec![FormatKind::Portrait, FormatKind::Landscape]);
    assert_eq!(untitled.name, "Untitled project");
    assert_eq!(untitled.settings.primary_format, FormatKind::Portrait);
}

/// The project open when the app quits opens again at the next launch.
#[test]
fn the_last_project_opens_again() {
    let dir = tempfile::tempdir().unwrap();
    let memo = dir.path().join("last-project");
    let state = ProjectSessionState::default();
    state.remember_last_in(memo.clone());
    let made = create_in(&state, dir.path(), new_project("Again", vec![])).unwrap();
    with_open(&state, |open| {
        apply_and_commit(open, &rename("Again, renamed"))
    })
    .unwrap();
    state.flush().unwrap();

    let next = ProjectSessionState::default();
    next.remember_last_in(memo);
    let opened = next.open_last().expect("the last project opens");
    assert_eq!(
        (opened.project.id, opened.project.name.as_str()),
        (made.project.id, "Again, renamed")
    );
}

#[test]
fn save_failures_read_as_plain_sentences() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "file, not a folder").unwrap();
    let file = blocker.join("x").join("p.klipolo");
    let err = save(&file, &Project::new("x"), Durability::Ordered).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Klipolo couldn't save your changes. The disk reported an error."
    );
    assert!(!err.detail().is_empty(), "the details go to the log");
    let full = SaveError::io(&std::io::Error::from_raw_os_error(libc::ENOSPC));
    assert_eq!(
        full.to_string(),
        "Your disk is full, so changes aren't being saved. Free up some space."
    );
    let denied = SaveError::io(&std::io::Error::from_raw_os_error(libc::EACCES));
    assert_eq!(
        denied.to_string(),
        "Klipolo can't write to that folder, so changes aren't being saved. Check its permissions."
    );
}

/// The page may read a project's media through the asset protocol, and nothing else its file names: not a pack's file,
/// not a file of another type (a project edited by hand to point at `~/.ssh/id_rsa`).
#[test]
fn only_a_projects_media_files_are_served_to_the_page() {
    use klipolo_model::project::{Asset, AssetKind};
    let asset = |path: &str| Asset {
        id: path.into(),
        kind: AssetKind::Video,
        path: path.into(),
        name: "x".into(),
        duration_us: 1_000_000,
        width: 16,
        height: 9,
        fps: 30.0,
        rotation: Some(0),
        has_audio: false,
        audio_lead_us: Some(0),
    };
    let mut project = Project::new("Served");
    project.assets = [
        "/clips/talk.MOV",
        "/music/song.mp3",
        "/pictures/logo.png",
        "/home/me/.ssh/id_rsa",
        "/home/me/notes.txt",
        "pack://fx@1/glow.png",
    ]
    .map(asset)
    .to_vec();
    assert_eq!(
        served_files(&project),
        ["/clips/talk.MOV", "/music/song.mp3", "/pictures/logo.png"].map(PathBuf::from)
    );
}

/// Every save says which opening and revision it wrote, so the page shows "Saving…" until its own revision is on the
/// disk: one save for edits that came while the last was waiting, the newest.
#[test]
fn every_save_says_what_it_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let (state, reports) = session(&dir.path().join("p.klipolo"), "Start");
    with_open(&state, |open| apply_and_commit(open, &rename("One"))).unwrap();
    state.flush().unwrap();
    with_open(&state, |open| apply_and_commit(open, &rename("Two"))).unwrap();
    with_open(&state, |open| apply_and_commit(open, &rename("Three"))).unwrap();
    state.flush().unwrap();
    assert!(eventually(|| reports.lock().unwrap().len() == 2));
    assert_eq!(
        reports.lock().unwrap().as_slice(),
        [SaveStatus::saved(1, 1), SaveStatus::saved(1, 3)]
    );
}

/// A new project can't open while the open one can't be saved (a full disk): the open one stays, and no file is left
/// behind for the project that didn't open.
#[test]
fn a_new_project_leaves_no_file_when_the_open_one_cant_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    std::fs::create_dir_all(&library).unwrap();
    let disk = Arc::new(WritesFailWith::default());
    let (state, _) = session_on(
        &dir.path().join("open.klipolo"),
        "Open",
        NEVER,
        disk.clone(),
    );
    disk.0
        .store(libc::ENOSPC, std::sync::atomic::Ordering::SeqCst);
    with_open(&state, |open| apply_and_commit(open, &rename("Edited"))).unwrap();
    assert!(create_in(&state, &library, new_project("New", vec![])).is_err());
    assert_eq!(project_name(&state), "Edited", "the open project stays");
    assert_eq!(
        std::fs::read_dir(&library).unwrap().count(),
        0,
        "no file for the project that didn't open"
    );
}

/// Two imported clips, each 10 s of `clip.mp4`, in a session saved to `file`.
fn two_clips(file: &Path) -> ProjectSessionState {
    use klipolo_model::project::{Asset, AssetKind};
    use klipolo_model::timeline::{Clip, ClipSource};
    let (state, _) = session(file, "Cuts");
    let asset = Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/clip.mp4".into(),
        name: "clip.mp4".into(),
        duration_us: 20_000_000,
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    };
    let clip = |id: &str, from: i64| {
        let mut clip = Clip::new(
            ClipSource::Asset { id: "v".into() },
            from,
            from + 10_000_000,
        );
        clip.id = id.into();
        clip
    };
    let ops = vec![
        Op::AddAssets {
            assets: vec![asset],
        },
        Op::InsertClip {
            clip: clip("a", 0),
            index: None,
        },
        Op::InsertClip {
            clip: clip("b", 10_000_000),
            index: None,
        },
    ];
    let add = Transaction {
        label: "media.import".into(),
        ops,
        ..rename("")
    };
    with_open(&state, |open| apply_and_commit(open, &add)).unwrap();
    state
}

/// A time range selected over the whole video and deleted (one `deleteRanges`, as the timeline sends it): the project
/// that is left has no clips, still checks out, saves and opens again; one undo brings both clips back.
#[test]
fn deleting_everything_leaves_valid_empty_project() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("p.klipolo");
    let state = two_clips(&file);
    let all = Transaction {
        label: "undo.delete.range".into(),
        ops: vec![Op::DeleteRanges {
            ranges: vec![klipolo_model::time::RangeUs {
                start: 0,
                end: 20_000_000,
            }],
        }],
        ..rename("")
    };
    let change = with_open(&state, |open| apply_and_commit(open, &all)).unwrap();
    assert_eq!(
        change.patch.main.as_deref(),
        Some(&[][..]),
        "the main track is empty"
    );
    let empty = lock(&state).as_ref().unwrap().document.project().clone();
    assert!(empty.timeline.main.is_empty());
    assert_eq!(empty.assets.len(), 1, "the file stays in the project");
    klipolo_model::validate::validate(&empty).expect("an empty project is valid");
    // It saves, and the file opens again as the same empty project.
    state.flush().unwrap();
    assert_eq!(read(&file).unwrap(), empty);
    // One step back: both clips again.
    with_open(&state, |open| step(open, false)).unwrap();
    let back = lock(&state)
        .as_ref()
        .unwrap()
        .document
        .project()
        .timeline
        .main
        .clone();
    assert_eq!(
        back.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
}
