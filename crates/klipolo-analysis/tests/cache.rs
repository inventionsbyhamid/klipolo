//! The per-file analysis cache: a key from the file's content (not its name or place), and JSON files stored under it.

use std::collections::BTreeMap;

use klipolo_analysis::cache::{self, Dir, SCHEMA};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MIB: usize = 1024 * 1024;

/// `len` bytes that differ along the file (a small generator, no rand crate).
fn bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 56) as u8
        })
        .collect()
}

#[test]
fn content_key_is_stable_and_changes_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mov");
    let mut data = bytes(10 * MIB, 1);
    std::fs::write(&path, &data).unwrap();
    let key = cache::content_key(&path).unwrap();
    assert_eq!(key.len(), 64);
    assert!(
        key.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{key}"
    );
    assert_eq!(cache::content_key(&path).unwrap(), key, "stable");

    // SHA-256 of the size (8 bytes, little-endian), the first 4 MiB and the last 4 MiB.
    let mut hash = Sha256::new();
    hash.update((data.len() as u64).to_le_bytes());
    hash.update(&data[..4 * MIB]);
    hash.update(&data[data.len() - 4 * MIB..]);
    let expected: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(key, expected);

    // The same bytes under another name and folder: the same key (a moved or copied file finds its analysis).
    let moved = dir.path().join("elsewhere");
    std::fs::create_dir(&moved).unwrap();
    std::fs::copy(&path, moved.join("renamed.mp4")).unwrap();
    assert_eq!(cache::content_key(&moved.join("renamed.mp4")).unwrap(), key);

    // A byte changed near the start or near the end, or one more byte: another key.
    let key_after = |data: &[u8]| {
        std::fs::write(&path, data).unwrap();
        cache::content_key(&path).unwrap()
    };
    data[MIB] ^= 1;
    assert_ne!(key_after(&data), key, "a change in the first 4 MiB");
    data[MIB] ^= 1;
    let last = data.len() - 1;
    data[last] ^= 1;
    assert_ne!(key_after(&data), key, "a change in the last 4 MiB");
    data[last] ^= 1;
    data.push(0);
    assert_ne!(key_after(&data), key, "a longer file");
    data.pop();
    // By design the middle isn't read (a long clip keys in two reads): a change there keeps the key.
    data[5 * MIB] ^= 1;
    assert_eq!(key_after(&data), key, "the middle is not part of the key");

    // A file smaller than 4 MiB is read whole, and an empty one has a key too.
    let small = dir.path().join("small.wav");
    std::fs::write(&small, b"tiny").unwrap();
    let small_key = cache::content_key(&small).unwrap();
    std::fs::write(&small, b"tinY").unwrap();
    assert_ne!(cache::content_key(&small).unwrap(), small_key);
    std::fs::write(&small, b"").unwrap();
    assert_eq!(cache::content_key(&small).unwrap().len(), 64);

    assert!(cache::content_key(&dir.path().join("gone.mov")).is_err());
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Words {
    words: Vec<String>,
    seconds: f64,
}

fn words() -> Words {
    Words {
        words: vec!["hello".into(), "there".into()],
        seconds: 1.5,
    }
}

#[test]
fn a_stored_file_loads_back_from_its_key_folder() {
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "abc123");
    assert_eq!(dir.path(), root.path().join("abc123"));
    assert_eq!(dir.load::<Words>("transcript"), None, "nothing stored yet");
    dir.store("transcript", &words()).unwrap();
    assert_eq!(dir.load::<Words>("transcript"), Some(words()));
    assert_eq!(
        Dir::new(root.path(), "abc123").load::<Words>("transcript"),
        Some(words())
    );
    assert_eq!(
        Dir::new(root.path(), "other").load::<Words>("transcript"),
        None
    );

    // `<root>/<key>/<name>.json` beside `meta.json`, and no temporary file left.
    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["meta.json", "transcript.json"]);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["schema"], 1);
    assert_eq!(SCHEMA, 1);
    let created = meta["createdAt"].as_str().unwrap();
    assert!(created.len() == 20 && created.ends_with('Z'), "{created}");

    // Storing again replaces the file.
    let mut changed = words();
    changed.seconds = 2.0;
    dir.store("transcript", &changed).unwrap();
    assert_eq!(dir.load::<Words>("transcript"), Some(changed));
}

#[test]
fn a_newer_schema_loads_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    dir.store("transcript", &words()).unwrap();
    std::fs::write(
        dir.path().join("meta.json"),
        br#"{"schema": 2, "createdAt": "2027-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    assert_eq!(dir.load::<Words>("transcript"), None);
    // Nor does a folder without its meta.json, or with one that doesn't read.
    std::fs::remove_file(dir.path().join("meta.json")).unwrap();
    assert_eq!(dir.load::<Words>("transcript"), None);
    std::fs::write(dir.path().join("meta.json"), b"{").unwrap();
    assert_eq!(dir.load::<Words>("transcript"), None);
}

#[test]
fn a_damaged_or_different_file_loads_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    dir.store("transcript", &words()).unwrap();
    std::fs::write(dir.path().join("transcript.json"), b"{\"words\": [").unwrap();
    assert_eq!(dir.load::<Words>("transcript"), None);
    dir.store("transcript", &vec![1, 2, 3]).unwrap();
    assert_eq!(dir.load::<Words>("transcript"), None);
}

#[test]
fn storing_over_an_older_or_damaged_folder_starts_it_again() {
    // An older version's folder, or a meta.json that doesn't parse: its files are dropped, so ours never sit beside
    // files of another shape.
    for old_meta in [&br#"{"schema": 0}"#[..], b"{"] {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::new(root.path(), "k");
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("meta.json"), old_meta).unwrap();
        std::fs::write(dir.path().join("scenes.json"), b"{}").unwrap();
        dir.store("transcript", &words()).unwrap();
        assert!(!dir.path().join("scenes.json").exists());
        assert_eq!(dir.load::<Words>("transcript"), Some(words()));
        assert_eq!(dir.meta().unwrap().schema, 1);
    }
}

#[test]
fn a_newer_schema_folder_is_never_overwritten() {
    // A newer app's results stay as they are: this version neither reads nor replaces them.
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    std::fs::create_dir_all(dir.path()).unwrap();
    std::fs::write(dir.path().join("meta.json"), br#"{"schema": 2}"#).unwrap();
    std::fs::write(dir.path().join("scenes.json"), b"{}").unwrap();
    assert!(dir.store("transcript", &words()).is_err());
    assert!(dir.set_model("scenes", "muse-spark-1.3").is_err());
    assert!(dir.path().join("scenes.json").exists());
    assert!(!dir.path().join("transcript.json").exists());
    assert_eq!(
        std::fs::read(dir.path().join("meta.json")).unwrap(),
        br#"{"schema": 2}"#
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_meta_is_never_taken_for_a_damaged_one() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    dir.store("transcript", &words()).unwrap();
    let meta = dir.path().join("meta.json");
    std::fs::set_permissions(&meta, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway: nothing to check then.
    let unreadable = std::fs::read(&meta).is_err();
    let stored = dir.store("scenes", &words());
    std::fs::set_permissions(&meta, std::fs::Permissions::from_mode(0o644)).unwrap();
    if unreadable {
        assert!(stored.is_err(), "a read error is not a damaged file");
        assert_eq!(
            dir.load::<Words>("transcript"),
            Some(words()),
            "nothing was dropped"
        );
    }
}

#[test]
fn model_ids_are_kept_in_meta() {
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    assert!(dir.meta().is_none());
    dir.store("transcript", &words()).unwrap();
    dir.set_model("transcript", "muse-voice-transcribe-1.0")
        .unwrap();
    dir.store("scenes", &words()).unwrap();
    dir.set_model("scenes", "muse-spark-1.3").unwrap();
    let models: BTreeMap<String, String> = dir.meta().unwrap().models;
    assert_eq!(models["transcript"], "muse-voice-transcribe-1.0");
    assert_eq!(models["scenes"], "muse-spark-1.3");
}

#[test]
fn names_stay_inside_the_folder() {
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    for name in ["meta", "../escape", "a/b", "", ".hidden"] {
        assert!(dir.store(name, &words()).is_err(), "{name:?}");
        assert_eq!(dir.load::<Words>(name), None, "{name:?}");
    }
    assert!(!root.path().join("escape.json").exists());
}

#[test]
fn two_writers_in_one_process_never_lose_a_result_or_a_model() {
    // The app runs one clip's transcript and scene steps at once: each stores its result and notes its model.
    let root = tempfile::tempdir().unwrap();
    let dir = Dir::new(root.path(), "k");
    let jobs = [
        ("transcript", "muse-voice-transcribe-1.0"),
        ("scenes", "muse-spark-1.3"),
    ];
    std::thread::scope(|s| {
        for (name, model) in jobs {
            let dir = &dir;
            s.spawn(move || {
                for i in 0..1000 {
                    dir.store(name, &i).unwrap();
                    dir.set_model(name, model).unwrap();
                    assert!(dir.meta().is_some(), "meta.json reads after {name} {i}");
                }
            });
        }
    });
    let meta = dir.meta().unwrap();
    for (name, model) in jobs {
        assert_eq!(meta.models[name], model);
        assert_eq!(dir.load::<i32>(name), Some(999));
    }
}
