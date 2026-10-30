//! Where the pass draws and how each look comes out: placed at its hotspot and clipped to the footage (its box, its
//! rounded corners, its shape mask), two slots in one frame, Dot and Arrow sharp at any size, the smear, the rings.

use klipolo_engine::plan::{CursorDraw, CursorMask, Ripple};
use klipolo_model::layout::BoxN;

use crate::{Frame, H, W, device, layer, pass, preload_list, render, take_with};

/// Pixels in `xs` along row `y` whose alpha is part-way (more than 25 %, less than 90 %): an edge's ramp.
fn ramp(f: &Frame, y: u32, xs: std::ops::Range<u32>) -> usize {
    xs.filter(|&x| (64..230).contains(&f.alpha(x, y))).count()
}

#[test]
#[ignore = "needs a GPU"]
fn a_cursor_draws_at_its_hotspot_and_nowhere_outside_the_footage() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let (_, looks) = take_with(dir.path(), 1, &[(8, 8, [255, 0, 0, 255])]);
    let mut p = pass(&dev);
    p.preload(&dev.device, &dev.queue, &preload_list(&looks))
        .unwrap();
    // 16 px high at (128, 72); the footage's box ends at x = 136, through the shape.
    let mut c = layer(looks[0].clone(), (0.5, 0.5), 16.0 / f64::from(H));
    c.clip = BoxN {
        x: 0.25,
        y: 0.25,
        w: 136.0 / f64::from(W) - 0.25,
        h: 0.5,
    };
    let f = render(&dev, &mut p, &[c], true);
    assert_eq!(
        f.at(131, 75),
        [255, 0, 0, 255],
        "the shape, from its hotspot"
    );
    assert_eq!(f.alpha(140, 75), 0, "the shape past the box");
    for y in 0..H {
        for x in 0..W {
            let inside_box = (64..136).contains(&x) && (36..108).contains(&y);
            let near_hotspot = (126..136).contains(&x) && (70..93).contains(&y);
            if !(inside_box && near_hotspot) {
                assert_eq!(f.alpha(x, y), 0, "({x}, {y})");
            }
        }
    }
}

#[test]
#[ignore = "needs a GPU"]
fn two_cursor_layers_in_a_transition_both_draw() {
    let dev = device();
    let mut p = pass(&dev);
    let arrow = layer(CursorDraw::Arrow, (0.25, 0.3), 0.4);
    let dot = layer(CursorDraw::Dot, (0.75, 0.5), 0.4);
    let alone = (
        render(&dev, &mut p, std::slice::from_ref(&arrow), true),
        render(&dev, &mut p, std::slice::from_ref(&dot), true),
    );
    let both = render(&dev, &mut p, &[arrow.clone(), dot.clone()], true);
    for y in 0..H {
        for x in 0..W {
            let own = if x < W / 2 { &alone.0 } else { &alone.1 };
            assert_eq!(both.at(x, y), own.at(x, y), "({x}, {y})");
        }
    }
    assert!(both.alpha(70, 60) > 200 && both.alpha(192, 72) > 200);
    // A third layer in one frame (no plan makes one) has no slot.
    p.begin_frame();
    assert!(p.prepare(&arrow, (W, H), true).is_some());
    assert!(p.prepare(&dot, (W, H), true).is_some());
    assert!(p.prepare(&dot, (W, H), true).is_none());
}

#[test]
#[ignore = "needs a GPU"]
fn the_dot_and_arrow_stay_sharp_at_four_times_size() {
    let dev = device();
    let mut p = pass(&dev);
    for height in [0.125, 0.5] {
        let px = height * f64::from(H);
        // The arrow's left edge is upright from its tip: half-way down it, the ramp from outside to inside.
        let f = render(
            &dev,
            &mut p,
            &[layer(CursorDraw::Arrow, (0.3, 0.1), height)],
            true,
        );
        let (tip, y) = (
            (0.3 * f64::from(W)) as u32,
            (0.1 * f64::from(H) + px / 2.0) as u32,
        );
        assert!(
            f.alpha(tip + 2, y) > 240 && f.alpha(tip - 3, y) < 64,
            "{height}"
        );
        assert!(ramp(&f, y, tip - 6..tip + 6) <= 1, "arrow at {px} px");
        // The dot's left edge on its middle row.
        let f = render(
            &dev,
            &mut p,
            &[layer(CursorDraw::Dot, (0.5, 0.5), height)],
            true,
        );
        let r = (px * 0.6 / 2.0) as u32;
        assert!(f.alpha(128, 72) > 240, "{height}");
        assert!(
            ramp(&f, 72, 128 - r - 6..128 - r + 6) <= 1,
            "dot at {px} px"
        );
    }
}

#[test]
#[ignore = "needs a GPU"]
fn motion_blur_spreads_alpha_along_the_vector() {
    let dev = device();
    let mut p = pass(&dev);
    let still = layer(CursorDraw::Dot, (0.5, 0.5), 0.25);
    let mut moving = still.clone();
    moving.blur = (40.0 / f64::from(W), 0.0);
    let (a, b) = (
        render(&dev, &mut p, &[still], true),
        render(&dev, &mut p, &[moving], true),
    );
    let (across, down) = (
        (a.extent(72, false, 25), b.extent(72, false, 25)),
        (a.extent(128, true, 25), b.extent(128, true, 25)),
    );
    assert!(
        across.0 < 26 && across.1 >= 50,
        "along the smear: {across:?}"
    );
    assert!(down.0.abs_diff(down.1) <= 4, "not across it: {down:?}");
    assert!(
        b.alpha(128, 72) < 230,
        "spread thinner: {}",
        b.alpha(128, 72)
    );
}

#[test]
#[ignore = "needs a GPU"]
fn ripples_draw_rings_that_fade() {
    let dev = device();
    let mut p = pass(&dev);
    // The cursor itself hidden (it faded out at rest): only the rings draw.
    let mut c = layer(CursorDraw::Arrow, (0.5, 0.5), 0.1);
    c.opacity = 0.0;
    let ring = |radius: f64, alpha: f64| Ripple {
        center: (0.5, 0.5),
        radius,
        alpha,
        color: [255, 0, 0, 255],
    };
    (c.ripples[0], c.ripples[1], c.ripple_count) = (ring(0.2, 0.8), ring(0.3, 0.3), 2);
    let f = render(&dev, &mut p, &[c], true);
    let near = |v: u8, want: f64| (f64::from(v) - want * 255.0).abs() <= 3.0;
    let young = f.at(128 + 29, 72);
    assert!(
        near(young[3], 0.8) && near(young[0], 0.8) && young[1] == 0,
        "{young:?}"
    );
    let old = f.at(128 + 43, 72);
    assert!(near(old[3], 0.3), "older, fainter: {old:?}");
    assert_eq!(f.alpha(128, 72), 0, "a ring, not a disc");
    assert_eq!(f.alpha(128 + 14, 72), 0);
    assert_eq!(f.alpha(128, 72 - 36), 0, "between the rings");
}

#[test]
#[ignore = "needs a GPU"]
fn a_shape_mask_and_rounded_corners_clip_the_cursor() {
    let dev = device();
    let mut p = pass(&dev);
    let dot = |x: f64, y: f64| layer(CursorDraw::Dot, (x / f64::from(W), y / f64::from(H)), 0.2);
    let circle = CursorMask {
        shape: 0,
        center: (0.5, 0.5),
        corner: 0.0,
        feather: 0.0,
    };
    // The footage masked to a circle 144 px across in the middle: a cursor inside it shows, one past it doesn't.
    let (mut inside, mut outside) = (dot(128.0, 72.0), dot(220.0, 72.0));
    inside.mask = Some(circle);
    outside.mask = Some(circle);
    let f = render(&dev, &mut p, &[inside, outside], true);
    assert!(f.alpha(128, 72) > 240);
    assert!((200..240).all(|x| (60..85).all(|y| f.alpha(x, y) == 0)));
    // Corners rounded at a quarter of the shorter side (36 px): the corner cuts it, the edge's middle doesn't.
    let (mut corner, mut edge) = (dot(10.0, 10.0), dot(128.0, 8.0));
    corner.radius = 0.25;
    edge.radius = 0.25;
    let f = render(&dev, &mut p, &[corner, edge], true);
    assert!(
        (0..8).all(|x| (0..8).all(|y| f.alpha(x, y) == 0)),
        "cut by the corner"
    );
    assert!(f.alpha(14, 14) > 240 && f.alpha(128, 8) > 240);
}
