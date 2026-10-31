//! The live picture's tests: it opens with Screen Recording allowed and never launches without it; a new choice restarts
//! it and the same one keeps it; one that doesn't stop is killed; and one whose worker ends by itself says so. Fake
//! workers only: nothing captures.

use klipolo_capture::backend::{Permission, Permissions, Target};
use klipolo_capture::protocol::{Command, Event};

use super::test_previews::*;
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
fn a_view_that_doesn_t_stop_is_killed() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    p.set(nothing()).unwrap();
    p.advance_ms(3_000);
    assert_eq!(p.launch.kills(), [true], "hard-stopped after the wait");
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
