//! Auto zooms planned by the smart camera: an Auto zoom over cursor data
//! is a camera run planned from the take's facts (the focus chat plans from too) with the person's Rect zooms inside
//! it as fixed shots and the raw cursor for its glide; elsewhere it looks at the centre on the springs. How the cursor
//! files load (bounded, damaged, off the thread, within the budget) is `cursor_zoom/loads.rs`; an hour at real scale
//! is `cursor_zoom/scale.rs`.

mod support;

#[path = "cursor_zoom/inputs.rs"]
mod inputs;
#[path = "cursor_zoom/loads.rs"]
mod loads;
#[path = "cursor_zoom/scale.rs"]
mod scale;

use inputs::{in_safe_zone, shows, track, tutorial, walk};

use std::sync::Arc;

use klipolo_cursor::CursorTrack;
use klipolo_cursor::camera::{Rules, View, check_plan};
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::PlanContext;
use klipolo_engine::plan::zoom::{CameraRun, auto_rect, zoom_items};
use klipolo_engine::registry::Registry;
use klipolo_model::cursor_style::{CursorCapture, FocusKind, Smoothing};
use klipolo_model::layout::BoxN;
use klipolo_model::project::Project;
use klipolo_model::timeline::{ItemContent, ZoomTarget};
use support::recording::auto_zoom_project;
use support::{bx, close, item, secs};

pub fn context(p: Project, tracks: CursorTracks) -> PlanContext {
    PlanContext::with_cursors(Arc::new(p), Registry::builtin(), Arc::new(tracks))
}

fn rules(scale: f64) -> Rules {
    Rules {
        max_scale: scale,
        ..Rules::default()
    }
}

/// The context's only camera run, checked against every rule of the camera (with its glide).
fn only_run(ctx: &PlanContext, scale: f64) -> &CameraRun {
    let cameras = ctx.zoom.cameras();
    assert_eq!(cameras.len(), 1, "one camera run");
    let run = &cameras[0].1;
    check_plan(&run.plan, rules(scale), run.end_us - run.start_us).expect("the camera's rules");
    run
}

#[test]
fn an_auto_zoom_arrives_at_each_click_before_it_lands() {
    let clicks = [
        (8.0, 0.2, 0.3),
        (12.0, 0.25, 0.35),
        (17.0, 0.7, 0.6),
        (23.0, 0.4, 0.8),
    ];
    let (p, tracks) = auto_zoom_project(tutorial(&clicks), 5.0, 30.0, 2.0);
    let ctx = context(p, tracks);
    let run = only_run(&ctx, 2.0);
    let events = run.plan.events();
    assert_eq!(events.len(), clicks.len(), "every click kept: {events:?}");
    for &(at, x, y) in events {
        let t = run.start_us + at - 300_000;
        let w = (ctx.zoom.window(t)).unwrap_or_else(|| panic!("not zoomed 0.3 s before {at}"));
        assert!(
            in_safe_zone(w, x.into(), y.into()),
            "{x}, {y} outside {w:?}"
        );
        let settled = ctx.zoom.window(run.start_us + at);
        assert_eq!(Some(w), settled, "settled through the pre-roll");
    }
    assert_eq!(
        ctx.zoom.window(secs(4.9)),
        None,
        "the whole frame before it"
    );
    assert!(
        zoom_items(&ctx.project, &ctx.cursors).is_empty(),
        "no springs"
    );
}

#[test]
fn an_auto_zoom_zooms_out_between_actions_far_apart() {
    let clicks = [(8.0, 0.15, 0.15), (20.0, 0.85, 0.85)];
    let (p, tracks) = auto_zoom_project(tutorial(&clicks), 5.0, 30.0, 2.0);
    let ctx = context(p, tracks);
    only_run(&ctx, 2.0);
    assert!(ctx.zoom.window(secs(8.0)).is_some() && ctx.zoom.window(secs(20.0)).is_some());
    assert_eq!(ctx.zoom.window(secs(14.0)), None, "out between them");
}

/// A pause at (0.3, 0.3) from 9 s, a click at 14 s while the cursor travels on to (0.8, 0.2), another pause there.
fn pauses_and_a_click() -> CursorTrack {
    let keys = [
        (8.0, 0.6, 0.6),
        (9.0, 0.3, 0.3),
        (12.0, 0.3, 0.3),
        (16.0, 0.8, 0.2),
    ];
    walk(&keys, &[14.0])
}

#[test]
fn an_auto_zoom_using_pauses_only_ignores_clicks() {
    let planned = |using: Vec<FocusKind>| {
        let (mut p, tracks) = auto_zoom_project(pauses_and_a_click(), 5.0, 30.0, 2.0);
        if let ItemContent::Zoom { target, .. } = &mut p.timeline.tracks[0].items[0].content {
            *target = ZoomTarget::Auto { using };
        }
        context(p, tracks)
    };
    let clicked = |ctx: &PlanContext| {
        let run = only_run(ctx, 2.0);
        (run.plan.events().iter()).any(|e| run.start_us + e.0 == secs(14.0))
    };
    assert!(
        clicked(&planned(Vec::new())),
        "every kind: the click is a shot"
    );
    // Narrowed: no shot for the click, and with the glide on it stays on screen.
    let pauses = planned(vec![FocusKind::Pauses]);
    assert!(!clicked(&pauses), "the click is no shot");
    assert!(
        !only_run(&pauses, 2.0).plan.events().is_empty(),
        "the pauses are"
    );
    let w = pauses.zoom.window(secs(14.0));
    assert!(shows(w, 0.55, 0.25), "the click off screen: {w:?}");
}

#[test]
fn the_persons_rect_zoom_inside_an_auto_zoom_is_kept_exactly() {
    let clicks = [(6.0, 0.2, 0.7), (22.0, 0.75, 0.25)];
    let (mut p, tracks) = auto_zoom_project(tutorial(&clicks), 0.0, 30.0, 2.0);
    let theirs = |id: &str, start: f64, length: f64, rect: BoxN| {
        let content = ItemContent::Zoom {
            target: ZoomTarget::Rect { rect },
            scale: 2.5,
        };
        item(id, start, length, bx(0.0, 0.0, 1.0, 1.0), content)
    };
    let rect = bx(0.4, 0.2, 0.4, 0.4);
    // One starts with the Auto zoom: a cut at its start, their placement, which the check exempts.
    let zooms = &mut p.timeline.tracks[0].items;
    zooms.push(theirs("theirs", 12.0, 3.0, rect));
    zooms.push(theirs("first", 0.0, 2.0, bx(0.1, 0.1, 0.4, 0.4)));
    let ctx = context(p, tracks);
    let run = only_run(&ctx, 2.0);
    let view = View {
        cx: rect.x + rect.w / 2.0,
        cy: rect.y + rect.h / 2.0,
        scale: 2.5,
    };
    for t in (secs(12.0)..secs(15.0)).step_by(50_000) {
        assert_eq!(run.plan.view_at(t - run.start_us), view, "at {t}");
    }
    assert_eq!(run.plan.view_at(0).scale, 2.5, "theirs from the start");
    assert!(
        zoom_items(&ctx.project, &ctx.cursors).is_empty(),
        "not on the springs"
    );
}

#[test]
fn a_three_second_auto_zoom_over_a_click_then_a_rest_still_zooms() {
    // Chat's `zoomOn region: auto` (3 s) or the inspector's Follow over a click, the cursor resting after it well past
    // the zoom's end: the span's end cuts the hold, never the zoom.
    // The cursor arrives 0.6 s in (its rest starts there), clicks 0.4 s later and rests to 30 s: too soon after the
    // zoom's start for a move in before the pre-roll, so the camera arrives late (the span's start is the person's too).
    let keys = [(4.0, 0.6, 0.6), (5.6, 0.3, 0.3), (30.0, 0.3, 0.3)];
    let (p, tracks) = auto_zoom_project(walk(&keys, &[6.0]), 5.0, 8.0, 2.0);
    let ctx = context(p, tracks);
    only_run(&ctx, 2.0);
    assert!(
        ctx.zoom.window(secs(6.0)).is_some(),
        "zooming in at the click"
    );
    let w = ctx.zoom.window(secs(6.5)).expect("zoomed on it");
    assert!(in_safe_zone(w, 0.3, 0.3), "{w:?}");
    assert_eq!(ctx.zoom.window(secs(8.0)), None, "the whole frame after it");
}

#[test]
fn a_plan_shown_while_its_rebuild_runs_is_never_kept_as_the_new_one() {
    // A 40 s Auto zoom (over INLINE_STEPS: planned off the actor). The person adds a Rect zoom inside it: the same span
    // with a new key, so the old plan shows while the new one is made. A context made from that partial track (another
    // edit before the build lands) plans again; it never takes the old plan for the new key.
    let clicks = [(8.0, 0.2, 0.3), (20.0, 0.7, 0.6), (32.0, 0.4, 0.8)];
    let (mut p, tracks) = auto_zoom_project(tutorial(&clicks), 2.0, 42.0, 2.0);
    let cursors = Arc::new(tracks);
    let (mut first, build) = PlanContext::starting(Arc::new(p.clone()), None, &cursors);
    assert!(first.with_zoom(build.expect("planned off the actor").finish()));
    let rect = bx(0.4, 0.2, 0.4, 0.4);
    let content = ItemContent::Zoom {
        target: ZoomTarget::Rect { rect },
        scale: 2.5,
    };
    let theirs = item("theirs", 14.0, 3.0, bx(0.0, 0.0, 1.0, 1.0), content);
    p.timeline.tracks[0].items.push(theirs);
    let p = Arc::new(p);
    let (shown, build) = PlanContext::starting(p.clone(), Some(&first), &cursors);
    assert!(build.is_some(), "the new key is planned off the actor");
    let at = secs(8.0);
    assert_eq!(
        shown.zoom.window(at),
        first.zoom.window(at),
        "the old plan shows meanwhile"
    );
    let (mut again, build) = PlanContext::starting(p, Some(&shown), &cursors);
    let build = build.expect("planned again, not the old plan kept");
    assert!(again.with_zoom(build.finish()));
    close(
        again.zoom.window(secs(15.0)).unwrap(),
        bx(0.4, 0.2, 0.4, 0.4),
    );
}

#[test]
fn rect_zooms_outside_auto_zooms_keep_the_springs() {
    // `plan.rs`'s spring test (zoom_eases_to_its_target_in_source_space), beside an Auto zoom over cursor data.
    let target = ZoomTarget::Rect {
        rect: bx(0.5, 0.5, 0.5, 0.5),
    };
    let content = ItemContent::Zoom { target, scale: 2.0 };
    let rect = item("z", 1.0, 3.0, bx(0.0, 0.0, 1.0, 1.0), content);
    let (mut p, tracks) = auto_zoom_project(tutorial(&[(25.0, 0.3, 0.3)]), 20.0, 30.0, 2.0);
    p.timeline.tracks[0].items.push(rect);
    let with_auto = context(p.clone(), tracks);
    assert_eq!(with_auto.zoom.cameras().len(), 1);
    p.timeline.tracks[0].items.retain(|i| i.id == "z");
    let alone = context(p, CursorTracks::default());
    let zoomed = with_auto.zoom.window(secs(2.5)).unwrap();
    for (got, want) in [
        (zoomed.x, 0.5),
        (zoomed.y, 0.5),
        (zoomed.w, 0.5),
        (zoomed.h, 0.5),
    ] {
        assert!((got - want).abs() < 0.02, "{zoomed:?}");
    }
    for t in (0..secs(10.0)).step_by(100_000) {
        assert_eq!(with_auto.zoom.window(t), alone.zoom.window(t), "at {t}");
    }
    assert_eq!(with_auto.zoom.window(secs(6.0)), None);
    assert_eq!(zoom_items(&with_auto.project, &with_auto.cursors).len(), 1);
}

#[test]
fn an_auto_zoom_over_an_imported_clip_looks_at_the_centre() {
    // The fixture's b-roll plays 20–30 s.
    let mut p = support::project();
    let auto = ItemContent::Zoom {
        target: ZoomTarget::Auto { using: vec![] },
        scale: 2.0,
    };
    let zoom = item("auto", 21.0, 8.0, bx(0.0, 0.0, 1.0, 1.0), auto);
    p.timeline.tracks = vec![support::zoom_track(vec![zoom])];
    let ctx = context(p, CursorTracks::default());
    assert!(ctx.zoom.cameras().is_empty());
    let w = ctx.zoom.window(secs(25.0)).unwrap();
    let centre = auto_rect(None, 2.0);
    for (got, want) in [
        (w.x, centre.x),
        (w.y, centre.y),
        (w.w, centre.w),
        (w.h, centre.h),
    ] {
        assert!((got - want).abs() < 0.02, "{w:?}");
    }
}

#[test]
fn the_camera_ignores_the_style_smoothing() {
    // The same take drawn Off and Strong (its frames without the cursor, so Strong builds a path) plans the
    // same camera, to the bit.
    let planned = |smoothing: Smoothing| {
        let clicks = [(8.0, 0.2, 0.3), (17.0, 0.7, 0.6)];
        let (mut p, tracks) = auto_zoom_project(tutorial(&clicks), 5.0, 30.0, 2.0);
        p.recordings[0].cursor.as_mut().unwrap().capture = CursorCapture::Separate;
        p.cursor.smoothing = smoothing;
        tracks.load_all(&wanted(&p));
        context(p, tracks)
    };
    let (off, strong) = (planned(Smoothing::Off), planned(Smoothing::Strong));
    let take = strong.takes.get(support::recording::CURSOR_PATH).unwrap();
    assert_eq!(take.variants(), 1, "Strong has its path");
    assert_eq!(
        off.zoom.cameras()[0].0,
        strong.zoom.cameras()[0].0,
        "the same key"
    );
    let bits = |w: Option<BoxN>| w.map(|w| [w.x, w.y, w.w, w.h].map(f64::to_bits));
    for t in (0..secs(31.0)).step_by(16_667) {
        assert_eq!(
            bits(off.zoom.window(t)),
            bits(strong.zoom.window(t)),
            "at {t}"
        );
    }
}
