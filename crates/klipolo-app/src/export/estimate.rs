//! What an export costs before it starts: the most its files take on the disk, and about how long it runs on this
//! computer. The time left shown while it runs is measured instead.

use klipolo_engine::api::{ExportQuality, PlannedFile};
use klipolo_engine::export::job::{AUDIO_BITRATE, MAX_FPS};
use klipolo_engine::export::quality::{EncoderCaps, VideoCodec, encode_settings};
use klipolo_model::project::Project;
use klipolo_model::recording::RecordingKind;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource};
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
/// The share of H.264's and HEVC's average bitrate a screen recording's video takes. VideoToolbox's average is a
/// ceiling that flat pictures (text, windows, little motion) stay well under: 49 s of a 1080p30 screen recording at
/// High made 54 MB, 62 % of the rate. Taken with a margin, so the size stays an honest upper estimate, within about a
/// quarter of the file. Camera and game footage, busy all over, comes close to the full rate and is estimated at it.
const SCREEN_RATE: f64 = 0.75;

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

/// `files` at `quality` for a timeline `duration_us` long at the project's `fps`, `screen` of it (0 to 1, `screen_share`)
/// a screen recording. Sizes are an upper estimate: VideoToolbox's average bitrate is a ceiling that simple footage
/// stays under, a screen recording well under (`SCREEN_RATE`).
pub fn estimate(
    files: &[PlannedFile],
    quality: ExportQuality,
    fps: u32,
    duration_us: i64,
    apple_silicon: bool,
    screen: f64,
) -> Estimate {
    let video_share = 1.0 - screen.clamp(0.0, 1.0) * (1.0 - SCREEN_RATE);
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
            _ => f64::from(settings.bitrate) * video_share + f64::from(AUDIO_BITRATE),
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

/// How much of `project`'s main track is a screen recording, 0 to 1 by length on the timeline: its clips of screen takes.
/// A camera or game take, or an imported file, counts as camera footage.
pub fn screen_share(project: &Project) -> f64 {
    let main = &project.timeline.main;
    let total: Us = main.iter().map(Clip::duration_us).sum();
    if total <= 0 {
        return 0.0;
    }
    let screen: Us = main
        .iter()
        .filter(|clip| match &clip.source {
            ClipSource::Recording { id } => project
                .recording(id)
                .is_some_and(|take| take.kind == RecordingKind::Screen),
            ClipSource::Asset { .. } => false,
        })
        .map(Clip::duration_us)
        .sum();
    screen as f64 / total as f64
}

/// Apple silicon, also when this Intel build runs under Rosetta (`PlatformCaps`'s CPU architecture): a media engine
/// encodes ProRes and VideoToolbox runs faster. Intel Macs, Windows and Linux encode Master's ProRes on the CPU.
pub fn apple_silicon(caps: &PlatformCaps) -> bool {
    caps.os.family == OsFamily::Macos && caps.os.cpu_arch == "aarch64"
}

#[cfg(test)]
mod tests {
    use klipolo_model::project::{FormatKind, Project};
    use klipolo_model::recording::{Recording, RecordingKind};
    use klipolo_model::timeline::{Clip, ClipSource};

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
        let high = estimate(&one, ExportQuality::High, 30, minute, false, 0.0);
        assert_eq!(high.bytes, (14_000_000 + 192_000) * 60 / 8);
        let master = estimate(&one, ExportQuality::Master, 30, minute, false, 0.0);
        assert!(
            master.bytes > 10 * high.bytes,
            "ProRes HQ is about 220 Mb/s"
        );
        let smaller = estimate(&one, ExportQuality::Standard, 30, minute, false, 0.0);
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
        let three = estimate(&all, ExportQuality::High, 30, minute, false, 0.0);
        // Three formats in one pass run at about 1 to 1.5 x realtime on Intel, not three times as fast.
        assert!((40.0..75.0).contains(&three.seconds), "{}", three.seconds);
        assert!(three.seconds > 2.0 * high.seconds);
        assert_eq!(
            estimate(&[], ExportQuality::High, 30, minute, false, 0.0).seconds,
            0.0
        );
    }

    /// A 49 s screen recording at 1080p30 and High made a 54 MB file (VideoToolbox stays well under its average
    /// bitrate on flat pictures), where the bitrate alone says 88 MB: the estimate stays above the file, by no more than
    /// a quarter. Camera footage comes close to the bitrate, so it is estimated at the full rate. A timeline that is
    /// half of each is estimated between the two.
    #[test]
    fn a_screen_recordings_size_is_estimated_within_a_quarter_above_the_file() {
        let one = [file(FormatKind::Landscape, 1920, 1080)];
        let length = 49_000_000;
        let made = 54_000_000.0;
        let screen = estimate(&one, ExportQuality::High, 30, length, false, 1.0).bytes as f64;
        assert!(
            (made..=made * 1.25).contains(&screen),
            "{screen} for a {made} file"
        );
        let camera = estimate(&one, ExportQuality::High, 30, length, false, 0.0).bytes;
        assert_eq!(camera, (14_000_000 + 192_000) * 49 / 8, "the full rate");
        let half = estimate(&one, ExportQuality::High, 30, length, false, 0.5).bytes as f64;
        assert!(screen < half && half < camera as f64);
        // Master's ProRes is sized by its own rate, whatever the footage.
        let master = |screen| estimate(&one, ExportQuality::Master, 30, length, false, screen).bytes;
        assert_eq!(master(1.0), master(0.0));
    }

    fn take(id: &str, kind: RecordingKind) -> Recording {
        Recording {
            id: id.into(),
            kind,
            sources: Vec::new(),
            primary: String::new(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        }
    }

    /// The share of the main track's time that is a screen recording: its clips of a screen take, by their length on the
    /// timeline (a camera or game take, or an imported file, counts as camera footage).
    #[test]
    fn the_screen_share_is_the_main_tracks_time_in_screen_takes() {
        let mut project = Project::new("Mixed");
        assert_eq!(screen_share(&project), 0.0, "an empty timeline");
        project.recordings = vec![
            take("screen", RecordingKind::Screen),
            take("game", RecordingKind::Gameplay),
        ];
        let of = |id: &str| ClipSource::Recording { id: id.into() };
        project.timeline.main = vec![
            Clip::new(of("screen"), 0, 30_000_000),
            Clip::new(ClipSource::Asset { id: "camera".into() }, 0, 10_000_000),
            Clip::new(of("game"), 0, 10_000_000),
        ];
        assert!((screen_share(&project) - 0.6).abs() < 1e-9);
        // Played at double speed, the screen clip is half as long on the timeline.
        project.timeline.main[0].speed = 2.0;
        assert!((screen_share(&project) - 15.0 / 35.0).abs() < 1e-9);
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
