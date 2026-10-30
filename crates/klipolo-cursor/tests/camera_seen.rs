//! No action happens off screen: an action whose shot was dropped
//! still lies inside the view at its time. With a strength under the widest shot, the repair's round cap
//! and inputs with no number or no end.

mod common;

use common::{check, click, input, on_screen};
use klipolo_cursor::camera::*;

#[test]
fn a_far_click_a_second_after_another_is_never_off_screen() {
    // The review's trace: the second click can't share a view with the first and comes too soon to move to, so its
    // shot is dropped; the camera must still show it, and the third click still gets its zoom.
    let clicks = [
        (10_000_000, 0.2, 0.3),
        (11_000_000, 0.8, 0.7),
        (20_000_000, 0.5, 0.5),
    ];
    let focus: Vec<Focus> = clicks.iter().map(|&(at, x, y)| click(at, x, y)).collect();
    let p = plan(&input(30_000_000, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 30_000_000, "the trace");
    for (at, x, y) in clicks {
        assert!(
            on_screen(&p, at, x, y),
            "the click at {at} off screen: {:?}",
            p.view_at(at)
        );
    }
    assert!(
        p.view_at(20_000_000).scale > 1.5,
        "the later click still gets its zoom"
    );
    assert_eq!(p.events().len() as u32 + p.ladder().dropped, 3);
}

#[test]
fn a_dense_tutorial_shows_every_click_at_every_spacing() {
    // Clicks alternating across the screen, from 4 s down to 1 s apart, for 2 minutes. The
    // lines printed are the report's table.
    let rules = |max_scale| Rules {
        max_scale,
        ..Rules::default()
    };
    for gap in [
        4_000_000, 3_000_000, 2_500_000, 2_000_000, 1_500_000, 1_000_000,
    ] {
        for max in [2.0, 3.0] {
            let n = 120_000_000 / gap;
            let side = |i: i64| if i % 2 == 0 { 0.15 } else { 0.85 };
            let focus: Vec<Focus> = (0..n)
                .map(|i| click(2_000_000 + i * gap, side(i), 0.5))
                .collect();
            let span = 2_000_000 + n * gap + 3_000_000;
            let p = plan(&input(span, &focus, &[], rules(max)));
            check(&p, rules(max), span, &format!("{max}× every {gap} µs"));
            for f in &focus {
                assert!(
                    on_screen(&p, f.at, f.rect.x, f.rect.y),
                    "{max}× every {gap} µs: {f:?} off screen"
                );
            }
            let scales: Vec<String> = p
                .shots()
                .iter()
                .map(|s| format!("{:.2}", s.view.scale))
                .collect();
            let (l, mut seen) = (p.ladder(), scales.clone());
            seen.sort();
            seen.dedup();
            eprintln!(
                "{max}× every {gap} µs: {n} clicks, {} shots {seen:?}, {l:?}",
                scales.len()
            );
            assert_eq!(p.events().len() as u32 + l.dropped, n as u32);
        }
    }
}

#[test]
fn a_strength_under_the_widest_shot_still_zooms() {
    // The model allows a zoom of 1.0–5.0; an Auto zoom asking for 1.1× zooms at 1.25×, the smallest zoom a shot
    // takes, rather than silently never zooming.
    let rules = Rules {
        max_scale: 1.1,
        ..Rules::default()
    };
    let p = plan(&input(
        30_000_000,
        &[click(10_000_000, 0.5, 0.5)],
        &[],
        rules,
    ));
    assert_eq!(p.view_at(10_000_000).scale, rules.min_scale);
    check(&p, rules, 30_000_000, "1.1×");
}

#[test]
fn a_dropped_typing_span_stays_on_screen_while_it_lasts() {
    // The failing trace: typing too wide to frame over 10–20 s, and a click elsewhere at 13 s. A typing span
    // is checked while it lasts (a pause only at its start).
    let rect = klipolo_cursor::Rect01 {
        x: 0.10,
        y: 0.15,
        w: 0.70,
        h: 0.05,
    };
    let typing = Focus {
        at: 10_000_000,
        until: 20_000_000,
        rect,
        weight: Weight::Typing,
    };
    let focus = [typing, click(13_000_000, 0.85, 0.85)];
    let p = plan(&input(30_000_000, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 30_000_000, "the m1 trace");
    for t in (10_000_000..=20_000_000).step_by(100_000) {
        assert!(
            on_screen(&p, t, 0.45, 0.175),
            "the typing off screen at {t}: {:?}",
            p.view_at(t)
        );
    }
    assert!(on_screen(&p, 13_000_000, 0.85, 0.85));
}

#[test]
fn a_focus_with_no_number_for_a_place_never_panics() {
    // `plan` is public (chat builds its input): a NaN coordinate reads as the frame's centre.
    let rect = klipolo_cursor::Rect01 {
        x: f32::NAN,
        y: 0.3,
        w: f32::NAN,
        h: 0.0,
    };
    let odd = Focus {
        at: 10_000_000,
        until: 10_000_000,
        rect,
        weight: Weight::Click,
    };
    let focus = [odd, click(20_000_000, 0.7, 0.7)];
    let p = plan(&input(30_000_000, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 30_000_000, "a NaN focus");
}

#[test]
fn the_round_cap_falls_back_to_the_whole_frame() {
    // The repair stops after a set number of rounds. At the cap every Auto shot goes, so every
    // action is on screen; the person's zooms stay. Cap 0 forces it on the trace that needs one round.
    let clicks = [
        (10_000_000, 0.2, 0.3),
        (11_000_000, 0.8, 0.7),
        (20_000_000, 0.5, 0.5),
    ];
    let focus: Vec<Focus> = clicks.iter().map(|&(at, x, y)| click(at, x, y)).collect();
    let theirs = [Fixed {
        from: 24_000_000,
        to: 26_000_000,
        view: View {
            cx: 0.6,
            cy: 0.6,
            scale: 2.0,
        },
    }];
    let (free, rounds) = plan_capped(
        &input(30_000_000, &focus, &theirs, Rules::default()),
        MAX_ROUNDS,
    );
    assert!(
        rounds >= 1 && free.shots().len() >= 2,
        "uncapped, it repairs and keeps zooms"
    );
    let made = || plan_capped(&input(30_000_000, &focus, &theirs, Rules::default()), 0).0;
    let (p, again) = (made(), made());
    check(&p, Rules::default(), 30_000_000, "capped at 0");
    assert_eq!(p.shots().len(), 1, "only the person's zoom is left");
    for (at, x, y) in clicks {
        assert_eq!(p.view_at(at), View::FULL, "the whole frame at {at}");
        assert!(on_screen(&p, at, x, y));
    }
    assert_eq!(
        p.view_at(25_000_000),
        theirs[0].view,
        "the person's zoom stays"
    );
    let bits = |v: View| [v.cx.to_bits(), v.cy.to_bits(), v.scale.to_bits()];
    for t in (0..30_000_000).step_by(37_441) {
        assert_eq!(
            bits(p.view_at(t)),
            bits(again.view_at(t)),
            "deterministic at the cap, {t}"
        );
    }
}

#[test]
fn a_typing_span_to_the_end_of_time_never_hangs() {
    // A span ending near Us::MAX (the crate's open end) walks with saturating steps and
    // stops; release builds must never loop.
    let rect = klipolo_cursor::Rect01 {
        x: 0.05,
        y: 0.4,
        w: 0.9,
        h: 0.1,
    };
    let typing = Focus {
        at: 10_000_000,
        until: i64::MAX - 50_000,
        rect,
        weight: Weight::Typing,
    };
    let focus = [typing, click(15_000_000, 0.9, 0.9)];
    let p = plan(&input(30_000_000, &focus, &[], Rules::default()));
    check(
        &p,
        Rules::default(),
        30_000_000,
        "a span to the end of time",
    );
    assert!(
        on_screen(&p, 20_000_000, 0.5, 0.45),
        "the typing at 20 s: {:?}",
        p.view_at(20_000_000)
    );
}
