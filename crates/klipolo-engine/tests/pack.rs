//! Version folders: a folder loads only as it was saved, every file within its
//! size, hashed, never through a link, its names and id well formed and agreeing, its definition clean and within the
//! caps; the registry built from what loaded lists every definition but draws only what the caller
//! gave a shader; and the shader cache assembles a body once across rebuilds.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_engine::registry::key::{ContribSource, contrib_key};
use klipolo_engine::registry::pack::{
    LoadedContrib, PackError, PackLimits, ShaderCache, hex, load_version_dir,
};
use klipolo_engine::registry::{Registry, ShaderKind};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind, PaddingSpec};
use sha2::{Digest, Sha256};

#[path = "pack/embedded.rs"]
mod embedded;
#[path = "pack/text.rs"]
mod text;

const PLUGIN: &str = "lib.k3f9q2ma";
const BODY: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";

fn waves(local: &str) -> ContribDef {
    ContribDef {
        params: serde_json::from_str(
            r#"[{"key":"speed","label":"Speed","type":"float","min":0,"max":4,"default":1}]"#,
        )
        .unwrap(),
        ..ContribDef::new(
            format!("{PLUGIN}/{local}").as_str(),
            ContribKind::Generator,
            "Water waves",
        )
    }
}

/// `files` in `<root>/<plugin>/<local>/`, with their `hashes.json`.
fn folder(root: &Path, local: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let dir = root.join(PLUGIN).join(local);
    std::fs::create_dir_all(&dir).unwrap();
    let mut hashes = std::collections::BTreeMap::new();
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).unwrap();
        hashes.insert(name.to_string(), hex(&Sha256::digest(bytes)));
    }
    std::fs::write(
        dir.join("hashes.json"),
        serde_json::to_vec(&hashes).unwrap(),
    )
    .unwrap();
    dir
}

/// A generator's folder holding `def` and `body`.
fn version(root: &Path, local: &str, def: &ContribDef, body: &str) -> PathBuf {
    folder(
        root,
        local,
        &[
            ("contrib.json", serde_json::to_vec(def).unwrap()),
            ("effect.wgsl", body.as_bytes().to_vec()),
            ("thumb.jpg", vec![0xFF, 0xD8, 0xFF, 0xD9]),
        ],
    )
}

fn id(local: &str) -> ContribId {
    ContribId::new(PLUGIN, local)
}

fn load(dir: &Path, local: &str) -> Result<LoadedContrib, PackError> {
    load_version_dir(dir, &id(local), &PackLimits::DEFAULT)
}

const V1: &str = "generator.water-waves.v1";

#[test]
fn a_valid_folder_loads_under_the_key_of_its_definition_and_source() {
    let root = tempfile::tempdir().unwrap();
    let def = waves(V1);
    let dir = version(root.path(), V1, &def, BODY);
    let loaded = load(&dir, V1).unwrap();
    assert_eq!(loaded.def, def);
    assert_eq!(
        loaded.key,
        contrib_key(&def, &ContribSource::Wgsl(BODY.into()))
    );
    let names: Vec<&str> = loaded.bytes.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["contrib.json", "effect.wgsl", "hashes.json", "thumb.jpg"]
    );
    // The Finder's own file in a folder a person opened is never read and never refused.
    std::fs::write(dir.join(".DS_Store"), b"finder").unwrap();
    assert!(load(&dir, V1).is_ok());
}

#[test]
fn a_folder_that_isnt_as_saved_is_refused_with_its_reason() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();
    let kind = |e: Result<LoadedContrib, PackError>| -> String {
        format!("{:?}", e.unwrap_err())
            .split(['(', ' ', '{'])
            .next()
            .unwrap()
            .to_owned()
    };
    // An extra file, a 40 KiB shader, a file changed after hashing.
    let dir = version(r, V1, &waves(V1), BODY);
    std::fs::write(dir.join("notes.txt"), b"hi").unwrap();
    assert_eq!(kind(load(&dir, V1)), "Extra");
    let big = "generator.big.v1";
    let dir = version(r, big, &waves(big), &"/".repeat(40 << 10));
    assert_eq!(kind(load(&dir, big)), "TooLarge");
    let changed = "generator.changed.v1";
    let dir = version(r, changed, &waves(changed), BODY);
    std::fs::write(dir.join("effect.wgsl"), format!("{BODY}\n")).unwrap();
    assert_eq!(kind(load(&dir, changed)), "Hash");
    // Folder names that aren't well formed.
    for local in ["../x", "Generator.Waves.V1"] {
        let dir = r.join(PLUGIN).join(local);
        assert_eq!(kind(load(&dir, local)), "BadId", "{local}");
    }
    // A definition whose id differs from its folder; a protective one.
    let other = "generator.other.v1";
    let dir = version(r, other, &waves(V1), BODY);
    assert_eq!(kind(load(&dir, other)), "IdMismatch");
    let guarded = "generator.guarded.v1";
    let protective = ContribDef {
        protective: true,
        ..waves(guarded)
    };
    let dir = version(r, guarded, &protective, BODY);
    assert_eq!(kind(load(&dir, guarded)), "Protective");
    // Names left uncleaned are refused, never cleaned here.
    let unclean = "generator.unclean.v1";
    let def = ContribDef {
        name: "Wa\u{202E}ter".into(),
        ..waves(unclean)
    };
    let dir = version(r, unclean, &def, BODY);
    assert_eq!(kind(load(&dir, unclean)), "NotClean");
    // The definition caps: a table of 5 columns, a padding that grows with a setting that isn't a number.
    let table = "scene.table.v1";
    let def = ContribDef {
        kind: ContribKind::Scene,
        params: serde_json::from_str(
            r#"[{"key":"bars","label":"Bars","type":"table","columns":["a","b","c","d","e"],"maxRows":10,"default":[]}]"#,
        )
        .unwrap(),
        ..waves(table)
    };
    let dir = folder(
        r,
        table,
        &[
            ("contrib.json", serde_json::to_vec(&def).unwrap()),
            ("scene.json", br#"{"scene":1}"#.to_vec()),
        ],
    );
    let refused = load(&dir, table).unwrap_err();
    assert!(
        matches!(&refused, PackError::Definition(why) if why.contains("columns")),
        "{refused:?}"
    );
    let padded = "effect.padded.v1";
    let def = ContribDef {
        kind: ContribKind::Effect,
        padding: Some(PaddingSpec {
            px: 0.0,
            param: Some("mode".into()),
            per_unit: 1.0,
            max_px: 8.0,
        }),
        params: serde_json::from_str(
            r#"[{"key":"mode","label":"Mode","type":"choice","options":["a"],"default":"a"}]"#,
        )
        .unwrap(),
        ..waves(padded)
    };
    let dir = version(r, padded, &def, BODY);
    let refused = load(&dir, padded).unwrap_err();
    assert!(
        matches!(&refused, PackError::Definition(why) if why.contains("isn't a number setting")),
        "{refused:?}"
    );
}

/// No symbolic link at any level: the file, the version folder, the plugin folder.
#[cfg(unix)]
#[test]
fn a_link_at_any_level_is_refused() {
    use std::os::unix::fs::symlink;
    let kept = tempfile::tempdir().unwrap();
    let real = version(kept.path(), V1, &waves(V1), BODY);
    // The file.
    let root = tempfile::tempdir().unwrap();
    let dir = version(root.path(), V1, &waves(V1), BODY);
    std::fs::remove_file(dir.join("effect.wgsl")).unwrap();
    symlink(real.join("effect.wgsl"), dir.join("effect.wgsl")).unwrap();
    assert!(matches!(load(&dir, V1), Err(PackError::Link(_))));
    // The version folder.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(PLUGIN)).unwrap();
    let dir = root.path().join(PLUGIN).join(V1);
    symlink(&real, &dir).unwrap();
    assert!(matches!(load(&dir, V1), Err(PackError::Link(_))));
    // The plugin folder.
    let root = tempfile::tempdir().unwrap();
    symlink(kept.path().join(PLUGIN), root.path().join(PLUGIN)).unwrap();
    let dir = root.path().join(PLUGIN).join(V1);
    assert!(matches!(load(&dir, V1), Err(PackError::Link(_))));
}

#[test]
fn the_registry_lists_every_definition_and_draws_only_what_it_was_given() {
    let root = tempfile::tempdir().unwrap();
    let loaded: Vec<LoadedContrib> = ["generator.a.v1", "generator.b.v1"]
        .iter()
        .map(|local| load(&version(root.path(), local, &waves(local), BODY), local).unwrap())
        .collect();
    let cache = ShaderCache::new();
    let runnable = id("generator.a.v1");
    let shader_for = |c: &LoadedContrib| {
        (c.def.id == runnable)
            .then(|| cache.get_or_assemble(c, ShaderKind::Generator).ok())
            .flatten()
    };
    let registry =
        Registry::with_contributions(&Registry::builtin(), &loaded, &shader_for, &|_| None);
    for c in &loaded {
        assert!(registry.catalog().get(&c.def.id).is_some(), "{}", c.def.id);
    }
    assert!(registry.shader(&runnable).is_some());
    assert!(registry.shader(&id("generator.b.v1")).is_none());
    assert!(
        registry
            .shader(&ContribId::from(
                klipolo_model::contrib::builtin::GENERATOR_GRADIENT
            ))
            .is_some()
    );
}

/// A rebuild links the shaders the cache has; naga runs once a key, and `retain` lets go of the rest. `holds` says
/// what is kept without assembling anything.
#[test]
fn the_shader_cache_assembles_a_body_once_across_rebuilds() {
    let root = tempfile::tempdir().unwrap();
    let local = "generator.once.v1";
    let loaded = vec![load(&version(root.path(), local, &waves(local), BODY), local).unwrap()];
    let cache = ShaderCache::new();
    // `holds` answers without assembling (the app's edits build from memory only).
    assert!(!cache.holds(loaded[0].key, ShaderKind::Generator));
    let shader_for = |c: &LoadedContrib| cache.get_or_assemble(c, ShaderKind::Generator).ok();
    let base = Registry::builtin();
    let first = Registry::with_contributions(&base, &loaded, &shader_for, &|_| None);
    assert!(cache.holds(loaded[0].key, ShaderKind::Generator));
    assert!(!cache.holds(loaded[0].key, ShaderKind::Filter));
    let second = Registry::with_contributions(&base, &loaded, &shader_for, &|_| None);
    assert_eq!(cache.assembled(), 1);
    assert!(Arc::ptr_eq(
        first.shader(&id(local)).unwrap(),
        second.shader(&id(local)).unwrap()
    ));
    cache.retain(&HashSet::new());
    assert!(cache.is_empty());
}
