//! ScreenCaptureKit's pure rules (`platform::macos` captures with them), built on every OS so the Linux
//! test them too:
//! - the stream's pacing: frames asked for 10 % faster than the take's rate, so a display that presents a frame a hair
//!   late never halves it; and how many buffers the stream may hold;
//! - the stop codes, as the heal policy's events;
//! - the picture's size: the preset's box fitted to the captured area's shape, never larger than its pixels;
//! - game sound's planar channels interleaved, the seam's PCM;
//! - a point of AppKit's global space in the captured picture (`cursor_in`, for the damage rule);
//! - how often a still screen's last frame is encoded again (the schedule is `still`'s), when a still cursor is
//!   sampled, and when a frame must be a keyframe;
//! - what a whole-display take leaves out (`left_out`): Klipolo's own processes and the notification apps.

use crate::pipe::PipeEvent;

/// `SCStreamErrorUserDeclined`: the person said no, or took the permission back.
pub const USER_DECLINED: isize = -3801;
/// `SCStreamErrorUserStopped`: the person pressed macOS's own "Stop sharing".
pub const USER_STOPPED: isize = -3817;
/// `SCStreamErrorSystemStoppedStream`: the system stopped the stream (the display slept, the lid closed).
pub const SYSTEM_STOPPED: isize = -3821;

/// The apps a whole-display take leaves out beside Klipolo's own, by bundle id: Notification Centre, which
/// draws every banner and the Notification Centre panel, so a notification never reaches the file, though the person
/// still sees it on their screen. Checked against macOS 15.6's running apps (`lsappinfo`, read only). The system's alert
/// panels (`UserNotificationCenter`: permission prompts, dialogs a tutorial may show being answered) stay in.
pub const NOTIFICATION_APPS: &[&str] = &["com.apple.notificationcenterui"];

/// A running app as ScreenCaptureKit lists it: its process and bundle id ("" without one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListedApp<'a> {
    pub pid: i32,
    pub bundle_id: &'a str,
}

/// The positions in `apps` a whole-display take leaves out (`filter.rs` builds the filter from them, and nothing else
/// decides it): this worker (`own`, whose windows are the HUD and the camera bubble), its parent (the app, the editor
/// included), any process with Klipolo's bundle id (`ours`), and the notification apps (`NOTIFICATION_APPS`). Their
/// windows and their sound are left out together.
pub fn left_out(apps: &[ListedApp<'_>], own: i32, parent: i32, ours: Option<&str>) -> Vec<usize> {
    let wanted = |a: &ListedApp<'_>| {
        a.pid == own
            || a.pid == parent
            || ours.is_some_and(|b| !b.is_empty() && a.bundle_id == b)
            || NOTIFICATION_APPS.contains(&a.bundle_id)
    };
    (apps.iter().enumerate())
        .filter(|(_, a)| wanted(a))
        .map(|(i, _)| i)
        .collect()
}

/// The stream's minimum frame interval as `(value, timescale)`: 1 / (fps × 1.1), exactly (60 fps asks for 1/66 s).
/// Cap rounds `1000 / fps` down to whole ms; a display a hair late against an exact interval would deliver every other
/// frame.
pub fn frame_interval_parts(fps: u32) -> (i64, i32) {
    let fps = i32::try_from(fps.clamp(1, 1_000)).unwrap_or(60);
    (10, fps * 11)
}

/// Buffers the stream may hold before it drops (Apple's default is 3, its most 8): 6, or 8 at 100 fps or more, so a
/// pool copy that waits on the GPU never makes ScreenCaptureKit drop.
pub fn queue_depth(fps: u32) -> isize {
    if fps >= 100 { 8 } else { 6 }
}

/// The heal policy's event for the error a stream stopped with: the system's stop restarts (the heal policy waits for a
/// sleeping display), the person's "Stop sharing" and a revoked permission stop and save, and anything else restarts as
/// a stall.
pub fn pipe_event_for(sc_error_code: isize) -> PipeEvent {
    match sc_error_code {
        SYSTEM_STOPPED => PipeEvent::StoppedBySystem,
        USER_STOPPED => PipeEvent::StoppedByUser,
        USER_DECLINED => PipeEvent::PermissionRevoked,
        _ => PipeEvent::Stalled,
    }
}

/// Two planar channels as interleaved stereo (`PcmBlock::samples`), as long as the shorter.
pub fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(left.len().min(right.len()) * 2);
    for (l, r) in left.iter().zip(right) {
        out.push(*l);
        out.push(*r);
    }
    out
}

/// A rectangle in points with a bottom-left origin, as AppKit's global coordinates are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CgRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Where a global point is in the captured picture, 0-1 from its top left (outside it is outside 0-1): `global` is in
/// `NSEvent.mouseLocation`'s points (bottom-left origin, the main display's corner at 0, 0), `display` the display's
/// frame in the same points and `captured` the recorded area in the display's own points (from its bottom-left corner):
/// the whole display, or a window on it. The damage rule's cursor and Klipolo's windows (`our_damage`); the
/// recorded cursor maps its own way since it became its own source (`cursor_map`).
pub fn cursor_in(global: (f64, f64), display: CgRect, captured: CgRect) -> (f32, f32) {
    let (x, y) = (global.0 - display.x, global.1 - display.y);
    let across = (x - captured.x) / captured.w.max(f64::MIN_POSITIVE);
    let down = (captured.y + captured.h - y) / captured.h.max(f64::MIN_POSITIVE);
    (across as f32, down as f32)
}

/// The picture's size for a preset's `frame` (1920×1080) over a captured area of `source` pixels: the frame turned to
/// the source's orientation (a portrait display records 1080×1920), then fitted inside it with the source's shape, so
/// nothing is stretched or letterboxed (a 16:10 display at 1080p records 1728×1080), never above the source's own
/// pixels, with even sides (4:2:0 chroma pairs), at least 2×2.
pub fn fit_size(frame: (u32, u32), source: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
    let (fw, fh) = if (frame.0 >= frame.1) == (sw >= sh) {
        frame
    } else {
        (frame.1, frame.0)
    };
    let scale = (f64::from(fw) / sw).min(f64::from(fh) / sh).min(1.0);
    let even = |n: f64| (((n / 2.0).floor() as u32) * 2).max(2);
    (even(sw * scale), even(sh * scale))
}

/// A still screen's last frame is encoded again once this much picture time has passed since it (`still`'s
/// schedule), so fragments keep closing and the GOP's keyframes keep coming (the file stays seekable).
pub const REFEED_NS: u64 = 500_000_000;

/// A still cursor a video pipe samples itself (one whose cursor comes with its frames: portal metadata, DDA) is
/// sampled this often, whatever the picture does (at least 2 Hz). macOS's cursor is its own source,
/// which beats on its own.
pub const CURSOR_BEAT_NS: u64 = 250_000_000;

/// Whether the cursor, last sampled at `last_ns`, is due a heartbeat sample at `now_ns`.
pub fn cursor_beat_due(last_ns: u64, now_ns: u64) -> bool {
    now_ns >= last_ns.saturating_add(CURSOR_BEAT_NS)
}

/// The longest stretch of picture time between two keyframes the screen pipe allows (the editor's scrub budget), so
/// a still screen, whose frames come only on its re-feeds, still gets one a second, give or take one re-feed.
pub const KEYFRAME_EVERY_NS: u64 = 1_000_000_000;

/// Whether a frame at `at_ns` is to be a keyframe, the last one being at `last_key_ns`.
pub fn keyframe_due(last_key_ns: u64, at_ns: u64) -> bool {
    at_ns >= last_key_ns.saturating_add(KEYFRAME_EVERY_NS)
}
