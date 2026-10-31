//! A finished take lands: one undo step at the end of the main track, its files left in the take's folder and readable
//! by the page, never twice; in the project it was armed in (opened again when another is open), or in a new project
//! from the start screen; and a take that failed, has nothing to edit or was cancelled changes nothing. Real fixture
//! media through the app's own probe; nothing captures.

use std::sync::{Arc, Mutex};

use klipolo_capture::encoder_pick::FALLBACK_NOTE;
use klipolo_capture::manifest::{read_manifest, write_manifest};
use klipolo_capture::protocol::Command;
use klipolo_capture::session::ImportTarget;
use klipolo_capture::shell::WorkerState::*;
use klipolo_model::coded::CodedError;
use klipolo_model::cursor_style::{ClickSource, CursorCapture, RecordingCursor};
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::ClipSource;

use super::destination::{Day, recording_name, today, utc_day};
use super::setup::Files;
use super::test_support::*;
use super::test_takes::*;
use crate::project::uses_path;

/// The take's own project, open.
fn armed_in(sessions: &Sessions) -> ImportTarget {
    ImportTarget::Project {
        id: sessions.project().id,
    }
}

#[test]
fn finished_take_lands_as_one_undo_step_at_the_end() {
    let sessions = Sessions::with_project();
    let before = sessions.project();
    assert_eq!(before.timeline.main.len(), 1);
    let take = TakeFixture::saved(Recorded::default(), armed_in(&sessions));
    let import = sessions
        .land(&take.done(false, sessions.open_file()), None)
        .unwrap();
    let after = sessions.project();
    assert_eq!(after.timeline.main.len(), 2);
    let last = after.timeline.main.last().unwrap();
    assert_eq!(
        last.source,
        ClipSource::Recording {
            id: "take-1".into()
        }
    );
    assert_eq!(
        (import.take.as_str(), import.recording_id.as_str()),
        ("take-1", "take-1")
    );
    assert_eq!(import.clip_id, last.id);
    assert_eq!(import.snapshot.project, after, "the project as it is now");
    assert_eq!(import.zooms, 0);
    assert_eq!(after.recordings.len(), 1);
    assert_eq!(after.assets.len(), 3, "the footage, the screen and the mic");
    let history = sessions.history();
    assert_eq!(history.undo_label.as_deref(), Some("undo.record"));
    assert!(take.dir.imported_mark().is_file(), "marked after the step");
    // One Undo takes the whole take back.
    sessions.undo();
    let undone = sessions.project();
    assert_eq!(undone.timeline.main, before.timeline.main);
    assert!(undone.recordings.is_empty());
    assert_eq!(undone.assets, before.assets, "no added assets");
}

#[test]
fn recording_keeps_its_cursor_file_and_click_source() {
    let sessions = Sessions::with_project();
    let recorded = Recorded {
        cursor: true,
        ..Recorded::default()
    };
    let take = TakeFixture::saved(recorded, armed_in(&sessions));
    sessions
        .land(&take.done(false, sessions.open_file()), None)
        .unwrap();
    let recording = sessions.project().recordings[0].clone();
    assert_eq!(
        recording.cursor,
        Some(RecordingCursor {
            path: take.dir.cursor().to_string_lossy().into_owned(),
            capture: CursorCapture::Separate,
            clicks: ClickSource::Events,
        })
    );
}

/// The recording's id is the take's: landing the same take again (its `Imported` line there or lost) finds it in the
/// project and adds nothing, and a take from the start screen landed again never makes a second project.
#[test]
fn the_same_take_is_never_added_twice() {
    let sessions = Sessions::with_project();
    let take = TakeFixture::saved(Recorded::default(), armed_in(&sessions));
    let done = take.done(false, sessions.open_file());
    let first = sessions.land(&done, None).unwrap();
    let revision = sessions.history().revision;
    let again = sessions.land(&done, None).unwrap();
    // A crash right after the step: no `Imported` line, no mark.
    std::fs::remove_file(take.dir.imported_mark()).unwrap();
    let text = std::fs::read_to_string(take.dir.journal()).unwrap();
    let kept: Vec<&str> = text.lines().filter(|l| !l.contains("imported")).collect();
    std::fs::write(take.dir.journal(), kept.join("\n") + "\n").unwrap();
    let after_crash = sessions.land(&done, None).unwrap();
    let project = sessions.project();
    assert_eq!(project.recordings.len(), 1);
    assert_eq!(project.timeline.main.len(), 2);
    assert_eq!(sessions.history().revision, revision, "no step");
    assert_eq!(again.clip_id, first.clip_id);
    assert_eq!(after_crash.clip_id, first.clip_id);

    let sessions = Sessions::start_screen();
    let take = TakeFixture::saved(Recorded::default(), ImportTarget::New);
    let done = take.done(false, None);
    sessions.land(&done, None).unwrap();
    sessions.land(&done, None).unwrap();
    assert_eq!(sessions.library_files().len(), 1, "no second project");
    assert_eq!(sessions.project().recordings.len(), 1);
}

#[test]
fn take_files_stay_in_the_take_folder() {
    let sessions = Sessions::with_project();
    let take = TakeFixture::saved(Recorded::default(), armed_in(&sessions));
    sessions
        .land(&take.done(false, sessions.open_file()), None)
        .unwrap();
    let folder = std::fs::canonicalize(&take.dir.root).unwrap();
    let project = sessions.project();
    let runs = project.recordings[0].sources.iter().flat_map(|s| &s.runs);
    let mut files = 0;
    for run in runs {
        let asset = project
            .assets
            .iter()
            .find(|a| a.id == run.asset_id)
            .unwrap();
        assert!(
            std::path::Path::new(&asset.path).starts_with(&folder),
            "{} is in the take's folder",
            asset.path
        );
        assert!(std::path::Path::new(&asset.path).is_file());
        files += 1;
    }
    assert_eq!(files, 2, "the screen's run and the mic's");
    assert!(sessions.library_files().is_empty(), "nothing copied");
}

/// A take with nothing to edit (its picture's file missing) is refused with the recorder's own words and changes
/// nothing; a take cancelled before it recorded never reaches the landing at all.
#[test]
fn failed_or_cancelled_take_adds_nothing() {
    let sessions = Sessions::with_project();
    let revision = sessions.history().revision;
    let recorded = Recorded {
        picture: false,
        ..Recorded::default()
    };
    let take = TakeFixture::saved(recorded, armed_in(&sessions));
    let refused = sessions
        .land(&take.done(false, sessions.open_file()), None)
        .unwrap_err();
    assert_eq!(refused.code, "capture.import.noVideo");
    assert_eq!(sessions.history().revision, revision);
    assert_eq!(sessions.project().timeline.main.len(), 1);
    assert!(!take.dir.imported_mark().exists());
    assert!(take.dir.root.join("mic").is_dir(), "its files stay");

    // Cancelled during the countdown: the worker ends with no take (exit 3), and nothing lands.
    let sessions = Arc::new(Sessions::with_project());
    let revision = sessions.history().revision;
    let rec = fixture();
    let landed = Arc::new(Mutex::new(Vec::new()));
    *rec.sink.lands.lock().unwrap() = Some(landing_into(sessions.clone(), landed.clone()));
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Countdown));
    rec.send(Command::Cancel).unwrap();
    rec.launch.emit(state(Stopping));
    rec.launch.exit(Some(3));
    rec.settle();
    assert!(rec.finished().is_empty());
    assert!(landed.lock().unwrap().is_empty());
    assert_eq!(sessions.history().revision, revision);
}

/// "Mirror my camera", on or off, is the clip's camera layout's.
#[test]
fn mirror_choice_reaches_the_clips_camera_layout() {
    for mirror in [true, false] {
        let sessions = Sessions::with_project();
        let recorded = Recorded {
            camera: Some(mirror),
            ..Recorded::default()
        };
        let take = TakeFixture::saved(recorded, armed_in(&sessions));
        sessions
            .land(&take.done(false, sessions.open_file()), None)
            .unwrap();
        let clip = sessions.project().timeline.main.last().unwrap().clone();
        assert_eq!(clip.camera.map(|c| c.mirror), Some(mirror));
    }
}

/// The page reads a file only when the open project uses it (`uses_path`: waveforms, thumbnail strips): every file of
/// the landed take is one of them.
#[test]
fn landed_files_are_allowed_to_the_page() {
    let sessions = Sessions::with_project();
    let recorded = Recorded {
        camera: Some(true),
        ..Recorded::default()
    };
    let take = TakeFixture::saved(recorded, armed_in(&sessions));
    let paths = || -> Vec<String> {
        let project = sessions.project();
        let runs: Vec<String> = (project.recordings.iter())
            .flat_map(|r| &r.sources)
            .flat_map(|s| &s.runs)
            .map(|run| run.asset_id.clone())
            .collect();
        (project.assets.iter())
            .filter(|a| runs.contains(&a.id))
            .map(|a| a.path.clone())
            .collect()
    };
    sessions
        .land(&take.done(false, sessions.open_file()), None)
        .unwrap();
    let landed = paths();
    assert_eq!(landed.len(), 3, "screen, camera and mic");
    for path in &landed {
        assert!(uses_path(&sessions.state, path), "{path}");
    }
}

#[test]
fn take_from_the_start_screen_lands_in_a_new_project() {
    let sessions = Sessions::start_screen();
    let take = TakeFixture::saved(Recorded::default(), ImportTarget::New);
    let import = sessions.land(&take.done(false, None), None).unwrap();
    assert_eq!(sessions.library_files(), ["Recording, Oct 31.klipolo"]);
    let project = sessions.project();
    assert_eq!(project.name, "Recording, Oct 31");
    assert_eq!(
        project.settings.formats,
        [
            FormatKind::Landscape,
            FormatKind::Portrait,
            FormatKind::Square
        ]
    );
    assert_eq!(project.settings.primary_format, FormatKind::Landscape);
    assert_eq!(project.timeline.main.len(), 1);
    assert_eq!(
        project.timeline.main[0].source,
        ClipSource::Recording {
            id: "take-1".into()
        }
    );
    assert_eq!(import.snapshot.project.id, project.id);
    assert!(import.snapshot.opening.is_some());
    assert_eq!(
        sessions.history().undo_label.as_deref(),
        Some("undo.record")
    );
    // One undo step: the new project's first.
    sessions.undo();
    assert!(sessions.project().timeline.main.is_empty());
    assert!(!sessions.history().can_undo);
}

/// A take armed on the start screen and cancelled makes no project; nor does one with nothing to edit.
#[test]
fn cancelled_take_from_the_start_screen_makes_no_project() {
    let sessions = Arc::new(Sessions::start_screen());
    let rec = fixture();
    *rec.project.lock().unwrap() = None;
    let landed = Arc::new(Mutex::new(Vec::new()));
    *rec.sink.lands.lock().unwrap() = Some(landing_into(sessions.clone(), landed.clone()));
    let id = rec.arm(options_screen()).unwrap().id;
    assert_eq!(rec.config(&id).target, ImportTarget::New);
    rec.launch.emit(state(Armed));
    rec.send(Command::Cancel).unwrap();
    rec.launch.emit(state(Stopping));
    rec.launch.exit(Some(3));
    rec.settle();
    assert!(landed.lock().unwrap().is_empty());
    assert!(sessions.library_files().is_empty());
    assert!(sessions.open_file().is_none(), "still the start screen");

    let recorded = Recorded {
        picture: false,
        ..Recorded::default()
    };
    let take = TakeFixture::saved(recorded, ImportTarget::New);
    assert!(sessions.land(&take.done(false, None), None).is_err());
    assert!(sessions.library_files().is_empty());
    assert!(sessions.open_file().is_none());
}

/// A take from the start screen whose step the new project refuses leaves no empty "Recording, {Mon D}" behind: the
/// project made for it is closed and its file goes. The take's files stay.
#[test]
fn a_start_screen_take_that_is_refused_leaves_no_project() {
    let sessions = Sessions::start_screen();
    let take = TakeFixture::saved(Recorded::default(), ImportTarget::New);
    let library = tempfile::tempdir().unwrap();
    let path = library.path().to_path_buf();
    let library_dir =
        move || -> Result<std::path::PathBuf, klipolo_model::coded::Said> { Ok(path.clone()) };
    // Every run's asset gets the same id, so the project refuses the step.
    let mut probe = |path: &std::path::Path, _key: &str| {
        crate::media::probe_asset(path).map(|asset| klipolo_model::project::Asset {
            id: "same".into(),
            ..asset
        })
    };
    let landing = super::import::Landing {
        library: &library_dir,
        today: OCT_31,
        files: None,
        probe: &mut probe,
    };
    let refused =
        super::import::land(&sessions.state, &take.done(false, None), landing).unwrap_err();
    assert_eq!(refused.code, "capture.import.notAdded");
    let left: Vec<_> = std::fs::read_dir(library.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(left.is_empty(), "no project left behind: {left:?}");
    assert!(sessions.open_file().is_none(), "still the start screen");
    assert!(take.dir.root.join("mic").is_dir(), "its files stay");
}

/// Should the person close the take's project, or open another, while the take lands, the page is still told it was
/// added: the project opens again, with the take, as for a take whose project isn't open when it lands.
#[test]
fn a_take_whose_project_closes_while_it_lands_is_still_added() {
    for close in [true, false] {
        let sessions = Sessions::with_project();
        let take = TakeFixture::saved(Recorded::default(), armed_in(&sessions));
        let done = take.done(false, sessions.open_file());
        let library = || -> Result<std::path::PathBuf, klipolo_model::coded::Said> {
            Err(klipolo_model::coded::Said::code("project.noVideosFolder"))
        };
        let mut probe = |path: &std::path::Path, _key: &str| crate::media::probe_asset(path);
        let to = super::destination::Destination {
            target: &done.target,
            file: done.project_file.as_deref(),
            library: &library,
            today: OCT_31,
        };
        let landed =
            super::import::import_take(&sessions.state, &take.dir, &to, &mut probe).unwrap();
        if close {
            crate::project::close_in(&sessions.state).unwrap();
        } else {
            sessions.open_new("Other");
        }
        let snapshot = super::import::snapshot_after(&sessions.state, &landed).unwrap();
        assert_eq!(snapshot.project.id, landed.project, "closed: {close}");
        assert_eq!(snapshot.project.recordings.len(), 1, "with the take");
        assert!(snapshot.opening.is_some());
        assert_eq!(sessions.project().id, landed.project, "open again");
    }
}

/// The take lands in the project it was armed in. Should another be open by then, that project opens again (the open
/// one saved first) and the take lands there; the page's snapshot is that project's.
#[test]
fn a_take_lands_in_the_project_it_was_armed_in_even_when_another_is_open() {
    let sessions = Sessions::with_project();
    let target = armed_in(&sessions);
    let armed_file = sessions.open_file();
    let (other, _) = sessions.open_new("Other");
    assert_eq!(sessions.project().id, other, "another project is open now");
    let take = TakeFixture::saved(Recorded::default(), target.clone());
    let import = sessions.land(&take.done(false, armed_file), None).unwrap();
    let project = sessions.project();
    assert_eq!(
        ImportTarget::Project {
            id: project.id.clone()
        },
        target
    );
    assert_eq!(project.name, "Edit");
    assert_eq!(project.recordings.len(), 1);
    assert_eq!(import.snapshot.project.id, project.id);
    assert!(sessions.library_files().is_empty(), "no new project");
}

/// A take whose project's file is gone by the time it lands goes into a new project rather than nowhere.
#[test]
fn a_take_whose_project_is_gone_lands_in_a_new_one() {
    let sessions = Sessions::with_project();
    let target = armed_in(&sessions);
    let armed_file = sessions.open_file().unwrap();
    sessions.open_new("Other");
    std::fs::remove_file(&armed_file).unwrap();
    let take = TakeFixture::saved(Recorded::default(), target);
    sessions
        .land(&take.done(false, Some(armed_file)), None)
        .unwrap();
    assert_eq!(sessions.library_files(), ["Recording, Oct 31.klipolo"]);
    assert_eq!(sessions.project().recordings.len(), 1);
}

/// A take recorded on another encoder than the probe's lets the probe's report go, so the next opening of the Record
/// dialog measures again; a take on the probe's own encoder keeps it.
#[test]
fn a_take_that_fell_back_to_another_encoder_lets_the_probe_report_go() {
    let kept = tempfile::tempdir().unwrap();
    let files = Files {
        dir: kept.path().to_path_buf(),
    };
    for fell_back in [false, true] {
        std::fs::write(files.probe(), b"{}").unwrap();
        let sessions = Sessions::with_project();
        let take = TakeFixture::saved(Recorded::default(), armed_in(&sessions));
        if fell_back {
            let mut manifest = read_manifest(&take.dir).unwrap();
            manifest.notes.push(CodedError::new(FALLBACK_NOTE));
            write_manifest(&take.dir, &manifest).unwrap();
        }
        let import = sessions
            .land(&take.done(false, sessions.open_file()), Some(&files))
            .unwrap();
        let noted = import.notes.iter().any(|n| n.code == FALLBACK_NOTE);
        assert_eq!(noted, fell_back, "the person hears it");
        assert_eq!(files.probe().exists(), !fell_back);
    }
}

#[test]
fn a_new_project_is_named_for_the_day_the_take_lands() {
    assert_eq!(
        recording_name(Day { month: 10, day: 31 }),
        "Recording, Oct 31"
    );
    assert_eq!(recording_name(Day { month: 1, day: 5 }), "Recording, Jan 5");
    assert_eq!(utc_day(0), Day { month: 1, day: 1 });
    // 2023-11-14 22:13:20 UTC.
    assert_eq!(utc_day(1_700_000_000), Day { month: 11, day: 14 });
    // 2026-10-31 12:00 UTC.
    assert_eq!(utc_day(1_793_448_000), Day { month: 10, day: 31 });
    let now = today();
    assert!((1..=12).contains(&now.month) && (1..=31).contains(&now.day));
}

/// What the page hears when a take couldn't land: the recorder's own code, never a project's.
#[test]
fn a_take_that_cannot_land_says_so_in_the_recorders_words() {
    let sessions = Sessions::start_screen();
    let take = TakeFixture::saved(Recorded::default(), ImportTarget::New);
    let refused = {
        let library = || -> Result<std::path::PathBuf, klipolo_model::coded::Said> {
            Err(klipolo_model::coded::Said::code("project.noVideosFolder"))
        };
        let mut probe = |path: &std::path::Path, _key: &str| crate::media::probe_asset(path);
        let landing = super::import::Landing {
            library: &library,
            today: OCT_31,
            files: None,
            probe: &mut probe,
        };
        super::import::land(&sessions.state, &take.done(false, None), landing).unwrap_err()
    };
    assert_eq!(refused.code, "capture.import.notAdded");
    assert_eq!(
        klipolo_capture::english::english(&refused),
        "The recording couldn't be added to your project. Its files are in Movies / Klipolo / Recordings."
    );
    assert!(!take.dir.imported_mark().exists());
}

/// A drive that stops answering while the take saves: what happens, and what to do next.
#[test]
fn a_stuck_drive_says_what_happens_and_what_to_do() {
    let stuck = CodedError::new("capture.error.driveStuck");
    assert_eq!(
        klipolo_capture::english::english(&stuck),
        "The drive stopped answering. Klipolo finishes the recording from what was saved. Check the drive before you \
         record again."
    );
}
