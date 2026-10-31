//! The live picture's tests: it opens with Screen Recording allowed and never launches without it; it closes when a take
//! arms and never opens during one; a new choice restarts it and the same one keeps it; it closes when the window goes
//! behind and its frames stop; one that doesn't stop is killed, and none outlives Klipolo; and one whose worker ends by
//! itself says so. Fake workers only: nothing captures.

use klipolo_capture::backend::{Permission, Permissions, Target};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::shell::WorkerState;

use super::test_previews::*;
use super::test_support::*;
use super::types::{PreviewId, PreviewKind, Previews};

const LIVE: &str = "--live-view";

/// The live picture of `what` alone.
fn live(what: super::types::WhatToRecord) -> Previews {
    Previews {
        live: Some(what),
        ..nothing()
    }
}

#[test]
fn it_opens_with_screen_recording_allowed_and_its_frames_reach_the_page() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    let args = p.launch.last_args();
    assert_eq!(args.len(), 3, "the small view: no size");
    assert_eq!(args[..2], ["--record-worker", LIVE]);
    let display = Target::Display {
        id: "display-1".into(),
    };
    assert_eq!(serde_json::from_str::<Target>(&args[2]).unwrap(), display);
    p.says(p.worker(LIVE), frame());
    let frames = p.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].0.kind, PreviewKind::Live);
    assert_eq!(frames[0].0.key, "display:display-1");
    assert_eq!(frames[0].1, JPEG);
    assert!(!p.state.active(), "it holds no mic or camera");
}

#[test]
fn it_never_launches_while_screen_recording_isn_t_allowed() {
    let p = previewing(Permissions {
        screen: Permission::NotAsked,
        ..all_allowed()
    });
    let refused = p.set(live(screen())).unwrap_err();
    assert_eq!(refused.code, "capture.permission.screen");
    assert_eq!(p.launch.spawned(), 0, "no worker");
}

#[test]
fn it_closes_when_a_take_arms_and_never_opens_during_one() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    let view = p.worker(LIVE);
    p.arm(options_screen()).unwrap();
    p.send(Command::Start).unwrap();
    assert_eq!(p.launch.commands(), [Command::Stop, Command::Start]);
    // A frame the closing view still sends never reaches the page.
    p.says(view, frame());
    assert!(p.frames().is_empty());
    // While a take runs, it never opens.
    let refused = p.set(live(screen())).unwrap_err();
    assert_eq!(refused.code, "capture.busy");
    assert_eq!(
        p.launch.spawned(),
        2,
        "the view's worker and the take's only"
    );
    // Once the take is over, it opens again.
    p.launch.emit(Event::State {
        state: WorkerState::Stopping,
        elapsed_us: 0,
    });
    p.launch.exit(Some(3));
    p.settle();
    p.set(live(screen())).unwrap();
    assert_eq!(p.launch.spawned(), 3);
}

#[test]
fn a_new_target_restarts_it_and_the_same_one_keeps_it() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    p.set(live(screen())).unwrap();
    assert_eq!(p.launch.spawned(), 1);
    p.set(live(notes())).unwrap();
    assert_eq!(p.launch.spawned(), 2);
    assert_eq!(p.launch.commands(), [Command::Stop], "the first view's");
    p.set(nothing()).unwrap();
    assert_eq!(p.launch.commands(), [Command::Stop, Command::Stop]);
}

#[test]
fn it_closes_when_the_window_goes_behind_and_its_frames_stop() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    p.state.window_in_front(false);
    assert_eq!(p.launch.commands(), [Command::Stop]);
    p.says(p.worker(LIVE), frame());
    assert!(p.frames().is_empty(), "a closing view's frame");
}

#[test]
fn a_view_that_doesn_t_stop_is_killed_and_none_outlives_the_app() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    p.set(nothing()).unwrap();
    p.advance_ms(3_000);
    assert_eq!(p.launch.kills(), [true], "hard-stopped after the wait");
    p.set(live(screen())).unwrap();
    p.state.previews_exit();
    assert_eq!(p.launch.kills(), [true, true], "at Klipolo's exit");
}

#[test]
fn a_view_whose_worker_ends_by_itself_says_so() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    p.launch.exit(None);
    let unavailable = |ev: &Event| matches!(ev, Event::Failed { error } if error.code == "capture.liveView.unavailable");
    let heard = p.heard();
    assert!(
        heard
            .iter()
            .any(|(id, ev)| *id == PreviewId::Live && unavailable(ev)),
        "{heard:?}"
    );
    // Gone: the next ask opens a new one.
    p.set(live(screen())).unwrap();
    assert_eq!(p.launch.spawned(), 2);
}
