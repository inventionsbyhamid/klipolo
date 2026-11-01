//! What the harness records with: its sources, their config, the computer's mics, and the synthetic video.

use klipolo_capture::backend::{DeviceInfo, PresetId, Target, Transport, VideoSpec};
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::session::{SessionConfig, SourceSpec};
use klipolo_capture::synthetic::SyntheticVideoSpec;
use klipolo_model::project::SourceRole;

use super::FPS;
use crate::common::config::config_with;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    Screen,
    Camera,
    Mic,
    GameSound,
    /// The whole computer's sound, from its own pipe (`SourceSpec::System`).
    ComputerSound,
}

/// A config of these sources (the screen at Light, 30 fps; the mic the "MV7"), with this countdown.
pub fn cfg(srcs: &[Src], countdown_s: u32) -> SessionConfig {
    let roles: Vec<(&str, SourceRole)> = srcs
        .iter()
        .map(|s| match s {
            Src::Screen => ("screen", SourceRole::Screen),
            Src::Camera => ("camera", SourceRole::Camera),
            Src::Mic => ("mic", SourceRole::Mic),
            Src::GameSound => ("game-sound", SourceRole::GameAudio),
            Src::ComputerSound => ("computer-sound", SourceRole::SystemAudio),
        })
        .collect();
    let mut cfg = config_with(&roles);
    for (src, s) in cfg.sources.iter_mut().zip(srcs) {
        match s {
            Src::Screen => {
                src.spec = SourceSpec::Screen(VideoSpec {
                    target: Target::Display {
                        id: "display-1".into(),
                    },
                    preset: PresetId::Light,
                    cursor: CursorPlan::Burned,
                    sound: true,
                    encoders: vec![],
                    bubble: false,
                    bubble_mirrored: false,
                });
            }
            Src::Camera => {
                src.spec = SourceSpec::Camera {
                    device: "camera-1".into(),
                    preset: PresetId::Camera720,
                    encoders: vec![],
                }
            }
            Src::Mic => {
                src.spec = SourceSpec::Mic {
                    device: "mic-usb".into(),
                }
            }
            Src::GameSound => {
                src.label = "capture.source.gameSound".into();
                src.spec = SourceSpec::AppSound {
                    of: "screen".into(),
                };
            }
            // `config_with`'s own: the computer's sound, labelled `capture.source.computerSound`.
            Src::ComputerSound => {}
        }
    }
    cfg.countdown_s = countdown_s;
    cfg
}

/// The computer's mics: the person's USB "MV7" and the built-in "MacBook Microphone".
pub fn mics() -> Vec<DeviceInfo> {
    let mic = |id: &str, name: &str, transport| DeviceInfo {
        id: id.into(),
        name: name.into(),
        transport,
        is_virtual: false,
        in_use: false,
        suspended: false,
    };
    vec![
        mic("mic-usb", "MV7", Transport::Usb),
        mic(
            "MacBook Microphone",
            "MacBook Microphone",
            Transport::BuiltIn,
        ),
    ]
}

/// The harness's synthetic video: 30 fps, a keyframe a second, a flash frame each second, small packets.
pub fn video_spec() -> SyntheticVideoSpec {
    SyntheticVideoSpec {
        fps: FPS as u32,
        gop_frames: FPS as u32,
        width: 1920,
        height: 1080,
        codec: klipolo_capture::media::Codec::H264,
        flash_every: Some(FPS as u32),
        packet_bytes: 64,
    }
}
