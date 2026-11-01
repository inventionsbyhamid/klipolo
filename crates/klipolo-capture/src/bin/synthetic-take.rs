//! A synthetic take in its own process, for the kill sweep (`tests/kill.rs`): `synthetic-take <root>`
//! makes `<root>/take.klipolorec` with a screen (the H.264 fixture, real packets that decode) and a mic, both from the fake
//! backend, starts at once with no countdown and records until killed. `--resume` goes on with that take after a kill
//! (`run::resume`); `--stop-after-ms <n>` sends Stop after `n` ms. Nothing here touches a device.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use klipolo_capture::backend::{PresetId, Target, VideoSpec};
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::protocol::Command;
use klipolo_capture::run::threads::run_with;
use klipolo_capture::session::{
    self, ImportTarget, SessionConfig, SessionDir, SourceConfig, SourceSpec,
};
use klipolo_capture::shell::KeyBindings;
use klipolo_capture::synthetic::{FakeBackend, Script, Signal, VideoFeed};
use klipolo_model::project::{FormatKind, RecordingKind, SourceRole};
use klipolo_platform::caps::OsFamily;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first().map(PathBuf::from) else {
        eprintln!("usage: synthetic-take <root> [--resume] [--stop-after-ms <n>]");
        std::process::exit(2);
    };
    let resume = args.iter().any(|a| a == "--resume");
    let stop_after = args
        .iter()
        .position(|a| a == "--stop-after-ms")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse::<u32>().ok());
    let dir = if resume {
        SessionDir::at(&root.join("take.klipolorec"))
    } else {
        let dir = SessionDir::create(&root, "take").expect("a new take's folder");
        session::write_config(&dir, &config()).expect("the take's config");
        dir
    };
    let mut script = Vec::new();
    if !resume {
        script.push((0, Command::Start));
    }
    if let Some(ms) = stop_after {
        script.push((ms, Command::Stop));
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    // A mic's noise floor is never exact zeros: a tone, so recovery's trim of zero pages past the last tick (a power
    // cut's) never reads its sound as one.
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        sound: Signal::Tone {
            hz: 440.0,
            dbfs: -30.0,
        },
        ..Script::default()
    });
    std::process::exit(run_with(Box::new(backend), dir, resume, script));
}

/// A screen at 30 fps (Light) and the fake mic, recorded into a new project, with no countdown.
fn config() -> SessionConfig {
    SessionConfig {
        id: "take".into(),
        created_ms: 1_760_000_000_000,
        kind: RecordingKind::Screen,
        sources: vec![
            SourceConfig {
                id: "screen".into(),
                role: SourceRole::Screen,
                label: "capture.source.screen".into(),
                spec: SourceSpec::Screen(VideoSpec {
                    target: Target::Display {
                        id: "display-1".into(),
                    },
                    preset: PresetId::Light,
                    cursor: CursorPlan::Burned,
                    sound: false,
                    encoders: vec![],
                    bubble: false,
                    bubble_mirrored: false,
                }),
            },
            SourceConfig {
                id: "mic".into(),
                role: SourceRole::Mic,
                label: "capture.source.mic".into(),
                spec: SourceSpec::Mic {
                    device: "mic-1".into(),
                },
            },
        ],
        countdown_s: 0,
        auto_pause: None,
        target: ImportTarget::NewProject {
            name: "Recording".into(),
            formats: vec![FormatKind::Landscape],
        },
        game: None,
        app_version: "0.0.0-synthetic".into(),
        os: "synthetic".into(),
        expected_minutes: 5,
        words: BTreeMap::new(),
        keys: KeyBindings::defaults(OsFamily::Macos),
        hud_corner: Default::default(),
        camera_bubble: false,
        mirror_camera: true,
    }
}
