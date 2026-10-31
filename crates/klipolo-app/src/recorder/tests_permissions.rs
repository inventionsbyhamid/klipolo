//! The recorder's permission steps, each only on the person's own press: Allow screen recording asks and then opens its
//! settings page, Reopen Klipolo saves the project before it restarts and is refused while a take runs, and Open
//! privacy settings opens the right page. After Reopen Klipolo, a setup that still reads Screen Recording as not
//! allowed says so. The fake backend and fake workers only: nothing is asked of the OS, nothing opens and nothing
//! quits.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use klipolo_capture::backend::Permission;
use klipolo_capture::protocol::Command;
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
    // Opening the dialog, arming and recording never ask.
    rec.state.setup().unwrap();
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.send(Command::Stop).unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
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
            || {
                steps.lock().unwrap().push("flush");
                Ok(())
            },
            || steps.lock().unwrap().push("restart"),
        )
    };
    reopen().unwrap();
    assert_eq!(*steps.lock().unwrap(), ["flush", "restart"]);
    // A failed save keeps Klipolo open, with what went wrong.
    let failed = rec.state.reopen(
        || Err(Said::Text("The disk is full.".into())),
        || panic!("never restarts with unsaved edits"),
    );
    assert_eq!(failed.unwrap_err(), Said::Text("The disk is full.".into()));
    // Armed through saving: refused, nothing saved or restarted.
    steps.lock().unwrap().clear();
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
    assert_eq!(failed.code, "capture.error.os");
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
    state.reopen(|| Ok(()), || ()).unwrap();
    assert!(setup_with(Permission::NotAsked).screen_refused_after_reopen);
    assert!(setup_with(Permission::Denied).screen_refused_after_reopen);
    // Allowed: nothing to explain, and a later refusal starts again from the first ask.
    assert!(!setup_with(Permission::Granted).screen_refused_after_reopen);
    assert!(!setup_with(Permission::NotAsked).screen_refused_after_reopen);
}
