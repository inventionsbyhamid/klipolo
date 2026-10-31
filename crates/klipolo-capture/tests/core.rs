//! `SessionCore` in virtual time: start, the countdown and the gate, writing, pause,
//! Stop at awkward moments, heals, the shell's inputs, the disk and the writer's queue, the sync matrix, resume and the
//! read-back. No process is spawned here.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::journal::Entry;
use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::core::{Effect, PipeCmd};
use klipolo_capture::shell::{HotkeyAction, PowerEvent, ShellInput, WorkerState};

#[test]
fn a_four_source_take_writes_every_source_from_the_gate_on() {
    let mut h = Harness::new(cfg(
        &[Src::Screen, Src::Camera, Src::Mic, Src::GameSound],
        3,
    ));
    h.command(Command::Start);
    h.feed(0.0, 3.5);
    assert_eq!(h.state(), WorkerState::Recording);
    let t0 = h.gate_s();
    assert!((3.0..3.2).contains(&t0), "{t0}");
    h.feed(3.5, 63.0);
    h.command(Command::Stop);
    assert_eq!(
        h.file_keys(),
        [
            "camera/run-01.mp4",
            "cursor.jsonl",
            "game-sound/run-01.caf",
            "mic/run-01.caf",
            "screen/run-01.mp4",
            "timing.jsonl"
        ]
    );
    assert!(h.video_starts_on_a_keyframe("screen/run-01.mp4"));
    assert!(
        h.journal()
            .iter()
            .any(|e| matches!(e, Entry::StopRequested { .. }))
    );
    assert!(h.effects().iter().any(|e| matches!(e, Effect::Finalize)));
}

#[test]
fn pause_and_resume_leave_no_gap_and_restart_on_a_keyframe() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(10.0);
    h.shell(ShellInput::Hotkey(HotkeyAction::Pause));
    h.feed_for(5.0); // ignored
    h.command(Command::Resume);
    assert!(h.effects().iter().any(|e| matches!(
        e,
        Effect::Pipe {
            cmd: PipeCmd::Keyframe,
            ..
        }
    )));
    h.feed_for(10.0);
    assert_eq!(h.run_count("screen"), 1, "a pause is not a new run");
    assert_eq!(h.run_count("mic"), 1, "nor for sound");
    let span = h.video_span_s("screen/run-01.mp4");
    assert!((19.9..20.1).contains(&span), "{span}: paused time left out");
    assert!(h.video_starts_on_a_keyframe_after_resume("screen/run-01.mp4"));
}

#[test]
fn stop_during_the_countdown_leaves_no_take() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    h.feed(0.0, 1.5);
    h.shell(ShellInput::Hotkey(HotkeyAction::StartStop));
    assert!(h.file_keys().is_empty());
    assert!(h.effects().iter().any(|e| matches!(e, Effect::Exit(3))));
    assert!(
        !h.events()
            .iter()
            .any(|e| matches!(e, Event::Finished { .. }))
    );
}

#[test]
fn stop_twice_is_one_stop() {
    let mut h = Harness::recording(&[Src::Screen]);
    h.feed_for(2.0);
    h.command(Command::Stop);
    h.command(Command::Stop);
    h.shell(ShellInput::Hotkey(HotkeyAction::StartStop));
    assert_eq!(
        h.effects()
            .iter()
            .filter(|e| matches!(e, Effect::Finalize))
            .count(),
        1
    );
}

#[test]
fn a_hotkey_during_a_heal_stops_cleanly() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(2.0);
    h.pipe_event("screen", PipeEvent::StoppedBySystem); // a restart is under way
    h.shell(ShellInput::Hotkey(HotkeyAction::StartStop));
    assert_eq!(h.state(), WorkerState::Stopping);
    assert!(h.effects().iter().any(|e| matches!(e, Effect::StopPipes)));
}

#[test]
fn a_rule_firing_in_the_countdown_pauses_it() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.mic_signal_zeros();
    h.command(Command::Start);
    h.feed(0.0, 6.0);
    assert_eq!(
        h.state(),
        WorkerState::Countdown,
        "held while the mic sends nothing"
    );
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, Event::Gate { missing } if missing == &["mic".to_string()]))
    );
    h.command(Command::WithoutSource {
        source: "mic".into(),
    });
    h.feed(6.0, 9.5);
    assert_eq!(h.state(), WorkerState::Recording);
}

#[test]
fn the_game_losing_focus_pauses_after_500_ms() {
    let mut h =
        Harness::recording_game("com.studio.game", &[Src::Screen, Src::GameSound, Src::Mic]);
    h.feed_for(5.0);
    h.shell(ShellInput::Focus {
        app_id: Some("com.apple.Safari".into()),
    });
    h.feed_for(0.4);
    assert_eq!(h.state(), WorkerState::Recording);
    h.feed_for(0.2);
    assert_eq!(h.state(), WorkerState::Paused);
    h.shell(ShellInput::Focus {
        app_id: Some("com.studio.game".into()),
    });
    h.feed_for(0.1);
    assert_eq!(h.state(), WorkerState::Recording);
}

#[test]
fn a_game_that_quits_pauses_video_and_keeps_sound() {
    let mut h = Harness::recording_game("com.studio.game", &[Src::Screen, Src::Mic]);
    h.feed_for(5.0);
    h.shell(ShellInput::AppQuit {
        app_id: "com.studio.game".into(),
    });
    h.feed_for(5.0);
    assert_eq!(
        h.state(),
        WorkerState::Recording,
        "the mic and camera keep going"
    );
    h.shell(ShellInput::AppLaunched {
        app_id: "com.studio.game".into(),
    });
    h.feed_for(5.0);
    assert_eq!(h.run_count("screen"), 2);
    assert!(
        h.journal()
            .iter()
            .any(|e| matches!(e, Entry::Health { code, .. } if code == "capture.health.appQuit"))
    );
}

#[test]
fn sleep_pauses_and_a_long_sleep_stops() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(5.0);
    h.shell(ShellInput::Power(PowerEvent::WillSleep));
    assert_eq!(h.state(), WorkerState::Paused);
    h.shell(ShellInput::Power(PowerEvent::Woke {
        slept_ms: 11 * 60 * 1_000,
    }));
    assert_eq!(h.state(), WorkerState::Stopping);
    assert!(
        h.journal()
            .iter()
            .any(|e| matches!(e, Entry::Health { code, .. } if code == "capture.stop.slept"))
    );
}

#[test]
fn a_take_that_reaches_the_floor_stops_and_saves() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(3.0);
    h.free_bytes(1_900_000_000);
    h.tick();
    assert_eq!(h.state(), WorkerState::Stopping);
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, Event::Health { note, .. } if note.code == "capture.stop.disk"))
    );
}

#[test]
fn a_slow_drive_stops_and_saves_at_20_seconds_queued() {
    let mut h = Harness::recording(&[Src::Screen]);
    h.queued_ms(6_000);
    h.tick();
    assert!(h.events().iter().any(
        |e| matches!(e, Event::Health { note, .. } if note.code == "capture.health.slowDrive")
    ));
    h.queued_ms(20_000);
    h.tick();
    assert_eq!(h.state(), WorkerState::Stopping);
}

#[test]
fn a_lost_mic_switches_to_the_built_in_one_as_a_new_run() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(5.0);
    h.pipe_event("mic", PipeEvent::DeviceLost);
    h.feed_for(3.1);
    assert!(
        h.effects()
            .iter()
            .any(|e| matches!(e, Effect::SwapMic { device, .. } if device == "MacBook Microphone"))
    );
    h.feed_for(2.0);
    assert_eq!(h.run_count("mic"), 2);
}

#[test]
fn the_sync_matrix_stays_within_ten_ms_after_drift_correction() {
    let mut h = Harness::recording_with_drift(&[Src::Screen, Src::Mic], 50.0);
    h.feed_for(600.0);
    h.command(Command::Stop);
    let worst_ms = h.sync_error_ms("screen", "mic");
    println!("the worst flash-to-blip error after drift correction: {worst_ms:.3} ms");
    assert!(worst_ms <= 10.0, "{worst_ms} ms after drift correction");
}

#[test]
fn a_dropped_packet_waits_for_the_next_keyframe() {
    // The fake screen pipe sends through a PacketGate on a channel of 2; the harness stalls it for 10 frames.
    let mut h = Harness::recording(&[Src::Screen]);
    h.feed_for(1.0);
    h.stall_packets("screen", 10);
    h.feed_for(2.0);
    h.command(Command::Stop);
    assert!(
        h.no_delta_after_a_gap("screen/run-01.mp4"),
        "after a drop, the next frame written is a keyframe"
    );
    assert!(
        h.effects().iter().any(|e| matches!(
            e,
            Effect::Pipe {
                cmd: PipeCmd::Keyframe,
                ..
            }
        )),
        "the gate asked for one"
    );
}

#[test]
fn a_resumed_worker_continues_the_session_clock_with_new_runs() {
    // The journal of a crashed take: gate at host 100 s, a pause from 110 to 115 s, runs 1 of screen and mic, no end.
    let from = common::harness::resumed_from(
        &[("screen", 1), ("mic", 1)],
        100.0,
        &[(110.0, 115.0)],
        &[("screen", 12.0), ("mic", 12.2)],
    );
    let mut h = Harness::resume(&[Src::Screen, Src::Mic], from, 130.0);
    assert_eq!(h.state(), WorkerState::Recording, "no countdown, no gate");
    h.feed_for(1.0);
    assert_eq!(h.file_keys_with_prefix("screen/"), ["screen/run-02.mp4"]);
    // Host 130 s is session 25 s (100 s gate, 5 s paused): the new run starts there.
    assert_eq!(h.run_offset_us("screen", 2) / 100_000, 250);
    assert!(
        h.journal()
            .iter()
            .any(|e| matches!(e, Entry::Health { code, .. } if code == "capture.note.gap"))
    );
}

#[test]
fn the_read_back_flags_a_damaged_file() {
    use klipolo_capture::readback;
    use klipolo_capture::synthetic::{FakeBackend, Script};
    let root = tempfile::tempdir().unwrap();
    let backend = FakeBackend::new(Script::default());
    let good = common::take::write_take(root.path(), "good", common::take::Take::default());
    assert_eq!(
        readback::check(&good.dir, &backend),
        vec![],
        "an undamaged take reads back"
    );
    let bad = common::take::write_take(root.path(), "bad", common::take::Take::default());
    let screen = bad.dir.path("screen/run-01.mp4");
    let mut bytes = std::fs::read(&screen).unwrap();
    bytes[..600].fill(0);
    std::fs::write(&screen, bytes).unwrap();
    let problems = readback::check(&bad.dir, &backend);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(
        (problems[0].source.as_str(), problems[0].what),
        ("screen", "head")
    );
    // The core makes it one critical note.
    let mut h = Harness::recording(&[Src::Screen]);
    let fx = h.core.read_back(problems, h.now);
    h.apply(fx);
    let notes: Vec<_> = h
        .events()
        .iter()
        .filter(
            |e| matches!(e, Event::Health { note, .. } if note.code == "capture.health.readBack"),
        )
        .collect();
    assert_eq!(notes.len(), 1);
}
