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
    let exporting = rec
        .state
        .reopen(&|| true, || panic!("never saves"), || panic!("never restarts"));
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
