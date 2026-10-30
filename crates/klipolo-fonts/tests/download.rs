//! Google Fonts downloads against a local server: whole and validated with the manifest last; cut off
//! halfway, the finished files stay and the rest is fetched; a file that isn't the face named is refused; a full disk
//! stops with the files kept; only the two allowed hosts are asked. Nothing here reaches Google.

#![cfg(feature = "test-hosts")]

mod common;
use std::sync::atomic::AtomicBool;

use common::server::{Server, family_on};
use klipolo_fonts::download::{FontError, Which, download_family};
use klipolo_fonts::store::FontStore;

#[test]
fn a_family_downloads_whole_validated_and_its_manifest_comes_last() {
    let server = Server::start();
    let fam = family_on(
        &server,
        "Testa",
        &[
            (400, false, "Inter/Inter-Regular.ttf"),
            (700, false, "Inter/Inter-Bold.ttf"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let m = download_family(
        &store,
        &fam,
        Which::All,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(m.files.len(), 2);
    assert!(m.files.iter().all(|f| f.sha256.len() == 64));
    assert_eq!(store.list().len(), 1);
    assert!(
        dir.path()
            .join("testa")
            .join(&fam.version)
            .join("OFL.txt")
            .is_file(),
        "its licence is kept"
    );
}

#[test]
fn a_family_cut_off_halfway_keeps_its_finished_files_and_resumes() {
    let server = Server::start();
    let fam = family_on(
        &server,
        "Testa",
        &[
            (400, false, "Inter/Inter-Regular.ttf"),
            (700, false, "Inter/Inter-Bold.ttf"),
        ],
    );
    server.drop_after_bytes("Inter-Bold", 10_000);
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    assert!(matches!(
        download_family(
            &store,
            &fam,
            Which::All,
            &|_, _| {},
            &AtomicBool::new(false)
        ),
        Err(FontError::Offline)
    ));
    assert!(
        store.list().is_empty(),
        "no manifest: nothing half-installed is used"
    );
    server.heal();
    let before = server.requests("Inter-Regular");
    download_family(
        &store,
        &fam,
        Which::All,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        server.requests("Inter-Regular"),
        before,
        "the finished file isn't fetched again"
    );
}

#[test]
fn a_file_that_is_not_the_named_face_is_refused() {
    let server = Server::start();
    // Says 700, serves the 400 file.
    let fam = family_on(&server, "Testa", &[(700, false, "Inter/Inter-Regular.ttf")]);
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let err = download_family(
        &store,
        &fam,
        Which::All,
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(err, FontError::BadFile(_)));
    assert!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .all(|e| !e.path().to_string_lossy().ends_with(".ttf"))
    );
    // Nor in its folder: neither the file nor its `.part`.
    let folder = dir.path().join("testa").join(&fam.version);
    assert!(
        std::fs::read_dir(&folder)
            .map(|d| d
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains(".ttf")))
            .unwrap_or(true)
    );
}

#[test]
fn a_full_disk_stops_with_the_files_kept() {
    let server = Server::start();
    let fam = family_on(
        &server,
        "Testa",
        &[
            (400, false, "Inter/Inter-Regular.ttf"),
            (700, false, "Inter/Inter-Bold.ttf"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    // The writer answers ENOSPC past 4 KB of the second file (a `test-hosts`-only hook in download.rs's writer).
    klipolo_fonts::download::test_disk_full_after("Testa-700", 4_096);
    assert!(matches!(
        download_family(
            &store,
            &fam,
            Which::All,
            &|_, _| {},
            &AtomicBool::new(false)
        ),
        Err(FontError::DiskFull)
    ));
    assert!(
        dir.path()
            .join("testa")
            .join(&fam.version)
            .join("Testa-400.ttf")
            .is_file(),
        "the finished file stays"
    );
    assert!(store.list().is_empty(), "no manifest yet");
}

#[test]
fn only_googles_two_hosts_are_ever_asked() {
    let fam = klipolo_fonts::google::GoogleFamily::for_tests_with_tail(
        "Testa",
        "../../evil.example/x.ttf",
    );
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    assert!(
        download_family(
            &store,
            &fam,
            Which::All,
            &|_, _| {},
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

/// A family fetched a weight at a time (CJK) adds each face to its manifest; progress is heard in steps, and a cancel
/// stops it with the finished faces kept.
#[test]
fn a_family_fetched_a_face_at_a_time_keeps_every_face_and_can_be_cancelled() {
    let server = Server::start();
    let fam = family_on(
        &server,
        "Testa",
        &[
            (400, false, "Inter/Inter-Regular.ttf"),
            (700, false, "Inter/Inter-Bold.ttf"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let heard = std::sync::Mutex::new(Vec::new());
    let one = Which::One {
        weight: 400,
        italic: false,
    };
    let m = download_family(
        &store,
        &fam,
        one,
        &|d, t| heard.lock().unwrap().push((d, t)),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(m.files.iter().map(|f| f.weight).collect::<Vec<_>>(), [400]);
    let heard = heard.into_inner().unwrap();
    assert_eq!(heard.last(), Some(&(342_408, 342_408)), "{heard:?}");
    assert!(
        heard
            .windows(2)
            .all(|w| w[1].0.saturating_sub(w[0].0) >= 64 * 1024 || w[1] == heard[heard.len() - 1]),
        "at most every 64 KB"
    );
    let stop = AtomicBool::new(true);
    let bold = Which::One {
        weight: 700,
        italic: false,
    };
    assert_eq!(
        download_family(&store, &fam, bold, &|_, _| {}, &stop),
        Err(FontError::Cancelled)
    );
    assert_eq!(store.list()[0].files.len(), 1, "the 400 stays");
    let m = download_family(&store, &fam, bold, &|_, _| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        m.files.iter().map(|f| f.weight).collect::<Vec<_>>(),
        [400, 700]
    );
    assert_eq!(store.faces().len(), 2);
    assert_eq!(store.faces()[0].alias, "kl:g:testa:v1");
}
