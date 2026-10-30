//! The cursor's effects: "as recorded", the press, ripples and motion blur.

use klipolo_cursor::ClickSource;
use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::plan::{ARROW_PTS, PRESS, RIPPLE_REACH, SHUTTER};
use klipolo_model::cursor_style::{ClickEffect, Rgba, Smoothing};
use klipolo_model::project::FormatKind::Landscape;

use crate::support::recording::cursor_project;
use crate::support::secs;
use crate::{
    AREA_H, cursor_at, export_ctx, footage, glide, mapped, near, press, rest, style, take, the_take,
};

#[test]
fn as_recorded_uses_the_raw_path_size_one_and_no_effects() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(glide((0.2, 0.5), (0.8, 0.5), 0.0, 2.0));
    lines.extend(press(0, (0.5, 0.5), 0.9, 1.3));
    lines.extend(rest(0.8, 0.5, 2.5, 8.0));
    let busy = style(|s| {
        s.as_recorded = true;
        s.size = 3.0;
        s.smoothing = Smoothing::Strong;
        s.click = ClickEffect::RippleAndPress;
        s.motion_blur = 1.0;
        s.hide_when_still_us = Some(500_000);
    });
    let ctx = export_ctx(cursor_project(dir.path(), &lines, busy));
    let take = the_take(&ctx);
    assert_eq!(take.variants(), 0, "nothing smoothed for it");
    // Moving with the button held, then long at rest.
    for t in [1.0, 1.1, 6.0] {
        let (v, c) = (footage(&ctx, t, Landscape), cursor_at(&ctx, t).unwrap());
        near(
            c.hotspot,
            mapped(&v, take.track.position(secs(t)).unwrap()),
            1e-9,
        );
        assert!((c.height - ARROW_PTS / AREA_H * v.dest.h / v.crop.h).abs() < 1e-12);
        assert_eq!((c.opacity, c.press, c.blur), (1.0, 1.0, (0.0, 0.0)), "{t}");
        assert_eq!(c.ripple_count, 0);
    }
}

#[test]
fn the_cursor_squishes_while_the_primary_button_is_held_and_eases_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 6.0));
    lines.extend(press(0, (0.5, 0.5), 1.0, 2.0));
    lines.extend(press(1, (0.5, 0.5), 3.0, 4.0));
    let squish = style(|s| s.click = ClickEffect::RippleAndPress);
    let mut p = cursor_project(dir.path(), &lines, squish);
    let ctx = export_ctx(p.clone());
    let at = |t: f64| cursor_at(&ctx, t).unwrap().press;
    assert_eq!(
        (at(0.9), at(1.0)),
        (1.0, 1.0),
        "from full size as it goes down"
    );
    assert!(at(1.03) < 1.0 && at(1.03) > PRESS, "{}", at(1.03));
    assert_eq!(at(1.5), PRESS, "after its 60 ms ease");
    assert!(
        at(2.03) > PRESS && at(2.03) < 1.0,
        "back over 60 ms: no jump"
    );
    assert_eq!(at(2.1), 1.0);
    assert_eq!(at(3.5), 1.0, "only the primary button");
    p.cursor.click = ClickEffect::Ripple;
    assert_eq!(cursor_at(&export_ctx(p), 1.5).unwrap().press, 1.0);
}

#[test]
fn ripples_grow_and_fade_after_each_press_and_at_most_four_show() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(rest(0.5, 0.5, 0.0, 8.0));
    lines.extend(press(0, (0.25, 0.5), 1.0, 1.05));
    // Six within 450 ms, of either button.
    for k in 0..6 {
        let down = 3.0 + 0.05 * f64::from(k);
        lines.extend(press(
            k % 2,
            (0.3 + 0.05 * f32::from(k), 0.6),
            down,
            down + 0.02,
        ));
    }
    let red = |s: &mut klipolo_model::cursor_style::CursorStyle| {
        s.click = ClickEffect::Ripple;
        s.click_color = Rgba([0xFF, 0, 0, 0x80]);
    };
    let mut p = cursor_project(dir.path(), &lines, style(red));
    let ctx = export_ctx(p.clone());
    let v = footage(&ctx, 1.0, Landscape);
    // One ring where it was clicked, growing while it fades over 450 ms.
    let mut wider = 0.0;
    for ms in [0, 50, 150, 300, 449] {
        let c = cursor_at(&ctx, 1.0 + f64::from(ms) / 1e3).unwrap();
        let [r] = c.active_ripples() else {
            panic!("{c:?}")
        };
        near(r.center, mapped(&v, (0.25, 0.5)), 1e-9);
        let u = f64::from(ms) / 450.0;
        assert!((r.alpha - (1.0 - u) * 128.0 / 255.0).abs() < 1e-9, "{r:?}");
        assert!(r.radius >= wider && r.radius <= RIPPLE_REACH * c.height);
        assert_eq!(r.color, [0xFF, 0, 0, 0x80]);
        wider = r.radius;
    }
    let half = cursor_at(&ctx, 1.225).unwrap();
    assert!(
        half.ripples[0].radius > 0.5 * RIPPLE_REACH * half.height,
        "eased out"
    );
    assert_eq!(cursor_at(&ctx, 1.46).unwrap().ripple_count, 0, "gone");
    // The newest four of six, oldest first: the older, the wider.
    let c = cursor_at(&ctx, 3.26).unwrap();
    assert_eq!(c.ripple_count, 4);
    for (k, r) in (2..6).zip(c.active_ripples()) {
        near(r.center, mapped(&v, (0.3 + 0.05 * k as f32, 0.6)), 1e-9);
    }
    assert!(
        c.active_ripples()
            .windows(2)
            .all(|w| w[0].radius > w[1].radius)
    );
    // The press alone, or no effect: no rings.
    for click in [ClickEffect::Press, ClickEffect::None] {
        p.cursor.click = click;
        assert_eq!(
            cursor_at(&export_ctx(p.clone()), 1.1).unwrap().ripple_count,
            0
        );
    }
    // Nor where the take recorded no clicks (Wayland).
    lines[0] = Line::Header {
        v: 2,
        capture: Capture::Separate,
        clicks: ClickSource::None,
        w: 1440.0,
        h: 900.0,
    };
    let dir = tempfile::tempdir().unwrap();
    let none = export_ctx(cursor_project(dir.path(), &lines, style(red)));
    assert_eq!(cursor_at(&none, 1.1).unwrap().ripple_count, 0);
}

#[test]
fn motion_blur_points_along_the_movement_and_is_zero_at_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = take(glide((0.2, 0.5), (0.8, 0.5), 0.0, 2.0));
    lines.extend(rest(0.8, 0.5, 2.5, 6.0));
    // A flick across the screen in 20 ms.
    lines.extend(glide((0.8, 0.5), (0.1, 0.5), 6.5, 6.52));
    lines.extend(rest(0.1, 0.5, 7.0, 9.0));
    let raw = |blur: f64| {
        style(move |s| {
            s.smoothing = Smoothing::Off;
            s.motion_blur = blur;
        })
    };
    let ctx = export_ctx(cursor_project(dir.path(), &lines, raw(1.0)));
    let take = the_take(&ctx);
    let v = footage(&ctx, 1.0, Landscape);
    let c = cursor_at(&ctx, 1.0).unwrap();
    let (vx, vy) = take.with_drawn(Smoothing::Off, |path| path.velocity(&take.track, secs(1.0)));
    let over = SHUTTER / 30.0;
    let want = (
        f64::from(vx) * over * v.dest.w / v.crop.w,
        f64::from(vy) * over * v.dest.h / v.crop.h,
    );
    near(c.blur, want, 1e-12);
    assert!(c.blur.0 > 0.0 && c.blur.1 == 0.0, "along the glide: {c:?}");
    assert_eq!(cursor_at(&ctx, 4.0).unwrap().blur, (0.0, 0.0), "at rest");
    // The flick's smear stops at 4 cursor heights (measured in canvas heights), still pointing left.
    let c = cursor_at(&ctx, 6.51).unwrap();
    let length = (c.blur.0 * 1920.0 / 1080.0).hypot(c.blur.1);
    assert!(
        (length - 4.0 * c.height).abs() < 1e-9 && c.blur.0 < 0.0,
        "{c:?}"
    );
    // Half the setting, half the smear.
    let dir = tempfile::tempdir().unwrap();
    let half = export_ctx(cursor_project(dir.path(), &lines, raw(0.5)));
    near(
        cursor_at(&half, 1.0).unwrap().blur,
        (want.0 / 2.0, want.1 / 2.0),
        1e-12,
    );
}
