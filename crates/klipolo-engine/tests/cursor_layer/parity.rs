//! Two cursors in a transition, preview against export, the session drawing a take once it lands, no allocation, and
//! the time a cursor takes to plan at real scale.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::api::ViewSpec;
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::{
    CursorLayer, Layer, PlanContext, cursor_layer, frame_plan, frame_start_us,
};
use klipolo_engine::session::blank::BlankBackend;
use klipolo_engine::session::{Engine, EngineOptions, ProjectFeed};
use klipolo_model::contrib::builtin;
use klipolo_model::cursor_style::{ClickEffect, CursorCapture, CursorStyle, Smoothing};
use klipolo_model::ops::Op;
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::timeline::Transition;
use tokio::sync::watch;

use crate::support::recording::cursor_project;
use crate::support::secs;
use crate::{
    ALLOCS, cursors_at, export_ctx, footage, glide, mapped, near, press, rest, style, take,
};

const WAIT: Duration = Duration::from_secs(30);

/// 40 s of a tutorial: glides, rests, clicks of both buttons and a drag, and a burn-in at 30 s.
fn busy() -> Vec<Line> {
    let mut lines = take(glide((0.2, 0.3), (0.7, 0.6), 0.0, 1.5));
    lines.extend(rest(0.7, 0.6, 2.0, 5.0));
    lines.extend(glide((0.7, 0.6), (0.3, 0.4), 5.1, 6.5));
    lines.extend(press(0, (0.45, 0.47), 5.6, 5.7));
    lines.extend(glide((0.3, 0.4), (0.5, 0.8), 7.0, 8.0));
    lines.extend(press(0, (0.3, 0.4), 6.9, 8.1));
    lines.extend(rest(0.5, 0.8, 8.5, 12.0));
    lines.extend(glide((0.5, 0.8), (0.9, 0.1), 12.1, 13.0));
    lines.extend(press(1, (0.9, 0.1), 13.5, 13.6));
    lines.extend(rest(0.9, 0.1, 14.0, 40.0));
    lines.push(Line::Burned { t: secs(30.0) });
    lines
}

fn busy_style() -> CursorStyle {
    style(|s| {
        s.smoothing = Smoothing::Strong;
        s.hide_when_still_us = Some(1_000_000);
        s.click = ClickEffect::RippleAndPress;
        s.motion_blur = 0.7;
    })
}

#[test]
fn a_transition_between_two_takes_plans_two_cursor_layers() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = crate::support::takes::cursor_takes(2, dir.path(), CursorCapture::Separate);
    for k in 1..=2 {
        p.assets.push(Asset {
            id: format!("screen{k}"),
            kind: AssetKind::Video,
            path: format!("/media/screen{k}"),
            name: format!("screen{k}"),
            duration_us: secs(10.0),
            width: 1920,
            height: 1080,
            fps: 30.0,
            has_audio: false,
            audio_lead_us: None,
            rotation: None,
        });
    }
    p.timeline.main[0].source_out_us = secs(9.0);
    p.timeline.main[1].source_in_us = secs(1.0);
    let dissolve = Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(1.0),
        params: Default::default(),
    };
    let set = Op::SetTransition {
        clip_id: "clip1".into(),
        transition: Some(dissolve),
    };
    let ctx = export_ctx(klipolo_model::apply_ops(&p, &[set]).unwrap());
    // The cut at 9 s: the outgoing side shows take 1 at 9 s, the incoming take 2 at 1 s, each with its own cursor
    // right after its footage.
    let plan = frame_plan(&ctx, secs(9.0), Landscape);
    let Layer::Transition(t) = &plan.layers[0] else {
        panic!("{:?}", plan.layers)
    };
    for (side, k, at) in [(&t.outgoing, 1, 9.0), (&t.incoming, 2, 1.0)] {
        let (Layer::Video(v), Layer::Cursor(c)) = (&side[0], &side[1]) else {
            panic!("{side:?}")
        };
        assert_eq!(c.id.as_ref(), format!("cursor:clip{k}"));
        let path = crate::support::takes::cursor_file(dir.path(), k);
        let take = ctx.cursors.get(&path.to_string_lossy()).unwrap();
        let point = take.with_drawn(Smoothing::Medium, |p| p.position(&take.track, secs(at)));
        near(c.hotspot, mapped(v, point.unwrap()), 1e-9);
    }
    assert_eq!(cursors_at(&ctx, 9.0, Landscape).len(), 2);
}

#[test]
fn preview_and_export_plan_the_same_cursor_for_2000_frames() {
    let dir = tempfile::tempdir().unwrap();
    let p = cursor_project(dir.path(), &busy(), busy_style());
    // The preview's takes as the session asks for them (`want` on each publish, read and smoothed off the actor),
    // waited for before planning; export's read before its first frame.
    let cursors = Arc::new(CursorTracks::default());
    let wants = wanted(&p);
    cursors.want(&wants);
    assert!(cursors.wait_ready(&wants, WAIT));
    let (preview, build) = PlanContext::starting(Arc::new(p.clone()), None, &cursors);
    assert!(build.is_none());
    let export = export_ctx(p);
    let mut seen = Vec::new();
    for n in 0..2000 {
        let t = frame_start_us(n, 60) as f64 / 1e6;
        let planned = cursors_at(&preview, t, Landscape);
        assert_eq!(planned, cursors_at(&export, t, Landscape), "frame {n}");
        seen.extend(planned);
    }
    // Drawn on about 9 s of the 33 (hidden at rest after 1 s, and burned in from 30 s), with every effect in play.
    let count = |f: fn(&CursorLayer) -> bool| seen.iter().filter(|c| f(c)).count();
    assert!(seen.len() > 400, "{}", seen.len());
    assert!(count(|c| c.ripple_count > 0) > 10 && count(|c| c.press < 1.0) > 10);
    assert!(count(|c| c.blur != (0.0, 0.0)) > 100 && count(|c| c.opacity < 1.0) > 10);
}

#[test]
fn planning_a_cursor_allocates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = export_ctx(cursor_project(dir.path(), &busy(), busy_style()));
    let (clip, rec) = (&ctx.project.timeline.main[0], &ctx.project.recordings[0]);
    let v = footage(&ctx, 1.0, Landscape);
    let mut s = ctx.strings.lock();
    assert!(cursor_layer(&ctx, clip, rec, secs(1.0), &v, Landscape, &mut s).is_some());
    ALLOCS.with(|n| n.set(0));
    // 0–15 s: glides, rests, presses, ripples, the drag and the idle fade.
    let drawn = (0..1_000)
        .filter(|k| cursor_layer(&ctx, clip, rec, k * 15_000, &v, Landscape, &mut s).is_some())
        .count();
    assert_eq!(ALLOCS.with(Cell::get), 0);
    assert!(drawn > 500, "{drawn}");
}

#[tokio::test]
async fn a_paused_preview_draws_the_cursor_once_its_take_lands() {
    // Ten minutes of gliding: the take is read and smoothed well after the first frame is drawn, and the frame is
    // drawn again when it lands.
    let dir = tempfile::tempdir().unwrap();
    let lines = crate::support::takes::gliding(Capture::Separate, 600.0);
    let p = cursor_project(dir.path(), &lines, CursorStyle::default());
    let (feed, rx) = watch::channel(None);
    let (backend, log, _) = BlankBackend::recording();
    let options = EngineOptions {
        allowed_origins: vec!["tauri://localhost".into()],
        max_preview_pixels: 921_600,
        socket_playback_pixels: 921_600,
        socket_rgba_only: false,
        adapter: "Test".into(),
        gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
    };
    let engine = Engine::start(options, rx, backend).await.unwrap();
    engine.set_view(ViewSpec {
        format: Landscape,
        width: 64,
        height: 36,
    });
    feed.send_replace(Some(ProjectFeed {
        project_id: "p".into(),
        revision: 0,
        opened: 1,
        project: Arc::new(p),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    let drawn = || {
        let log = log.lock().unwrap();
        let cursor = |l: &Layer| matches!(l, Layer::Cursor(_));
        log.last().is_some_and(|r| r.plan.layers.iter().any(cursor))
    };
    let deadline = Instant::now() + WAIT;
    while !drawn() {
        assert!(Instant::now() < deadline, "never drawn with its cursor");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Where the 2-hour take's `k`th glide ends: a spread of places over the screen.
fn spot(k: u32) -> (f32, f32) {
    let f = |m: u32| (k.wrapping_mul(m) % 100) as f32 / 125.0 + 0.1;
    (f(37), f(61))
}

#[test]
#[ignore = "real scale, release: cargo test -p klipolo-engine --release --test cursor_layer -- --ignored"]
fn a_cursor_plans_in_under_20_us() {
    // A 2-hour tutorial take: a 1.5 s glide every 6 s with a click at its end, at rest between (25 % moving, 120 Hz).
    let mut lines = take([]);
    for k in 0..1_200 {
        let t = f64::from(k) * 6.0;
        let (a, b) = (spot(k), spot(k + 1));
        lines.extend(glide(a, b, t, t + 1.5));
        lines.extend(press(0, b, t + 1.55, t + 1.65));
        lines.extend(rest(b.0, b.1, t + 2.0, t + 5.5));
    }
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let ctx = export_ctx(cursor_project(dir.path(), &lines, busy_style()));
    let loaded = started.elapsed();
    let (clip, rec) = (&ctx.project.timeline.main[0], &ctx.project.recordings[0]);
    let v = footage(&ctx, 1.0, Landscape);
    let mut s = ctx.strings.lock();
    let mut times = Vec::with_capacity(10_000);
    let mut drawn = 0;
    for k in 0..10_000_i64 {
        let at = k * 719_999 + (k * 7_919) % 100_000;
        let start = Instant::now();
        let c = cursor_layer(&ctx, clip, rec, at, &v, Landscape, &mut s);
        times.push(start.elapsed());
        drawn += usize::from(std::hint::black_box(c).is_some());
    }
    times.sort();
    let (p50, p99, max) = (times[5_000], times[9_900], times[9_999]);
    println!(
        "2-hour take ({} lines): loaded and smoothed in {loaded:?}; {drawn} of 10,000 frames drawn; per frame p50 \
         {p50:?}, p99 {p99:?}, max {max:?}",
        lines.len()
    );
    assert!(p99 < Duration::from_micros(20), "p99 {p99:?}");
}
