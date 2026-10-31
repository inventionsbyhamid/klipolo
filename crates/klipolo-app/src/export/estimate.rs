//! What an export costs before it starts: the most its files take on the disk, and about how long it runs on this
//! computer. The time left shown while it runs is measured instead.

use klipolo_engine::api::{ExportQuality, PlannedFile};
use klipolo_engine::export::job::{AUDIO_BITRATE, MAX_FPS};
use klipolo_engine::export::quality::{EncoderCaps, VideoCodec, encode_settings};
use klipolo_platform::caps::{OsFamily, PlatformCaps};

/// ProRes 422 HQ at 1920 x 1080 and 29.97 fps (Apple's ProRes white paper); it scales with pixels and frame rate.
const PRORES_HQ_1080P30: f64 = 220e6;
/// Master's sound: 24-bit stereo PCM at 48 kHz.
const PCM24: f64 = 48_000.0 * 2.0 * 24.0;
/// Megapixels a second of H.264 High (2 B-frames) that VideoToolbox gets through with every session of a pass busy.
/// An Intel Mac measured 160 to 380 depending on heat; exports run long and warm, so the low end.
const INTEL_MPX_PER_S: f64 = 170.0;
const APPLE_SILICON_MPX_PER_S: f64 = 400.0;
/// Start-up: the worker process, the sound measurement and the encoder sessions (0.1 to 1.1 s, plus the launch).
const SET_UP_S: f64 = 2.0;

/// Encoder work relative to H.264 High with B-frames.
fn cost(codec: VideoCodec, b_frames: u32, apple_silicon: bool) -> f64 {
    match codec {
        // B-frames halve VideoToolbox's H.264 throughput on Intel (Standard 175 fps, High 93 fps).
        VideoCodec::H264High if b_frames == 0 => 0.55,
        VideoCodec::H264High => 1.0,
        VideoCodec::HevcMain10 => 1.3,
        // A media engine on Apple silicon; on Intel the CPU encodes ProRes while it also draws the frames.
        VideoCodec::ProRes422Hq if apple_silicon => 0.5,
        VideoCodec::ProRes422Hq => 1.6,
    }
}

pub struct Estimate {
    pub bytes: u64,
    pub seconds: f64,
}

/// `files` at `quality` for a timeline `duration_us` long at the project's `fps`. Sizes are an upper bound:
/// VideoToolbox's average bitrate is a target that simple footage stays under.
pub fn estimate(
    files: &[PlannedFile],
    quality: ExportQuality,
    fps: u32,
    duration_us: i64,
    apple_silicon: bool,
) -> Estimate {
    let fps = fps.clamp(1, MAX_FPS);
    let length_s = duration_us.max(0) as f64 / 1e6;
    let (mut bits_per_s, mut work) = (0.0, 0.0);
    for f in files {
        let settings = encode_settings(quality, (f.width, f.height), fps, &EncoderCaps::ASSUMED);
        let pixels = f64::from(f.width) * f64::from(f.height);
        bits_per_s += match settings.codec {
            VideoCodec::ProRes422Hq => {
                PRORES_HQ_1080P30 * pixels / (1920.0 * 1080.0) * f64::from(fps) / 29.97 + PCM24
            }
            _ => f64::from(settings.bitrate) + f64::from(AUDIO_BITRATE),
        };
        work +=
            pixels / 1e6 * f64::from(fps) * cost(settings.codec, settings.b_frames, apple_silicon);
    }
    let rate = if apple_silicon {
        APPLE_SILICON_MPX_PER_S
    } else {
        INTEL_MPX_PER_S
    };
    Estimate {
        bytes: (bits_per_s * length_s / 8.0) as u64,
        seconds: if files.is_empty() {
            0.0
        } else {
            SET_UP_S + work * length_s / rate
        },
    }
}

/// Apple silicon, also when this Intel build runs under Rosetta (`PlatformCaps`'s CPU architecture): a media engine
/// encodes ProRes and VideoToolbox runs faster. Intel Macs, Windows and Linux encode Master's ProRes on the CPU.
pub fn apple_silicon(caps: &PlatformCaps) -> bool {
    caps.os.family == OsFamily::Macos && caps.os.cpu_arch == "aarch64"
}

#[cfg(test)]
mod tests {
    use klipolo_model::project::FormatKind;

    use super::*;

    fn file(format: FormatKind, width: u32, height: u32) -> PlannedFile {
        PlannedFile {
            format,
            width,
            height,
            name: String::new(),
            path: String::new(),
        }
    }

    #[test]
    fn sizes_follow_the_bitrates_and_more_formats_take_longer() {
        let one = [file(FormatKind::Landscape, 1920, 1080)];
        let minute = 60_000_000;
        // High at 1080p30: 14 Mb/s of video and 192 kb/s of sound for a minute.
        let high = estimate(&one, ExportQuality::High, 30, minute, false);
        assert_eq!(high.bytes, (14_000_000 + 192_000) * 60 / 8);
        let master = estimate(&one, ExportQuality::Master, 30, minute, false);
        assert!(
            master.bytes > 10 * high.bytes,
            "ProRes HQ is about 220 Mb/s"
        );
        let smaller = estimate(&one, ExportQuality::Standard, 30, minute, false);
        assert!(smaller.bytes < high.bytes, "Smaller file is smaller");
        assert!(
            smaller.seconds < high.seconds,
            "and faster without B-frames"
        );
        let all = [
            file(FormatKind::Landscape, 1920, 1080),
            file(FormatKind::Portrait, 1080, 1920),
            file(FormatKind::Square, 1080, 1080),
        ];
        let three = estimate(&all, ExportQuality::High, 30, minute, false);
        // Three formats in one pass run at about 1 to 1.5 x realtime on Intel, not three times as fast.
        assert!((40.0..75.0).contains(&three.seconds), "{}", three.seconds);
        assert!(three.seconds > 2.0 * high.seconds);
        assert_eq!(
            estimate(&[], ExportQuality::High, 30, minute, false).seconds,
            0.0
        );
    }

    #[test]
    fn only_apple_silicon_counts_as_fast() {
        let caps = |family, cpu_arch: &str| {
            let mut caps = klipolo_platform::caps::current();
            caps.os.family = family;
            caps.os.cpu_arch = cpu_arch.into();
            caps
        };
        assert!(
            !apple_silicon(&caps(OsFamily::Macos, "x86_64")),
            "an Intel Mac"
        );
        assert!(
            apple_silicon(&caps(OsFamily::Macos, "aarch64")),
            "Apple silicon, also under Rosetta"
        );
        assert!(!apple_silicon(&caps(OsFamily::Windows, "aarch64")));
    }
}
