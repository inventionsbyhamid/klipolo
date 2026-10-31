//! Auto zoom on a take's clicks: the camera plans the take once with the spec's rules, and the plan is cut into
//! pieces where the view rests on the whole frame. Facts are built by hand on the timeline.

use klipolo_cursor::Us;
use klipolo_cursor::camera::{Plan, Rules};
use klipolo_cursor::timeline::{ClickAt, TimelineFacts};
use klipolo_cursor::zooms::{CLICKS, auto_plan, pieces};

const S: Us = 1_000_000;
const SPAN: (Us, Us) = (0, 20 * S);

/// Two clicks 0.8 s apart, close together on screen.
const NEAR: [(Us, f32, f32); 2] = [(2 * S, 0.30, 0.40), (2_800_000, 0.32, 0.42)];
/// The same two, then two more 8 s later across the screen.
const APART: [(Us, f32, f32); 4] = [
    NEAR[0],
    NEAR[1],
    (10 * S, 0.80, 0.70),
    (10_500_000, 0.82, 0.72),
];

fn clicks(at: &[(Us, f32, f32)]) -> TimelineFacts {
    TimelineFacts {
        clicks: (at.iter())
            .map(|&(timeline_us, x, y)| ClickAt { timeline_us, x, y })
            .collect(),
        ..Default::default()
    }
}

fn planned(at: &[(Us, f32, f32)], span: (Us, Us)) -> Plan {
    auto_plan(&clicks(at), CLICKS, span, Rules::default())
}

fn cut(p: &Plan) -> Vec<(Us, Us)> {
    pieces(p.keys(), Rules::default().out_gap_us)
}

#[test]
fn rules_default_are_the_spec_values() {
    let r = Rules::default();
    assert_eq!(r.pre_roll_us, 300_000);
    assert_eq!(r.merge_us, 1_500_000);
    assert_eq!(r.max_scale, 2.0);
    assert_eq!(r.safe, 0.15);
    assert_eq!(r.hold_us, 1_200_000);
    assert_eq!(r.out_gap_us, 2_500_000);
}

#[test]
fn clicks_under_1_5_s_apart_make_one_piece() {
    let pieces = cut(&planned(&NEAR, SPAN));
    assert_eq!(pieces.len(), 1, "{pieces:?}");
}

#[test]
fn clicks_8_s_apart_make_two_pieces() {
    let pieces = cut(&planned(&APART, SPAN));
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    assert!(pieces[1].0 > pieces[0].1, "{pieces:?}");
    assert!(pieces[0].0 < 2 * S && pieces[0].1 > 2_800_000, "{pieces:?}");
    assert!(
        pieces[1].0 < 10 * S && pieces[1].1 > 10_500_000,
        "{pieces:?}"
    );
}

#[test]
fn a_piece_arrives_0_3_s_before_its_first_click() {
    let p = planned(&NEAR, SPAN);
    let pieces = cut(&p);
    assert!(pieces[0].0 <= 1_700_000, "{pieces:?}");
    // Settled by then: the view the click sees.
    let at_click = p.view_at(2 * S);
    assert!(at_click.scale > 1.0, "{at_click:?}");
    assert_eq!(p.view_at(1_700_000), at_click);
}

#[test]
fn a_piece_holds_at_least_1_2_s_after_its_last_click() {
    let p = planned(&NEAR, SPAN);
    let pieces = cut(&p);
    assert!(pieces[0].1 >= 4 * S, "{pieces:?}");
    assert_eq!(p.view_at(4 * S), p.view_at(2_800_000), "still holding");
}

#[test]
fn every_click_sits_inside_the_view_with_a_15_percent_margin() {
    let p = planned(&APART, SPAN);
    for &(t, x, y) in &APART {
        let v = p.view_at(t);
        assert!(v.scale > 1.0, "zoomed at {t}: {v:?}");
        let r = v.rect();
        let (mx, my) = (0.15 * r.w, 0.15 * r.h);
        let eps = 1e-6;
        assert!(
            r.x + mx - eps <= x && x <= r.x + r.w - mx + eps,
            "x of the click at {t} in {r:?}"
        );
        assert!(
            r.y + my - eps <= y && y <= r.y + r.h - my + eps,
            "y of the click at {t} in {r:?}"
        );
    }
}

#[test]
fn the_view_never_zooms_past_2x() {
    let most = |p: &Plan| (p.keys().iter()).fold(1.0_f64, |m, k| m.max(k.view.scale));
    let p = planned(&APART, SPAN);
    assert!(most(&p) > 1.0 && most(&p) <= 2.0 + 1e-9, "{}", most(&p));

    // A click every 0.5 s from 1 s to 31 s, wandering across the screen.
    let busy: Vec<(Us, f32, f32)> = (0..=60)
        .map(|i| {
            let x = 0.25 + 0.5 * i as f32 / 60.0;
            let y = if i % 2 == 0 { 0.40 } else { 0.46 };
            (S + i * S / 2, x, y)
        })
        .collect();
    let p = planned(&busy, (0, 40 * S));
    assert!(most(&p) > 1.0 && most(&p) <= 2.0 + 1e-9, "{}", most(&p));
    assert_eq!(cut(&p).len(), 1, "{:?}", cut(&p));
}

#[test]
fn a_click_right_at_the_start_still_zooms() {
    let pieces = cut(&planned(&[(100_000, 0.30, 0.40)], SPAN));
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    assert!((0..=100_000).contains(&pieces[0].0), "{pieces:?}");
}

#[test]
fn a_click_on_the_last_frame_adds_nothing_past_the_end() {
    // The camera holds a shot through its click, then needs a whole move (at least 0.35 s) to reach the whole frame
    // by the end: a click 10 ms before the end can't have both, so it is dropped, never zoomed past the end.
    let p = planned(&[(4_990_000, 0.30, 0.40)], (0, 5 * S));
    assert_eq!(cut(&p), vec![]);
    assert_eq!(p.ladder().dropped, 1);
}

#[test]
fn a_click_0_4_s_before_the_end_still_zooms() {
    let p = planned(&[(4_600_000, 0.30, 0.40)], (0, 5 * S));
    let pieces = cut(&p);
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    assert!(pieces[0].1 <= 5 * S, "{pieces:?}");
    assert!(
        p.view_at(4_600_000).scale > 1.0,
        "{:?}",
        p.view_at(4_600_000)
    );
}
