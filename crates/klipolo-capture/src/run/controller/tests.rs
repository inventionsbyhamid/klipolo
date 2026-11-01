//! The run controller with the fake backend's real-time pipe: `Lower` restarts the pipe one preset lighter,
//! not at the preset it failed at.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel, sync_channel};
use std::time::{Duration, Instant};

use super::Controller;
use crate::backend::{Backend, PresetId, Target, VideoSpec};
use crate::run::effect::{PipeCmd, Started};
use crate::run::pipes::{Ctl, Owned};
use crate::run::threads::Input;
use crate::synthetic::{FakeBackend, Script, SyntheticVideoSpec, VideoFeed};

/// The next start's size, and the spacing of its first packets in ms.
fn next_start(inputs: &Receiver<Input>) -> ((u32, u32), f64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut started = None;
    let mut stamps = vec![];
    while stamps.len() < 6 {
        let left = deadline.saturating_duration_since(Instant::now());
        match inputs
            .recv_timeout(left)
            .expect("the pipe starts and sends")
        {
            Input::Started {
                start_no,
                result: Ok(Started::Video(info)),
                ..
            } => started = Some((start_no, (info.width, info.height))),
            Input::Packet {
                start_no, packet, ..
            } if started.is_some_and(|(n, _)| n == start_no) => stamps.push(packet.host_ns),
            _ => {}
        }
    }
    let spacing = (stamps[5] - stamps[0]) as f64 / 5.0 / 1e6;
    (started.expect("a start").1, spacing)
}

#[test]
fn lower_restarts_the_pipe_one_preset_lighter() {
    let script = Script {
        video: VideoFeed::Synthetic(SyntheticVideoSpec {
            fps: 30,
            gop_frames: 30,
            width: 320,
            height: 180,
            codec: crate::media::Codec::H264,
            flash_every: None,
            packet_bytes: 200,
        }),
        ..Script::default()
    };
    let backend: Arc<dyn Backend> = Arc::new(FakeBackend::new(script));
    let spec = VideoSpec {
        target: Target::Display {
            id: "display-1".into(),
        },
        preset: PresetId::Sharp,
        cursor: crate::preflight::cursor::CursorPlan::None,
        sound: false,
        encoders: vec![],
        bubble: false,
        bubble_mirrored: false,
    };
    let pipe = backend.video(&spec).unwrap().video;
    let (core, inputs) = sync_channel(64);
    let owned = Owned::Video {
        pipe,
        cursor: crate::run::pipes::CursorFeed::None,
    };
    let c = Controller::new(
        0,
        owned,
        core,
        Arc::default(),
        Arc::default(),
        backend,
        None,
    );
    let (tx, rx) = channel();
    let thread = std::thread::spawn(move || c.run(rx));

    tx.send(Ctl::Start).unwrap();
    assert_eq!(next_start(&inputs).0, (320, 180), "as built");
    tx.send(Ctl::Cmd(PipeCmd::Lower)).unwrap();
    let (size, ms) = next_start(&inputs);
    assert_eq!(size, (1920, 1080), "Smooth, one lighter than Sharp");
    assert!((ms - 1000.0 / 60.0).abs() < 2.0, "at 60 fps: {ms} ms apart");
    tx.send(Ctl::Cmd(PipeCmd::Lower)).unwrap();
    let (size, ms) = next_start(&inputs);
    assert_eq!(size, (1920, 1080), "Light");
    assert!((ms - 1000.0 / 30.0).abs() < 2.0, "at 30 fps: {ms} ms apart");
    // Nothing is lighter than Light: the pipe restarts as it was.
    tx.send(Ctl::Cmd(PipeCmd::Lower)).unwrap();
    let (_, ms) = next_start(&inputs);
    assert!(
        (ms - 1000.0 / 30.0).abs() < 2.0,
        "still 30 fps: {ms} ms apart"
    );

    tx.send(Ctl::Cmd(PipeCmd::Stop)).unwrap();
    drop(inputs);
    thread.join().unwrap();
}
