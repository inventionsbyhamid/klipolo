//! The vertical follow-crop: a portrait or square version of a
//! screen recording with cursor data fills and follows by default (`klipolo_model::framing::default_framing`), its crop
//! sampled from a follow path built from the raw cursor with the take's screen activity, never the style's smoothing,
//! kept inside the frame, the same in preview and export. How a take keeps its follow paths is `follow_crop/paths.rs`.

mod support;

#[path = "follow_crop/paths.rs"]
mod paths;

use std::sync::Arc;
use std::time::Duration;

use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::framing::{FULL, follow_crop};
use klipolo_engine::plan::zoom::zoom_crop;
use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_model::cursor_style::CursorStyle;
use klipolo_model::framing::effective_framing;
use klipolo_model::layout::BoxN;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{FramingMode, ItemContent, ZoomTarget};
use support::recording::cursor_project;
use support::takes::header as head;
use support::{bx, close, item, secs, videos};

/// The cursor moving from the middle to (x, y) over the first second, then resting there to 60 s (a line every 0.5 s).
pub fn resting_at(x: f32, y: f32) -> Vec<Line> {
    let mut lines = vec![head(Capture::Separate)];
    lines.extend((0..=30).map(|k| {
        let u = k as f32 / 30.0;
        Line::Move {
            t: k * 33_333,
            x: 0.5 + (x - 0.5) * u,
            y: 0.5 + (y - 0.5) * u,
        }
    }));
    lines.extend((3..=120).map(|k| Line::Move {
        t: k * 500_000,
        x,
        y,
    }));
    lines
}

/// The 60 s screen take of `lines` made in landscape, portrait and square, drawn with `style`.
pub fn take(dir: &std::path::Path, lines: &[Line], style: CursorStyle) -> Project {
    let mut p = cursor_project(dir, lines, style);
    p.settings.formats = vec![
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    p
}

pub fn exported(p: &Project) -> PlanContext {
    let tracks = CursorTracks::default();
    tracks.load_all(&wanted(p));
    PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), Arc::new(tracks))
}

/// The main footage's crop and where it lands at `t`.
fn main_at(ctx: &PlanContext, t: i64, format: FormatKind) -> (BoxN, BoxN) {
    let plan = frame_plan(ctx, t, format);
    let v = videos(&plan)[0];
    (v.crop, v.dest)
}

/// `x` inside the crop's central 40 % across (where the follow-crop holds it), or the crop at the frame's edge it is
/// near.
fn held(crop: BoxN, x: f64) -> bool {
    let near = (x - centre(crop).0).abs() <= 0.2 * crop.w + 1e-3;
    near || (x > 0.5 && (crop.x + crop.w - 1.0).abs() < 1e-9) || (x < 0.5 && crop.x.abs() < 1e-9)
}

fn centre(b: BoxN) -> (f64, f64) {
    (b.x + b.w / 2.0, b.y + b.h / 2.0)
}

#[test]
fn a_portrait_screen_recording_fills_and_follows_by_default_when_it_has_cursor_data() {
    let dir = tempfile::tempdir().unwrap();
    let p = take(dir.path(), &resting_at(0.8, 0.5), CursorStyle::default());
    let framing = effective_framing(&p, &p.timeline.main[0], FormatKind::Portrait);
    assert_eq!(framing.mode, FramingMode::Fill);
    assert!(framing.follow.is_some(), "{framing:?}");
    let ctx = exported(&p);
    let (crop, dest) = main_at(&ctx, secs(10.0), FormatKind::Portrait);
    assert!(
        (crop.w - 0.5625 / (16.0 / 9.0)).abs() < 1e-6 && crop.h == 1.0,
        "{crop:?}"
    );
    assert!(held(crop, 0.8), "follows the cursor: {crop:?}");
    assert_eq!(dest.h, 1.0, "fills");
    // Square: the same.
    let (square, _) = main_at(&ctx, secs(10.0), FormatKind::Square);
    assert!(held(square, 0.8), "{square:?}");
    // Landscape fits, as before; without cursor data portrait fits too.
    assert_eq!(main_at(&ctx, secs(10.0), FormatKind::Landscape).0, FULL);
    let mut none = p.clone();
    none.recordings[0].cursor = None;
    let (crop, dest) = main_at(&exported(&none), secs(10.0), FormatKind::Portrait);
    assert_eq!(crop, FULL);
    assert!(dest.h < 1.0, "letterboxed: {dest:?}");
}

#[test]
fn the_follow_crop_never_shows_past_the_frame() {
    // Into a corner and across to the opposite one.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = resting_at(0.99, 0.01);
    lines.extend((0..=60).map(|k| Line::Move {
        t: 61_000_000 + k * 100_000,
        x: 0.99 - 0.98 * k as f32 / 60.0,
        y: 0.01 + 0.98 * k as f32 / 60.0,
    }));
    let mut p = take(dir.path(), &lines, CursorStyle::default());
    support::lengthen(&mut p, 70.0);
    p.timeline.main[0].source_out_us = secs(70.0);
    let ctx = exported(&p);
    for format in [FormatKind::Portrait, FormatKind::Square] {
        for t in (0..secs(68.0)).step_by(100_000) {
            let (c, _) = main_at(&ctx, t, format);
            let inside =
                c.x >= 0.0 && c.y >= 0.0 && c.x + c.w <= 1.0 + 1e-9 && c.y + c.h <= 1.0 + 1e-9;
            assert!(inside, "{format:?} at {t}: {c:?}");
        }
    }
    let base = bx(0.3, 0.0, 0.3, 1.0);
    for (at, closer) in [
        ((5.0, -3.0), 1.5),
        ((f64::NAN, 0.5), 2.0),
        ((0.0, 1.0), f64::NAN),
    ] {
        let c = follow_crop(base, at, closer);
        assert!(
            c.x >= 0.0 && c.y >= 0.0 && c.x + c.w <= 1.0 && c.y + c.h <= 1.0,
            "{c:?}"
        );
    }
    close(
        follow_crop(base, (f64::NAN, f64::NAN), 2.0),
        bx(0.425, 0.25, 0.15, 0.5),
    );
}

#[test]
fn a_zoom_inside_a_following_crop_centres_the_zoom() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = take(dir.path(), &resting_at(0.8, 0.5), CursorStyle::default());
    let target = ZoomTarget::Rect {
        rect: bx(0.15, 0.35, 0.3, 0.3),
    };
    let zoom = item(
        "z",
        5.0,
        10.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom { target, scale: 2.0 },
    );
    p.timeline.tracks = vec![support::zoom_track(vec![zoom])];
    let ctx = exported(&p);
    let window = ctx.zoom.window(secs(10.0)).expect("zoomed");
    let (crop, _) = main_at(&ctx, secs(10.0), FormatKind::Portrait);
    let mut alone = p.clone();
    alone.timeline.tracks.clear();
    let (followed, _) = main_at(&exported(&alone), secs(10.0), FormatKind::Portrait);
    assert!(held(followed, 0.8), "{followed:?}");
    close(crop, zoom_crop(followed, window));
    assert!(
        (centre(crop).0 - centre(window).0).abs() < 1e-9,
        "centred on the zoom: {crop:?} {window:?}"
    );
}

#[test]
fn preview_and_export_crop_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = resting_at(0.7, 0.4);
    lines.extend((0..=40).map(|k| Line::Move {
        t: 61_000_000 + k * 50_000,
        x: 0.7 - 0.5 * k as f32 / 40.0,
        y: 0.4,
    }));
    let mut p = take(dir.path(), &lines, CursorStyle::default());
    support::lengthen(&mut p, 70.0);
    p.timeline.main[0].source_out_us = secs(70.0);
    let export = exported(&p);
    // The preview reads its takes in the background, as the session asks for them.
    let wants = wanted(&p);
    let cursors = Arc::new(CursorTracks::default());
    cursors.want(&wants);
    assert!(cursors.wait_ready(&wants, Duration::from_secs(10)));
    let (preview, build) = PlanContext::starting(Arc::new(p.clone()), None, &cursors);
    assert!(build.is_none());
    assert_eq!(preview.follows.len(), 2, "portrait and square");
    for format in [FormatKind::Portrait, FormatKind::Square] {
        for t in (0..secs(68.0)).step_by(33_333) {
            let (a, b) = (
                main_at(&preview, t, format).0,
                main_at(&export, t, format).0,
            );
            assert_eq!(
                [a.x, a.y, a.w, a.h].map(f64::to_bits),
                [b.x, b.y, b.w, b.h].map(f64::to_bits),
                "{format:?} at {t}"
            );
        }
    }
}
