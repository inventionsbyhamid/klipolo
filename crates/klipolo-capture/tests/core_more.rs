//! The core's finer rules, one test each, in virtual time: the
//! mic's own device coming back, a gated mic, the rules' clock while paused, sound across pauses, one stall one restart
//! and a slow first frame, a sleeping display (and one that slept in a pause), a blank screen at the gate, the low-disk
//! note, a silent game sound, a full writer queue, and nothing before Record.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg, mics};
use klipolo_capture::journal::Entry;
use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::core::{Effect, PipeCmd};
use klipolo_capture::shell::{
    Flash, HotkeyAction, Level, PowerEvent, ShellInput, ShellOutput, WorkerState,
};
use klipolo_capture::synthetic::Signal;
use klipolo_capture::writer::WriterError;

fn notes(h: &Harness, code: &str) -> usize {
    h.events()
        .iter()
        .filter(|e| matches!(e, Event::Health { note, .. } if note.code == code))
        .count()
}

fn restarts(h: &Harness, source: &str) -> usize {
    h.effects()
        .iter()
        .filter(|e| matches!(e, Effect::Pipe { source: s, cmd: PipeCmd::Restart } if s == source))
        .count()
}

#[test]
fn a_mic_that_comes_back_records_on_the_person_s_own_mic_even_with_nothing_to_fall_back_to() {
    // A USB blip under 3 s: the same mic, a new run; never the built-in one.
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(5.0);
    h.pipe_event("mic", PipeEvent::DeviceLost);
    h.feed_for(2.0);
    h.pipe_event("mic", PipeEvent::DeviceBack);
    h.feed_for(3.0);
    assert!(
        !h.effects()
            .iter()
            .any(|e| matches!(e, Effect::SwapMic { .. })),
        "the person's own mic"
    );
    assert_eq!((restarts(&h, "mic"), h.run_count("mic")), (1, 2));

    // A Mac mini: no other mic. The mic waits for its own, says it is lost, and gets the voice back when it returns.
    let only_mine = mics().into_iter().filter(|m| m.id == "mic-usb").collect();
    let mut h = Harness::new_with_mics(cfg(&[Src::Screen, Src::Mic], 3), only_mine);
    h.start_recording();
    h.feed_for(5.0);
    h.pipe_event("mic", PipeEvent::DeviceLost);
    h.feed_for(8.0);
    assert!(
        !h.effects()
            .iter()
            .any(|e| matches!(e, Effect::SwapMic { .. }))
    );
    assert_eq!(notes(&h, "capture.health.lost"), 1);
    h.pipe_event("mic", PipeEvent::DeviceBack);
    h.feed_for(2.0);
    assert_eq!(h.run_count("mic"), 2, "the voice is back");
    let gap = h
        .journal()
        .iter()
        .any(|e| matches!(e, Entry::RunStarted { source, run: 2, .. } if source == "mic"));
    assert!(gap);
}

#[test]
fn a_gated_mic_warns_after_30_s_of_zeros_and_flashes_at_most_once_a_minute() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(5.0); // speech
    h.set_signal("mic", Signal::Zeros);
    h.feed_for(25.0);
    assert_eq!(
        notes(&h, "capture.health.micZeros"),
        0,
        "a gated mic's pause between phrases"
    );
    h.feed_for(6.0);
    assert_eq!(notes(&h, "capture.health.micZeros"), 1);
    h.set_signal("mic", Signal::Bursts { dbfs: -20.0 });
    h.feed_for(2.0);
    h.set_signal("mic", Signal::Zeros);
    h.feed_for(31.0);
    assert_eq!(
        notes(&h, "capture.health.micZeros"),
        2,
        "again after speech"
    );
    let flashes = h
        .shell_out
        .iter()
        .filter(|o| matches!(o, ShellOutput::Flash(Flash::Problem)))
        .count();
    assert_eq!(flashes, 1, "the HUD flashes for it once a minute at most");
}

#[test]
fn the_mic_rules_clock_stops_while_the_take_is_paused() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(3.0);
    h.command(Command::Pause);
    h.feed_for(70.0);
    h.command(Command::Resume);
    h.feed_for(2.0);
    for code in [
        "capture.health.micZeros",
        "capture.health.micQuiet",
        "capture.health.micNotHeard",
    ] {
        assert_eq!(notes(&h, code), 0, "{code}: paused time isn't silence");
    }
}

#[test]
fn the_status_lines_are_in_the_person_s_words() {
    // The item's lines come from the session's words, nested names included; English only where they word nothing
    //.
    let mut c = cfg(&[Src::Screen, Src::Mic], 3);
    c.words = [
        ("capture.source.mic", "Mikrofon"),
        ("capture.menu.fps", "{source}: {fps} B/s"),
    ]
    .map(|(code, words)| (code.to_string(), words.to_string()))
    .into();
    let mut h = Harness::recording_cfg(c);
    h.feed_for(2.0);
    let lines = (h.shell_out.iter().rev())
        .find_map(|o| match o {
            ShellOutput::Status(v) => Some(v.lines.clone()),
            _ => None,
        })
        .unwrap();
    assert!(lines.contains(&"Mikrofon".to_string()), "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Screen: ") && l.ends_with(" B/s")),
        "{lines:?}"
    );
}

#[test]
fn a_running_level_comes_after_the_tick_that_pushed_its_sound() {
    // A running level vouches for sound on the disk: the tick that pushes that sound goes to the writer first, as a
    // Durable line does.
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(4.0);
    let (mut ticked, mut levels) = (false, 0);
    for e in h.effects() {
        match e {
            Effect::Emit(Event::Heartbeat { .. }) => ticked = false,
            Effect::Tick => ticked = true,
            Effect::Journal(Entry::Levels {
                through: Some(_), ..
            }) => {
                assert!(ticked, "a running level before its tick");
                levels += 1;
            }
            _ => {}
        }
    }
    assert!(levels >= 3, "{levels} running levels");
}

#[test]
fn ten_pauses_keep_one_sound_run_in_sync() {
    // Each block reaches the core at its end, so the block a pause press interrupts arrives after the pause began: it
    // is written up to the pause, and every source keeps one run across the pauses.
    // Irregular stretches, so steps at the pauses can't pass for drift; one press lands on a block's edge.
    let mut h = Harness::recording_with_drift(&[Src::Screen, Src::Mic], 50.0);
    for (recording_s, paused_s) in [
        (2.0037, 1.3711),
        (11.5021, 0.4019),
        (4.0, 2.2063),
        (20.0089, 1.0007),
        (3.3007, 5.5031),
        (7.7051, 0.9013),
        (15.0003, 3.0049),
        (2.5099, 1.7021),
        (9.1041, 0.6003),
        (5.0077, 2.4011),
    ] {
        h.feed_for(recording_s);
        h.command(Command::Pause);
        h.feed_for(paused_s);
        h.command(Command::Resume);
    }
    h.feed_for(5.0);
    h.command(Command::Stop);
    assert_eq!((h.run_count("screen"), h.run_count("mic")), (1, 1));
    let worst_ms = h.sync_error_ms("screen", "mic");
    println!("the worst flash-to-blip error across ten pauses: {worst_ms:.3} ms");
    assert!(worst_ms <= 10.0, "{worst_ms} ms");
}

#[test]
fn one_stall_spends_one_restart_and_a_slow_first_frame_gets_a_grace() {
    let mut h = Harness::recording(&[Src::Screen, Src::Camera]);
    h.first_frame_after("camera", 2.5);
    h.feed_for(2.0);
    // The pipe says it stalled; the restarted camera takes 2.5 s to its first frame, which the stall rule would call
    // another stall without the grace.
    h.pipe_event("camera", PipeEvent::Stalled);
    h.feed_for(6.0);
    assert_eq!(restarts(&h, "camera"), 1, "one outage, one restart");
    // The other way round: the stall rule sees it first, then the pipe's own late report of the same outage.
    h.go_quiet("camera");
    h.feed_for(3.0);
    assert_eq!(restarts(&h, "camera"), 2);
    h.pipe_says("camera", PipeEvent::Stalled);
    h.feed_for(5.0);
    assert_eq!(restarts(&h, "camera"), 2, "one outage, one restart");
    assert_eq!(h.run_count("camera"), 3);
}

#[test]
fn a_sound_pipe_that_fails_right_after_its_restart_is_restarted_again() {
    // Sound has no stall rule to catch a restart that failed, so the grace never swallows its stop.
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(2.0);
    h.pipe_event("mic", PipeEvent::StoppedBySystem);
    h.pipe_event("mic", PipeEvent::StoppedBySystem);
    assert_eq!(restarts(&h, "mic"), 2);
    h.feed_for(2.0);
    assert_eq!(
        h.run_count("mic"),
        2,
        "one new run: the first restart never sent a sample"
    );
}

#[test]
fn a_screen_that_stops_while_the_display_sleeps_restarts_once_it_wakes() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(2.0);
    h.display_awake = false;
    h.tick();
    for _ in 0..5 {
        h.pipe_event("screen", PipeEvent::StoppedBySystem); // −3821, again and again
        h.feed_for(1.0);
    }
    assert_eq!(
        restarts(&h, "screen"),
        0,
        "no restart budget burnt while it sleeps"
    );
    h.display_awake = true;
    h.feed_for(3.0);
    assert_eq!(restarts(&h, "screen"), 1);
    assert_eq!(h.run_count("screen"), 2);
}

#[test]
fn a_screen_the_display_s_sleep_stopped_in_a_pause_comes_back_at_resume_with_its_budget_whole() {
    // Locked: the take pauses, the display sleeps, and capture stops before any tick saw the display asleep (the usual
    // order). That spends no restart; the display wakes during the pause, and Resume brings the screen back
    //.
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(3.0);
    h.command(Command::Pause);
    h.display_awake = false;
    h.pipe_says("screen", PipeEvent::StoppedBySystem);
    assert_eq!(restarts(&h, "screen"), 0, "the sleep isn't a fault");
    h.feed_for(5.0);
    h.display_awake = true;
    h.feed_for(2.0);
    h.command(Command::Resume);
    h.feed_for(3.0);
    assert_eq!(
        (restarts(&h, "screen"), h.run_count("screen")),
        (1, 2),
        "the screen records again"
    );
    // The budget is whole: three real stops restart three times, and the source isn't given up.
    for _ in 0..3 {
        h.pipe_says("screen", PipeEvent::StoppedBySystem);
        h.feed_for(4.0);
    }
    assert_eq!(restarts(&h, "screen"), 4);
    assert_eq!(notes(&h, "capture.health.lost"), 0);
}

#[test]
fn a_blank_screen_never_passes_the_gate_and_is_noted_while_recording() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    h.pipe_event("screen", PipeEvent::Blank(true));
    h.feed(0.0, 4.0);
    assert_eq!(
        h.state(),
        WorkerState::Countdown,
        "frames of a blank screen don't count"
    );
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, Event::Gate { missing } if missing == &["screen".to_string()]))
    );
    h.pipe_event("screen", PipeEvent::Blank(false));
    h.feed(4.0, 5.0);
    assert_eq!(h.state(), WorkerState::Recording);
    h.pipe_event("screen", PipeEvent::Blank(true));
    h.feed_for(5.0);
    let blank =
        |e: &Event| matches!(e, Event::Health { note, .. } if note.code == "capture.health.lost");
    assert_eq!(
        h.events().iter().filter(|e| blank(e)).count(),
        1,
        "polled once a second, noted once"
    );
}

#[test]
fn the_low_disk_note_comes_once_each_time_the_level_changes() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.free_bytes(9_000_000_000);
    h.feed_for(5.0);
    assert_eq!(notes(&h, "capture.health.diskLow"), 1, "not every tick");
    h.free_bytes(50_000_000_000);
    h.feed_for(2.0);
    h.free_bytes(9_000_000_000);
    h.feed_for(2.0);
    assert_eq!(notes(&h, "capture.health.diskLow"), 2);
    assert!(
        h.shell_out
            .iter()
            .any(|o| matches!(o, ShellOutput::Flash(Flash::LowSpace)))
    );
}

#[test]
fn a_game_sound_that_stays_silent_gets_a_mild_note_at_stop_and_every_run_its_levels() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::GameSound, Src::Mic], 3));
    h.set_signal("game-sound", Signal::Zeros);
    h.start_recording();
    h.feed_for(10.0);
    assert_eq!(
        notes(&h, "capture.note.silentSound"),
        0,
        "never during the take"
    );
    h.command(Command::Stop);
    let mild = h.events().iter().find_map(|e| match e {
        Event::Health {
            note,
            level,
            source,
        } if note.code == "capture.note.silentSound" => Some((*level, source.clone())),
        _ => None,
    });
    assert_eq!(mild, Some((Level::Ok, Some("game-sound".into()))));
    let levels = |file: &str| {
        h.journal().iter().find_map(|e| match e {
            Entry::Levels {
                file: f,
                peaks,
                through: None,
            } if f == file => Some(peaks.clone()),
            _ => None,
        })
    };
    assert_eq!(levels("game-sound/run-01.caf"), Some(vec![0.0, 0.0]));
    assert!(levels("mic/run-01.caf").is_some_and(|p| p.len() == 1 && p[0] > 0.1));
    let m = h.manifest();
    let mic = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs[0];
    assert!(
        mic.channel_peaks.is_some(),
        "the manifest carries them for import"
    );
}

#[test]
fn a_full_writer_queue_ends_the_run_and_the_next_opens_at_a_keyframe() {
    let mut h = Harness::recording(&[Src::Screen, Src::Mic]);
    h.feed_for(2.0);
    let fx = h
        .core
        .writer_refused("screen/run-01.mp4", WriterError::Full, None, h.now);
    h.apply(fx);
    assert!(
        h.journal()
            .iter()
            .any(|e| matches!(e, Entry::RunEnded { source, run: 1, .. } if source == "screen"))
    );
    assert!(h.effects().iter().any(
        |e| matches!(e, Effect::Pipe { source, cmd: PipeCmd::Keyframe } if source == "screen")
    ));
    h.feed_for(2.0);
    assert_eq!(h.run_count("screen"), 2);
    assert!(h.video_starts_on_a_keyframe("screen/run-02.mp4"));
}

#[test]
fn nothing_starts_before_the_person_s_own_record_press() {
    // Capture consent: armed, no pipe starts, whatever the shell reports; only Record or its key does.
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Camera, Src::Mic], 3));
    h.shell(ShellInput::Focus {
        app_id: Some("com.apple.Safari".into()),
    });
    h.shell(ShellInput::Power(PowerEvent::Locked));
    h.shell(ShellInput::Hotkey(HotkeyAction::Pause));
    h.shell(ShellInput::Hotkey(HotkeyAction::Marker));
    h.command(Command::Resume);
    h.feed(0.0, 5.0);
    assert!(
        !h.effects()
            .iter()
            .any(|e| matches!(e, Effect::StartPipes(_)))
    );
    assert_eq!(h.state(), WorkerState::Armed);
    assert!(h.file_keys().is_empty() && h.journal().is_empty());
    h.shell(ShellInput::Hotkey(HotkeyAction::StartStop));
    assert!(
        h.effects()
            .iter()
            .any(|e| matches!(e, Effect::StartPipes(_))),
        "the person's key press"
    );
    // Cancel while armed for "Start from my game" ends with no take.
    let mut h = Harness::new(cfg(&[Src::Screen], 3));
    h.command(Command::Disarm);
    assert!(h.effects().iter().any(|e| matches!(e, Effect::Exit(3))));
}
