//! Helpers for scenes that aren't built in: a library scene as a reader loads it, a verdict as
//! this computer's check gives one, and the registry the snapshot builds from them (`compile_verified` through a
//! `CompiledScenes`, only for a scene with a verdict).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use klipolo_engine::make::budget::{Cost, CostClass, DecidedBy, ExportMs, Pcts};
use klipolo_engine::make::envelope::Envelope;
use klipolo_engine::registry::Registry;
use klipolo_engine::registry::pack::{LoadedContrib, Verdict, hex, load_files};
use klipolo_engine::registry::pack_scene::{CompiledScenes, VerifiedScene};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind};
use klipolo_model::params::ParamValue;
use klipolo_model::project::Project;
use klipolo_model::timeline::ItemContent;
use sha2::{Digest, Sha256};

use super::super::{bx, item, overlay_track};

/// Library scene `local` (`lib.k3f9q2ma/<local>`) named `name`, holding `json`, as a reader loads its version folder:
/// its definition's settings are the scene's.
pub fn loaded_scene(local: &str, name: &str, json: &str) -> LoadedContrib {
    let id = ContribId::new("lib.k3f9q2ma", local);
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let def = ContribDef {
        params: serde_json::from_value(v["params"].clone()).unwrap_or_default(),
        ..ContribDef::new(id.as_str(), ContribKind::Scene, name)
    };
    let mut files = BTreeMap::new();
    files.insert(
        "contrib.json".to_string(),
        serde_json::to_vec(&def).unwrap(),
    );
    files.insert("scene.json".to_string(), json.as_bytes().to_vec());
    let hashes: BTreeMap<&String, String> = files
        .iter()
        .map(|(n, b)| (n, hex(&Sha256::digest(b))))
        .collect();
    let hashes = serde_json::to_vec(&hashes).unwrap();
    files.insert("hashes.json".to_string(), hashes);
    load_files(&id, files, PathBuf::new()).unwrap_or_else(|e| panic!("{local}: {e}"))
}

/// A Light verdict measured within `envelope`.
pub fn light_verdict(envelope: Envelope) -> Verdict {
    Verdict::of(Cost {
        preview_ms: Pcts::flat(0.3),
        export_ms: ExportMs {
            p1080: 0.6,
            p2160: 2.4,
        },
        cpu_us: None,
        gpu_bytes: Default::default(),
        class: CostClass::Light,
        decided_ms: 0.3,
        at_half: false,
        decided_by: DecidedBy::Time,
        budget_ms: 0.5,
        runs_at: Default::default(),
        resolved: None,
        envelope,
        machine: "vulkan|llvmpipe|test".into(),
        advice: Vec::new(),
        cut: false,
        fenced: None,
    })
    .unwrap()
}

/// An envelope that measured number setting `key` from `lo` to `hi`.
pub fn range_envelope(key: &str, lo: f64, hi: f64) -> Envelope {
    let mut envelope = Envelope::default();
    envelope.ranges.insert(key.into(), (lo, hi));
    envelope
}

/// The registry the snapshot builds for `c`: its definition in the catalogue, and its scene drawn only with `verdict`,
/// compiled through `scenes` (its key's compiled scene, compiled once).
pub fn snapshot_registry(
    c: &LoadedContrib,
    verdict: Option<&Verdict>,
    scenes: &CompiledScenes,
) -> Arc<Registry> {
    let base = Registry::with_definitions(&Registry::builtin(), [&c.def]);
    let scene_for = |c: &LoadedContrib| {
        let verdict = verdict?;
        let scene = scenes.get_or_compile(c, base.catalog(), 0).ok()?;
        Some(VerifiedScene::new(scene, verdict))
    };
    Arc::new(Registry::with_contributions(
        &base,
        [c],
        &|_| None,
        &scene_for,
    ))
}

/// A project of one full-frame overlay layer showing contribution `id` with `params`, from 0 for `duration` seconds.
pub fn project_showing(id: &ContribId, duration: f64, params: &[(&str, f64)]) -> Project {
    let mut p = Project::new("Scene");
    p.id = "scene-project".into();
    let params = params
        .iter()
        .map(|(k, v)| (k.to_string(), ParamValue::Number(*v)))
        .collect();
    let content = ItemContent::generator(id.clone(), params);
    p.timeline.tracks.push(overlay_track(
        "titles",
        vec![item(
            "layer",
            0.0,
            duration,
            bx(0.0, 0.0, 1.0, 1.0),
            content,
        )],
    ));
    p
}
