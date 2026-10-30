//! Export quality presets: output sizes, codecs and bitrates, and which encoders this computer has
//! (VideoToolbox on a Mac).

use klipolo_model::project::{AssetKind, FormatKind, Project};
use klipolo_model::timeline::ClipSource;
use serde::{Deserialize, Serialize};

use crate::api::ExportQuality;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoCodec {
    /// `h264_videotoolbox`, High profile, 8-bit 4:2:0.
    H264High,
    /// `hevc_videotoolbox`, Main 10 (`p010le` input, tagged `hvc1`).
    HevcMain10,
    /// `prores_videotoolbox`, 422 HQ, 10-bit 4:2:2.
    ProRes422Hq,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Container {
    Mp4,
    Mov,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioCodec {
    /// AAC-LC 48 kHz stereo at 192 kb/s.
    Aac192,
    /// 24-bit PCM 48 kHz stereo (Master).
    Pcm24,
}

/// How one output is encoded. Bitrates are bits per second (0 for ProRes, which has none).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodeSettings {
    pub codec: VideoCodec,
    pub container: Container,
    pub bitrate: u32,
    /// The peak over one second (VideoToolbox `DataRateLimits`).
    pub max_bitrate: u32,
    pub b_frames: u32,
    /// Frames between keyframes.
    pub gop: u32,
    pub audio: AudioCodec,
}

impl EncodeSettings {
    /// The file extension, without the dot.
    pub fn extension(&self) -> &'static str {
        match self.container {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
        }
    }
}

/// The encoders this computer has (`probe_caps`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderCaps {
    /// An H.264 encoder exists (looked up, not opened: every Mac has VideoToolbox's; Klipolo's LGPL FFmpeg has none on
    /// Windows and Linux yet, so those exports stop with "no H.264 encoder").
    pub h264: bool,
    pub hevc_main10: bool,
    pub prores: bool,
    /// ProRes runs on a media engine (Apple Silicon Pro/Max and later) rather than the CPU.
    pub prores_hardware: bool,
}

impl EncoderCaps {
    /// What the parent plans with before the worker has probed: every encoder present (the worker re-resolves each
    /// output's settings with the caps it measured).
    pub const ASSUMED: EncoderCaps = EncoderCaps {
        h264: true,
        hevc_main10: true,
        prores: true,
        prores_hardware: false,
    };
}

/// Landscape sizes from smallest to largest; portrait and square are always 1080 wide.
const LANDSCAPE: [(u32, u32); 3] = [(1920, 1080), (2560, 1440), (3840, 2160)];

/// The tallest video the main track shows as its main footage (screens, imported clips, camera-only recordings).
fn tallest_main_source(project: &Project) -> u32 {
    let video = |id: &str| {
        project
            .asset(id)
            .filter(|a| a.kind == AssetKind::Video)
            .map(|a| a.height)
    };
    project
        .timeline
        .main
        .iter()
        .filter_map(|clip| match &clip.source {
            ClipSource::Asset { id } => video(id),
            // Every file of the main footage (its screen, else its camera): a heal may have changed its size.
            ClipSource::Recording { id } => project
                .recording(id)
                .and_then(|r| r.primary_source())
                .and_then(|s| s.runs.iter().filter_map(|r| video(&r.asset_id)).max()),
        })
        .max()
        .unwrap_or(0)
}

/// The output size of `format`: portrait 1080 × 1920 and square 1080 × 1080; landscape the largest of 1080p, 1440p
/// and 4K whose height doesn't exceed the tallest main-track source, never below 1080p.
pub fn output_size(project: &Project, format: FormatKind) -> (u32, u32) {
    match format {
        FormatKind::Portrait => (1080, 1920),
        FormatKind::Square => (1080, 1080),
        FormatKind::Landscape => {
            let tallest = tallest_main_source(project);
            LANDSCAPE
                .iter()
                .rev()
                .copied()
                .find(|&(_, h)| h <= tallest)
                .unwrap_or(LANDSCAPE[0])
        }
    }
}

/// YouTube's recommended SDR upload bitrate for a size and frame rate (the Standard quality).
fn standard_bitrate((width, height): (u32, u32), fps: u32) -> u32 {
    let long = width.max(height);
    let (low, high) = if long <= 1920 {
        (8_000_000, 12_000_000)
    } else if long <= 2560 {
        (16_000_000, 24_000_000)
    } else {
        (40_000_000, 60_000_000)
    };
    if fps > 30 { high } else { low }
}

/// The encoder, bitrate and container for `quality` at `size` and `fps` on a Mac with `caps`.
pub fn encode_settings(
    quality: ExportQuality,
    size: (u32, u32),
    fps: u32,
    caps: &EncoderCaps,
) -> EncodeSettings {
    let standard = standard_bitrate(size, fps);
    let gop = 2 * fps.max(1);
    let h264 = |bitrate: u32, b_frames: u32| EncodeSettings {
        codec: VideoCodec::H264High,
        container: Container::Mp4,
        bitrate,
        max_bitrate: bitrate / 2 * 3,
        b_frames,
        gop,
        audio: AudioCodec::Aac192,
    };
    match quality {
        ExportQuality::Standard => h264(standard, 0),
        ExportQuality::High => h264(standard / 4 * 7, 2),
        ExportQuality::Maximum if caps.hevc_main10 => EncodeSettings {
            codec: VideoCodec::HevcMain10,
            ..h264(standard * 3, 2)
        },
        ExportQuality::Maximum => h264(standard * 3, 2),
        ExportQuality::Master => EncodeSettings {
            codec: VideoCodec::ProRes422Hq,
            container: Container::Mov,
            bitrate: 0,
            max_bitrate: 0,
            b_frames: 0,
            gop: 1,
            audio: AudioCodec::Pcm24,
        },
    }
}

/// The SoftwareSafe retry: H.264 High through VideoToolbox's software encoder at the same bitrate; ProRes
/// stays ProRes (on Intel it is software already).
pub fn software_safe(settings: EncodeSettings) -> EncodeSettings {
    match settings.codec {
        VideoCodec::ProRes422Hq => settings,
        VideoCodec::H264High | VideoCodec::HevcMain10 => EncodeSettings {
            codec: VideoCodec::H264High,
            ..settings
        },
    }
}

/// How many VideoToolbox sessions an output counts as toward the one-pass limit: 4K and ProRes count twice.
pub fn session_weight(settings: &EncodeSettings, size: (u32, u32)) -> u32 {
    if settings.codec == VideoCodec::ProRes422Hq || size.0.max(size.1) > 2560 {
        2
    } else {
        1
    }
}

/// Sessions one decode pass may feed (three 1080p formats; many more were measured opening fine, but the
/// shared encoder engine gains nothing past that).
pub const ONE_PASS_SESSIONS: u32 = 3;

/// The encoders this computer has. Each 10-bit or ProRes check opens a small one-frame session (VideoToolbox: about
/// 0.6 s on Intel); only the encoders `needed` asks for are opened. H.264 is only looked up.
pub fn probe_caps_for(needed: &[ExportQuality]) -> EncoderCaps {
    let hevc = needed.contains(&ExportQuality::Maximum);
    let prores = needed.contains(&ExportQuality::Master);
    EncoderCaps {
        h264: super::codecs::encoder_name(VideoCodec::H264High).is_some(),
        hevc_main10: hevc && super::codecs::probe(VideoCodec::HevcMain10, false),
        prores: prores && super::codecs::probe(VideoCodec::ProRes422Hq, false),
        prores_hardware: prores && super::codecs::probe(VideoCodec::ProRes422Hq, true),
    }
}

/// Every encoder (`--export-worker --version`).
pub fn probe_caps() -> EncoderCaps {
    probe_caps_for(&[ExportQuality::Maximum, ExportQuality::Master])
}
