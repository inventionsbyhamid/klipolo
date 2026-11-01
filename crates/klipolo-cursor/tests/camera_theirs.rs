//! The person's own zooms inside an Auto zoom: each held exactly over its own span, the moves into and out of it and
//! the link between two of them theirs, and never reached early by the camera.

mod common;

use common::{check, click, cursor_through, in_span, input, on_screen, random_fixed, random_focus};
use klipolo_cursor::camera::*;

fn theirs(from: i64, to: i64, (cx, cy): (f64, f64), scale: f64) -> Fixed {
    Fixed {
        from,
        to,
        view: View { cx, cy, scale },
    }
}

#[test]
fn the_camera_never_reaches_the_persons_zoom_early() {
    // Trace (b): a shot that must zoom out after its hold, a click elsewhere at 8.4 s, then the person's zoom at 9.3 s.
    let fixed = [theirs(9_300_000, 11_300_000, (0.75, 0.75), 2.0)];
    let focus = [click(6_500_000, 0.7, 0.7), click(8_400_000, 0.1, 0.1)];
    let p = plan(&input(30_000_000, &focus, &fixed, Rules::default()));
    assert!(
        on_screen(&p, 8_400_000, 0.1, 0.1),
        "the click at 8.4 s: {:?}",
        p.view_at(8_400_000)
    );
    for t in [8_400_000, 9_000_000, 9_200_000] {
        assert_ne!(
            p.view_at(t),
            fixed[0].view,
            "their zoom before its start, at {t}"
        );
    }
    for t in (9_300_000..11_300_000).step_by(100_000) {
        assert_eq!(p.view_at(t), fixed[0].view, "their zoom at {t}");
    }
    check(&p, Rules::default(), 30_000_000, "trace (b)");
}

#[test]
fn the_move_between_two_of_the_persons_zooms_is_theirs() {
    // Trace (a): two of their zooms 1.5 s apart, and a click between them that no Auto zoom can frame.
    let fixed = [
        theirs(5_000_000, 8_000_000, (0.3, 0.3), 2.0),
        theirs(9_500_000, 12_000_000, (0.35, 0.35), 2.0),
    ];
    let p = plan(&input(
        30_000_000,
        &[click(9_000_000, 0.9, 0.9)],
        &fixed,
        Rules::default(),
    ));
    check(&p, Rules::default(), 30_000_000, "trace (a)");
}

#[test]
fn a_persons_zoom_with_no_scale_is_left_out() {
    // A zoom at scale 0, or with no number for its centre, never reaches the planner's arithmetic.
    let fixed = [
        theirs(5_000_000, 8_000_000, (0.5, 0.5), 0.0),
        theirs(12_000_000, 14_000_000, (f64::NAN, 0.5), 2.0),
    ];
    let p = plan(&input(
        30_000_000,
        &[click(10_000_000, 0.4, 0.4)],
        &fixed,
        Rules::default(),
    ));
    assert!(p.shots().iter().all(|s| s.weight != Weight::Person));
    check(&p, Rules::default(), 30_000_000, "zooms with no scale");
}

#[test]
fn the_property_holds_around_the_persons_own_zooms() {
    for seed in 0..10u64 {
        let (focus, fixed) = (
            random_focus(seed, 120_000_000),
            random_fixed(seed, 120_000_000),
        );
        assert!(!fixed.is_empty(), "seed {seed}: some zooms of their own");
        let p = plan(&input(120_000_000, &focus, &fixed, Rules::default()))
            .with_glide(&cursor_through(seed, &focus));
        let label = format!("seed {seed} with {} zooms of theirs", fixed.len());
        check(&p, Rules::default(), 120_000_000, &label);
        let all = p.events().len() as u32 + p.ladder().dropped;
        assert_eq!(
            all,
            in_span(&focus, 120_000_000),
            "{label}: each kept or counted"
        );
    }
}

#[test]
fn two_of_their_zooms_closer_than_their_move_pass_the_check() {
    // Their zooms 0.3 s apart at very different views (a squeezed move): the move between them
    // is as they placed it, and so is a 3× zoom starting 0.2 s into the span. Both pass, glided or not.
    let fixed = [
        theirs(200_000, 5_000_000, (0.2, 0.2), 3.0),
        theirs(5_300_000, 9_000_000, (0.8, 0.8), 3.0),
    ];
    let focus = [click(15_000_000, 0.5, 0.5)];
    let p = plan(&input(30_000_000, &focus, &fixed, Rules::default()));
    check(&p, Rules::default(), 30_000_000, "a squeezed pair");
    let glided = plan(&input(30_000_000, &focus, &fixed, Rules::default()))
        .with_glide(&cursor_through(3, &focus));
    check(
        &glided,
        Rules::default(),
        30_000_000,
        "a squeezed pair, glided",
    );
}

#[test]
fn their_zoom_at_the_whole_frame_is_not_an_out_and_ins_leg() {
    // A click's shot (held to 11.5 s) that must zoom out after its hold for a click it can't frame at 11.6 s,
    // then their 1× zoom at 13 s: the zoom-out isn't theirs, so the dropped click is on screen.
    let fixed = [theirs(13_000_000, 15_000_000, (0.5, 0.5), 1.0)];
    let focus = [click(10_000_000, 0.05, 0.05), click(11_600_000, 0.95, 0.95)];
    let p = plan(&input(30_000_000, &focus, &fixed, Rules::default()));
    assert!(
        on_screen(&p, 11_600_000, 0.95, 0.95),
        "(i) at 11.6 s: {:?}",
        p.view_at(11_600_000)
    );
    check(&p, Rules::default(), 30_000_000, "out, then their zoom");
    // Their 1× zoom, then a click's shot 4 s later; typing before the click loses to it, and the zoom in
    // must not hide it.
    let fixed = [theirs(5_000_000, 8_000_000, (0.5, 0.5), 1.0)];
    let typing = Focus {
        at: 11_000_000,
        until: 11_600_000,
        rect: klipolo_cursor::Rect01 {
            x: 0.1,
            y: 0.1,
            w: 0.02,
            h: 0.02,
        },
        weight: Weight::Typing,
    };
    let focus = [typing, click(12_000_000, 0.8, 0.8)];
    let p = plan(&input(30_000_000, &focus, &fixed, Rules::default()));
    for t in (11_000_000..=11_600_000).step_by(100_000) {
        assert!(
            on_screen(&p, t, 0.11, 0.11),
            "(ii) at {t}: {:?}",
            p.view_at(t)
        );
    }
    check(&p, Rules::default(), 30_000_000, "their zoom, then in");
}

#[test]
fn the_gap_between_two_of_their_zooms_at_one_view_is_theirs() {
    // Their 2× zooms at one view over 5–8 s and 9–12 s, and a click at 8.5 s that no shot can frame.
    let fixed = [
        theirs(5_000_000, 8_000_000, (0.3, 0.3), 2.0),
        theirs(9_000_000, 12_000_000, (0.3, 0.3), 2.0),
    ];
    let focus = [click(8_500_000, 0.9, 0.9)];
    let p = plan(&input(30_000_000, &focus, &fixed, Rules::default()));
    for t in (5_000_000..12_000_000).step_by(100_000) {
        assert_eq!(p.view_at(t), fixed[0].view, "theirs at {t}");
    }
    check(&p, Rules::default(), 30_000_000, "between their zooms");
}
