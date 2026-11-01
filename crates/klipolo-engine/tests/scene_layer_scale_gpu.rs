//! A scene under its layer's scaling motion on the GPU (lavapipe in CI): its pixel sizes follow the layer's scale, as
//! its text does, so a box that mixes a fraction and pixels keeps its share of the design through a pop.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

const SIZE: (u32, u32) = (640, 360);

/// A red bar 0.3 of the layer wide and a green bar 192 1080p pixels wide (0.1 of a 1920-wide layer: a third of the red
/// one in a full frame), both in fractions down.
const BARS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"type":"rect","box":[0.1,0.4,0.3,0.2],"fill":"#FF0000"},
  {"type":"rect","box":[0.1,0.65,"192px",0.2],"fill":"#00FF00"}]}"##;

/// The longest run of pixels on one row where `pick` holds.
fn widest(img: &image::RgbaImage, pick: impl Fn([u8; 3]) -> bool) -> u32 {
    let mut most = 0;
    for y in 0..img.height() {
        let mut run = 0;
        for x in 0..img.width() {
            if pick(rgb_at(img, x, y)) {
                run += 1;
                most = most.max(run);
            } else {
                run = 0;
            }
        }
    }
    most
}

/// Through a `pop` (from 0.85 of its size, overshooting, settling), the green bar's px width scales with the layer
/// like the red bar's fraction does: a third of it in every frame. Drawn at the frame's own `kl.px` (px kept on screen
/// while the layer scales) it was 64 px wide at 0.85 against the red's 163.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_box_mixing_fractions_and_pixels_scales_with_its_layer() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) = project_with_scene("bars", BARS, 0.0, 2.0);
    let layer = scene_item(&mut p, "bars").clone();
    *scene_item(&mut p, "bars") = animated(layer, Some("pop"), None);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    for t in [0.02, 0.08, 0.14, 0.2, 0.26, 1.0] {
        let (img, _) = scene_frame(&mut h, &p, t, SIZE).await;
        let red = widest(&img, |[r, g, b]| {
            r > g.saturating_add(40) && r > b.saturating_add(40)
        });
        let green = widest(&img, |[r, g, b]| {
            g > r.saturating_add(40) && g > b.saturating_add(40)
        });
        assert!(red > 100, "the red bar shows at {t}: {red}");
        let share = f64::from(green) / f64::from(red);
        assert!(
            (share - 1.0 / 3.0).abs() < 0.025,
            "at {t} s the px bar is {green} px against {red} px ({share:.3} of it, not a third)"
        );
    }
}
