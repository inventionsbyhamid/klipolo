//! A camera's pure rules (`platform::macos` captures with them, and Windows and Linux can), built on every OS
//! so the Linux CI runs test them too:
//! - which of the camera's formats records a preset (`pick_format`), which of a format's frame-rate ranges paces it
//!   (`pick_rate`), and the frame durations to try, from the ranges' rates (`frame_durations`);
//! - what the camera's notices mean for the session (`CameraWatch`): its own disconnect and return by its unique id,
//!   the session's trouble, and a suspended camera (a closed lid, a privacy shutter);
//! - how a suspended camera counts (`suspended_blanks`): as blank pictures, the way ScreenCaptureKit reports a
//!   sleeping display, so the stall rule doesn't restart a camera that is only covered. The heal rules are the crate's.
//! - how the camera bubble's picture is mirrored (`BubbleMirroring`).

use crate::backend::VideoSpec;
use crate::pipe::PipeEvent;

/// What a camera format offers: its size, its fastest rate, and whether its pixels are NV12 already (`420v`/`420f`),
/// which the capture output then passes on without converting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FormatFacts {
    pub width: u32,
    pub height: u32,
    pub max_fps: f64,
    pub nv12: bool,
}

/// A rate within this of the one asked for keeps it (29.97 keeps 30).
const RATE_SLACK: f64 = 0.1;

/// The format for `want` (width, height, fps): the smallest at least that size and rate with NV12; else the smallest at
/// least that size and rate in any pixel format (a USB webcam's are YUY2 or MJPEG); else the largest below, those that
/// keep the rate first, NV12 first among them; else (every format is larger but slower) the smallest. A slow camera
/// still records. None only without formats.
pub fn pick_format(formats: &[FormatFacts], want: (u32, u32, u32)) -> Option<usize> {
    let (w, h, fps) = want;
    let area = |f: &FormatFacts| u64::from(f.width) * u64::from(f.height);
    let fits = |f: &FormatFacts| f.width >= w && f.height >= h;
    let keeps = |f: &FormatFacts| f.max_fps + RATE_SLACK >= f64::from(fps);
    let all = || formats.iter().enumerate();
    let smallest = |of: &dyn Fn(&FormatFacts) -> bool| {
        all()
            .filter(|(_, f)| of(f))
            .min_by_key(|(i, f)| (area(f), *i))
            .map(|(i, _)| i)
    };
    smallest(&|f| fits(f) && keeps(f) && f.nv12)
        .or_else(|| smallest(&|f| fits(f) && keeps(f)))
        .or_else(|| {
            all()
                .filter(|(_, f)| !fits(f))
                .max_by_key(|(i, f)| (keeps(f), f.nv12, area(f), std::cmp::Reverse(*i)))
                .map(|(i, _)| i)
        })
        .or_else(|| smallest(&|_| true))
}

/// How a format's frame-rate range paces a take (`pick_rate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateChoice {
    /// The range holds the rate asked for: its frame duration is 1 / fps.
    Exact(usize),
    /// The range's fastest, a hair under the rate asked for (29.97 for 30) or the closest below it.
    Fastest(usize),
    /// Every range runs faster than asked: the slowest one's slowest.
    Slowest(usize),
}

/// Which of a format's frame-rate `ranges` (min fps, max fps) paces `fps`. None without ranges.
pub fn pick_rate(ranges: &[(f64, f64)], fps: u32) -> Option<RateChoice> {
    let want = f64::from(fps);
    let all = || ranges.iter().copied().enumerate();
    let held = all().find(|&(_, (lo, hi))| lo <= want + RATE_SLACK && want <= hi + RATE_SLACK);
    if let Some((i, (_, hi))) = held {
        return Some(if want <= hi {
            RateChoice::Exact(i)
        } else {
            RateChoice::Fastest(i)
        });
    }
    let below = all()
        .filter(|&(_, (_, hi))| hi < want)
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1));
    if let Some((i, _)) = below {
        return Some(RateChoice::Fastest(i));
    }
    all()
        .min_by(|a, b| a.1.0.total_cmp(&b.1.0))
        .map(|(i, _)| RateChoice::Slowest(i))
}

/// The active format's minimum frame duration for `fps` paced by `ranges` (min, max fps), as `pick_rate` chooses: the
/// (value, timescale) values to try in order, the first one macOS accepts is set. They are worked out from the ranges'
/// rates alone, never from AVFoundation's frame-duration getters, which return a struct that Cap avoids reading through
/// cidre on Intel Macs ("it crashes otherwise").
/// - A range that spans the rate asked for (the 1-30 of many cameras): 1/fps.
/// - A range of one rate (the T2's FaceTime camera lists 30.00-30.00, 29.00-29.00 and so on, each stated as
///   1000000/30000030), or the range's fastest or slowest when none holds the rate: macOS accepts only its own stated
///   duration, so first 1000000/(rate × 10⁶) as its driver states it (the device smoke: 1/30 was refused), then the
///   rate's exact fraction or its microseconds rounded toward the inside (`duration_of`).
pub fn frame_durations(ranges: &[(f64, f64)], fps: u32) -> Vec<(i64, i32)> {
    let one_rate = |(lo, hi): (f64, f64)| hi - lo < 0.001;
    let (rate, round) = match pick_rate(ranges, fps) {
        None => return vec![],
        Some(RateChoice::Exact(i)) if !one_rate(ranges[i]) => {
            return i32::try_from(fps).map_or_else(|_| vec![], |n| vec![(1, n)]);
        }
        Some(RateChoice::Exact(i) | RateChoice::Fastest(i)) => (ranges[i].1, Round::Longer),
        // Its slowest, its longest frame: rounded shorter, so it never goes past the range.
        Some(RateChoice::Slowest(i)) => (ranges[i].0, Round::Shorter),
    };
    let stated = (rate.is_finite() && rate > 0.0 && rate < 2_000.0)
        .then(|| (1_000_000, (rate * 1e6).round() as i32));
    let mut tries: Vec<(i64, i32)> = stated.into_iter().chain(duration_of(rate, round)).collect();
    tries.dedup();
    tries
}

/// Which way a duration that has no exact fraction is rounded to the microsecond: toward the inside of the camera's
/// range, so macOS never refuses it and falls back to the format's default rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    Longer,
    Shorter,
}

/// One frame's duration at `rate` fps as an exact fraction where there is one: a whole rate as 1/n, an NTSC rate
/// (n × 1000/1001, so 29.97 and 59.94) as 1001/(n × 1000), a half rate (7.5) as 2/(2r); else to the microsecond,
/// rounded `round`.
pub fn duration_of(rate: f64, round: Round) -> Option<(i64, i32)> {
    if !(rate.is_finite() && rate > 0.0 && rate < 10_000.0) {
        return None;
    }
    let near = |x: f64| (x - x.round()).abs() < 0.001;
    Some(if near(rate) {
        (1, rate.round() as i32)
    } else if near(rate * 1.001) {
        (1001, (rate * 1.001).round() as i32 * 1000)
    } else if near(rate * 2.0) {
        (2, (rate * 2.0).round() as i32)
    } else {
        let us = 1e6 / rate;
        let us = match round {
            Round::Longer => us.ceil(),
            Round::Shorter => us.floor(),
        };
        (us as i64, 1_000_000)
    })
}

/// How often a suspended camera counts one blank picture: often enough that the stall rule (2 s) never sees it as
/// stuck.
pub const SUSPENDED_BLANK_NS: u64 = 500_000_000;

/// The blank pictures a camera suspended since `since_ns` has counted by `now_ns`: one at once, then one every
/// `SUSPENDED_BLANK_NS`.
pub fn suspended_blanks(since_ns: u64, now_ns: u64) -> u64 {
    now_ns.saturating_sub(since_ns) / SUSPENDED_BLANK_NS + 1
}

/// What one camera's notices mean for the session, deduplicated (several can describe one loss).
#[derive(Clone, Debug, Default)]
pub struct CameraWatch {
    lost: bool,
    suspended: bool,
}

impl CameraWatch {
    /// A camera disconnected: `DeviceLost` when it is ours (`id` is its unique id) and not already lost.
    pub fn disconnected(&mut self, id: &str, ours: &str) -> Option<PipeEvent> {
        (id == ours && !std::mem::replace(&mut self.lost, true)).then_some(PipeEvent::DeviceLost)
    }

    /// A camera connected: `DeviceBack` when it is ours coming back.
    pub fn connected(&mut self, id: &str, ours: &str) -> Option<PipeEvent> {
        (id == ours && std::mem::replace(&mut self.lost, false)).then_some(PipeEvent::DeviceBack)
    }

    /// The session hit a runtime error or was interrupted: `Stalled` (the heal policy restarts it), unless the camera
    /// is gone, which is a loss (once) rather than a stall.
    pub fn trouble(&mut self, connected: bool) -> Option<PipeEvent> {
        if self.lost {
            None
        } else if connected {
            Some(PipeEvent::Stalled)
        } else {
            self.lost = true;
            Some(PipeEvent::DeviceLost)
        }
    }

    /// The camera's `isSuspended` changed: `Blank(suspended)` when it did.
    pub fn suspended(&mut self, suspended: bool) -> Option<PipeEvent> {
        (std::mem::replace(&mut self.suspended, suspended) != suspended)
            .then_some(PipeEvent::Blank(suspended))
    }
}

/// How the camera bubble's preview layer shows the picture: as the person's "Mirror my camera" says,
/// always set by Klipolo. AVFoundation's automatic mirroring (`automaticallyAdjustsVideoMirroring`) goes by the camera's
/// position, so a Mac's camera and an iPhone's would differ; it is turned off first, since `videoMirrored` can't be set
/// while it is on. Only the layer's picture flips: the recorded file is the camera's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BubbleMirroring {
    /// `automaticallyAdjustsVideoMirroring`: always false.
    pub automatic: bool,
    /// `videoMirrored`.
    pub mirrored: bool,
}

impl BubbleMirroring {
    /// The bubble's mirroring for a camera's `spec`; None when the camera has no bubble.
    pub fn of(spec: &VideoSpec) -> Option<Self> {
        spec.bubble.then_some(Self {
            automatic: false,
            mirrored: spec.bubble_mirrored,
        })
    }
}
