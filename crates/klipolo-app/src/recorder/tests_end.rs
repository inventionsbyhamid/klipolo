//! A take's end through the recorder: quitting during a take stops it and exits once it has landed (or after 30 s),
//! the engine's background media work waits from arming until the take has landed, and a take the worker stopped
//! saving lands from its files and says so. Fake workers, real take files and a real project session; nothing
//! captures.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::session::SessionDir;
use klipolo_capture::shell::WorkerState::*;
use klipolo_model::coded::CodedError;

use super::quitting::QUIT_WAIT_MS;
use super::state::{Opened, RecorderState};
use super::test_support::*;
use super::test_takes::*;
use super::types::TakeImport;

/// Polls `done` for up to 5 s.
fn wait_until(done: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    done()
}

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

/// Quitting while a take records: the exit is held, the take is stopped as the person's Stop would, and Klipolo exits
/// once, after the take has landed in its project. A second quit meanwhile is held too; the exit that follows the
/// landing goes through.
#[test]
fn quit_during_a_take_stops_it_and_lands_it_before_exit() {
    let sessions = Arc::new(Sessions::with_project());
    let (rec, landed) = landing_in(&sessions);
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.settle();
    // Each exit, with how many takes had landed by then.
    let exits = Arc::new(Mutex::new(Vec::new()));
    let (seen, at_exit) = (landed.clone(), exits.clone());
    let held = rec.state.hold_quit(Box::new(move || {
        at_exit.lock().unwrap().push(seen.lock().unwrap().len());
    }));
    assert!(held, "the exit waits");
    assert_eq!(
        rec.launch.commands(),
        [Command::Start, Command::Stop],
        "stopped as the person's Stop"
    );
    assert!(rec.state.hold_quit(Box::new(|| panic!("one exit only"))));
    assert_eq!(rec.launch.commands().len(), 2, "stopped once");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        exits.lock().unwrap().is_empty(),
        "held while the take saves"
    );
    // The worker saves the take, it lands, and then Klipolo exits.
    write_saved(&SessionDir::at(&rec.take_dir(&id)), Recorded::default());
    rec.launch.emit(state(Stopping));
    rec.launch.emit(Event::Finished {
        session: id.clone(),
    });
    rec.launch.exit(Some(0));
    assert!(wait_until(|| !exits.lock().unwrap().is_empty()));
    assert_eq!(*exits.lock().unwrap(), [1], "after the landing, once");
    assert!(landed.lock().unwrap()[0].is_ok());
    assert_eq!(sessions.project().recordings.len(), 1);
    assert!(
        !rec.state.hold_quit(Box::new(|| ())),
        "the exit goes through"
    );
}

/// A take that doesn't land within 30 s doesn't keep Klipolo open: the exit goes ahead and the take saves on in its
/// worker. The wait belongs to that take: a quit that something else keeps from happening leaves the next take to hold
/// the next quit.
#[test]
fn quitting_waits_30_s_at_most_for_the_take() {
    let rec = fixture();
    let first = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.settle();
    let exited = Arc::new(AtomicBool::new(false));
    let flag = exited.clone();
    assert!(
        rec.state
            .hold_quit(Box::new(move || flag.store(true, Ordering::SeqCst)))
    );
    rec.move_clock_ms(QUIT_WAIT_MS - 1_000);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!exited.load(Ordering::SeqCst), "29 s: still waiting");
    rec.move_clock_ms(1_000);
    assert!(wait_until(|| exited.load(Ordering::SeqCst)), "30 s: exits");
    assert!(rec.state.active(), "the take saves on");
    assert!(
        !rec.state.hold_quit(Box::new(|| panic!("never called"))),
        "the exit that follows goes through"
    );
    // The take ends after all (the quit was kept from happening), and the next take holds the next quit.
    rec.launch.emit(state(Stopping));
    rec.launch.emit(Event::Finished { session: first });
    rec.launch.exit(Some(0));
    rec.settle();
    rec.arm(options_screen()).unwrap();
    rec.launch.emit(state(Armed));
    rec.settle();
    assert!(rec.state.hold_quit(Box::new(|| ())));
}

/// Quitting never waits for the recorder's lock, which covers file work: with it held (the take's journal read on a
/// drive that stopped answering), the exit is still held at once, and Stop goes out once the lock is free.
#[test]
fn quitting_never_waits_for_the_recorders_lock() {
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.settle();
    let (held_tx, held) = mpsc::channel();
    let (free, free_rx) = mpsc::channel::<()>();
    let recorder = rec.rec().clone();
    let holder = std::thread::spawn(move || {
        let _slot = recorder.lock();
        held_tx.send(()).unwrap();
        let _ = free_rx.recv();
    });
    held.recv().unwrap();
    let exited = Arc::new(AtomicBool::new(false));
    let flag = exited.clone();
    let state = &rec.state;
    let answered = std::thread::scope(|s| {
        let (tx, rx) = mpsc::channel();
        s.spawn(move || {
            let held = state.hold_quit(Box::new(move || flag.store(true, Ordering::SeqCst)));
            let _ = tx.send(held);
        });
        let answered = rx.recv_timeout(Duration::from_secs(1));
        free.send(()).unwrap();
        answered
    });
    holder.join().unwrap();
    assert_eq!(answered, Ok(true), "held at once, without the lock");
    assert!(wait_until(|| rec
        .launch
        .commands()
        .contains(&Command::Stop)));
    assert!(!exited.load(Ordering::SeqCst), "waiting for the take");
    rec.move_clock_ms(QUIT_WAIT_MS);
    assert!(wait_until(|| exited.load(Ordering::SeqCst)), "30 s: exits");
}

/// No take, a take that has ended, or a test run: quitting goes straight ahead.
#[test]
fn a_quit_without_a_take_is_never_held() {
    let rec = fixture();
    assert!(
        !rec.state
            .hold_quit(Box::new(|| panic!("not held, so not called")))
    );
    rec.arm(options_screen()).unwrap();
    rec.launch.emit(state(Armed));
    rec.send(Command::Disarm).unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
    assert!(!rec.state.hold_quit(Box::new(|| ())));
    let off = RecorderState::build(true, || unreachable!("no parts in a test run"));
    assert!(!off.hold_quit(Box::new(|| ())));
}

/// A take armed but not started, at quit: Stop ends it with nothing recorded, and Klipolo exits at once after.
#[test]
fn quitting_with_an_armed_take_ends_it_and_exits() {
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.launch.emit(state(Armed));
    rec.settle();
    let exited = Arc::new(AtomicBool::new(false));
    let flag = exited.clone();
    assert!(
        rec.state
            .hold_quit(Box::new(move || flag.store(true, Ordering::SeqCst)))
    );
    assert_eq!(rec.launch.commands(), [Command::Stop]);
    // Stop before the gate: the worker ends with no take.
    rec.launch.emit(state(Stopping));
    rec.launch.exit(Some(3));
    assert!(wait_until(|| exited.load(Ordering::SeqCst)));
    assert!(rec.finished().is_empty());
    assert_eq!(rec.tagged_events().last(), Some(&(id, state(Idle))));
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
