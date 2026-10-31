//! The Record dialog's previews: only what is allowed starts; the person's Allow press starts the camera's or the mic's
//! preview once; the screen's wait for Screen Recording and launch nothing; a new choice moves only the live picture;
//! every preview stops when Klipolo goes behind and when a take arms, before its worker is launched; and in a test run
//! none starts. The stills start at most once a second, never again by themselves once the system refused the screen,
//! and each says when it came. Fake workers and a fake page only: nothing captures and nothing asks.

use klipolo_capture::backend::{Permission, Permissions};
use klipolo_capture::protocol::{Command, Event};

use super::frames::now_ms;
use super::state::RecorderState;
use super::test_launch::Logged;
use super::test_previews::*;
use super::test_support::*;
use super::types::{PreviewId, PreviewKind, Previews, PrivacyPage};

const THUMBS: &str = "--thumbs";
const LIVE: &str = "--live-view";
const CAMERA: &str = "--check-camera";
const METER: &str = "--meter";

#[test]
fn previews_start_only_what_is_granted() {
    let p = previewing(Permissions {
        camera: Permission::NotAsked,
        ..all_allowed()
    });
    p.set(everything()).unwrap();
    assert_eq!(p.launched(THUMBS).len(), 1);
    assert_eq!(p.launched(LIVE).len(), 1);
    assert_eq!(
        p.launched(METER),
        [["--record-worker", "--meter", "mic-1"]],
        "allowed already: no --ask"
    );
    assert!(
        p.launched(CAMERA).is_empty(),
        "the camera waits for its press"
    );
    // Allowed later, in System Settings: the next ask starts the camera's, and keeps the others.
    p.allow(|a| a.camera = Permission::Granted);
    p.set(everything()).unwrap();
    assert_eq!(
        p.launched(CAMERA),
        [["--record-worker", "--check-camera", "camera-1"]]
    );
    assert_eq!(p.launch.spawned(), 4);
    assert!(p.launch.commands().is_empty(), "nothing else stopped");
    // A refused mic never opens, even on the press.
    p.allow(|a| a.mic = Permission::Denied);
    let refused_mic = Previews {
        mic: Some("mic-2".into()),
        ask: Some(PrivacyPage::Mic),
        ..everything()
    };
    p.set(refused_mic).unwrap();
    assert_eq!(p.told(p.worker(METER)), [Command::Stop], "mic-1's meter");
    assert_eq!(p.launched(METER).len(), 1);
}

#[test]
fn ask_camera_starts_the_camera_preview_once() {
    let p = previewing(Permissions {
        camera: Permission::NotAsked,
        mic: Permission::NotAsked,
        ..all_allowed()
    });
    let want = Previews {
        camera: Some("camera-1".into()),
        mic: Some("mic-1".into()),
        ..nothing()
    };
    // Opening the dialog asks for nothing: each row offers its Allow.
    p.set(want.clone()).unwrap();
    assert_eq!(p.launch.spawned(), 0);
    // "Allow camera": the camera's preview starts, and macOS asks now.
    let allow_camera = Previews {
        ask: Some(PrivacyPage::Camera),
        ..want.clone()
    };
    p.set(allow_camera.clone()).unwrap();
    assert_eq!(p.launched(CAMERA).len(), 1);
    assert!(p.launched(METER).is_empty());
    // The same press again, and the asks after it while macOS asks, keep that one preview.
    p.set(allow_camera.clone()).unwrap();
    p.set(want.clone()).unwrap();
    assert_eq!(p.launched(CAMERA).len(), 1);
    assert!(p.told(p.worker(CAMERA)).is_empty());
    // "Allow microphone": the meter opens, and may ask.
    let allow_mic = Previews {
        ask: Some(PrivacyPage::Mic),
        ..want.clone()
    };
    p.set(allow_mic).unwrap();
    assert_eq!(
        p.launched(METER),
        [["--record-worker", "--meter", "mic-1", "--ask"]]
    );
    // Refused in macOS's question: the camera's preview ends and says why, and another press opens nothing.
    let camera = p.worker(CAMERA);
    p.allow(|a| a.camera = Permission::Denied);
    p.says(camera, failed("capture.permission.camera"));
    p.launch.exit_to(camera, Some(1));
    assert_eq!(
        p.heard(),
        [(PreviewId::Camera, failed("capture.permission.camera"))]
    );
    p.set(allow_camera).unwrap();
    assert_eq!(p.launched(CAMERA).len(), 1);
}

#[test]
fn screen_previews_are_refused_until_granted_and_launch_nothing() {
    let p = previewing(Permissions {
        screen: Permission::NotAsked,
        ..all_allowed()
    });
    let screen_only = Previews {
        thumbs: true,
        live: Some(screen()),
        ..nothing()
    };
    let refused = p.set(screen_only).unwrap_err();
    assert_eq!(refused.code, "capture.permission.screen");
    assert_eq!(p.launch.spawned(), 0, "no worker");
    // The camera and the mic don't wait for the screen.
    let refused = p.set(everything()).unwrap_err();
    assert_eq!(refused.code, "capture.permission.screen");
    assert!(p.launched(THUMBS).is_empty() && p.launched(LIVE).is_empty());
    assert_eq!(p.launched(CAMERA).len() + p.launched(METER).len(), 2);
    // Refused outright: the same.
    p.allow(|a| a.screen = Permission::Denied);
    assert!(p.set(everything()).is_err());
    assert_eq!(p.launch.spawned(), 2);
    // Allowed (and Klipolo reopened): they start, and the camera and the mic go on.
    p.allow(|a| a.screen = Permission::Granted);
    p.set(everything()).unwrap();
    assert_eq!(p.launched(THUMBS).len(), 1);
    assert_eq!(p.launched(LIVE).len(), 1);
    assert_eq!(p.launch.spawned(), 4);
    assert!(p.launch.commands().is_empty());
}

#[test]
fn changing_the_choice_moves_only_the_live_view() {
    let p = previewing(all_allowed());
    p.set(everything()).unwrap();
    let (thumbs, live) = (p.worker(THUMBS), p.worker(LIVE));
    let (camera, meter) = (p.worker(CAMERA), p.worker(METER));
    let on_notes = Previews {
        live: Some(notes()),
        ..everything()
    };
    p.set(on_notes.clone()).unwrap();
    assert_eq!(p.told(live), [Command::Stop]);
    for other in [thumbs, camera, meter] {
        assert!(p.told(other).is_empty(), "worker {other} kept");
    }
    let target = serde_json::json!({ "kind": "app", "appId": NOTES, "display": "display-1" });
    let args = p.launch.args_of(p.worker(LIVE));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&args[2]).unwrap(),
        target
    );
    assert_eq!(p.launch.spawned(), 5);
    // The same choice again keeps it.
    p.set(on_notes).unwrap();
    assert_eq!(p.launch.spawned(), 5);
    // A frame of the earlier choice, still closing, never reaches the page; the new one's does, under its card.
    p.says(live, frame());
    p.says(p.worker(LIVE), frame());
    let frames = p.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].0.kind, PreviewKind::Live);
    assert_eq!(frames[0].0.key, format!("app:{NOTES}"));
    assert_eq!(frames[0].1, JPEG);
}

#[test]
fn leaving_the_window_stops_every_preview() {
    let p = previewing(all_allowed());
    p.set(everything()).unwrap();
    let workers = [THUMBS, LIVE, CAMERA, METER].map(|flag| p.worker(flag));
    p.state.window_in_front(false);
    for worker in workers {
        assert_eq!(p.told(worker), [Command::Stop], "worker {worker}");
    }
    // What they still send never reaches the page.
    p.says(workers[0], still("display:display-1"));
    p.says(workers[1], frame());
    p.says(workers[2], frame());
    let meter = Event::Meter {
        source: "mic".into(),
        peak_dbfs: -12.0,
    };
    p.says(workers[3], meter);
    assert!(p.frames().is_empty() && p.heard().is_empty());
    // Behind, a late ask starts nothing.
    p.set(everything()).unwrap();
    assert_eq!(p.launch.spawned(), 4);
    // In front again, the page's ask starts them again (the stills on their next turn).
    p.state.window_in_front(true);
    p.set(everything()).unwrap();
    assert_eq!(p.launch.spawned(), 7);
    p.advance_ms(1_000);
    assert_eq!(p.launch.spawned(), 8);
    assert_eq!(p.launched(THUMBS).len(), 2);
    // The four that were told to stop and didn't are killed.
    p.advance_ms(1_000);
    assert_eq!(p.launch.kills(), [true; 4]);
}

#[test]
fn arming_stops_every_preview_before_the_take_spawns() {
    let p = previewing(all_allowed());
    p.set(everything()).unwrap();
    let workers = [THUMBS, LIVE, CAMERA, METER].map(|flag| p.worker(flag));
    let take = p.arm(options_screen()).unwrap();
    let log = p.launch.log();
    let launched = (log.iter())
        .position(|l| matches!(l, Logged::Spawn(args) if !args[1].starts_with("--")))
        .expect("the take's worker");
    for worker in workers {
        let stopped = log
            .iter()
            .position(|l| *l == Logged::Command(worker, Command::Stop));
        assert!(
            stopped.is_some_and(|at| at < launched),
            "worker {worker}: {log:?}"
        );
    }
    // While the take is the recorder's, nothing starts, and the page hears why.
    let refused = p.set(everything()).unwrap_err();
    assert_eq!(refused.code, "capture.busy");
    assert_eq!(p.launch.spawned(), 5);
    // A frame a closing preview still sends never reaches the page.
    p.says(workers[2], frame());
    assert!(p.frames().is_empty());
    // Record: the take's own Start goes only to the take.
    p.send(Command::Start).unwrap();
    assert_eq!(p.launch.commands().last(), Some(&Command::Start));
    assert!(!take.id.starts_with("preview-"));
}

#[test]
fn test_run_starts_no_preview() {
    let state = RecorderState::build(true, || panic!("a test run makes no recorder"));
    let page = std::sync::Arc::new(Page::default());
    let refused = state.previews(&everything(), page).unwrap_err();
    assert_eq!(refused.code, "capture.error.testRun");
    // The stops have nothing to stop.
    state.previews_off();
    state.window_in_front(false);
    state.previews_exit();
}

#[test]
fn stills_start_at_most_once_a_second() {
    let p = previewing(all_allowed());
    let thumbs = Previews {
        thumbs: true,
        ..nothing()
    };
    p.set(thumbs.clone()).unwrap();
    // Every display and app of the setup, each under its card's key.
    let targets = serde_json::json!([
        ["display:display-1", { "kind": "display", "id": "display-1" }],
        [format!("app:{NOTES}"), { "kind": "app", "appId": NOTES, "display": "display-1" }],
    ]);
    let args = p.launch.args_of(p.worker(THUMBS));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&args[2]).unwrap(),
        targets
    );
    // Off and on again at once: the next one waits its turn, a second after the last one started.
    p.state.previews_off();
    p.set(thumbs.clone()).unwrap();
    assert_eq!(p.launched(THUMBS).len(), 1);
    p.advance_ms(600);
    assert_eq!(p.launched(THUMBS).len(), 1);
    p.advance_ms(400);
    assert_eq!(p.launched(THUMBS).len(), 2);
    // A turn waited for ends with the dialog: nothing starts after it closed.
    p.state.previews_off();
    p.set(thumbs).unwrap();
    p.state.previews_off();
    p.advance_ms(2_000);
    assert_eq!(p.launched(THUMBS).len(), 2);
}

#[test]
fn stills_refused_by_the_system_never_start_again_by_themselves() {
    let p = previewing(all_allowed());
    p.set(everything()).unwrap();
    let (thumbs, live) = (p.worker(THUMBS), p.worker(LIVE));
    // The person said no to macOS's question while the stills ran.
    p.says(thumbs, failed("capture.permission.screen"));
    p.launch.exit_to(thumbs, Some(1));
    assert_eq!(
        p.heard(),
        [(PreviewId::Thumbs, failed("capture.permission.screen"))]
    );
    p.advance_ms(3_000);
    assert_eq!(p.launched(THUMBS).len(), 1);
    // The page's next ask doesn't start them, nor keeps the live picture: the question would come again.
    let refused = p.set(everything()).unwrap_err();
    assert_eq!(refused.code, "capture.permission.screen");
    assert_eq!(p.launched(THUMBS).len(), 1);
    assert_eq!(p.told(live), [Command::Stop]);
    assert!(p.told(p.worker(CAMERA)).is_empty(), "the camera goes on");
    // The dialog reopens (it reads its setup again): they start again.
    p.state.setup().unwrap();
    p.set(everything()).unwrap();
    assert_eq!(p.launched(THUMBS).len(), 2);
    assert_eq!(p.launched(LIVE).len(), 2);
}

#[test]
fn each_still_says_when_it_came() {
    let p = previewing(all_allowed());
    p.set(Previews {
        thumbs: true,
        ..nothing()
    })
    .unwrap();
    let before = now_ms();
    p.says(p.worker(THUMBS), still(&format!("app:{NOTES}")));
    let after = now_ms();
    let frames = p.frames();
    assert_eq!(frames.len(), 1);
    let (header, jpeg) = &frames[0];
    assert_eq!(header.kind, PreviewKind::Thumb);
    assert_eq!(header.key, format!("app:{NOTES}"));
    assert!((before..=after).contains(&header.at_ms), "{header:?}");
    assert_eq!(jpeg, &JPEG);
}

#[test]
fn the_mic_s_peaks_reach_the_page_as_its_events() {
    let p = previewing(all_allowed());
    p.set(Previews {
        mic: Some("mic-1".into()),
        ..nothing()
    })
    .unwrap();
    let peak = Event::Meter {
        source: "mic".into(),
        peak_dbfs: -6.0,
    };
    p.says(p.worker(METER), peak.clone());
    // A meter that ends by itself without saying why: the page isn't left with its last level.
    p.launch.exit_to(p.worker(METER), Some(0));
    let ended = p.heard();
    assert_eq!(ended[0], (PreviewId::Mic, peak));
    assert!(
        matches!(&ended[1], (PreviewId::Mic, Event::Failed { error }) if error.code == "capture.error.os")
    );
    assert!(p.frames().is_empty());
}
