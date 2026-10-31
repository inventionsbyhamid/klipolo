//! A take's end through the recorder: the engine's background media work waits from arming until the take has landed,
//! and a take the worker stopped saving lands from its files and says so. Fake workers, real take files and a real project session; nothing
//! captures.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::session::SessionDir;
use klipolo_capture::shell::WorkerState::*;
use klipolo_model::coded::CodedError;

use super::state::Opened;
use super::test_support::*;
use super::test_takes::*;
use super::types::TakeImport;

/// A recorder whose takes land in `sessions`' open project, and what each landing came to.
type Landings = Arc<Mutex<Vec<Result<TakeImport, CodedError>>>>;

fn landing_in(sessions: &Arc<Sessions>) -> (Fixture, Landings) {
    let rec = fixture();
    *rec.project.lock().unwrap() = Some(Opened {
        id: sessions.project().id,
        file: sessions.open_file().unwrap(),
    });
    let landed = Arc::new(Mutex::new(Vec::new()));
    *rec.sink.lands.lock().unwrap() = Some(landing_into(sessions.clone(), landed.clone()));
    (rec, landed)
}

/// The engine's background media work (thumbnail scans, the activity pass) waits from arming until the take has
/// landed, and resumes then; a take that never records lets it go too, and a refused arm never holds it.
#[test]
fn background_media_work_holds_while_armed_and_resumes_after() {
    let sessions = Arc::new(Sessions::with_project());
    let (rec, _landed) = landing_in(&sessions);
    let holds = || rec.sink.holds.lock().unwrap().clone();
    assert!(holds().is_empty());
    let id = rec.arm(options_screen()).unwrap().id;
    assert_eq!(holds(), [true], "held from arming");
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.send(Command::Stop).unwrap();
    rec.launch.emit(state(Stopping));
    rec.settle();
    assert_eq!(holds(), [true], "held while saving");
    write_saved(&SessionDir::at(&rec.take_dir(&id)), Recorded::default());
    rec.launch.emit(Event::Finished {
        session: id.clone(),
    });
    rec.settle();
    assert_eq!(sessions.project().recordings.len(), 1, "landed");
    assert_eq!(holds(), [true], "held through the landing");
    rec.launch.exit(Some(0));
    rec.settle();
    assert_eq!(holds(), [true, false], "resumed once the take is over");

    // Armed, then disarmed: let go when it ends.
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Disarm).unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
    assert_eq!(holds(), [true, false, true, false]);
    // A refused arm holds nothing.
    rec.launch.0.refuse.store(true, Ordering::SeqCst);
    rec.arm(options_screen()).unwrap_err();
    assert_eq!(holds().len(), 4);
}

/// The worker stops while it saves (it crashed, or was killed): the take is finished from its files, lands as any
/// take does, and the person is told plainly what happened, first among its notes. Then Klipolo comes back.
#[test]
fn a_take_the_worker_stopped_saving_lands_from_its_files_and_says_so() {
    let sessions = Arc::new(Sessions::with_project());
    let (rec, landed) = landing_in(&sessions);
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    write_cut_off(&SessionDir::at(&rec.take_dir(&id)));
    rec.send(Command::Stop).unwrap();
    rec.launch.emit(state(Stopping));
    rec.launch.exit(None);
    rec.settle();
    assert_eq!(rec.launch.spawned(), 1, "never resumed");
    let finished = rec.finished();
    assert!(finished.len() == 1 && finished[0].recover);
    let import = landed.lock().unwrap()[0].clone().unwrap();
    assert_eq!(
        import.notes[0],
        CodedError::new("capture.note.savedFromFiles")
    );
    assert_eq!(
        klipolo_capture::english::english(&import.notes[0]),
        "The recorder stopped while saving, so Klipolo finished the recording from its files."
    );
    let project = sessions.project();
    assert_eq!(project.recordings.len(), 1);
    assert_eq!(project.timeline.main.len(), 2);
    assert_eq!(rec.tagged_events().last(), Some(&(id, state(Idle))));
    assert!(rec.visible());
}

/// A take that keeps crashing says why itself (`capture.stop.crashLoop`), so its landing adds no second note.
#[test]
fn a_crash_looped_take_is_not_told_twice() {
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    for _ in 0..4 {
        rec.launch.emit(state(Recording));
        rec.launch.exit(Some(9));
        rec.advance_s(10);
    }
    let finished = rec.finished();
    assert!(finished.len() == 1 && finished[0].recover);
    assert_eq!(finished[0].note, None);
}
