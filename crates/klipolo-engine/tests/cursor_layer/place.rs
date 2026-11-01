//! Where the cursor lands and how big it is, and where nothing is drawn.

use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::plan::{ARROW_PTS, CursorDraw, PlanContext, cursor_layer_id};
use klipolo_model::cursor_style::{CursorCapture, CursorLook, CursorStyle, Smoothing};
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::Project;
use klipolo_model::timeline::{Framing, FramingMode, ItemContent, ZoomTarget};

use crate::support::recording::{cursor_project, split_at};
use crate::support::{bx, item, secs, zoom_track};
use crate::{
    AREA_H, cursor_at, cursors_at, export_ctx, footage, glide, mapped, near, rest, style, take,
    the_take,
};

/// A 2× Rect zoom on the right half of the frame from 1 s to 9 s.
fn zoomed(p: &mut Project) {
    let target = ZoomTarget::Rect {
        rect: bx(0.5, 0.25, 0.5, 0.5),
    };
    let content = ItemContent::Zoom { target, scale: 2.0 };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "z",
        1.0,
        8.0,
        bx(0.0, 0.0, 1.0, 1.0),
        content,
    )])];
}

#[test]
fn the_cursor_lands_where_the_footage_shows_its_point_through_zoom_and_framing() {
    let dir = tempfile::tempdir().unwrap();
    let lines = take(rest(0.7, 0.4, 0.0, 10.0));
    let mut p = cursor_project(dir.path(), &lines, CursorStyle::default());
    zoomed(&mut p);
    let fill = Framing {
        mode: FramingMode::Fill,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    };
    p.timeline.main[0].framing.insert(Portrait, fill);
    let ctx = export_ctx(p);
    let point = the_take(&ctx).track.position(secs(3.0)).unwrap();
    for (t, format) in [(0.5, Landscape), (5.0, Landscape), (5.0, Portrait)] {
        let v = footage(&ctx, t, format);
        let c = cursors_at(&ctx, t, format).pop().expect("drawn");
        near(c.hotspot, mapped(&v, point), 1e-9);
        assert_eq!(
            (c.clip, c.radius),
            (v.dest, v.radius),
            "clipped to the footage"
        );
        assert_eq!(c.id.as_ref(), cursor_layer_id("take"));
    }
    // Zoomed on the right half, the point is drawn further right and down than unzoomed.
    let (plain, zoom) = (cursor_at(&ctx, 0.5).unwrap(), cursor_at(&ctx, 5.0).unwrap());
    assert!(footage(&ctx, 5.0, Landscape).crop.w < 0.55);
    assert!(zoom.hotspot.0 < plain.hotspot.0 && zoom.hotspot.1 < plain.hotspot.1);
    // Portrait Fill without the zoom crops the point out of the picture: nothing is drawn.
    assert!(cursors_at(&ctx, 0.5, Portrait).is_empty());
}

#[test]
fn it_scales_with_the_zoom_and_the_style_size() {
    let dir = tempfile::tempdir().unwrap();
    let lines = take(rest(0.7, 0.4, 0.0, 10.0));
    let mut p = cursor_project(dir.path(), &lines, style(|s| s.size = 1.0));
    zoomed(&mut p);
    let ctx = export_ctx(p.clone());
    let height = |ctx: &PlanContext, t: f64, size: f64| {
        let v = footage(ctx, t, Landscape);
        let c = cursor_at(ctx, t).unwrap();
        // pts / area_h × box.h / crop.h × size: the Arrow look's 22 points (the take has no shapes).
        let want = ARROW_PTS / AREA_H * v.dest.h / v.crop.h * size;
        assert!((c.height - want).abs() < 1e-12, "{} != {want}", c.height);
        c.height
    };
    let plain = height(&ctx, 0.5, 1.0);
    assert!(
        (height(&ctx, 5.0, 1.0) / plain - 2.0).abs() < 0.01,
        "2× zoom"
    );
    p.cursor.size = 2.0;
    let ctx = export_ctx(p);
    assert!(
        (height(&ctx, 0.5, 2.0) / plain - 2.0).abs() < 1e-12,
        "size 2"
    );
}

#[test]
fn nothing_is_drawn_where_the_frames_already_show_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 10.0));
    lines.push(Line::Burned { t: secs(5.0) });
    let p = cursor_project(dir.path(), &lines, CursorStyle::default());
    let capture = |p: &Project| p.recordings[0].cursor.as_ref().unwrap().capture;
    assert_eq!(capture(&p), CursorCapture::BurnedFrom { at_us: secs(5.0) });
    let ctx = export_ctx(p.clone());
    assert!(cursor_at(&ctx, 4.9).is_some());
    assert!(cursor_at(&ctx, 5.0).is_none() && cursor_at(&ctx, 8.0).is_none());
    // A take recorded burned in draws nothing anywhere.
    let mut burned = p.clone();
    burned.recordings[0].cursor.as_mut().unwrap().capture = CursorCapture::Burned;
    let ctx = export_ctx(burned);
    assert!(
        [0.0, 1.0, 4.9, 7.0]
            .iter()
            .all(|&t| cursor_at(&ctx, t).is_none())
    );
    // Nor does one whose own file says the frames show it, whatever the project says.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 10.0));
    lines[0] = crate::support::takes::header(Capture::Burned);
    let mut p = cursor_project(dir.path(), &lines, CursorStyle::default());
    p.recordings[0].cursor.as_mut().unwrap().capture = CursorCapture::Separate;
    assert!(cursor_at(&export_ctx(p), 1.0).is_none());
}

#[test]
fn the_last_cursor_is_held_across_a_gap_before_the_burn_in() {
    // The source went silent at 3 s and the recorder burned the cursor in at 4.6 s: in between, the
    // frames show no cursor, so the last one is drawn where it was.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(glide((0.2, 0.5), (0.6, 0.5), 0.0, 2.0));
    lines.extend(rest(0.6, 0.5, 2.5, 3.0));
    lines.push(Line::Burned { t: secs(4.6) });
    let ctx = export_ctx(cursor_project(dir.path(), &lines, CursorStyle::default()));
    let last = the_take(&ctx).track.position(secs(3.0)).unwrap();
    let c = cursor_at(&ctx, 4.0).expect("held");
    near(
        c.hotspot,
        mapped(&footage(&ctx, 4.0, Landscape), last),
        1e-9,
    );
    assert_eq!(c.opacity, 1.0);
    assert!(cursor_at(&ctx, 4.6).is_none());
}

#[test]
fn a_clip_with_its_own_hidden_style_draws_no_cursor_and_its_neighbour_does() {
    let dir = tempfile::tempdir().unwrap();
    let lines = take(rest(0.5, 0.5, 0.0, 10.0));
    let mut p = cursor_project(dir.path(), &lines, CursorStyle::default());
    split_at(&mut p, 5.0);
    p.timeline.main[0].cursor = Some(style(|s| s.size = 2.0));
    p.timeline.main[1].cursor = Some(style(|s| s.hidden = true));
    let first = p.timeline.main[0].id.clone();
    let ctx = export_ctx(p);
    let c = cursor_at(&ctx, 2.0).expect("the first clip draws");
    assert_eq!(c.id.as_ref(), cursor_layer_id(&first));
    let v = footage(&ctx, 2.0, Landscape);
    assert!((c.height - ARROW_PTS / AREA_H * v.dest.h / v.crop.h * 2.0).abs() < 1e-12);
    assert!(cursor_at(&ctx, 7.0).is_none(), "the second clip hides it");
}

#[test]
fn a_heal_gap_shows_one_cursor_the_held_one_then_the_healed_runs() {
    // The live switch failed: the recorder healed with a new screen run from 5 s whose frames show the cursor. Across
    // the gap the footage holds its nearest frame: the first run's last (no cursor) up to the middle, the healed run's
    // first (with its cursor) after it. Never two cursors, never none.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 3.0));
    lines.push(Line::Burned { t: secs(5.0) });
    let mut p = cursor_project(dir.path(), &lines, CursorStyle::default());
    let mut healed = p.assets[0].clone();
    healed.id = "screen-2".into();
    p.assets.push(healed);
    let screen = &mut p.recordings[0].sources[0];
    screen.runs[0].duration_us = secs(3.0);
    let mut run = screen.runs[0].clone();
    (run.asset_id, run.offset_us, run.duration_us) = ("screen-2".into(), secs(5.0), secs(55.0));
    screen.runs.push(run);
    let ctx = export_ctx(p);
    let shown = |t: f64| footage(&ctx, t, Landscape).source.asset_id.to_string();
    for t in [3.5, 4.0] {
        assert_eq!(shown(t), "screen", "{t}");
        assert!(
            cursor_at(&ctx, t).is_some(),
            "held over a frame without one: {t}"
        );
    }
    for t in [4.1, 4.9, 5.0, 6.0] {
        assert_eq!(shown(t), "screen-2", "{t}");
        assert!(cursor_at(&ctx, t).is_none(), "the frame shows its own: {t}");
    }
}

#[test]
fn the_cursor_rides_the_screen_sources_sync_nudge_and_clip_speed() {
    let dir = tempfile::tempdir().unwrap();
    let lines = take(glide((0.1, 0.5), (0.9, 0.5), 0.0, 8.0));
    let mut p = cursor_project(dir.path(), &lines, style(|s| s.smoothing = Smoothing::Off));
    p.recordings[0].sources[0].nudge_us = 100_000;
    p.timeline.main[0].speed = 2.0;
    let ctx = export_ctx(p);
    let take = the_take(&ctx);
    // 1 s into the clip at 2× is 2 s of the take; the nudged screen shows its frame from 1.9 s there.
    let v = footage(&ctx, 1.0, Landscape);
    assert_eq!(v.source.time_us, secs(1.9));
    let c = cursor_at(&ctx, 1.0).unwrap();
    near(
        c.hotspot,
        mapped(&v, take.track.position(secs(1.9)).unwrap()),
        1e-9,
    );
    // The smear covers the movement over the shutter on the timeline: twice the take's at 2×.
    let (vx, _) = take.with_drawn(Smoothing::Off, |path| path.velocity(&take.track, secs(1.9)));
    let want =
        f64::from(vx) * 2.0 * klipolo_engine::plan::SHUTTER / 30.0 * 0.5 * v.dest.w / v.crop.w;
    assert!(want > 0.0 && (c.blur.0 - want).abs() < 1e-12, "{c:?}");
}

/// A PNG's first 24 bytes (signature, IHDR) for a `w` × `h` image: all a take checks of it.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    b.extend(b"IHDR");
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    b.extend([8, 6, 0, 0, 0]);
    b
}

#[test]
fn a_missing_shape_draws_the_arrow_look() {
    let dir = tempfile::tempdir().unwrap();
    let cursors = dir.path().join("take").join("cursors");
    std::fs::create_dir_all(&cursors).unwrap();
    std::fs::write(cursors.join("0.png"), png(32, 32)).unwrap();
    let shape = |id: u16, pts| Line::ShapeDef {
        id,
        key: format!("{:016x}", id + 1),
        file: format!("cursors/{id}.png"),
        pw: 32,
        ph: 32,
        hx: 8.0,
        hy: 4.0,
        pts,
        sys: None,
    };
    // Shape 1's image never reached the disk.
    let mut lines = take([shape(0, 24.0), shape(1, 30.0), Line::Shape { t: 0, id: 0 }]);
    lines.push(Line::Shape {
        t: secs(3.0),
        id: 1,
    });
    lines.extend(rest(0.5, 0.5, 0.0, 6.0));
    let mut p = cursor_project(dir.path(), &lines, style(|s| s.size = 1.0));
    let ctx = export_ctx(p.clone());
    let path = &p.recordings[0].cursor.as_ref().unwrap().path;
    let file = std::fs::canonicalize(cursors.join("0.png")).unwrap();
    let c = cursor_at(&ctx, 1.0).unwrap();
    let CursorDraw::Shape {
        take,
        shape,
        file: drawn,
        hotspot,
        aspect,
    } = &c.look
    else {
        panic!("{:?}", c.look)
    };
    assert_eq!((take.as_ref(), *shape), (path.as_str(), 0));
    assert_eq!(drawn.as_ref(), file.to_str().unwrap());
    assert_eq!((*hotspot, *aspect), ((0.25, 0.125), 1.0));
    let v = footage(&ctx, 1.0, Landscape);
    let scale = v.dest.h / v.crop.h;
    assert!(
        (c.height - 24.0 / AREA_H * scale).abs() < 1e-12,
        "its own points"
    );
    let c = cursor_at(&ctx, 4.0).unwrap();
    assert_eq!(c.look, CursorDraw::Arrow);
    assert!(
        (c.height - ARROW_PTS / AREA_H * scale).abs() < 1e-12,
        "the arrow's points"
    );
    // Klipolo's own looks never use the take's shapes.
    for (look, draw) in [
        (CursorLook::Dot, CursorDraw::Dot),
        (CursorLook::Arrow, CursorDraw::Arrow),
    ] {
        p.cursor.look = look;
        assert_eq!(cursor_at(&export_ctx(p.clone()), 1.0).unwrap().look, draw);
    }
}
