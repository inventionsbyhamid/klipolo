//! The cursor and the view it is drawn in: ripples finish where the click was when the
//! cursor hides or leaves the zoomed view, they and the press run on screen time at any clip speed, a shape mask on
//! the footage masks the cursor too, and a burned-in take never reads the cursor cache.

use klipolo_cursor::format::Capture;
use klipolo_engine::plan::{CursorMask, PRESS, frame_plan};
use klipolo_model::contrib::builtin;
use klipolo_model::cursor_style::{ClickEffect, CursorStyle, Rgba};
use klipolo_model::params::{ParamValue, PointN};
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::timeline::{EffectInstance, ItemContent, ZoomTarget};

use crate::support::recording::cursor_project;
use crate::support::{bx, item, secs, zoom_track};
use crate::{cursor_at, export_ctx, footage, glide, mapped, near, press, rest, style, take};

fn rippling(s: &mut CursorStyle) {
    s.click = ClickEffect::RippleAndPress;
    s.click_color = Rgba([0xFF, 0, 0, 0x80]);
}

#[test]
fn ripples_finish_where_the_click_was_when_the_cursor_hides_or_leaves_the_view() {
    let dir = tempfile::tempdir().unwrap();
    // At rest (hidden once still for half a second), clicked where it rests at 3 s without moving.
    let mut lines = take(rest(0.5, 0.5, 0.0, 4.0));
    lines.extend(press(0, (0.5, 0.5), 3.0, 3.05));
    // Clicked at (0.7, 0.4) at 6 s, then gone left out of the zoomed view within 70 ms.
    lines.extend(rest(0.7, 0.4, 5.0, 6.0));
    lines.extend(press(0, (0.7, 0.4), 6.0, 6.03));
    lines.extend(glide((0.7, 0.4), (0.1, 0.4), 6.03, 6.1));
    lines.extend(rest(0.1, 0.4, 6.5, 9.0));
    let mut p = cursor_project(
        dir.path(),
        &lines,
        style(|s| {
            rippling(s);
            s.hide_when_still_us = Some(500_000);
        }),
    );
    let zoom = ItemContent::Zoom {
        target: ZoomTarget::Rect {
            rect: bx(0.5, 0.25, 0.5, 0.5),
        },
        scale: 2.0,
    };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "z",
        5.0,
        4.0,
        bx(0.0, 0.0, 1.0, 1.0),
        zoom,
    )])];
    let ctx = export_ctx(p);
    assert!(cursor_at(&ctx, 2.9).is_none(), "hidden at rest");
    let c = cursor_at(&ctx, 3.1).expect("its ripple shows");
    assert_eq!((c.opacity, c.ripple_count), (0.0, 1), "{c:?}");
    near(
        c.ripples[0].center,
        mapped(&footage(&ctx, 3.1, Landscape), (0.5, 0.5)),
        1e-9,
    );
    assert!(cursor_at(&ctx, 3.46).is_none(), "and goes when it ends");
    // Out of the zoomed view: the cursor isn't drawn, the ring finishes where the click was, clipped to the view.
    let v = footage(&ctx, 6.2, Landscape);
    assert!(v.crop.w < 0.6, "zoomed in: {v:?}");
    let c = cursor_at(&ctx, 6.2).expect("its ripple shows");
    assert_eq!(
        (c.opacity, c.ripple_count, c.clip),
        (0.0, 1, v.dest),
        "{c:?}"
    );
    near(c.ripples[0].center, mapped(&v, (0.7, 0.4)), 1e-9);
    assert!(cursor_at(&ctx, 6.46).is_none());
}

#[test]
fn ripples_and_the_press_run_on_screen_time_at_any_clip_speed() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 8.0));
    lines.extend(press(0, (0.5, 0.5), 2.0, 3.0));
    let mut p = cursor_project(dir.path(), &lines, style(rippling));
    for speed in [0.5, 1.0, 2.0] {
        p.timeline.main[0].speed = speed;
        let ctx = export_ctx(p.clone());
        // The take's 2 s plays at 2 / speed on the timeline; from there everything takes the same screen time.
        let at = 2.0 / speed;
        let c = cursor_at(&ctx, at + 0.225).unwrap();
        let [r] = c.active_ripples() else {
            panic!("{speed}: {c:?}")
        };
        assert!(
            (r.alpha - 0.5 * 128.0 / 255.0).abs() < 1e-4,
            "{speed}: {r:?}"
        );
        assert_eq!(
            cursor_at(&ctx, at + 0.46).unwrap().ripple_count,
            0,
            "{speed}"
        );
        let half = cursor_at(&ctx, at + 0.03).unwrap().press;
        assert!(
            (half - (1.0 - (1.0 - PRESS) / 2.0)).abs() < 1e-3,
            "{speed}: {half}"
        );
        assert_eq!(cursor_at(&ctx, at + 0.07).unwrap().press, PRESS, "{speed}");
    }
}

#[test]
fn a_shape_mask_on_the_footage_masks_the_cursor_too() {
    let dir = tempfile::tempdir().unwrap();
    let lines = take(rest(0.5, 0.5, 0.0, 8.0));
    let mut p = cursor_project(dir.path(), &lines, CursorStyle::default());
    let mut mask = EffectInstance::new(builtin::EFFECT_MASK);
    for (k, v) in [
        ("shape", ParamValue::Text("rounded".into())),
        ("center", ParamValue::Point(PointN { x: 0.25, y: 0.5 })),
        ("cornerRadius", ParamValue::Number(0.3)),
        ("feather", ParamValue::Number(4.0)),
    ] {
        mask.params.insert(k.into(), v);
    }
    let vignette = EffectInstance::new(builtin::EFFECT_VIGNETTE);
    p.timeline.main[0].effects = vec![vignette.clone(), mask.clone()];
    let want = CursorMask {
        shape: 1,
        center: (0.25, 0.5),
        corner: 0.3,
        feather: 4.0,
    };
    assert_eq!(
        cursor_at(&export_ctx(p.clone()), 1.0).unwrap().mask,
        Some(want)
    );
    // Switched off, or another effect alone: nothing masks it.
    p.timeline.main[0].effects[1].enabled = false;
    assert_eq!(cursor_at(&export_ctx(p.clone()), 1.0).unwrap().mask, None);
    p.timeline.main[0].effects = vec![vignette];
    assert_eq!(cursor_at(&export_ctx(p), 1.0).unwrap().mask, None);
}

#[test]
fn a_burned_in_take_never_reads_the_cursor_cache() {
    // Every take was burned in (and still is on Windows and Linux), so its frames must cost what they did
    // before the cursor layer: the plan returns before the cache's lock. A separate take reads it once
    // a frame.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 8.0));
    lines[0] = crate::support::takes::header(Capture::Burned);
    let mut p = cursor_project(dir.path(), &lines, CursorStyle::default());
    p.recordings[0].cursor.as_mut().unwrap().capture =
        klipolo_model::cursor_style::CursorCapture::Burned;
    let ctx = export_ctx(p.clone());
    let before = ctx.cursors.reads();
    for k in 0..100 {
        frame_plan(&ctx, secs(f64::from(k) * 0.05), Landscape);
    }
    assert_eq!(ctx.cursors.reads(), before);
    p.recordings[0].cursor.as_mut().unwrap().capture =
        klipolo_model::cursor_style::CursorCapture::Separate;
    let ctx = export_ctx(p);
    let before = ctx.cursors.reads();
    frame_plan(&ctx, secs(1.0), Landscape);
    assert_eq!(ctx.cursors.reads(), before + 1);
}
