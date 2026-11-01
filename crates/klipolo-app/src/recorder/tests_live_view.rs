//! The live picture's tests: it opens with Screen Recording allowed and never launches without it; it closes when a take
//! arms and never opens during one; a new choice restarts it and the same one keeps it; it closes when the window goes
//! behind and its frames stop; one that doesn't stop is killed, and none outlives Klipolo; one whose worker ends by
//! itself says so; and it opens for its box's pixels and follows a new size without another worker, while the cards
//! stay small. Fake workers only: nothing captures.

use klipolo_capture::backend::{Permission, Permissions, Target};
use klipolo_capture::live_view_size::ViewPixels;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::shell::WorkerState;

use super::test_previews::*;
use super::test_support::*;
use super::types::{PreviewId, PreviewKind, Previews, WhatToRecord};

const LIVE: &str = "--live-view";

/// The live picture of `what` alone: the small picture.
fn live(what: WhatToRecord) -> Previews {
    Previews {
        live: Some(what),
        ..nothing()
    }
}

/// The live picture of `what` alone, for a box of `width` × `height` device pixels.
fn live_in(what: WhatToRecord, width: u32, height: u32) -> Previews {
    Previews {
        live_pixels: Some(pixels(width, height)),
        ..live(what)
    }
}

fn pixels(width: u32, height: u32) -> ViewPixels {
    ViewPixels { width, height }
}

/// The box's size a live view's worker was launched for: its last argument after the target.
fn launched_for(args: &[String]) -> Option<ViewPixels> {
    assert_eq!(args[..2], ["--record-worker", LIVE]);
    (args.get(3)).map(|view| serde_json::from_str(view).expect("a view's size as JSON"))
}

#[test]
fn it_opens_for_its_box_s_own_pixels() {
    // The dialog's 672 × 378 box on a 2x display.
    let p = previewing(all_allowed());
    p.set(live_in(screen(), 1344, 756)).unwrap();
    let args = p.launch.last_args();
    assert_eq!(args.len(), 4, "the target, then the box's size");
    assert_eq!(launched_for(&args), Some(pixels(1344, 756)));
    assert!(p.told(p.worker(LIVE)).is_empty());
}

#[test]
fn a_new_size_goes_to_the_running_picture_and_never_opens_another() {
    let p = previewing(all_allowed());
    p.set(live_in(screen(), 1344, 756)).unwrap();
    let view = p.worker(LIVE);
    // The window got narrower: its worker is told once, and the same size again says nothing.
    p.set(live_in(screen(), 1200, 676)).unwrap();
    p.set(live_in(screen(), 1200, 676)).unwrap();
    let narrower = Command::ViewSize {
        view: Some(pixels(1200, 676)),
    };
    assert_eq!(p.told(view), [narrower.clone()]);
    // No size any more: the small picture.
    p.set(live(screen())).unwrap();
    assert_eq!(p.told(view), [narrower, Command::ViewSize { view: None }]);
    p.advance_ms(500);
    assert_eq!(p.launch.spawned(), 1, "never another worker");
    // Its frames still reach the page.
    p.says(view, frame());
    assert_eq!(p.frames().len(), 1);
}

#[test]
fn a_launch_waiting_its_turn_opens_at_the_latest_size() {
    let p = previewing(all_allowed());
    p.set(live_in(screen(), 1344, 756)).unwrap();
    let first = p.worker(LIVE);
    p.set(live_in(notes(), 1344, 756)).unwrap();
    p.set(live_in(notes(), 1200, 676)).unwrap();
    assert_eq!(p.launch.spawned(), 1, "it waits its turn");
    p.advance_ms(500);
    assert_eq!(p.launch.spawned(), 2);
    assert_eq!(launched_for(&p.launch.last_args()), Some(pixels(1200, 676)));
    assert_eq!(
        p.told(first),
        [Command::Stop],
        "the first is only told to stop"
    );
    assert!(p.told(p.worker(LIVE)).is_empty());
}

#[test]
fn a_size_never_opens_a_picture() {
    // Its worker ended by itself: a new size opens nothing until the dialog opens again.
    let p = previewing(all_allowed());
    p.set(live_in(screen(), 1344, 756)).unwrap();
    p.launch.exit(None);
    p.advance_ms(500);
    p.set(live_in(screen(), 1200, 676)).unwrap();
    assert_eq!(p.launch.spawned(), 1);
    // Behind: nothing starts, whatever the size.
    let p = previewing(all_allowed());
    p.state.window_in_front(false);
    p.set(live_in(screen(), 1344, 756)).unwrap();
    assert_eq!(p.launch.spawned(), 0);
    // Without Screen Recording: refused, and nothing starts.
    let p = previewing(Permissions {
        screen: Permission::NotAsked,
        ..all_allowed()
    });
    let refused = p.set(live_in(screen(), 1344, 756)).unwrap_err();
    assert_eq!(refused.code, "capture.permission.screen");
    assert_eq!(p.launch.spawned(), 0);
}

#[test]
fn a_sized_picture_still_stops_on_close_and_on_arming() {
    let p = previewing(all_allowed());
    p.set(live_in(screen(), 1344, 756)).unwrap();
    let view = p.worker(LIVE);
    p.state.previews_off();
    assert_eq!(p.told(view), [Command::Stop], "the dialog closed");
    p.advance_ms(3_000);
    assert_eq!(p.launch.kills(), [true], "hard-stopped after the wait");
    p.set(live_in(screen(), 1344, 756)).unwrap();
    assert_eq!(p.launch.spawned(), 2);
    p.arm(options_screen()).unwrap();
    assert_eq!(p.launch.kills(), [true, true], "killed at arming");
}

#[test]
fn the_cards_stay_small() {
    let p = previewing(all_allowed());
    p.set(Previews {
        thumbs: true,
        ..live_in(screen(), 1344, 756)
    })
    .unwrap();
    let stills = p.launched("--thumbs");
    assert_eq!(stills.len(), 1);
    assert_eq!(stills[0].len(), 3, "the cards' targets alone: no size");
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
    assert_eq!(p.launch.kills(), [true], "killed at arming");
    assert_eq!(p.launch.commands(), [Command::Start]);
    assert!(p.told(view).is_empty());
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
    p.advance_ms(500);
    assert_eq!(p.launch.spawned(), 3);
}

#[test]
fn a_new_target_restarts_it_and_the_same_one_keeps_it() {
    let p = previewing(all_allowed());
    p.set(live(screen())).unwrap();
    p.set(live(screen())).unwrap();
    assert_eq!(p.launch.spawned(), 1);
    p.set(live(notes())).unwrap();
    p.advance_ms(500);
    assert_eq!(p.launch.spawned(), 2, "on its turn");
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
    // Gone: the page's next asks don't launch it again; another choice opens; the dialog opening again tries it again.
    p.advance_ms(500);
    p.set(live(screen())).unwrap();
    assert_eq!(p.launch.spawned(), 1);
    p.set(live(notes())).unwrap();
    assert_eq!(p.launch.spawned(), 2);
    p.state.setup().unwrap();
    p.advance_ms(500);
    p.set(live(screen())).unwrap();
    assert_eq!(p.launch.spawned(), 3);
}
