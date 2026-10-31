//! A camera's pure rules (`platform::common::camera`), on every OS: the format and frame rate a preset
//! records at, and what the camera's notices mean for the session.

use klipolo_capture::backend::{PresetId, Target, VideoSpec};
use klipolo_capture::media::Codec;
use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::platform::common::camera::{
    BubbleMirroring, CameraWatch, FormatFacts, RateChoice, Round, duration_of, frame_durations,
    pick_format, pick_rate, suspended_blanks,
};
use klipolo_capture::platform::common::vt_ids::camera_own_order;
use klipolo_capture::preflight::cursor::CursorPlan;

fn f(w: u32, h: u32, fps: f64, nv12: bool) -> FormatFacts {
    FormatFacts {
        width: w,
        height: h,
        max_fps: fps,
        nv12,
    }
}

#[test]
fn a_webcam_without_nv12_records_its_smallest_format_that_meets_the_preset() {
    // A USB webcam's 720p30 and 1080p30 come as MJPEG, its YUY2 at 5 fps: 720p30 for Camera720, never 640×480.
    let formats = [
        f(640, 480, 30.0, false),
        f(1280, 720, 10.0, false),
        f(1280, 720, 30.0, false),
        f(1920, 1080, 30.0, false),
        f(3840, 2160, 30.0, false),
    ];
    assert_eq!(pick_format(&formats, (1280, 720, 30)), Some(2));
    assert_eq!(pick_format(&formats, (1920, 1080, 30)), Some(3));
    // A camera whose only formats are larger but too slow still records, at the smallest.
    let slow = [f(3840, 2160, 15.0, true), f(1920, 1080, 15.0, true)];
    assert_eq!(pick_format(&slow, (1280, 720, 30)), Some(1));
    assert_eq!(pick_format(&[], (1280, 720, 30)), None);
}

#[test]
fn the_frame_rate_range_that_paces_thirty() {
    // A FaceTime camera's one range, discrete rates, NTSC's 29.97, a slow camera and a 60-only one.
    assert_eq!(pick_rate(&[(1.0, 30.0)], 30), Some(RateChoice::Exact(0)));
    assert_eq!(
        pick_rate(&[(15.0, 15.0), (30.0, 30.0)], 30),
        Some(RateChoice::Exact(1))
    );
    assert_eq!(
        pick_rate(&[(29.97, 29.97)], 30),
        Some(RateChoice::Fastest(0))
    );
    assert_eq!(
        pick_rate(&[(5.0, 10.0), (5.0, 15.0)], 30),
        Some(RateChoice::Fastest(1))
    );
    assert_eq!(
        pick_rate(&[(60.0, 60.0), (50.0, 50.0)], 30),
        Some(RateChoice::Slowest(1))
    );
    assert_eq!(pick_rate(&[], 30), None);
}

#[test]
fn the_camera_s_notices_are_one_loss_one_return_stalls_and_blank_pictures() {
    let mut w = CameraWatch::default();
    assert_eq!(w.disconnected("other", "cam"), None, "another camera");
    assert_eq!(
        w.trouble(true),
        Some(PipeEvent::Stalled),
        "a runtime error while connected"
    );
    assert_eq!(w.disconnected("cam", "cam"), Some(PipeEvent::DeviceLost));
    assert_eq!(w.disconnected("cam", "cam"), None, "one loss");
    assert_eq!(w.trouble(false), None, "the loss's own runtime error");
    assert_eq!(w.connected("other", "cam"), None);
    assert_eq!(w.connected("cam", "cam"), Some(PipeEvent::DeviceBack));
    assert_eq!(w.connected("cam", "cam"), None, "back once");
    // A runtime error with the camera gone, before its disconnect notice: the loss, once.
    assert_eq!(w.trouble(false), Some(PipeEvent::DeviceLost));
    assert_eq!(w.disconnected("cam", "cam"), None);
    assert_eq!(w.suspended(true), Some(PipeEvent::Blank(true)));
    assert_eq!(w.suspended(true), None);
    assert_eq!(w.suspended(false), Some(PipeEvent::Blank(false)));
    // A suspended camera counts a blank picture at once, then two a second: the 2 s stall rule never fires.
    assert_eq!(suspended_blanks(10, 10), 1);
    assert_eq!(suspended_blanks(10, 10 + 1_999_999_999), 4);
}

#[test]
fn the_frame_durations_come_from_the_ranges_rates() {
    // A camera whose range spans 30 (1-30): 1/30.
    assert_eq!(frame_durations(&[(1.0, 30.0)], 30), [(1, 30)]);
    // The T2's FaceTime camera (the device smoke): one-rate ranges stated 1000000/30000030, which refused 1/30. Its own
    // statement comes first, then the exact fraction.
    let facetime = [
        (30.000_030_000_03, 30.000_030_000_03),
        (29.000_049, 29.000_049),
        (25.0, 25.0),
    ];
    assert_eq!(
        frame_durations(&facetime, 30),
        [(1_000_000, 30_000_030), (1, 30)]
    );
    // NTSC's 29.97 as a one-rate range: as stated, then 1001/30000.
    assert_eq!(
        frame_durations(&[(29.970_029_97, 29.970_029_97)], 30),
        [(1_000_000, 29_970_030), (1001, 30_000)]
    );
    // A 15-only camera: its fastest; a 60-only one: its slowest (no struct getter is read).
    assert_eq!(
        frame_durations(&[(5.0, 15.0)], 30),
        [(1_000_000, 15_000_000), (1, 15)]
    );
    assert_eq!(
        frame_durations(&[(60.0, 60.0)], 30),
        [(1_000_000, 60_000_000), (1, 60)]
    );
    assert!(frame_durations(&[], 30).is_empty());
    assert_eq!(
        duration_of(59.940_059_94, Round::Longer),
        Some((1001, 60_000))
    );
    assert_eq!(duration_of(7.5, Round::Longer), Some((2, 15)));
    assert_eq!(duration_of(f64::NAN, Round::Longer), None);
    assert_eq!(duration_of(0.0, Round::Shorter), None);
}

#[test]
fn an_odd_rate_s_duration_stays_inside_the_camera_s_range() {
    // No exact fraction: rounded toward the inside, so macOS never refuses it.
    let seconds = |(v, t): (i64, i32)| v as f64 / f64::from(t);
    // The fastest of 5-12.34 fps: 1/12.34 s is 81 037.3 µs, so 81 038, not below the range's shortest frame.
    let fastest = frame_durations(&[(5.0, 12.34)], 30);
    assert_eq!(fastest.last(), Some(&(81_038, 1_000_000)));
    // The slowest of 40.3-60 fps (a camera faster than asked): 24 813.9 µs, so 24 813, not past its longest frame.
    let slowest = frame_durations(&[(40.3, 60.0)], 30);
    assert_eq!(slowest.last(), Some(&(24_813, 1_000_000)));
    for d in fastest {
        assert!(
            seconds(d) >= 1.0 / 12.34 - 1e-12 && seconds(d) <= 1.0 / 5.0,
            "{d:?}"
        );
    }
    for d in slowest {
        assert!(
            seconds(d) <= 1.0 / 40.3 + 1e-12 && seconds(d) >= 1.0 / 60.0,
            "{d:?}"
        );
    }
}

#[test]
fn without_a_list_the_camera_takes_a_dedicated_engine_only_last() {
    // The take named no encoders: a shared hardware H.264 first, then software, then
    // the T2's AVE or Apple silicon's media engine last (the screen's own pick takes those first), so the camera always
    // starts where any H.264 encoder exists.
    let vt = |e: &str| format!("com.apple.videotoolbox.videoencoder.{e}");
    let (gva, ave, hevc_ave, sw) = (vt("h264.gva"), vt("ave.avc"), vt("ave.hevc"), vt("h264"));
    let t2 = [
        (hevc_ave.as_str(), Codec::Hevc, true),
        (ave.as_str(), Codec::H264, true),
        (gva.as_str(), Codec::H264, true),
        (sw.as_str(), Codec::H264, false),
    ];
    assert_eq!(camera_own_order(&t2, "x86_64"), [gva, sw.clone(), ave]);
    let media = vt("h264.media");
    let apple = [
        (media.as_str(), Codec::H264, true),
        (sw.as_str(), Codec::H264, false),
    ];
    assert_eq!(camera_own_order(&apple, "aarch64"), [sw, media.clone()]);
    let only_media = [(media.as_str(), Codec::H264, true)];
    assert_eq!(camera_own_order(&only_media, "aarch64"), [media]);
    assert!(camera_own_order(&[(hevc_ave.as_str(), Codec::Hevc, true)], "x86_64").is_empty());
}

/// The camera bubble's mirroring: set by Klipolo, never AVFoundation's automatic rule, and mirrored as
/// "Mirror my camera" says; a camera without a bubble has none. Nothing captures.
#[test]
fn the_bubble_is_mirrored_as_the_switch_says() {
    let spec = |bubble, bubble_mirrored| VideoSpec {
        target: Target::Camera {
            id: "camera-1".into(),
        },
        preset: PresetId::Camera720,
        cursor: CursorPlan::None,
        sound: false,
        encoders: Vec::new(),
        bubble,
        bubble_mirrored,
    };
    let set = |mirrored| {
        Some(BubbleMirroring {
            automatic: false,
            mirrored,
        })
    };
    assert_eq!(BubbleMirroring::of(&spec(true, true)), set(true));
    assert_eq!(BubbleMirroring::of(&spec(true, false)), set(false));
    assert_eq!(BubbleMirroring::of(&spec(false, true)), None);
}
