//! A project's copies: read through
//! folder handles, never through a link at any level, within their caps (64 contributions, 4 MiB); a folder that
//! doesn't load is named with why, and the rest load.

use klipolo_engine::registry::pack::{
    Embedded, MAX_EMBEDDED_BYTES, PackError, PackLimits, read_embedded_dir,
};

use super::*;

#[test]
fn a_projects_copies_load_and_what_does_not_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    version(
        &contrib,
        "generator.waves.v1",
        &waves("generator.waves.v1"),
        BODY,
    );
    // A copy being written, and the Finder's own file, are passed over.
    std::fs::create_dir_all(contrib.join(PLUGIN).join(".tmp-generator.waves.v2")).unwrap();
    std::fs::write(contrib.join(".DS_Store"), b"x").unwrap();
    // A file changed after it was hashed is named.
    let changed = version(
        &contrib,
        "generator.waves.v3",
        &waves("generator.waves.v3"),
        BODY,
    );
    std::fs::write(
        changed.join("effect.wgsl"),
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(0.0); }",
    )
    .unwrap();
    let found = read_embedded_dir(&contrib, &PackLimits::DEFAULT);
    let ids: Vec<&str> = found.loaded.iter().map(|c| c.def.id.as_str()).collect();
    assert_eq!(ids, [format!("{PLUGIN}/generator.waves.v1").as_str()]);
    assert_eq!(found.refused.len(), 1);
    assert!(matches!(found.refused[0].why, PackError::Hash(_)));
    assert!(found.refused[0].folder.contains("generator.waves.v3"));
    // No folder holds nothing.
    let none = read_embedded_dir(&dir.path().join("none"), &PackLimits::DEFAULT);
    assert_eq!(none, Embedded::default());
}

/// `contrib/`, a plugin folder, a version folder and a file are each refused when they are links, and nothing
/// is read through them.
#[cfg(unix)]
#[test]
fn a_link_at_any_level_of_a_projects_copies_is_refused() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    let real_version = version(
        &real,
        "generator.waves.v1",
        &waves("generator.waves.v1"),
        BODY,
    );
    let link = |at: &str| {
        let contrib = dir.path().join(at).join("contrib");
        std::fs::create_dir_all(&contrib).unwrap();
        contrib
    };
    let refused = |contrib: &Path| {
        let found = read_embedded_dir(contrib, &PackLimits::DEFAULT);
        found.loaded.is_empty()
            && matches!(
                found.refused.first().map(|r| &r.why),
                Some(PackError::Link(_))
            )
    };
    // `contrib/` itself.
    let p1 = dir.path().join("p1");
    std::fs::create_dir_all(&p1).unwrap();
    symlink(&real, p1.join("contrib")).unwrap();
    assert!(refused(&p1.join("contrib")), "contrib/");
    // The plugin's folder.
    let p2 = link("p2");
    symlink(real.join(PLUGIN), p2.join(PLUGIN)).unwrap();
    assert!(refused(&p2), "the plugin's folder");
    // The version's folder.
    let p3 = link("p3");
    std::fs::create_dir_all(p3.join(PLUGIN)).unwrap();
    symlink(&real_version, p3.join(PLUGIN).join("generator.waves.v1")).unwrap();
    assert!(refused(&p3), "the version's folder");
    // One of its files.
    let p4 = link("p4");
    let v = version(
        &p4,
        "generator.waves.v1",
        &waves("generator.waves.v1"),
        BODY,
    );
    std::fs::remove_file(v.join("effect.wgsl")).unwrap();
    symlink(real_version.join("effect.wgsl"), v.join("effect.wgsl")).unwrap();
    assert!(refused(&p4), "a file");
}

/// At most 64 contributions are read (the rest named), and at most 4 MiB in all.
#[test]
fn a_projects_copies_are_capped_in_number_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    for n in 0..66 {
        let local = format!("generator.w{n:02}.v1");
        version(&contrib, &local, &waves(&local), BODY);
    }
    let found = read_embedded_dir(&contrib, &PackLimits::DEFAULT);
    assert_eq!(found.loaded.len(), 64);
    assert!(
        found
            .refused
            .iter()
            .all(|r| matches!(r.why, PackError::Extra(_))),
        "{:?}",
        found.refused
    );
    // The listing of `contrib/` itself stops after 64 entries, the rest named.
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    for n in 0..70 {
        std::fs::create_dir_all(contrib.join(format!("lib.p{n:02}"))).unwrap();
    }
    let found = read_embedded_dir(&contrib, &PackLimits::DEFAULT);
    assert!(found.loaded.is_empty());
    assert!(
        found
            .refused
            .iter()
            .any(|r| r.folder.contains("contrib") && matches!(r.why, PackError::Extra(_))),
        "{:?}",
        found.refused
    );
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    let thumb = vec![7u8; 250 << 10];
    let fits = (MAX_EMBEDDED_BYTES / (250 << 10)) as usize;
    for n in 0..fits + 1 {
        let local = format!("generator.t{n:02}.v1");
        let def = waves(&local);
        folder(
            &contrib,
            &local,
            &[
                ("contrib.json", serde_json::to_vec(&def).unwrap()),
                ("effect.wgsl", BODY.as_bytes().to_vec()),
                ("thumb.jpg", thumb.clone()),
            ],
        );
    }
    let found = read_embedded_dir(&contrib, &PackLimits::DEFAULT);
    assert!(found.loaded.len() < fits + 1);
    assert!(found.refused.iter().any(|r| matches!(
        &r.why,
        PackError::TooLarge { limit, .. } if *limit == MAX_EMBEDDED_BYTES
    )));
}

/// At the loader, where crafted projects first arrive: a setting key the worker's taming would change, or one
/// over 64 characters, is refused; the same text as a label loads.
#[test]
fn a_setting_key_the_worker_would_tame_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let contrib = dir.path().join("contrib");
    let with = |local: &str, key: &str, label: &str| {
        let mut def = waves(local);
        def.params = serde_json::from_value(serde_json::json!([
            {"key": key, "label": label, "type": "float", "min": 0, "max": 4, "default": 1}
        ]))
        .unwrap();
        version(&contrib, local, &def, BODY);
    };
    with("generator.a.v1", "a\u{200D}b", "Speed");
    with("generator.b.v1", "speed", "a\u{200D}b");
    with("generator.c.v1", &"k".repeat(65), "Speed");
    let found = read_embedded_dir(&contrib, &PackLimits::DEFAULT);
    let ids: Vec<&str> = found.loaded.iter().map(|c| c.def.id.as_str()).collect();
    assert_eq!(ids, [format!("{PLUGIN}/generator.b.v1").as_str()]);
    assert!(
        found
            .refused
            .iter()
            .all(|r| matches!(r.why, PackError::Hidden(_)))
    );
}
