//! The recorder's permission steps, each only on the person's own press: Allow screen recording asks and then opens its
//! settings page, Reopen Klipolo saves the project before it restarts and is refused while a take runs, and Open
//! privacy settings opens the right page. After Reopen Klipolo, a setup that still reads Screen Recording as not
//! allowed says so. The fake backend and fake workers only: nothing is asked of the OS, nothing opens and nothing
//! quits.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use klipolo_capture::backend::Permission;
use klipolo_capture::shell::WorkerState::*;
use klipolo_capture::synthetic::Script;
use klipolo_model::coded::Said;

use super::test_support::*;
use super::types::PrivacyPage;

const SCREEN_PAGE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// An opener that records each page with how many Screen Recording requests had been made by then.
struct Opener<'a> {
    rec: &'a Fixture,
    opened: Mutex<Vec<(String, u64)>>,
}

impl Opener<'_> {
    fn open(&self, url: &str) -> std::io::Result<()> {
        let asked = self.rec.screen_requests.load(Ordering::SeqCst);
        self.opened.lock().unwrap().push((url.to_owned(), asked));
        Ok(())
    }

    fn opened(&self) -> Vec<(String, u64)> {
        self.opened.lock().unwrap().clone()
    }
}

fn opener(rec: &Fixture) -> Opener<'_> {
    Opener {
        rec,
        opened: Mutex::default(),
    }
}

#[test]
fn allow_screen_asks_only_when_pressed() {
    let rec = fixture_with(Script {
        screen_permission: Permission::NotAsked,
        ..Script::default()
    });
    // Opening the dialog never asks, and Record isn't armed until it is allowed: the worker's start would make macOS
    // ask.
    rec.state.setup().unwrap();
    let refused = rec.arm(options_screen()).unwrap_err();
    assert_eq!(refused.code, "capture.error.permission");
    assert_eq!(rec.launch.spawned(), 0, "no worker");
    assert_eq!(rec.screen_requests.load(Ordering::SeqCst), 0);
    // The press asks once, then opens Screen Recording's page, where macOS lists Klipolo once it has asked.
    let pages = opener(&rec);
    rec.state.allow_screen(&|url| pages.open(url)).unwrap();
    assert_eq!(rec.screen_requests.load(Ordering::SeqCst), 1);
    assert_eq!(pages.opened(), [(SCREEN_PAGE.to_owned(), 1)]);
}

#[test]
fn reopen_is_refused_during_a_take_and_flushes_first() {
    let rec = fixture();
    let steps = Mutex::new(Vec::new());
    let reopen = || {
        rec.state.reopen(
            &|| false,
            || {
                steps.lock().unwrap().push("flush");
                Ok(())
            },
            || steps.lock().unwrap().push("restart"),
        )
    };
    // An export runs: the restart would end it.
    let exporting = rec.state.reopen(
        &|| true,
        || panic!("never saves"),
        || panic!("never restarts"),
    );
    assert_eq!(
        code_of(&exporting.unwrap_err()),
        "capture.permission.reopenExporting"
    );
    assert_eq!(
        klipolo_capture::english::english(&klipolo_model::coded::CodedError::new(
            "capture.permission.reopenExporting"
        )),
        "An export is running. Reopen Klipolo when it's done."
    );
    // A failed save keeps Klipolo open, with what went wrong, and takes arm again.
    let failed = rec.state.reopen(
        &|| false,
        || Err(Said::Text("The disk is full.".into())),
        || panic!("never restarts with unsaved edits"),
    );
    assert_eq!(failed.unwrap_err(), Said::Text("The disk is full.".into()));
    // Armed through saving: refused, nothing saved or restarted.
    for said in [None, Some(Recording), Some(Stopping)] {
        if rec.rec().take_id().is_none() {
            rec.arm(options_screen()).unwrap();
        }
        if let Some(s) = said {
            rec.launch.emit(state(s));
        }
        rec.settle();
        let refused = reopen().unwrap_err();
        assert_eq!(code_of(&refused), "capture.busy", "{said:?}");
    }
    assert!(steps.lock().unwrap().is_empty());
    // With no take: the project is saved first, then Klipolo restarts.
    let rec = fixture();
    let steps = Mutex::new(Vec::new());
    rec.state
        .reopen(
            &|| false,
            || {
                steps.lock().unwrap().push("flush");
                Ok(())
            },
            || steps.lock().unwrap().push("restart"),
        )
        .unwrap();
    assert_eq!(*steps.lock().unwrap(), ["flush", "restart"]);
}

/// Klipolo run from its disk image, or from the copy macOS translocated it to, may not open again once it quits: Reopen
/// Klipolo says to move it to Applications first, and doesn't save, quit or remember a reopen. Takes still arm.
#[test]
fn reopen_from_a_disk_image_asks_to_move_klipolo_first() {
    let rec = fixture_changed(Script::default(), |parts| parts.must_move = true);
    let refused = rec.state.reopen(
        &|| false,
        || panic!("never saves"),
        || panic!("never quits"),
    );
    assert_eq!(
        code_of(&refused.unwrap_err()),
        "capture.permission.reopenMoveFirst"
    );
    assert_eq!(
        klipolo_capture::english::english(&klipolo_model::coded::CodedError::new(
            "capture.permission.reopenMoveFirst"
        )),
        "Move Klipolo to your Applications folder, then open it again."
    );
    assert!(!rec.files.dir.join("screen-reopen.json").exists());
    rec.arm(options_screen()).unwrap();
}

/// A Record press that lands while Reopen Klipolo saves the project is refused: the restart would cut the take off
/// armed. Once the reopen doesn't go ahead, takes arm again.
#[test]
fn no_take_arms_while_klipolo_reopens() {
    let rec = fixture();
    let refused = rec.state.reopen(
        &|| false,
        || {
            let busy = rec.state.arm(&options_screen()).unwrap_err();
            assert_eq!(busy.code, "capture.busy");
            Err(Said::Text("The disk is full.".into()))
        },
        || panic!("never restarts"),
    );
    assert!(refused.is_err());
    assert_eq!(rec.launch.spawned(), 0);
    rec.arm(options_screen()).unwrap();
    // A reopen that goes ahead keeps refusing until Klipolo is gone.
    let rec = fixture();
    rec.state.reopen(&|| false, || Ok(()), || ()).unwrap();
    assert_eq!(rec.arm(options_screen()).unwrap_err().code, "capture.busy");
}

/// A second press of Reopen Klipolo while it saves does nothing more (no second save, no second restart, and no "A
/// recording is already running"). An export that starts while the project saves keeps Klipolo open: the restart would
/// end it. Takes arm again then.
#[test]
fn a_second_reopen_press_does_nothing_more_and_an_export_started_meanwhile_keeps_klipolo_open() {
    let rec = fixture();
    let steps = Mutex::new(Vec::new());
    rec.state
        .reopen(
            &|| false,
            || {
                steps.lock().unwrap().push("flush");
                let again = rec.state.reopen(
                    &|| false,
                    || panic!("never saves twice"),
                    || panic!("never restarts twice"),
                );
                assert_eq!(again, Ok(()));
                Ok(())
            },
            || steps.lock().unwrap().push("restart"),
        )
        .unwrap();
    assert_eq!(*steps.lock().unwrap(), ["flush", "restart"]);

    let rec = fixture();
    let exporting = std::sync::atomic::AtomicBool::new(false);
    let refused = rec.state.reopen(
        &|| exporting.load(Ordering::SeqCst),
        || {
            exporting.store(true, Ordering::SeqCst);
            Ok(())
        },
        || panic!("never restarts during an export"),
    );
    assert_eq!(
        code_of(&refused.unwrap_err()),
        "capture.permission.reopenExporting"
    );
    rec.arm(options_screen()).unwrap();
}

/// Reopen Klipolo's quit relaunches Klipolo only once the project is saved: never when the save fails, an export runs,
/// or one starts while the project saves. The command's quit asks to exit with the reopen's code, which the app's exit
/// hears (`relaunch_on_exit`).
#[test]
fn the_relaunch_is_started_only_after_the_flush_succeeds() {
    use super::relaunch::REOPEN_EXIT_CODE;
    let rec = fixture();
    let relaunch = rec.state.relaunch();
    let quit = || rec.state.exit_requested(Some(REOPEN_EXIT_CODE), false);
    // An export runs.
    let _ = rec.state.reopen(&|| true, || panic!("never saves"), quit);
    assert!(!relaunch.due());
    // The save fails.
    let failed = rec.state.reopen(
        &|| false,
        || Err(Said::Text("The disk is full.".into())),
        quit,
    );
    assert!(failed.is_err());
    assert!(!relaunch.due());
    // An export starts while the project saves.
    let exporting = std::sync::atomic::AtomicBool::new(false);
    let refused = rec.state.reopen(
        &|| exporting.load(Ordering::SeqCst),
        || {
            exporting.store(true, Ordering::SeqCst);
            Ok(())
        },
        quit,
    );
    assert!(refused.is_err());
    assert!(!relaunch.due());
    // Saved, and nothing exports: the quit goes ahead and Klipolo opens again, once.
    let saved = Mutex::new(false);
    rec.state
        .reopen(
            &|| false,
            || {
                assert!(!relaunch.due(), "not before the save");
                *saved.lock().unwrap() = true;
                Ok(())
            },
            || {
                assert!(*saved.lock().unwrap());
                quit();
            },
        )
        .unwrap();
    assert!(relaunch.due());
    assert!(!relaunch.due(), "once");
}

/// Reopen Klipolo's quit, held by a quit guard (an export started while the project saved, and asks first), leaves
/// nothing armed: Reopen Klipolo and Record work again, and a later quit nothing asks before (the Dock's Quit, logging
/// out) doesn't relaunch Klipolo. The export's Quit asks the exit again with the reopen's code, which then goes ahead and
/// relaunches.
#[test]
fn a_held_reopen_leaves_nothing_armed() {
    use super::relaunch::REOPEN_EXIT_CODE;
    let held = |rec: &Fixture| rec.state.exit_requested(Some(REOPEN_EXIT_CODE), true);
    let rec = fixture();
    rec.state
        .reopen(&|| false, || Ok(()), || held(&rec))
        .unwrap();
    assert!(
        !rec.state.relaunch().due(),
        "a later Dock Quit doesn't relaunch"
    );
    let restarts = Mutex::new(0);
    let restart = || {
        *restarts.lock().unwrap() += 1;
        held(&rec);
    };
    rec.state.reopen(&|| false, || Ok(()), restart).unwrap();
    assert_eq!(*restarts.lock().unwrap(), 1, "Reopen Klipolo works again");
    rec.arm(options_screen()).unwrap();
    // The person picks the export's Quit instead.
    let rec = fixture();
    rec.state
        .reopen(&|| false, || Ok(()), || held(&rec))
        .unwrap();
    rec.state.exit_requested(Some(REOPEN_EXIT_CODE), false);
    assert!(rec.state.relaunch().due());
}

#[test]
fn privacy_pages_open_the_right_settings() {
    let rec = fixture();
    let pages = opener(&rec);
    for page in [PrivacyPage::Screen, PrivacyPage::Camera, PrivacyPage::Mic] {
        rec.state
            .open_privacy(page, &|url| pages.open(url))
            .unwrap();
    }
    let urls: Vec<String> = pages.opened().into_iter().map(|(url, _)| url).collect();
    assert_eq!(
        urls,
        [
            SCREEN_PAGE,
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera",
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        ]
    );
    assert_eq!(
        rec.screen_requests.load(Ordering::SeqCst),
        0,
        "opening a page never asks"
    );
    // A page that couldn't open says so.
    let failed = rec
        .state
        .open_privacy(PrivacyPage::Mic, &|_| {
            Err(std::io::ErrorKind::NotFound.into())
        })
        .unwrap_err();
    assert_eq!(failed.code, "capture.permission.settingsDidntOpen");
}

#[test]
fn still_refused_after_reopen_the_step_explains_until_it_is_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let setup_with = |screen| {
        let (parts, _world) = world(
            dir.path(),
            Script {
                screen_permission: screen,
                ..Script::default()
            },
        );
        super::state::RecorderState::build(false, || parts)
            .setup()
            .unwrap()
    };
    assert!(
        !setup_with(Permission::NotAsked).screen_refused_after_reopen,
        "the first ask"
    );
    // Reopen Klipolo went ahead, and macOS still says no after the relaunch.
    let (parts, _world) = world(dir.path(), Script::default());
    let state = super::state::RecorderState::build(false, || parts);
    state.reopen(&|| false, || Ok(()), || ()).unwrap();
    assert!(setup_with(Permission::NotAsked).screen_refused_after_reopen);
    assert!(setup_with(Permission::Denied).screen_refused_after_reopen);
    // Allowed: nothing to explain, and a later refusal starts again from the first ask.
    assert!(!setup_with(Permission::Granted).screen_refused_after_reopen);
    assert!(!setup_with(Permission::NotAsked).screen_refused_after_reopen);
}

/// Klipolo opens again just after Reopen Klipolo: the page gets the project open then, once, to open it again with the
/// Record dialog. From the start screen there is none.
#[test]
fn back_from_reopen_klipolo_gives_the_project_that_was_open_once() {
    use super::types::BackFromReopen;
    let dir = tempfile::tempdir().unwrap();
    let launch = || {
        let (parts, world) = world(dir.path(), Script::default());
        (super::state::RecorderState::build(false, || parts), world)
    };
    let (state, world) = launch();
    assert_eq!(state.back_from_reopen(), None, "an ordinary launch");
    state.reopen(&|| false, || Ok(()), || ()).unwrap();
    let file = world.project.lock().unwrap().clone().unwrap().file;
    let (again, again_world) = launch();
    let project = Some(file.to_string_lossy().into_owned());
    assert_eq!(again.back_from_reopen(), Some(BackFromReopen { project }));
    assert_eq!(again.back_from_reopen(), None, "once");
    // Pressed on the start screen.
    *again_world.project.lock().unwrap() = None;
    again.reopen(&|| false, || Ok(()), || ()).unwrap();
    let (back, _) = launch();
    assert_eq!(
        back.back_from_reopen(),
        Some(BackFromReopen { project: None })
    );
}

/// A launch long after Reopen Klipolo isn't its reopen, and the memory stays for the setup's "still refused".
#[test]
fn a_launch_long_after_reopen_klipolo_is_not_its_reopen() {
    use super::screen_reopen::{back_from, refused_after_reopen, remember};
    let dir = tempfile::tempdir().unwrap();
    let files = super::setup::Files {
        dir: dir.path().join("recorder"),
    };
    let tour = std::path::Path::new("/p/Tour.klipolo");
    remember(&files, 1_000, Some(tour)).unwrap();
    let back = back_from(&files, 1_000 + 120_000).unwrap();
    assert_eq!(back.to.project.as_deref(), Some("/p/Tour.klipolo"));
    assert!(!back.once_allowed);
    assert_eq!(back_from(&files, 1_000 + 120_001), None);
    assert!(refused_after_reopen(&files, Permission::Denied));
}

/// macOS's own alert: once Klipolo is turned on in Screen Recording's list, its "Quit & Reopen" quits Klipolo with no
/// word beforehand, then opens it again. A run that found Screen Recording refused while the person chose what to
/// record (the Record dialog's step showed, or they pressed Allow) comes back at the next launch to the project open
/// then, with the Record dialog, once Screen Recording is allowed. Once only: the launch after is an ordinary one.
#[test]
fn quit_and_reopen_from_macos_comes_back_to_the_project_once_allowed() {
    use super::types::BackFromReopen;
    let dir = tempfile::tempdir().unwrap();
    let launch = |screen| {
        let script = Script {
            screen_permission: screen,
            ..Script::default()
        };
        let (parts, world) = world(dir.path(), script);
        (super::state::RecorderState::build(false, || parts), world)
    };
    // The Record dialog's step shows; the person turns Klipolo on in System Settings and picks macOS's Quit & Reopen.
    let (state, world) = launch(Permission::NotAsked);
    state.setup().unwrap();
    state.exits();
    let file = world.project.lock().unwrap().clone().unwrap().file;
    let project = Some(file.to_string_lossy().into_owned());
    let (back, _) = launch(Permission::Granted);
    assert_eq!(back.back_from_reopen(), Some(BackFromReopen { project }));
    assert_eq!(back.back_from_reopen(), None, "once");
    back.setup().unwrap();
    back.exits();
    let (ordinary, _) = launch(Permission::Granted);
    assert_eq!(ordinary.back_from_reopen(), None, "the launch after");
    // From the start screen: Allow pressed, then Quit & Reopen.
    let (state, world) = launch(Permission::NotAsked);
    *world.project.lock().unwrap() = None;
    state.allow_screen(&|_| Ok(())).unwrap();
    state.exits();
    let (back, _) = launch(Permission::Granted);
    assert_eq!(
        back.back_from_reopen(),
        Some(BackFromReopen { project: None })
    );
    // Record refused for it (no setup read since it was allowed): the same.
    let (state, _) = launch(Permission::Denied);
    assert!(state.arm(&options_screen()).is_err());
    state.exits();
    let (back, _) = launch(Permission::Granted);
    assert!(back.back_from_reopen().is_some());
}

/// A launch is back from a quit only when that quit's run found Screen Recording refused, and it is allowed now: never
/// when the dialog never asked, when it read Screen Recording allowed, or when it is still refused (and the memory goes
/// with that launch). It never makes the step say Screen Recording is still refused after Reopen Klipolo.
#[test]
fn any_other_quit_is_followed_by_an_ordinary_launch() {
    use super::screen_reopen::{back_from, refused_after_reopen, remember_asked};
    let dir = tempfile::tempdir().unwrap();
    let launch = |screen| {
        let script = Script {
            screen_permission: screen,
            ..Script::default()
        };
        let (parts, world) = world(dir.path(), script);
        (super::state::RecorderState::build(false, || parts), world)
    };
    // The Record dialog never opened.
    let (state, _) = launch(Permission::NotAsked);
    state.exits();
    assert_eq!(launch(Permission::Granted).0.back_from_reopen(), None);
    // It opened with Screen Recording allowed: no step.
    let (state, _) = launch(Permission::Granted);
    state.setup().unwrap();
    state.exits();
    assert_eq!(launch(Permission::Granted).0.back_from_reopen(), None);
    // The step showed, and Screen Recording is still refused at the next launch: an ordinary one, as is the one after.
    let (state, _) = launch(Permission::NotAsked);
    state.setup().unwrap();
    state.exits();
    let (still, _) = launch(Permission::Denied);
    assert_eq!(still.back_from_reopen(), None);
    assert!(!still.setup().unwrap().screen_refused_after_reopen);
    assert_eq!(launch(Permission::Granted).0.back_from_reopen(), None);
    // Long after the quit: not back.
    let files = super::setup::Files {
        dir: dir.path().join("recorder"),
    };
    let tour = std::path::Path::new("/p/Tour.klipolo");
    remember_asked(&files, 1_000, Some(tour)).unwrap();
    assert_eq!(back_from(&files, 1_000 + 120_001), None);
    assert_eq!(back_from(&files, 1_000), None, "taken by the launch before");
    remember_asked(&files, 1_000, Some(tour)).unwrap();
    assert!(!refused_after_reopen(&files, Permission::Denied));
    let back = back_from(&files, 1_000 + 120_000).unwrap();
    assert_eq!(back.to.project.as_deref(), Some("/p/Tour.klipolo"));
    assert!(back.once_allowed);
}
