//! The span's ends are the person's, as their own zooms are. The end cuts the last shot's hold: an Auto zoom a person
//! ends shortly after the action, while the cursor still rests or the typing goes on, still zooms, holding until the
//! move out that reaches the whole frame by the end, never before its last event. The start lets the first shot arrive
//! late: a zoom whose click comes right at its start arrives as soon as it legally can.

mod common;

use common::{check, click, input, on_screen};
use klipolo_cursor::Rect01;
use klipolo_cursor::camera::*;

const S: i64 = 1_000_000;

fn pause(at: i64, until: i64, x: f32, y: f32) -> Focus {
    Focus {
        at,
        until,
        rect: Rect01 {
            x,
            y,
            w: 0.0,
            h: 0.0,
        },
        weight: Weight::Dwell,
    }
}

#[test]
fn a_three_second_auto_zoom_over_a_click_and_a_long_rest_still_zooms() {
    // `zoomOn region: auto` makes 3 s; the cursor rests after the click, well past the zoom's end.
    let focus = [click(S, 0.3, 0.3), pause(1_200_000, 9 * S, 0.3, 0.3)];
    let p = plan(&input(3 * S, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 3 * S, "a click and a long rest");
    assert!(
        p.events().iter().any(|e| e.0 == S),
        "the click is kept: {:?}",
        p.events()
    );
    // Its way in from the whole frame fits the 0.7 s before its pre-roll at a wider view.
    assert!(
        p.view_at(S).scale > 1.2,
        "zoomed at the click: {:?}",
        p.view_at(S)
    );
    assert!(on_screen(&p, S, 0.3, 0.3));
    assert_eq!(p.view_at(3 * S), View::FULL, "the whole frame at the end");
}

#[test]
fn earlier_clicks_merged_into_the_rest_keep_their_zoom() {
    let focus = [
        click(2 * S, 0.3, 0.3),
        click(3 * S, 0.32, 0.3),
        pause(3_200_000, 20 * S, 0.32, 0.3),
    ];
    let p = plan(&input(8 * S, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 8 * S, "clicks merged into the rest");
    for at in [2 * S, 3 * S] {
        assert!(p.events().iter().any(|e| e.0 == at), "the click at {at}");
        assert!(
            p.view_at(at).scale > 1.5,
            "zoomed at {at}: {:?}",
            p.view_at(at)
        );
    }
}

#[test]
fn typing_past_the_end_keeps_its_zoom_until_the_move_out() {
    let typing = Focus {
        at: 4 * S,
        until: 15 * S,
        rect: Rect01 {
            x: 0.55,
            y: 0.4,
            w: 0.05,
            h: 0.02,
        },
        weight: Weight::Typing,
    };
    let p = plan(&input(10 * S, &[typing], &[], Rules::default()));
    check(&p, Rules::default(), 10 * S, "typing past the end");
    let keys = p.keys();
    let out = keys.last().map(|k| k.at).unwrap();
    assert_eq!(out, 10 * S, "back to the whole frame at the end");
    let leaves = keys[keys.len() - 2].at;
    assert!(
        (4 * S + 1..10 * S).contains(&leaves),
        "the move out at {leaves}"
    );
    let zoomed = p.view_at(4 * S);
    assert!(zoomed.scale > 1.5, "{zoomed:?}");
    for t in (4 * S..=leaves).step_by(100_000) {
        assert_eq!(p.view_at(t), zoomed, "held at {t}");
    }
}

#[test]
fn a_three_second_auto_zoom_with_its_click_right_at_the_start_still_zooms_on_it() {
    // No move from the whole frame fits the 0.2 s before the click's pre-roll: the camera arrives late, as soon as the
    // move allows, and holds the click in its safe zone from then.
    let focus = [click(200_000, 0.3, 0.3)];
    let p = plan(&input(3 * S, &focus, &[], Rules::default()));
    check(&p, Rules::default(), 3 * S, "a click at the start");
    assert!(
        p.events().iter().any(|e| e.0 == 200_000),
        "the click is kept: {:?}",
        p.events()
    );
    let keys = p.keys();
    assert_eq!(
        (keys[0].at, keys[0].view),
        (0, View::FULL),
        "it moves in from the start"
    );
    let arrived = keys[1].at;
    assert!(arrived > 200_000, "late: at {arrived}");
    let v = p.view_at(arrived);
    assert!(v.scale > 1.5 && on_screen(&p, arrived, 0.3, 0.3), "{v:?}");
    let glided =
        plan(&input(3 * S, &focus, &[], Rules::default())).with_glide(&|_| Some((0.3, 0.3)));
    check(
        &glided,
        Rules::default(),
        3 * S,
        "a click at the start, glided",
    );
}

#[test]
fn short_auto_zooms_with_a_click_at_the_start_never_snap() {
    // Shorter than the move in plus the move out, the late arrival and the end's cut met, and
    // the move out started before the move in ended (keys out of time order: a snap). Now the end link widens or drops
    // the shot: a zoom or the whole frame, never a jump.
    for span in [500_000, 1_500_000, 2 * S] {
        for at in [100_000, 200_000] {
            let focus = [click(at, 0.3, 0.3)];
            let label = format!("a {span} µs zoom, the click at {at}");
            let p = plan(&input(span, &focus, &[], Rules::default()));
            let keys = p.keys();
            assert!(
                keys.windows(2).all(|k| k[0].at <= k[1].at),
                "{label}: {keys:?}"
            );
            check(&p, Rules::default(), span, &label);
            let glided =
                plan(&input(span, &focus, &[], Rules::default())).with_glide(&|_| Some((0.3, 0.3)));
            check(&glided, Rules::default(), span, &format!("{label}, glided"));
        }
    }
}

#[test]
fn a_short_zoom_with_its_click_at_the_start_still_zooms_at_the_widest_view_that_fits() {
    // Each wider view arrives sooner, so the end link tries each with its own arrival
    // before dropping the shot. At 2×, a 1 s zoom with its click at 0.1 s zooms 1.30× (it was the whole frame), and a
    // 1.5 s one 1.55× (it was 1.25×).
    for (span, want) in [(S, 1.30), (1_500_000, 1.55)] {
        let focus = [click(100_000, 0.3, 0.3)];
        let label = format!("a {span} µs zoom, the click at 0.1 s");
        let p = plan(&input(span, &focus, &[], Rules::default()));
        let keys = p.keys();
        assert!(
            keys.windows(2).all(|k| k[0].at <= k[1].at),
            "{label}: {keys:?}"
        );
        assert_eq!(p.events().len(), 1, "{label}: the click is kept");
        let scale = p.shots()[0].view.scale;
        assert!((scale - want).abs() < 1e-6, "{label}: {scale}×");
        assert!(on_screen(&p, 100_000, 0.3, 0.3), "{label}");
        check(&p, Rules::default(), span, &label);
        let glided =
            plan(&input(span, &focus, &[], Rules::default())).with_glide(&|_| Some((0.3, 0.3)));
        check(&glided, Rules::default(), span, &format!("{label}, glided"));
    }
}

#[test]
fn a_late_shots_own_click_stays_in_view_while_it_zooms_in() {
    // A menu-bar click 0.4 s into an Auto zoom, before the late arrival. The view zooming in
    // toward it bulges inward at the frame's edge: the click must stay inside it at its time.
    let focus = [click(400_000, 0.5, 0.011)];
    let p = plan(&input(3 * S, &focus, &[], Rules::default()));
    check(
        &p,
        Rules::default(),
        3 * S,
        "a menu-bar click while zooming in",
    );
    assert!(
        on_screen(&p, 400_000, 0.5, 0.011),
        "{:?}",
        p.view_at(400_000)
    );
}

#[test]
fn typing_at_the_frame_edge_past_the_end_passes_the_check() {
    // A kept typing box running past the span's end is a kept event, not a dropped action, so the
    // check doesn't also ask for its centre through the move out (where the view bulges in at the edge).
    let typing = Focus {
        at: 4 * S,
        until: 15 * S,
        rect: Rect01 {
            x: 0.5,
            y: 0.985,
            w: 0.02,
            h: 0.012,
        },
        weight: Weight::Typing,
    };
    let p = plan(&input(10 * S, &[typing], &[], Rules::default()));
    assert!(!p.events().is_empty(), "the typing is kept");
    check(&p, Rules::default(), 10 * S, "edge typing past the end");
}
