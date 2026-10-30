use super::*;
use crate::render::compositor::staged::effect_size;

fn quad(x: f64, y: f64, w: f64, h: f64, rotation_deg: f64) -> ImageQuad {
    ImageQuad {
        center: XY::new(x + w / 2.0, y + h / 2.0),
        size: XY::new(w, h),
        rotation_deg,
        opacity: 1.0,
        rounding: 0.0,
        source_rotation: 0,
        mirror: false,
        blend: klipolo_model::motion::Blend::Normal,
    }
}

const FRAME: (u32, u32) = (1920, 1080);

fn still(q: &ImageQuad, pad_px: f64, px: f32) -> Option<Plan> {
    plan(q, effect_size(q, 1.0), 1.0, pad_px, px, FRAME)
}

fn near(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

fn margins(p: &Plan) -> (f32, f32, f32, f32) {
    let (g, i) = (p.padding.grown, p.padding.inner);
    (i.x, i.y, g.0 as f32 - i.x - i.w, g.1 as f32 - i.y - i.h)
}

#[test]
fn a_layer_inside_the_frame_grows_by_the_margin_times_px() {
    let q = quad(400.0, 300.0, 100.0, 100.0, 0.0);
    let p = still(&q, 16.0, 1.0).unwrap();
    assert_eq!(p.padding.grown, (132, 132));
    assert_eq!(margins(&p), (16.0, 16.0, 16.0, 16.0));
    assert_eq!((p.quad.size.x, p.quad.size.y), (132.0, 132.0));
    assert_eq!((p.quad.center.x, p.quad.center.y), (q.center.x, q.center.y));
    // The passes on the grown target measure shapes from the layer's own box, and keep the frame's px.
    let info = p.info(FrameInfo::new(3, 1.0));
    assert_eq!(info.px, 1.0);
    let s = 132.0;
    assert!(near(
        &info.content,
        &[16.0 / s, 16.0 / s, 100.0 / s, 100.0 / s]
    ));
    assert_eq!(p.padding.look.open, [0.0; 4]);
    // At 4K (2 pixels per 1080p pixel), twice as wide.
    let p = still(&quad(800.0, 600.0, 200.0, 200.0, 0.0), 16.0, 2.0).unwrap();
    assert_eq!(p.padding.inner.x, 32.0);
    assert!(still(&q, 0.0, 1.0).is_none(), "no margin declared");
}

/// A side keeps only the margin inside the frame, in quarters (so a layer sliding across an edge
/// changes its target a few times, not every frame), and the layer's own pixels stay where they were.
#[test]
fn the_margin_stops_at_the_frame_in_quarters_and_the_layer_stays_where_it_was() {
    assert!(
        still(&quad(0.0, 0.0, 1920.0, 1080.0, 0.0), 16.0, 1.0).is_none(),
        "a full-frame layer never grows"
    );
    // 5 px from the left edge (a half of the margin kept), past the bottom one (none).
    let q = quad(5.0, 1000.0, 100.0, 100.0, 0.0);
    let p = still(&q, 16.0, 1.0).unwrap();
    assert_eq!(margins(&p), (8.0, 16.0, 16.0, 0.0));
    assert_eq!(
        p.padding.look.open,
        [0.0, 0.0, 0.0, 1.0],
        "PAD leaves the bottom edge to the quad"
    );
    let g = p.quad;
    assert_eq!(
        (g.center.x - g.size.x / 2.0, g.center.y - g.size.y / 2.0),
        (-3.0, 984.0)
    );
    assert_eq!((g.size.x, g.size.y), (124.0, 116.0));
    let c = p.info(FrameInfo::new(0, 1.0)).content;
    assert!(
        near(&[c[0] * 124.0, c[1] * 116.0], &[8.0, 16.0]),
        "shapes keep to the layer: {c:?}"
    );
}

#[test]
fn a_turned_layer_keeps_its_margin_and_its_centre() {
    let q = quad(0.0, 0.0, 400.0, 300.0, 30.0);
    let p = still(&q, 8.0, 1.0).unwrap();
    assert_eq!(margins(&p), (8.0, 8.0, 8.0, 8.0));
    assert_eq!((p.quad.center.x, p.quad.center.y), (q.center.x, q.center.y));
    assert_eq!(p.quad.rotation_deg, 30.0);
}

#[test]
fn a_capped_target_keeps_the_margin_against_the_drawn_layer_and_stays_within_the_side() {
    // Drawn 8192 wide, processed at 4096: the margin is 16 drawn pixels, 8 target pixels, but the side is full.
    let q = quad(-3000.0, 100.0, 8192.0, 400.0, 10.0);
    let at = effect_size(&q, 1.0);
    assert_eq!(at.size.0, MAX_EFFECT_SIDE);
    let p = plan(&q, at, 1.0, 16.0, 1.0, FRAME).unwrap();
    assert_eq!(margins(&p), (0.0, 8.0, 0.0, 8.0), "no room left across");
    assert_eq!(p.quad.size.x, q.size.x);
    assert!((p.quad.size.y - (q.size.y + 32.0)).abs() < 1e-9);
    assert_eq!(fit(100, 300, 4000), (24, 72));
}

/// A 100 px image with a 16 px margin zooming 1 → 0.01 → 1 over 60 frames keeps one grown target (at most
/// two sizes), never bigger than its margin at one target pixel per drawn pixel, and keeps at least its margin drawn
/// around the layer on every frame, the layer landing where it is drawn.
#[test]
fn a_zooming_layer_keeps_one_grown_target_and_its_margin() {
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for k in 0..60 {
        let t = f64::from(k) / 59.0;
        let scale = 1.0 - 0.99 * (1.0 - (2.0 * t - 1.0).abs());
        let side = 100.0 * scale;
        let q = quad(450.0 - side / 2.0, 350.0 - side / 2.0, side, side, 0.0);
        let reach = 1.0 / scale;
        let at = effect_size(&q, reach);
        assert_eq!(at.size, (100, 100), "the layer's own target never changes");
        let p = plan(&q, at, reach, 16.0, 1.0, FRAME).unwrap();
        let g = p.padding.grown;
        assert!(g.0 <= 132 && g.1 <= 132, "frame {k}: {g:?} over the cap");
        if !sizes.contains(&g) {
            sizes.push(g);
        }
        // At least 16 drawn pixels of margin on each side, and the layer's own box exactly where it is drawn.
        let s = f64::from(p.info(FrameInfo::new(0, 1.0)).px);
        let (l, t2, r, b) = margins(&p);
        for m in [l, t2, r, b] {
            assert!(f64::from(m) / s >= 16.0 - 1e-3, "frame {k}: {m} / {s}");
        }
        let left = p.quad.center.x - p.quad.size.x / 2.0 + f64::from(l) / s;
        assert!((left - q.center.x + side / 2.0).abs() < 1e-3, "frame {k}");
        assert!((f64::from(p.padding.inner.w) / s - side).abs() < 1e-3);
    }
    assert!(sizes.len() <= 2, "{sizes:?}");
}

#[test]
fn a_texts_own_box_leaves_out_its_margin() {
    // A 300 × 100 texture laid out at 2× with a 10 px margin at 1 px per 1080p pixel: 20 texture pixels each side.
    assert!(near(
        &text_content((300, 100), 10.0, 1.0, 2.0),
        &[20.0 / 300.0, 0.2, 1.0 - 40.0 / 300.0, 0.6]
    ));
    assert_eq!(text_content((300, 100), 0.0, 1.0, 2.0), FrameInfo::WHOLE);
}
