//! A draft's audit objects: made once per variant, never shared by
//! two draws at once, and dropped when a draw fails, so a readback left pending never reaches another draw.

use klipolo_engine::make::audit::{AuditDraw, audit, trips_per_pixel};
use klipolo_engine::make::draft::DraftLayer;

use crate::support::*;

/// `layer`'s audit draw at `at` s, with its own cache.
fn draw_of(layer: &DraftLayer, at: f32) -> AuditDraw<'_> {
    AuditDraw {
        effect: layer.shader().unwrap(),
        params: klipolo_engine::plan::pack_params(layer.shader().unwrap(), &layer.params),
        size: (128, 72),
        at,
        duration: 4.0,
        cache: Some(&layer.audit),
    }
}

/// A draft's audit variants are compiled once and kept, whatever the draws.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_drafts_audit_variants_are_compiled_once() {
    let h = headless_alone().await;
    let layer = draft(draft_def(), WAVES_BODY);
    for at in [0.0, 1.0, 2.0] {
        let draw = draw_of(&layer, at);
        audit(h.device(), &draw).await.unwrap();
        trips_per_pixel(h.device(), &draw).await.unwrap();
    }
    assert_eq!(layer.audit.made(), 2, "raw and trips, once each");
}

/// An audit draw that fails (here as one whose readback timed out, its map left pending) drops the draft's
/// audit objects, so the next draw makes them again and the device takes no fault.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_failed_audit_draw_never_poisons_the_next() {
    let h = headless_alone().await;
    let layer = draft(draft_def(), WAVES_BODY);
    audit(h.device(), &draw_of(&layer, 0.0)).await.unwrap();
    layer.audit.fail_next_read();
    assert!(audit(h.device(), &draw_of(&layer, 1.0)).await.is_err());
    let next = audit(h.device(), &draw_of(&layer, 2.0)).await;
    assert!(next.is_ok(), "{next:?}");
    assert_eq!(layer.audit.made(), 2, "made again after the failure");
    assert_eq!(h.device().take_fault(), None);
}

/// Two audit draws of one draft at once never share its objects: while the first is held at its readback
/// (submitted, its map requested), the second makes its own, and both answer. The hold makes the overlap certain
///.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn two_audit_draws_at_once_never_share_their_objects() {
    let h = headless_alone().await;
    let layer = draft(draft_def(), WAVES_BODY);
    audit(h.device(), &draw_of(&layer, 0.0)).await.unwrap();
    let release = layer.audit.hold_next_read();
    let (first, second) = (draw_of(&layer, 1.0), draw_of(&layer, 2.0));
    let mut a = Box::pin(audit(h.device(), &first));
    assert!(
        futures::poll!(a.as_mut()).is_pending(),
        "held at its readback"
    );
    let b = audit(h.device(), &second).await;
    release.send(()).unwrap();
    let a = a.await;
    assert!(a.is_ok() && b.is_ok(), "{a:?} {b:?}");
    assert_eq!(h.device().take_fault(), None);
    assert_eq!(layer.audit.made(), 2, "the second made its own");
}
