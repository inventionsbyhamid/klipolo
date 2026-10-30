//! The download store: removal by a downloaded family's id only, never outside its root; what a
//! quit or crash left is removed at start; a manifest whose files aren't all there isn't listed.

use klipolo_fonts::store::{DownloadedId, FontStore, MANIFEST, Manifest, ManifestFile};

#[test]
fn removal_takes_only_a_downloaded_family_and_never_leaves_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("keep.txt"), b"mine").unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    for family in ["../x", "..", "/etc", outside.path().to_str().unwrap()] {
        let id = DownloadedId {
            family: family.into(),
            version: "v1".into(),
        };
        assert!(store.remove(&id).is_err(), "{family}");
    }
    assert!(
        outside.path().join("keep.txt").exists(),
        "nothing outside the fonts folder is touched"
    );
}

#[test]
fn orphans_are_removed_at_start() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("testa/v1")).unwrap();
    std::fs::write(dir.path().join("testa/v1/Testa-400.ttf.part"), b"half").unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    assert_eq!(store.tidy().unwrap(), 1);
    assert!(!dir.path().join("testa/v1/Testa-400.ttf.part").exists());
}

/// A family as a finished download leaves it: Inter's Regular as Testa 400, and its manifest.
fn installed(root: &std::path::Path, family: &str, folder: &str) {
    let dir = root.join(folder).join("v1");
    std::fs::create_dir_all(&dir).unwrap();
    let inter = klipolo_fonts::bundled::bundled_dir()
        .unwrap()
        .join("Inter/Inter-Regular.ttf");
    let bytes = std::fs::copy(inter, dir.join("Testa-400.ttf")).unwrap();
    let m = Manifest {
        family: family.into(),
        version: "v1".into(),
        licence: "ofl".into(),
        coverage: 0,
        files: vec![ManifestFile {
            weight: 400,
            italic: false,
            file: "Testa-400.ttf".into(),
            bytes,
            sha256: "0".repeat(64),
        }],
        left_out: vec![],
    };
    std::fs::write(dir.join(MANIFEST), serde_json::to_vec(&m).unwrap()).unwrap();
}

#[test]
fn a_downloaded_family_is_listed_removed_by_its_id_and_never_by_a_folder_it_doesnt_own() {
    let dir = tempfile::tempdir().unwrap();
    installed(dir.path(), "Testa", "testa");
    // A manifest naming another family than its folder's isn't ours to list or remove.
    installed(dir.path(), "Other", "testa-2");
    let store = FontStore::open(dir.path().into()).unwrap();
    assert_eq!(
        store
            .list()
            .iter()
            .map(|m| m.family.as_str())
            .collect::<Vec<_>>(),
        ["Testa"]
    );
    // A file gone since: the family is incomplete, not listed.
    let other = DownloadedId {
        family: "Other".into(),
        version: "v1".into(),
    };
    assert!(store.remove(&other).is_err());
    assert!(dir.path().join("testa-2/v1").is_dir(), "untouched");
    let id = DownloadedId {
        family: "Testa".into(),
        version: "v1".into(),
    };
    store.remove(&id).unwrap();
    assert!(store.list().is_empty());
    assert!(!dir.path().join("testa").exists());
    // A file missing makes a manifest incomplete: never used, and nothing else of it deleted by itself
    // `tidy` removes only what a quit left half-done or no longer checks out; the next download fetches the rest).
    installed(dir.path(), "Testa", "testa");
    std::fs::write(dir.path().join("testa/v1/Testa-700.ttf.part"), b"half").unwrap();
    std::fs::remove_file(dir.path().join("testa/v1/Testa-400.ttf")).unwrap();
    assert!(store.list().is_empty());
    assert_eq!(store.tidy().unwrap(), 1, "the .part only");
    assert!(dir.path().join("testa/v1").join(MANIFEST).is_file());
}

/// A download a quit cut off: Inter's Regular as `Testa-400.ttf` in `<slug>/<version>`, recorded with its SHA-256 in
/// `partial.json`, no manifest.
fn unfinished(root: &std::path::Path, slug: &str, version: &str) -> std::path::PathBuf {
    use sha2::Digest;
    let dir = root.join(slug).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    let inter = klipolo_fonts::bundled::bundled_dir()
        .unwrap()
        .join("Inter/Inter-Regular.ttf");
    let bytes = std::fs::read(inter).unwrap();
    std::fs::write(dir.join("Testa-400.ttf"), &bytes).unwrap();
    let record = ManifestFile {
        weight: 400,
        italic: false,
        file: "Testa-400.ttf".into(),
        bytes: bytes.len() as u64,
        sha256: hex::encode(sha2::Sha256::digest(&bytes)),
    };
    std::fs::write(
        dir.join(klipolo_fonts::store::PARTIAL),
        serde_json::to_vec(&vec![record]).unwrap(),
    )
    .unwrap();
    dir
}

/// An unfinished download at a version Google's catalogue no longer offers can never resume, so
/// start-up's tidy removes it whole; one still offered keeps its finished face for the next try.
#[test]
fn tidy_removes_unfinished_downloads_at_versions_no_longer_offered() {
    let dir = tempfile::tempdir().unwrap();
    let gone = unfinished(dir.path(), "testa", "v1");
    let offered = unfinished(dir.path(), "testa", "v2");
    let store = FontStore::open(dir.path().into()).unwrap();
    store.tidy_keeping([("Testa", "v2")]).unwrap();
    assert!(!gone.exists(), "v1 can't be resumed: gone");
    assert!(
        offered.join("Testa-400.ttf").is_file(),
        "v2's finished face stays"
    );
}
