//! What the cursor costs an export, measured (run by hand, in release; the Mac's figures come from a Mac run):
//! - a frame drawn with the cursor layer against the same frame without it, relative, under 3 %;
//! - the preparation before frame 1 for a 2-hour take: loading and smoothing it, and putting its shapes in the atlas.
//!
//! `WGPU_BACKEND=vulkan cargo test --release -p klipolo-engine --test export_cursor -- --ignored --nocapture timing`

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_cursor::format::Line;
use klipolo_engine::export::prepare::{cursor_shapes, load_cursors, prepare_cursors};
use klipolo_engine::headless::Headless;
use klipolo_engine::registry::Registry;
use klipolo_engine::render::cursor::CursorPass;
use klipolo_model::cursor_style::CursorCapture;
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::Project;

use crate::{FPS, Shape, add_take, click, device, fixture, glide, rest, write_take};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: needs a GPU and scripts/gen-engine-fixtures.sh; run in release"]
async fn export_overhead_with_the_cursor_layer_is_under_3_percent() {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(fixture(dir.path(), true));
    let mut hidden = (*p).clone();
    hidden.cursor.hidden = true;
    let hidden = Arc::new(hidden);
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    h.load_cursors(&p);
    let frames: Vec<i64> = (0..60)
        .map(|n| klipolo_engine::plan::frame_start_us(n, FPS))
        .collect();
    // Alternating rounds, the fastest of each: the host's other work shows in both alike.
    let mut best = [Duration::MAX; 2];
    for round in 0..6 {
        for (k, project) in [&p, &hidden].into_iter().enumerate() {
            let started = Instant::now();
            for &t in &frames {
                h.render(project.clone(), t, Landscape, (1280, 720))
                    .await
                    .unwrap();
            }
            if round > 0 {
                best[k] = best[k].min(started.elapsed());
            }
        }
    }
    let overhead = best[0].as_secs_f64() / best[1].as_secs_f64() - 1.0;
    println!(
        "60 export frames at 1280 × 720: with the cursor {:?}, without {:?}: {:+.2} %",
        best[0],
        best[1],
        overhead * 100.0
    );
    assert!(overhead < 0.03, "{:.2} %", overhead * 100.0);
}

#[test]
#[ignore = "a measurement at real scale: needs a GPU; run in release"]
fn preparing_a_2_hour_take_before_frame_1_is_bounded() {
    // A 2-hour tutorial take: a 1.5 s glide every 6 s with a click at its end and a new shape each time (12 of them),
    // at rest between: 25 % moving at 120 Hz, as the speed test.
    let dir = tempfile::tempdir().unwrap();
    let shapes: Vec<Shape> = (0..12)
        .map(|id| Shape {
            id,
            side: 64,
            rgba: [20 * id as u8, 255 - 20 * id as u8, 128, 255],
            damaged: false,
        })
        .collect();
    let spot = |k: u32| {
        let f = |m: u32| (k.wrapping_mul(m) % 100) as f32 / 125.0 + 0.1;
        (f(37), f(61))
    };
    let mut lines: Vec<Line> = Vec::new();
    for k in 0..1_200_u32 {
        let t = f64::from(k) * 6.0;
        let (a, b) = (spot(k), spot(k + 1));
        lines.push(Line::Shape {
            t: crate::support::secs(t),
            id: (k % 12) as u16,
        });
        lines.extend(glide(a, b, t, t + 1.5));
        lines.extend(click(b, t + 1.55, t + 1.65));
        lines.extend(rest(b.0, b.1, t + 2.0, t + 5.5));
    }
    let take = write_take(dir.path(), 1, &shapes, lines);
    let mut p = Project::new("Two hours");
    add_take(
        &mut p,
        1,
        "/media/screen",
        7_200.0,
        (&take, CursorCapture::Separate),
        (0.0, 7_200.0),
    );
    let started = Instant::now();
    let loaded = load_cursors(&p);
    assert!(loaded.missing.is_empty(), "readable");
    let load = started.elapsed();
    let dev = device();
    let mut pass = CursorPass::new(&dev.device, Arc::default());
    let started = Instant::now();
    let shapes = cursor_shapes(&p, &loaded.cursors);
    assert_eq!(
        prepare_cursors(&loaded.cursors, &shapes, &mut pass, &dev),
        0
    );
    let preload = started.elapsed();
    let take = loaded.cursors.get(&take.to_string_lossy()).expect("held");
    println!(
        "2-hour take: loaded and smoothed in {load:?} ({:.1} MB held), 12 shapes preloaded in {preload:?}",
        take.bytes() as f64 / 1e6
    );
    assert!(load < Duration::from_secs(2) && preload < Duration::from_secs(1));
}
