//! Guards the TS contract: run after `cargo test -p klipolo-model export_bindings`. And the `scene@1` JSON Schema, which
//! `authoringGuide` serves to agents, against its golden (`UPDATE_GOLDEN=1` rewrites it).
mod common;

use std::fs;
use std::path::PathBuf;

fn bindings_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/bindings")
}

#[test]
fn generated_types_use_numbers_and_camel_case() {
    let dir = bindings_dir();
    let read = |name: &str| {
        fs::read_to_string(dir.join(name))
            .unwrap_or_else(|_| panic!("{name} not generated; run `pnpm bindings`"))
    };
    let asset = read("Asset.ts");
    assert!(asset.contains("durationUs: number"), "{asset}");
    let op = read("Op.ts");
    assert!(op.contains("deleteRanges"), "{op}");
    assert!(op.contains("atUs: number"), "{op}");
    assert!(op.contains("clipId"), "{op}");
    assert!(op.contains("addEffect"), "{op}");
    let id = read("ContribId.ts");
    assert!(id.contains("= string"), "{id}");
    let effect = read("EffectInstance.ts");
    assert!(effect.contains("params"), "{effect}");
    let content = read("ItemContent.ts");
    assert!(
        content.contains("sourceInUs: number") && !content.contains("animIn"),
        "{content}"
    );
    let patch = read("ClipPatch.ts");
    assert!(patch.contains("speed?: number"), "{patch}");
    assert!(!read("Project.ts").contains("bigint") && !op.contains("bigint"));
}

#[test]
fn the_scene_schema_is_its_golden() {
    let schema = serde_json::to_value(klipolo_model::scene::json_schema()).unwrap();
    let text = schema.to_string();
    assert!(
        text.contains("fromTo") && text.contains("strokeWidth") && text.contains("family"),
        "{text}"
    );
    common::assert_golden(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/scene-1.schema.json"),
        &common::sorted_keys(schema),
    );
}
