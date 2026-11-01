//! `Verify`: the worker loads the folder the app wrote into its `in/` through the one folder reader, checks and
//! measures it as `Compile` does and answers with the key it measured and the hardened text's hash; a folder that
//! doesn't read is refused before anything runs, and a scene folder is checked as a submitted scene is. Its machine key
//! and hardened text's hash are the app's own.

use std::collections::BTreeMap;
use std::path::PathBuf;

use klipolo_engine::make::budget::this_computer;
use klipolo_engine::make::draft::shader_kind;
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::make::protocol::WorkerReply;
use klipolo_engine::registry::key::{ContribSource, contrib_key};
use klipolo_engine::registry::pack::{ShaderCache, hex, load_files};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind};
use sha2::{Digest, Sha256};

use super::*;

/// A version folder's files for `def` and `body`, with their `hashes.json`.
fn files(def: &ContribDef, source: (&str, &[u8])) -> Vec<(String, Vec<u8>)> {
    let mut files = BTreeMap::new();
    files.insert("contrib.json".to_string(), serde_json::to_vec(def).unwrap());
    files.insert(source.0.to_string(), source.1.to_vec());
    let hashes: BTreeMap<&String, String> = files
        .iter()
        .map(|(n, b)| (n, hex(&Sha256::digest(b))))
        .collect();
    let hashes = serde_json::to_vec(&hashes).unwrap();
    files.insert("hashes.json".to_string(), hashes);
    files.into_iter().collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn verify_checks_the_folder_the_app_sent_and_answers_its_key() {
    let mut w = InProcessWorker::new(FakeMeter::new(2.0)).await;
    let def = ContribDef {
        id: ContribId::new("lib.k3f9q2ma", "generator.test.v1"),
        ..draft_def()
    };
    let sent = files(&def, ("effect.wgsl", WAVES_BODY.as_bytes()));
    let lines = w.verify(&def.id, &sent).await;
    let reply: WorkerReply = serde_json::from_str(lines.last().unwrap()).unwrap();
    let WorkerReply::Compiled {
        ok: true,
        cost: Some(cost),
        key: Some(key),
        hardened: Some(hardened),
        ..
    } = reply
    else {
        panic!("{lines:?}")
    };
    let expected = contrib_key(&def, &ContribSource::Wgsl(WAVES_BODY.into()));
    assert_eq!(key.0, expected, "the key of what it read");
    // The two equalities the app rests on: the worker's machine key is the one the app keys its
    // verified store with, and the hardened text it measured is the one the app makes of the same folder.
    let here = this_computer().await.expect("an adapter");
    assert_eq!(cost.machine, here.as_str());
    let loaded = load_files(&def.id, sent.iter().cloned().collect(), PathBuf::new()).unwrap();
    let kind = shader_kind(def.kind).unwrap();
    let made = ShaderCache::new().get_or_assemble(&loaded, kind).unwrap();
    let made: [u8; 32] = Sha256::digest(made.source.as_bytes()).into();
    assert_eq!(hardened.0, made, "the app's hardened text is the worker's");
    assert!(cost.preview_ms.p95 > 0.0);
    assert!(lines.iter().any(|l| l.contains("\"thumbnail\"")));
    // A file changed after it was hashed: refused before anything runs.
    let mut changed = sent.clone();
    changed[1].1 = b"fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }".to_vec();
    let lines = w.verify(&def.id, &changed).await;
    assert!(lines.last().unwrap().contains("\"failed\""), "{lines:?}");
    assert!(lines.last().unwrap().contains("doesn't match its hash"));
    // A scene folder is checked as a submitted scene is: one that isn't a scene is refused by its path.
    let scene = ContribDef::new("lib.k3f9q2ma/scene.title.v1", ContribKind::Scene, "Title");
    let lines = w
        .verify(&scene.id, &files(&scene, ("scene.json", b"{}")))
        .await;
    let last = lines.last().unwrap();
    assert!(
        last.contains("\"ok\":false") && last.contains("\"pointer\":\"/scene\""),
        "{lines:?}"
    );
}
