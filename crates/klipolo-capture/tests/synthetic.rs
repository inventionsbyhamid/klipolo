#![cfg(feature = "synthetic")]
mod common;

use std::collections::BTreeMap;
use std::sync::mpsc::sync_channel;

use klipolo_capture::backend::{AudioSpec, Backend, PresetId, Target, VideoSpec};
use klipolo_capture::media::Codec;
use klipolo_capture::pipe::{AudioOut, PACKET_CHANNEL, PipeError, PipeEvent, VideoOut};
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::shell::{HotkeyAction, KeyBindings, Shell, ShellInput, ShellIo, ShellOutput};
use klipolo_capture::synthetic::*;
use klipolo_platform::caps::OsFamily;
use klipolo_platform::clock::host_ns;

fn video() -> SyntheticVideoSpec {
    SyntheticVideoSpec {
        fps: 60,
        gop_frames: 60,
        width: 1920,
        height: 1080,
        codec: Codec::H264,
        flash_every: Some(60),
        packet_bytes: 2_000,
    }
}

#[test]
fn packets_carry_their_frame_keyframes_every_gop_and_host_times() {
    let p = synthetic_packets(&video(), 0, 121, 1_000_000_000);
    assert_eq!(p.len(), 121);
    assert!(p[0].keyframe && !p[1].keyframe && p[60].keyframe && p[120].keyframe);
    assert_eq!(frame_of(&p[61]), Some((61, false)));
    assert_eq!(frame_of(&p[60]), Some((60, true)));
    assert_eq!(p[60].host_ns - p[0].host_ns, 1_000_000_000);
}

#[test]
fn a_fast_device_delivers_more_frames_per_host_second() {
    let spec = SyntheticAudioSpec {
        rate: 48_000,
        channels: 1,
        block_ms: 10,
        drift_ppm: 100.0,
        signal: Signal::Zeros,
        blip_every_ms: None,
    };
    let frames: usize = synthetic_blocks(&spec, 0, 100_000_000_000, 0)
        .iter()
        .map(|b| b.samples.len())
        .sum();
    assert_eq!(frames, 4_800_480, "100 s at 48 kHz, 100 ppm fast");
}

#[test]
fn silence_is_exact_zeros_and_noise_sits_at_its_level() {
    let base = SyntheticAudioSpec {
        rate: 48_000,
        channels: 1,
        block_ms: 10,
        drift_ppm: 0.0,
        signal: Signal::Zeros,
        blip_every_ms: None,
    };
    assert!(
        synthetic_blocks(&base, 0, 1_000_000_000, 0)
            .iter()
            .all(|b| b.samples.iter().all(|s| *s == 0.0))
    );
    let noise = SyntheticAudioSpec {
        signal: Signal::Noise { dbfs: -80.0 },
        ..base
    };
    let peak = synthetic_blocks(&noise, 0, 1_000_000_000, 0)
        .iter()
        .flat_map(|b| b.samples.iter())
        .fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.0 && 20.0 * peak.log10() < -70.0, "{peak}");
}

#[test]
fn blips_land_on_the_host_seconds_of_the_flashes_even_on_a_drifting_clock() {
    let rate = 48_000.0 * (1.0 + 300e-6);
    let spec = SyntheticAudioSpec {
        rate: 48_000,
        channels: 2,
        block_ms: 10,
        drift_ppm: 300.0,
        signal: Signal::Zeros,
        blip_every_ms: Some(1_000),
    };
    let (mut onsets, mut quiet) = (vec![], 100);
    for block in synthetic_blocks(&spec, 5_000_000_000, 15_000_000_000, 0) {
        for (i, frame) in block.samples.chunks(2).enumerate() {
            if frame[0].abs() < 0.01 {
                quiet += 1;
                continue;
            }
            // A blip begins after a quiet stretch (a 1 kHz tone crosses zero every 24 frames).
            if quiet >= 100 {
                onsets.push(block.host_ns as f64 + i as f64 * 1e9 / rate - 5e9);
            }
            quiet = 0;
        }
    }
    assert_eq!(onsets.len(), 10);
    for (k, at) in onsets.iter().enumerate() {
        // The blip starts at sin(0); the first audible frame is one later (21 µs).
        assert!(
            (at - k as f64 * 1e9).abs() < 100_000.0,
            "blip {k} at {at} ns"
        );
    }
}

#[test]
fn the_fake_backend_runs_its_script_in_real_time() {
    let script = Script {
        live_after_ms: BTreeMap::from([("mic".into(), 300)]),
        events: vec![
            (200, "screen".into(), PipeEvent::DeviceLost),
            (400, "screen".into(), PipeEvent::DeviceBack),
        ],
        start_errors: BTreeMap::from([("camera".into(), PipeError::Busy)]),
        ..Script::default()
    };
    let backend = FakeBackend::new(script);
    let spec = |target| VideoSpec {
        target,
        preset: PresetId::Smooth,
        cursor: CursorPlan::None,
        sound: false,
        encoders: vec![],
        bubble: false,
        bubble_mirrored: false,
    };

    let (camera_out, _, _) = video_out(2);
    let mut camera = backend
        .video(&spec(Target::Camera {
            id: "camera-1".into(),
        }))
        .unwrap()
        .video;
    assert_eq!(camera.start(camera_out).unwrap_err(), PipeError::Busy);

    let (out, packets, events) = video_out(PACKET_CHANNEL);
    let (stuck_out, _stuck, _) = video_out(2);
    let mut screen = backend
        .video(&spec(Target::Display {
            id: "display-1".into(),
        }))
        .unwrap()
        .video;
    let mut stuck = backend
        .video(&spec(Target::Display {
            id: "display-1".into(),
        }))
        .unwrap()
        .video;
    let (blocks_tx, blocks) = sync_channel(klipolo_capture::pipe::PCM_CHANNEL);
    let (mic_events, _) = sync_channel(8);
    let mut mic = backend
        .audio(&AudioSpec::Mic {
            device: "mic-1".into(),
        })
        .unwrap();
    let t0 = host_ns();
    screen.start(out).unwrap();
    stuck.start(stuck_out).unwrap();
    mic.start(AudioOut {
        blocks: blocks_tx,
        events: mic_events,
        stopping: Default::default(),
    })
    .unwrap();

    let got = common::received_within(&packets, 700);
    screen.stop();
    stuck.stop();
    mic.stop();
    let ms = |p: &klipolo_capture::EncodedPacket| (p.host_ns.saturating_sub(t0)) / 1_000_000;
    assert!(
        got[0].keyframe && ms(&got[0]) < 50,
        "frames come at once, from a keyframe"
    );
    assert!(
        !got.iter().any(|p| (210..390).contains(&ms(p))),
        "nothing while the device is lost"
    );
    let back = got
        .iter()
        .position(|p| ms(p) >= 300)
        .expect("frames again after DeviceBack");
    assert!(got[back].keyframe, "it comes back on a keyframe");
    assert!(
        (10..=24).contains(&got.len()),
        "about 30 fps for 0.5 s of flowing time: {}",
        got.len()
    );
    assert_eq!(
        events.try_iter().collect::<Vec<_>>(),
        [PipeEvent::DeviceLost, PipeEvent::DeviceBack]
    );
    assert!(
        stuck.stats().dropped_channel > 0,
        "a full channel drops and counts"
    );

    let blocks: Vec<_> = blocks.try_iter().collect();
    assert!(
        !blocks.is_empty() && blocks.iter().all(|b| b.host_ns >= t0 + 300_000_000),
        "the mic is live after 300 ms"
    );
    assert_eq!(mic.stats().callbacks, blocks.len() as u64);
}

#[test]
fn the_fake_shell_sends_its_inputs_on_time_and_hands_back_outputs_until_quit() {
    let (shell, outputs) = FakeShell::new(vec![
        (80, ShellInput::Hotkey(HotkeyAction::StartStop)),
        (0, ShellInput::ParentGone),
    ]);
    let (input, inputs) = sync_channel(8);
    let (to_shell, output) = sync_channel(8);
    let io = ShellIo {
        input,
        output,
        keys: KeyBindings::defaults(OsFamily::Linux),
        words: BTreeMap::new(),
        hud_corner: Default::default(),
        hud_display: None,
        end: Default::default(),
        camera_bubble: false,
    };
    let t0 = host_ns();
    let running = std::thread::spawn(move || Box::new(shell).run(io));
    assert_eq!(inputs.recv().unwrap(), ShellInput::ParentGone);
    assert_eq!(
        inputs.recv().unwrap(),
        ShellInput::Hotkey(HotkeyAction::StartStop)
    );
    assert!(host_ns() - t0 >= 80_000_000, "at its time");
    to_shell.send(ShellOutput::Keys(true)).unwrap();
    to_shell.send(ShellOutput::Quit).unwrap();
    running.join().unwrap();
    assert_eq!(
        outputs.try_iter().collect::<Vec<_>>(),
        [ShellOutput::Keys(true), ShellOutput::Quit]
    );
}

/// After a drop the fake pipe makes a keyframe only when the gate asks for one, at most every 2 s
/// (`KEYFRAME_ASK_NS`), as a real encoder would. Forcing one after every drop would make every frame that fits a
/// keyframe on a slow drain, a burst no real encoder makes.
#[test]
fn a_slow_drain_gets_a_keyframe_at_most_every_two_seconds_besides_the_gops() {
    let backend = FakeBackend::new(Script::default());
    let (out, packets, _events) = video_out(1);
    let mut screen = backend
        .video(&VideoSpec {
            target: Target::Display {
                id: "display-1".into(),
            },
            preset: PresetId::Smooth,
            cursor: CursorPlan::None,
            sound: false,
            encoders: vec![],
            bubble: false,
            bubble_mirrored: false,
        })
        .unwrap()
        .video;
    screen.start(out).unwrap();
    let mut keys = 0;
    for _ in 0..17 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        keys += packets.try_iter().take(1).filter(|p| p.keyframe).count();
    }
    screen.stop();
    // 2.5 s at 30 fps with a GOP of 1 s: the first frame, two or three GOP keyframes and at most two asked for.
    assert!((1..=6).contains(&keys), "{keys} keyframes");
}

#[test]
fn a_portal_pipe_sends_a_fresh_restore_token_on_every_start() {
    let backend = FakeBackend::new(Script::default());
    let portal = |token: Option<&str>| VideoSpec {
        target: Target::Portal {
            restore_token: token.map(String::from),
            app_id: None,
        },
        preset: PresetId::Smooth,
        cursor: CursorPlan::None,
        sound: false,
        encoders: vec![],
        bubble: false,
        bubble_mirrored: false,
    };
    let mut tokens = vec![];
    for last in [None, Some("fake-restore-1")] {
        let (out, _packets, events) = video_out(PACKET_CHANNEL);
        let mut pipe = backend.video(&portal(last)).unwrap().video;
        pipe.start(out).unwrap();
        let first = events.recv_timeout(std::time::Duration::from_secs(1));
        pipe.stop();
        match first {
            Ok(PipeEvent::RestoreToken(token)) => tokens.push(token),
            other => panic!("no restore token: {other:?}"),
        }
    }
    // Single-use: the second take gets a new one to save to the game's profile.
    assert_eq!(tokens, ["fake-restore-1", "fake-restore-2"]);
}

#[test]
fn a_portal_target_with_an_app_records_that_apps_sound() {
    let script = Script {
        start_errors: BTreeMap::from([("computerSound".into(), PipeError::Busy)]),
        ..Script::default()
    };
    let backend = FakeBackend::new(script);
    let portal = |app_id: Option<&str>| Target::Portal {
        restore_token: None,
        app_id: app_id.map(String::from),
    };
    let game = portal(Some("steam_app_1091500"));
    assert_eq!(
        serde_json::to_value(&game).unwrap()["appId"],
        "steam_app_1091500",
        "saved in the session's config"
    );
    let sound_of = |target| {
        let spec = VideoSpec {
            target,
            preset: PresetId::Smooth,
            cursor: CursorPlan::None,
            sound: true,
            encoders: vec![],
            bubble: false,
            bubble_mirrored: false,
        };
        let mut sound = backend
            .video(&spec)
            .unwrap()
            .sound
            .expect("sound was asked for");
        let (blocks, _) = sync_channel(klipolo_capture::pipe::PCM_CHANNEL);
        let (events, _) = sync_channel(8);
        let started = sound.start(AudioOut {
            blocks,
            events,
            stopping: Default::default(),
        });
        sound.stop();
        started
    };
    assert!(
        sound_of(game).is_ok(),
        "the game's own sound (\"gameSound\")"
    );
    assert_eq!(
        sound_of(portal(None)).unwrap_err(),
        PipeError::Busy,
        "no app: the computer's sound"
    );
}

fn video_out(
    capacity: usize,
) -> (
    VideoOut,
    std::sync::mpsc::Receiver<klipolo_capture::EncodedPacket>,
    std::sync::mpsc::Receiver<PipeEvent>,
) {
    let (packets, packets_rx) = sync_channel(capacity);
    let (events, events_rx) = sync_channel(klipolo_capture::pipe::EVENT_CHANNEL);
    (
        VideoOut {
            packets,
            cursor: None,
            events,
            stopping: Default::default(),
        },
        packets_rx,
        events_rx,
    )
}
