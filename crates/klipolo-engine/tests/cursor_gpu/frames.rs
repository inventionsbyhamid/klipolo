//! Whole frames: drawing allocates nothing after warm-up, and a separate take drawn through the compositor (its
//! golden), where a burned-in take still draws nothing.

use std::cell::Cell;
use std::sync::Arc;

use klipolo_cursor::format::{Capture, Line};
use klipolo_engine::headless::Headless;
use klipolo_engine::media::cursor::{CursorTracks, wanted};
use klipolo_engine::plan::{CursorDraw, Layer, PlanContext, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_model::cursor_style::{CursorCapture, CursorStyle};
use klipolo_model::project::FormatKind::Landscape;

use crate::support::recording::cursor_project;
use crate::support::{fixture, secs, takes::header};
use crate::{
    ALLOCS, H, W, busy, device, draw_into, pass, preload_list, read_back, render, take_with, target,
};

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

#[test]
#[ignore = "needs a GPU"]
fn drawing_a_cursor_frame_allocates_nothing_after_warm_up() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let (_, looks) = take_with(dir.path(), 1, &[(32, 32, [255, 0, 0, 255])]);
    let mut p = pass(&dev);
    p.preload(&dev.device, &dev.queue, &preload_list(&looks))
        .unwrap();
    let aspect = f64::from(W) / f64::from(H);
    let mut a = busy(looks[0].clone(), (0.3, 0.4), 0.2, aspect);
    let b = busy(CursorDraw::Dot, (0.7, 0.5), 0.2, aspect);
    for _ in 0..3 {
        render(&dev, &mut p, &[a.clone(), b.clone()], true);
    }
    let texture = target(&dev.device, W, H);
    let mut encoder = dev.device.create_command_encoder(&Default::default());
    let (mut preparing, mut drawing, mut uploading) = (0, 0, 0);
    // 500 frames, the cursor moving, in one render pass: wgpu's own command list grows by doubling.
    draw_into(&mut encoder, &texture, |rp| {
        for k in 0..500 {
            a.hotspot.0 = 0.2 + 0.001 * f64::from(k);
            let before = allocs();
            // What the compositor calls on every strict frame before drawing.
            p.begin_frame();
            p.upload_ready(&dev.device, &dev.queue);
            p.ensure(&dev.device, &dev.queue, &a);
            p.ensure(&dev.device, &dev.queue, &b);
            let ra = p.prepare(&a, (W, H), true).unwrap();
            let rb = p.prepare(&b, (W, H), true).unwrap();
            let prepared = allocs();
            p.draw(rp, ra);
            p.draw(rp, rb);
            let drawn = allocs();
            p.finish_frame(&dev.queue);
            uploading += allocs() - drawn;
            (preparing, drawing) = (preparing + prepared - before, drawing + drawn - prepared);
        }
    });
    let pixels = read_back(&dev, encoder, &texture);
    assert!(pixels.chunks_exact(4).any(|px| px[3] > 0), "it drew");
    println!(
        "500 frames, 2 slots: begin, upload_ready, ensure and prepare {preparing}, draw {drawing} (wgpu's \
         command list), upload {uploading} (wgpu)"
    );
    assert_eq!(preparing, 0, "a frame's preparation allocates nothing");
    assert!(
        drawing <= 16,
        "draw adds nothing but wgpu's list growing: {drawing}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn a_separate_take_draws_its_cursor_through_the_compositor() {
    // A take whose frames have no cursor: a 32 px red shape of 64 points, its hotspot at its top left, resting at a
    // quarter of the way in and down, over the 2 s solid blue fixture.
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("take").join("cursors").join("0.png");
    std::fs::create_dir_all(png.parent().unwrap()).unwrap();
    image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 0, 0, 255]))
        .save(&png)
        .unwrap();
    let mut lines = vec![
        header(Capture::Separate),
        Line::ShapeDef {
            id: 0,
            key: format!("{:016x}", 1),
            file: "cursors/0.png".into(),
            pw: 32,
            ph: 32,
            hx: 0.0,
            hy: 0.0,
            pts: 64.0,
            sys: None,
        },
        Line::Shape { t: 0, id: 0 },
    ];
    lines.extend((0..=4).map(|k| Line::Move {
        t: k * 500_000,
        x: 0.25,
        y: 0.25,
    }));
    let style = CursorStyle {
        size: 1.0,
        ..CursorStyle::default()
    };
    let mut p = cursor_project(dir.path(), &lines, style);
    p.assets[0].path = fixture("solid-blue.mp4").display().to_string();
    p.assets[0].duration_us = secs(2.0);
    p.recordings[0].sources[0].runs[0].duration_us = secs(2.0);
    p.timeline.main[0].source_out_us = secs(2.0);
    let frame = |p: klipolo_model::project::Project| async move {
        let mut headless = Headless::new(Registry::builtin()).await.unwrap();
        headless.load_cursors(&p);
        let f = headless
            .render(Arc::new(p), secs(1.0), Landscape, (640, 360))
            .await
            .unwrap();
        (f.bytes.to_vec(), f.stride)
    };
    let (drawn, stride) = frame(p.clone()).await;
    // Where the plan put it.
    let tracks = CursorTracks::default();
    tracks.load_all(&wanted(&p));
    let ctx = PlanContext::with_cursors(Arc::new(p.clone()), Registry::builtin(), Arc::new(tracks));
    let c = frame_plan(&ctx, secs(1.0), Landscape)
        .layers
        .into_iter()
        .find_map(|l| match l {
            Layer::Cursor(c) => Some(c),
            _ => None,
        })
        .expect("a separate take's cursor");
    let (x, y, size) = (c.hotspot.0 * 640.0, c.hotspot.1 * 360.0, c.height * 360.0);
    assert!(size > 20.0, "{size}");
    let rgb = |f: &[u8], px: f64, py: f64| {
        let i = (py as u32 * stride + px as u32 * 4) as usize;
        [f[i], f[i + 1], f[i + 2]]
    };
    let near = |a: [u8; 3], b: [u8; 3], t: i16| {
        a.iter()
            .zip(b)
            .all(|(p, q)| (i16::from(*p) - i16::from(q)).abs() <= t)
    };
    assert!(
        near(rgb(&drawn, x + size / 2.0, y + size / 2.0), [255, 0, 0], 2),
        "its shape"
    );
    assert!(
        near(rgb(&drawn, x - 6.0, y + size / 2.0), [0x1E, 0x64, 0xDC], 6),
        "the footage beside it"
    );
    // Hidden, and burned in (as every take was before the cursor was captured apart): no cursor at all, pixel for
    // pixel the same frame.
    let mut hidden = p.clone();
    hidden.cursor.hidden = true;
    let mut burned = p.clone();
    burned.recordings[0].cursor.as_mut().unwrap().capture = CursorCapture::Burned;
    let (hidden, _) = frame(hidden).await;
    assert_eq!(frame(burned).await.0, hidden);
    // The cursor changes nothing outside its own box and the shadow under it.
    for py in 0..360 {
        for px in 0..640 {
            let (fx, fy) = (f64::from(px), f64::from(py));
            let inside =
                fx > x - 3.0 && fx < x + size + 3.0 && fy > y - 3.0 && fy < y + 1.3 * size + 3.0;
            if !inside {
                assert_eq!(rgb(&drawn, fx, fy), rgb(&hidden, fx, fy), "({px}, {py})");
            }
        }
    }
}
