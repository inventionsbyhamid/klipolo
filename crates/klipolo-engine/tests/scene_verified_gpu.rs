//! Verified scenes on the GPU (lavapipe in CI): every registry the snapshot builds for a scene this
//! computer checked shares its compiled scene (`CompiledScenes`, by key), so a rebuild (each edit, check or switch
//! publishes one) keeps the drawer's instance and its held target: nothing is built again.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::registry::pack_scene::CompiledScenes;
use support::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_rebuilt_registry_keeps_the_scenes_instance() {
    let _gpu = gpu_turn().await;
    let c = loaded_scene("scene.lower-third.v1", "Lower third", LOWER_THIRD);
    let scenes = CompiledScenes::new();
    let verdict = light_verdict(Default::default());
    let p = Arc::new(project_showing(&c.def.id, 4.0, &[]));
    let mut h = Headless::new(snapshot_registry(&c, Some(&verdict), &scenes))
        .await
        .unwrap();
    let (first, _) = scene_frame(&mut h, &p, 1.5, H_SIZE).await;
    assert_eq!(h.scene_stats().builds, 1);
    for (i, s) in [1.6, 1.7, 1.8].into_iter().enumerate() {
        // As the snapshot publishes after an edit, a check or the switch: a new registry, the same scene.
        h.set_registry(snapshot_registry(&c, Some(&verdict), &scenes));
        let (img, _) = scene_frame(&mut h, &p, s, H_SIZE).await;
        assert_eq!(
            h.scene_stats().builds,
            1,
            "rebuild {i}: no instance built again"
        );
        assert_eq!(img, first, "held at rest");
    }
    assert_eq!(scenes.compiled(), 1);
}
