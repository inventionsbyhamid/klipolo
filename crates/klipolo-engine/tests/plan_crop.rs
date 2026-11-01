//! A main clip's crop: the edges cut off its footage before it is framed, in every format. Fit shows what is left
//! whole, Fill covers its area from it (the Fill focus and the follow-crop move inside it), mirrored footage is cut on
//! the side the person sees, and a camera bubble keeps its whole picture.

mod support;

use klipolo_engine::plan::framing::{FULL, cover_crop, follow_crop_in};
use klipolo_engine::plan::{VideoRole, fill_crop, frame_plan};
use klipolo_model::crop::{Crop, within};
use klipolo_model::cursor_style::{ClickSource, CursorCapture, RecordingCursor};
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::{FormatKind, Project, RecordingKind};
use klipolo_model::timeline::{Framing, FramingMode, ItemContent, ZoomTarget};
use support::{bx, close, ctx, item, project, secs, videos, zoom_track};

fn crop(left: f64, top: f64, right: f64, bottom: f64) -> Crop {
    Crop {
        left,
        top,
        right,
        bottom,
    }
}

fn framing(mode: FramingMode) -> Framing {
    Framing {
        mode,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    }
}

/// The main footage's crop and box at `t` in `format`.
fn main_at(
    p: &Project,
    t: f64,
    format: FormatKind,
) -> (klipolo_model::layout::BoxN, klipolo_model::layout::BoxN) {
    let plan = frame_plan(&ctx(p.clone()), secs(t), format);
    let v = videos(&plan);
    let main = v
        .iter()
        .find(|v| v.role == VideoRole::Main)
        .expect("main footage");
    (main.crop, main.dest)
}

#[test]
fn fit_shows_the_whole_of_what_the_crop_leaves() {
    let mut p = project();
    // The 16:9 video with a quarter cut off each side: 960×1080 is left, 8:9.
    p.timeline.main[1].crop = Some(crop(0.25, 0.0, 0.25, 0.0));
    p.timeline.main[1]
        .framing
        .insert(Landscape, framing(FramingMode::Fit));
    let (shown, dest) = main_at(&p, 25.0, Landscape);
    close(shown, bx(0.25, 0.0, 0.5, 1.0));
    close(dest, bx(0.25, 0.0, 0.5, 1.0));
}

#[test]
fn fill_covers_its_area_from_what_the_crop_leaves() {
    let mut p = project();
    p.timeline.main[1].crop = Some(crop(0.25, 0.1, 0.25, 0.0));
    // Imported video fills portrait by default: 960×972 left (aspect 0.9877), cut across to 9:16.
    let (shown, dest) = main_at(&p, 25.0, Portrait);
    close(dest, FULL);
    let kept = bx(0.25, 0.1, 0.5, 0.9);
    let aspect = (16.0 / 9.0) * 0.5 / 0.9;
    close(
        shown,
        within(kept, cover_crop(aspect, 9.0 / 16.0, 0.5, 0.5)),
    );
    assert!(
        shown.x >= 0.25 - 1e-9 && shown.x + shown.w <= 0.75 + 1e-9,
        "{shown:?}"
    );
    assert!(
        (shown.y - 0.1).abs() < 1e-9 && (shown.h - 0.9).abs() < 1e-9,
        "{shown:?}"
    );
    // The focus moves the crop inside what is left: all the way left starts at the cut.
    p.timeline.main[1].framing.insert(
        Portrait,
        Framing {
            focus_x: 0.0,
            ..framing(FramingMode::Fill)
        },
    );
    let (left, _) = main_at(&p, 25.0, Portrait);
    assert!((left.x - 0.25).abs() < 1e-9, "{left:?}");
}

#[test]
fn a_clip_without_a_crop_frames_as_before() {
    let p = project();
    let (shown, _) = main_at(&p, 21.0, Portrait);
    close(shown, bx(0.341796875, 0.0, 0.31640625, 1.0));
}

#[test]
fn mirrored_footage_is_cut_on_the_side_the_person_sees() {
    let mut p = project();
    // A camera-only take, mirrored: the camera is the main footage.
    let rec = &mut p.recordings[0];
    rec.sources.remove(0);
    rec.sources[0].runs[0].offset_us = 0;
    rec.primary = "camera".into();
    rec.kind = RecordingKind::Camera;
    let clip = &mut p.timeline.main[0];
    clip.camera.as_mut().unwrap().mirror = true;
    clip.framing.insert(Landscape, framing(FramingMode::Fit));
    // The person cuts the left fifth as they see it: in the file, the right fifth.
    clip.crop = Some(crop(0.2, 0.0, 0.0, 0.0));
    let (shown, _) = main_at(&p, 2.0, Landscape);
    close(shown, bx(0.0, 0.0, 0.8, 1.0));
}

#[test]
fn the_camera_bubble_keeps_its_whole_picture() {
    let mut p = project();
    p.timeline.main[0].crop = Some(crop(0.1, 0.1, 0.1, 0.1));
    let plan = frame_plan(&ctx(p), secs(2.0), Landscape);
    let v = videos(&plan);
    let screen = v.iter().find(|v| v.role == VideoRole::Main).unwrap();
    let camera = v.iter().find(|v| v.role == VideoRole::Camera).unwrap();
    close(screen.crop, bx(0.1, 0.1, 0.8, 0.8));
    // The bubble fills its square from the camera's centre, as without a crop.
    let without = frame_plan(&ctx(project()), secs(2.0), Landscape);
    let before = videos(&without)
        .into_iter()
        .find(|v| v.role == VideoRole::Camera)
        .unwrap()
        .crop;
    close(camera.crop, before);
}

#[test]
fn the_fill_focus_offers_the_way_fill_crops_what_is_left() {
    let mut p = project();
    let b = 1;
    // A 16:9 video cut to a square fills a square frame whole: no focus to move.
    p.timeline.main[b].crop = Some(crop(0.21875, 0.0, 0.21875, 0.0));
    close(fill_crop(&p, &p.timeline.main[b], Square).unwrap(), FULL);
    // In 16:9 the square crops down; in 9:16 across.
    let wide = fill_crop(&p, &p.timeline.main[b], Landscape).unwrap();
    assert!(wide.w > 0.999 && wide.h < 0.6, "{wide:?}");
    let tall = fill_crop(&p, &p.timeline.main[b], Portrait).unwrap();
    assert!(tall.h > 0.999 && tall.w < 0.6, "{tall:?}");
}

#[test]
fn the_follow_crop_stays_inside_what_the_crop_leaves() {
    let bounds = bx(0.2, 0.0, 0.6, 1.0);
    let base = bx(0.3, 0.0, 0.3, 1.0);
    for (centre, closer) in [((0.95, 0.5), 1.0), ((0.0, 0.5), 1.5), ((0.5, 0.5), 2.0)] {
        let c = follow_crop_in(bounds, base, centre, closer);
        assert!(
            c.x >= 0.2 - 1e-9 && c.x + c.w <= 0.8 + 1e-9 && c.y >= 0.0 && c.y + c.h <= 1.0 + 1e-9,
            "{centre:?} {closer}: {c:?}"
        );
    }
    // A window as tall as what a top crop leaves: the room above it rounds to just under none, never a bad clamp.
    let below_a_top_cut = bx(0.0, 0.1, 1.0, 0.9);
    let tall = bx(0.3, 0.1, 0.3, 0.9);
    for centre in [(0.5, 0.5), (0.5, 0.0), (0.5, 1.0), (f64::NAN, f64::NAN)] {
        let c = follow_crop_in(below_a_top_cut, tall, centre, 1.0);
        assert!(
            (c.y - 0.1).abs() < 1e-9 && (c.h - 0.9).abs() < 1e-9,
            "{centre:?}: {c:?}"
        );
    }
    // The same across, beside a left cut.
    let beside_a_left_cut = bx(0.2, 0.0, 0.8, 1.0);
    let wide = bx(0.2, 0.3, 0.8, 0.3);
    let c = follow_crop_in(beside_a_left_cut, wide, (0.0, 0.5), 1.0);
    assert!(
        (c.x - 0.2).abs() < 1e-9 && (c.w - 0.8).abs() < 1e-9,
        "{c:?}"
    );
    // Without a crop it is the frame's follow-crop.
    let whole = follow_crop_in(FULL, base, (0.95, 0.5), 1.0);
    close(
        whole,
        klipolo_engine::plan::framing::follow_crop(base, (0.95, 0.5), 1.0),
    );
}

#[test]
fn a_screen_take_that_follows_the_cursor_plans_with_its_top_cut() {
    // A screen take with a cursor track fills and follows in 9:16 and 1:1 by default; the crop cuts the menu bar off.
    let mut p = project();
    p.recordings[0].cursor = Some(RecordingCursor {
        path: "/media/rec/cursor.jsonl".into(),
        capture: CursorCapture::Separate,
        clicks: ClickSource::Events,
    });
    p.settings.formats = vec![Landscape, Portrait, Square];
    for top in [0.1, 0.2] {
        p.timeline.main[0].crop = Some(crop(0.0, top, 0.0, 0.0));
        for format in [Portrait, Square] {
            let (shown, _) = main_at(&p, 2.0, format);
            assert!(
                shown.y >= top - 1e-9 && shown.y + shown.h <= 1.0 + 1e-9,
                "{format:?} top {top}: {shown:?}"
            );
        }
    }
    p.timeline.main[0].crop = Some(crop(0.0, 0.1, 0.0, 0.1));
    let (shown, _) = main_at(&p, 2.0, Portrait);
    assert!(
        shown.y >= 0.1 - 1e-9 && shown.y + shown.h <= 0.9 + 1e-9,
        "{shown:?}"
    );
}

#[test]
fn a_zoom_near_a_cut_edge_stays_inside_what_the_crop_leaves() {
    // The left 30 % is cut (a sidebar), and a 2x zoom aims at x 0.2 to 0.5, across the cut edge.
    let mut p = project();
    p.timeline.main[1].crop = Some(crop(0.3, 0.0, 0.0, 0.0));
    p.timeline.main[1]
        .framing
        .insert(Landscape, framing(FramingMode::Fit));
    let target = ZoomTarget::Rect {
        rect: bx(0.2, 0.25, 0.3, 0.5),
    };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "z",
        21.0,
        3.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Zoom { target, scale: 2.0 },
    )])];
    let mut zoomed = false;
    for t in [21.5, 22.0, 23.0, 23.5] {
        let (shown, _) = main_at(&p, t, Landscape);
        assert!(shown.x >= 0.3 - 1e-9, "at {t}: {shown:?}");
        assert!(shown.x + shown.w <= 1.0 + 1e-9, "at {t}: {shown:?}");
        zoomed |= shown.w < 0.69;
    }
    assert!(zoomed, "the zoom never came in");
    // A mirrored camera-only take: the cut is on the side the person sees, and the zoom stays off it too.
    let mut cam = p.clone();
    let rec = &mut cam.recordings[0];
    rec.sources.remove(0);
    rec.sources[0].runs[0].offset_us = 0;
    rec.primary = "camera".into();
    rec.kind = RecordingKind::Camera;
    let clip = &mut cam.timeline.main[0];
    clip.camera.as_mut().unwrap().mirror = true;
    clip.framing.insert(Landscape, framing(FramingMode::Fit));
    clip.crop = Some(crop(0.3, 0.0, 0.0, 0.0));
    cam.timeline.tracks[0].items[0].start_us = secs(1.0);
    for t in [2.0, 3.0] {
        let (shown, _) = main_at(&cam, t, Landscape);
        // In the file, the person's left 30 % is the right 30 %.
        assert!(shown.x + shown.w <= 0.7 + 1e-9, "at {t}: {shown:?}");
    }
}
