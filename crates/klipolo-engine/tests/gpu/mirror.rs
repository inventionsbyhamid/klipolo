//! A mirrored video layer is drawn mirrored, on each path an overlay video takes: plain (the
//! image quad's flip), with effects (flipped in the effect stage's first pass, so the effects act on the picture as
//! shown) and overlay-blended (the read-below pass), in preview and in export.
//!
//! The layer fills the frame with the bars fixture, so a mirrored render's pixel at x must be the unmirrored render's
//! at `W - 1 - x`. The fixture isn't symmetric, which the checks first make sure of.

use std::sync::Arc;

use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::contrib::builtin;
use klipolo_model::motion::Blend;
use klipolo_model::params::{ParamValue, PointN};
use klipolo_model::project::{BackgroundKind, FormatKind, Project};
use klipolo_model::timeline::{EffectInstance, ItemContent};

use super::support::*;
use super::{near, pixel, use_fixture};

const W: u32 = 640;
const H: u32 = 360;

/// Where the renders are compared: away from the frame's middle, on both halves, across the bars.
const POINTS: [(u32, u32); 6] = [
    (40, 60),
    (120, 180),
    (200, 300),
    (440, 60),
    (520, 180),
    (600, 300),
];

/// The bars as one full-frame layer over a mid-grey canvas (overlay blending over 50 % grey gives the layer back).
fn layer_over_grey(mirror: bool, effects: Vec<EffectInstance>, blend: Option<Blend>) -> Project {
    let mut p = project();
    use_fixture(&mut p, "broll", "bars-30fps-0.5gop.mp4", 4.0);
    p.timeline.main.clear();
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#808080".into();
    let content = ItemContent::Video {
        asset_id: "broll".into(),
        source_in_us: 0,
        speed: 1.0,
        volume: 1.0,
        muted: true,
        mirror,
        tie: None,
    };
    let mut layer = item("v", 0.0, 4.0, bx(0.0, 0.0, 1.0, 1.0), content);
    layer.effects = effects;
    layer.layout.blend = blend;
    p.timeline.tracks = vec![overlay_track("o", vec![layer])];
    p
}

/// A circle mask `x` across the layer's travel (0: touching its left edge, 1: its right).
fn circle_at(x: f64) -> Vec<EffectInstance> {
    let mask = EffectInstance {
        id: "m".into(),
        ..EffectInstance::new(builtin::EFFECT_MASK)
    };
    vec![
        mask.with("shape", ParamValue::Text("circle".into()))
            .with("center", ParamValue::Point(PointN { x, y: 0.5 })),
    ]
}

/// `a` is `b` seen in a mirror, at every compared point.
fn mirrors(a: &[u8], b: &[u8], stride: u32, what: &str) {
    for (x, y) in POINTS {
        let (got, want) = (pixel(a, stride, x, y), pixel(b, stride, W - 1 - x, y));
        assert!(
            near(got, want, 8),
            "{what}: ({x}, {y}) is {got:?}, its mirror {want:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU and scripts/gen-engine-fixtures.sh"]
async fn a_mirrored_video_layer_is_drawn_mirrored_plain_with_effects_and_blended() {
    for export in [false, true] {
        let mut backend = if export {
            GpuBackend::for_export().await.unwrap()
        } else {
            GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
                .await
                .unwrap()
                .wait_for_assets()
        };
        let mut render = async |p: Project| {
            let plan = frame_plan(
                &PlanContext::new(Arc::new(p)),
                secs(1.0),
                FormatKind::Landscape,
            );
            let request = RenderRequest {
                plan,
                width: W,
                height: H,
                output: Output::Rgba,
            };
            let frame = backend
                .render(request)
                .await
                .unwrap()
                .finish()
                .await
                .unwrap();
            assert!(frame.warnings.is_empty(), "{:?}", frame.warnings);
            frame
        };
        let side = if export { "export" } else { "preview" };
        let plain = render(layer_over_grey(false, Vec::new(), None)).await;
        let stride = plain.stride;
        // The bars differ between the halves at most points, so a picture that isn't flipped can't pass.
        let asymmetric = (POINTS.iter())
            .filter(|&&(x, y)| {
                let (l, r) = (
                    pixel(&plain.bytes, stride, x, y),
                    pixel(&plain.bytes, stride, W - 1 - x, y),
                );
                !near(l, r, 40)
            })
            .count();
        assert!(asymmetric >= 4, "{side}: the fixture looks symmetric");

        let mirrored = render(layer_over_grey(true, Vec::new(), None)).await;
        mirrors(
            &mirrored.bytes,
            &plain.bytes,
            stride,
            &format!("{side}, plain"),
        );

        // A mask on the left of the picture as shown is the mask on the right of the unmirrored one, mirrored: the
        // flip comes before the effects.
        let masked = render(layer_over_grey(true, circle_at(0.0), None)).await;
        let unmirrored = render(layer_over_grey(false, circle_at(1.0), None)).await;
        mirrors(
            &masked.bytes,
            &unmirrored.bytes,
            stride,
            &format!("{side}, with a mask"),
        );

        let blended = render(layer_over_grey(true, Vec::new(), Some(Blend::Overlay))).await;
        let unblended = render(layer_over_grey(false, Vec::new(), Some(Blend::Overlay))).await;
        mirrors(
            &blended.bytes,
            &unblended.bytes,
            stride,
            &format!("{side}, overlay-blended"),
        );
    }
}
