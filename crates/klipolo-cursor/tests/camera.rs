//! The smart camera: planned ahead, settled before every kept
//! event with it inside the safe zone, every move within its limits, conflicts dropped or merged and never rushed.

mod common;

use common::{
    check, click, cursor_through, in_span, input, random_focus, random_focus_counts, walk_to_edge,
};
use klipolo_cursor::Rect01;
use klipolo_cursor::camera::*;

fn inside_safe(v: View, x: f64, y: f64, safe: f64) -> bool {
    let r = v.rect();
    let m = r.w as f64 * safe;
    x >= r.x as f64 + m
        && x <= (r.x + r.w) as f64 - m
        && y >= r.y as f64 + m
        && y <= (r.y + r.h) as f64 - m
}

#[test]
fn the_camera_arrives_before_the_click() {
    let focus = [click(10_000_000, 0.8, 0.2)];
    let p = plan(&input(30_000_000, &focus, &[], Rules::default()));
    assert!(p.view_at(10_000_000).scale > 1.5);
    check(&p, Rules::default(), 30_000_000, "one click");
}

#[test]
fn clicks_close_together_share_one_shot_and_far_ones_zoom_out_between() {
    let focus = [
        click(5_000_000, 0.3, 0.3),
        click(5_600_000, 0.35, 0.32),
        click(20_000_000, 0.9, 0.9),
    ];
    let p = plan(&input(40_000_000, &focus, &[], Rules::default()));
    assert_eq!(p.shots().len(), 2);
    assert_eq!(
        p.view_at(12_000_000),
        View::FULL,
        "zoomed out between actions 14 s apart"
    );
    check(&p, Rules::default(), 40_000_000, "two shots");
}

#[test]
fn every_kept_event_is_settled_before_and_safe_and_every_move_is_legal() {
    for seed in 0..50u64 {
        let focus = random_focus(seed, 120_000_000);
        let p = plan(&input(120_000_000, &focus, &[], Rules::default()));
        check(&p, Rules::default(), 120_000_000, &format!("seed {seed}"));
        let all = p.events().len() as u32 + p.ladder().dropped;
        assert_eq!(
            all,
            in_span(&focus, 120_000_000),
            "seed {seed}: each kept or counted"
        );
        // A dropped shot's events must show, but it mustn't take every neighbour with it (the repair in `seen`).
        assert!(!p.shots().is_empty(), "seed {seed}: the camera still zooms");
    }
}

#[test]
#[ignore = "the wide sweep"]
fn every_kept_event_is_settled_before_and_safe_and_every_move_is_legal_wide() {
    for seed in 50..250u64 {
        let focus = random_focus(seed, 120_000_000);
        let p = plan(&input(120_000_000, &focus, &[], Rules::default()));
        check(&p, Rules::default(), 120_000_000, &format!("seed {seed}"));
    }
}

#[test]
fn a_three_times_zoom_widens_until_its_moves_are_legal() {
    // 3× from the whole frame needs 1.7 s at 1.2 ln(scale) a second: longer than a move may take, so it is widened.
    let rules = Rules {
        max_scale: 3.0,
        ..Rules::default()
    };
    let focus = [click(10_000_000, 0.5, 0.5)];
    let p = plan(&input(30_000_000, &focus, &[], rules));
    let s = p.view_at(10_000_000).scale;
    assert!(s > 2.4 && s < 3.0, "widened to {s}");
    check(&p, rules, 30_000_000, "3×");
    for seed in 0..20u64 {
        let focus = random_focus(seed, 120_000_000);
        let p = plan(&input(120_000_000, &focus, &[], rules));
        check(&p, rules, 120_000_000, &format!("3× seed {seed}"));
    }
}

#[test]
fn a_dense_tutorial_widens_or_merges_rather_than_rushing() {
    // Clicks every 3 s alternating across the screen for 2 minutes.
    let focus: Vec<Focus> = (0..40)
        .map(|i| {
            click(
                2_000_000 + i * 3_000_000,
                if i % 2 == 0 { 0.15 } else { 0.85 },
                0.5,
            )
        })
        .collect();
    let p = plan(&input(125_000_000, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 125_000_000, "dense");
    let l = p.ladder();
    eprintln!("dense tutorial: {} shots, {l:?}", p.shots().len()); // reported in the task report
    assert_eq!(p.events().len() as u32 + l.dropped, 40);
}

#[test]
fn a_click_beats_a_dwell_and_the_persons_own_zoom_is_never_moved() {
    // The dwell starts before the camera starts moving for the click (a dropped action stays in view;
    // at 9.8 s, as first written, no zoom on the click could also show the dwell, so the click would lose its zoom).
    let dwell = Focus {
        at: 8_000_000,
        until: 11_000_000,
        rect: Rect01 {
            x: 0.1,
            y: 0.8,
            w: 0.0,
            h: 0.0,
        },
        weight: Weight::Dwell,
    };
    let fixed = [Fixed {
        from: 30_000_000,
        to: 34_000_000,
        view: View {
            cx: 0.5,
            cy: 0.5,
            scale: 3.0,
        },
    }];
    let focus = [
        dwell,
        click(10_000_000, 0.9, 0.1),
        click(31_000_000, 0.1, 0.1),
    ];
    let p = plan(&input(40_000_000, &focus, &fixed, Rules::default()));
    assert!(inside_safe(p.view_at(10_000_000), 0.9, 0.1, 0.15));
    for t in (30_000_000..34_000_000).step_by(100_000) {
        assert_eq!(p.view_at(t), fixed[0].view);
    }
    assert_eq!(
        p.view_at(37_000_000),
        View::FULL,
        "the person's zoom ends where they ended it"
    );
}

#[test]
fn a_click_at_the_frames_edge_is_kept_and_framed_against_the_edge() {
    // A menu-bar click: no view up to 3× holds it 15 % inside its own edge, but the frame's edge is the view's there.
    let focus = [click(10_000_000, 0.02, 0.005)];
    let p = plan(&input(30_000_000, &focus, &[], Rules::default()));
    assert_eq!(p.events().len(), 1);
    let r = p.view_at(10_000_000).rect();
    assert!(p.view_at(10_000_000).scale > 1.5);
    assert_eq!((r.x, r.y), (0.0, 0.0), "framed against the top left");
    check(&p, Rules::default(), 30_000_000, "edge");
}

#[test]
fn typing_too_wide_for_the_widest_view_gets_no_zoom() {
    let typed = |w: f32| Focus {
        at: 10_000_000,
        until: 14_000_000,
        rect: Rect01 {
            x: 0.1,
            y: 0.4,
            w,
            h: 0.1,
        },
        weight: Weight::Typing,
    };
    let wide = [typed(0.7)];
    let p = plan(&input(30_000_000, &wide, &[], Rules::default()));
    assert!(p.shots().is_empty());
    assert_eq!(p.ladder().dropped, 1);
    assert_eq!(p.view_at(12_000_000), View::FULL);
    let line = [typed(0.3)];
    let p = plan(&input(30_000_000, &line, &[], Rules::default()));
    assert_eq!(p.shots().len(), 1);
    assert!(p.view_at(12_000_000).scale > 1.5);
    check(
        &p,
        Rules::default(),
        30_000_000,
        "a typed line, framed whole",
    );
}

#[test]
fn glide_moves_only_when_the_cursor_leaves_the_safe_zone() {
    let focus = [click(1_000_000, 0.5, 0.5)];
    let base = plan(&input(10_000_000, &focus, &[], Rules::default()));
    let still =
        plan(&input(10_000_000, &focus, &[], Rules::default())).with_glide(&|_| Some((0.5, 0.5)));
    for t in (0..10_000_000).step_by(50_000) {
        assert_eq!(
            still.view_at(t),
            base.view_at(t),
            "a still cursor: no glide"
        );
    }
    let walking = plan(&input(10_000_000, &focus, &[], Rules::default()))
        .with_glide(&walk_to_edge(1_500_000, 2.5)); // walks from the middle to x = 0.95 over 1.5–4 s
    let late = walking.view_at(4_500_000);
    assert!(late.cx > base.view_at(4_500_000).cx, "followed rightwards");
    assert!(late.rect().x + late.rect().w <= 1.0, "never past the frame");
    check(&walking, Rules::default(), 10_000_000, "glide");
}

#[test]
fn every_kept_event_still_holds_with_the_glide_following_a_wandering_cursor() {
    for seed in 0..30u64 {
        let focus = random_focus(seed, 120_000_000);
        let p = plan(&input(120_000_000, &focus, &[], Rules::default()));
        let glided = plan(&input(120_000_000, &focus, &[], Rules::default()))
            .with_glide(&cursor_through(seed, &focus));
        let moved = (0..120_000_000)
            .step_by(100_000)
            .any(|t| glided.view_at(t) != p.view_at(t));
        assert!(
            moved,
            "seed {seed}: the glide followed the cursor somewhere"
        );
        check(
            &glided,
            Rules::default(),
            120_000_000,
            &format!("glide seed {seed}"),
        );
    }
}

#[test]
fn the_same_input_plans_the_same_bits() {
    // Glided, every field, at a step no frame rate uses, and read backwards as a seek would.
    let focus = random_focus(5, 60_000_000);
    let made = || {
        plan(&input(60_000_000, &focus, &[], Rules::default()))
            .with_glide(&cursor_through(5, &focus))
    };
    let (a, b) = (made(), made());
    let bits = |v: View| [v.cx.to_bits(), v.cy.to_bits(), v.scale.to_bits()];
    let times: Vec<i64> = (0..60_000_000).step_by(37_441).collect();
    for &t in times.iter().rev() {
        assert_eq!(bits(a.view_at(t)), bits(b.view_at(t)), "at {t}");
    }
    let forward: Vec<_> = times.iter().map(|&t| bits(a.view_at(t))).collect();
    let backward: Vec<_> = times.iter().rev().map(|&t| bits(a.view_at(t))).collect();
    assert!(
        forward.iter().eq(backward.iter().rev()),
        "the order of reading changes nothing"
    );
}

#[test]
#[ignore = "real scale: a 2-hour take"]
fn a_two_hour_take_plans_in_well_under_a_second() {
    let focus = random_focus_counts(9, 7_200_000_000, 10_000, 2_000, 5_000);
    let start = std::time::Instant::now();
    let p = plan(&input(7_200_000_000, &focus, &[], Rules::default()));
    let took = start.elapsed();
    eprintln!(
        "{} shots in {took:?}, {} bytes, {:?}",
        p.shots().len(),
        p.bytes(),
        p.ladder()
    );
    assert!(took.as_millis() < if cfg!(debug_assertions) { 1_000 } else { 50 });
    // The repair's rounds, and the round cap forced: right away, and after every round but the last (the most work a
    // plan can do before it keeps the whole frame), both inside the same budget.
    let (_, rounds) = plan_capped(
        &input(7_200_000_000, &focus, &[], Rules::default()),
        MAX_ROUNDS,
    );
    for cap in [0, rounds.saturating_sub(1)] {
        let start = std::time::Instant::now();
        let (capped, ran) = plan_capped(&input(7_200_000_000, &focus, &[], Rules::default()), cap);
        let took = start.elapsed();
        eprintln!(
            "{rounds} rounds uncapped; capped at {cap}: {ran} rounds, {} shots, in {took:?}",
            capped.shots().len()
        );
        assert!(took.as_millis() < if cfg!(debug_assertions) { 1_000 } else { 50 });
    }
    // The glide over every shot at 60 Hz, reading a raw cursor by binary search as the engine's does.
    let cursor = cursor_through(9, &focus);
    let start = std::time::Instant::now();
    let p = p.with_glide(&cursor);
    let took = start.elapsed();
    eprintln!("glided in {took:?}, {} bytes", p.bytes());
    assert!(took.as_millis() < if cfg!(debug_assertions) { 3_000 } else { 100 });
}
