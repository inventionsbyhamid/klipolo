//! A session config for tests: these sources on fake devices, recorded into a new project.

use std::collections::BTreeMap;

use klipolo_capture::backend::{PresetId, Target, VideoSpec};
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::session::{ImportTarget, SessionConfig, SourceConfig, SourceSpec};
use klipolo_capture::shell::KeyBindings;
use klipolo_model::project::{FormatKind, RecordingKind, SourceRole};
use klipolo_platform::caps::OsFamily;

/// A config with these sources on fake devices, recorded into a new project.
pub fn config_with(sources: &[(&str, SourceRole)]) -> SessionConfig {
    let source = |(id, role): &(&str, SourceRole)| {
        let (label, spec) = match role {
            SourceRole::Screen | SourceRole::Game => (
                "capture.source.screen",
                SourceSpec::Screen(VideoSpec {
                    target: Target::Display {
                        id: "fake-display".into(),
                    },
                    preset: PresetId::Smooth,
                    cursor: CursorPlan::Burned,
                    sound: false,
                    encoders: vec![],
                    bubble: false,
                    bubble_mirrored: false,
                }),
            ),
            SourceRole::Camera => (
                "capture.source.camera",
                SourceSpec::Camera {
                    device: "fake-camera".into(),
                    preset: PresetId::Camera1080,
                    encoders: vec![],
                },
            ),
            SourceRole::Mic => (
                "capture.source.mic",
                SourceSpec::Mic {
                    device: "fake-mic".into(),
                },
            ),
            _ => ("capture.source.computerSound", SourceSpec::System),
        };
        SourceConfig {
            id: id.to_string(),
            role: *role,
            label: label.into(),
            spec,
        }
    };
    SessionConfig {
        id: "take".into(),
        created_ms: 1_760_000_000_000,
        kind: RecordingKind::Screen,
        sources: sources.iter().map(source).collect(),
        countdown_s: 3,
        auto_pause: None,
        target: ImportTarget::NewProject {
            name: "Recording".into(),
            formats: vec![FormatKind::Landscape],
        },
        game: None,
        app_version: "0.0.0-test".into(),
        os: "test".into(),
        expected_minutes: 30,
        words: BTreeMap::from([("capture.menu.stop".into(), "Stop and save".into())]),
        keys: KeyBindings::defaults(OsFamily::Macos),
        hud_corner: Default::default(),
        camera_bubble: false,
        mirror_camera: true,
    }
}
