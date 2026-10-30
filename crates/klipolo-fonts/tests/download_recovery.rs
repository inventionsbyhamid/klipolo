//! Downloads against a local server, as Google's own files are: Google's Thin and ExtraLight
//! weights; a face that isn't the one named is left out of a family fetched whole; nothing but Google's files is asked;
//! a cancel goes out before any request; a finished face is kept by what was written, also across a quit.

#![cfg(feature = "test-hosts")]

mod common;
use std::sync::atomic::AtomicBool;

use common::server::{Server, family_on};
use klipolo_fonts::download::{FontError, Which, download_family};
use klipolo_fonts::store::FontStore;

const REGULAR: (u16, bool, &str) = (400, false, "Inter/Inter-Regular.ttf");
const BOLD: (u16, bool, &str) = (700, false, "Inter/Inter-Bold.ttf");

fn go(
    store: &FontStore,
    fam: &klipolo_fonts::google::GoogleFamily,
) -> Result<klipolo_fonts::store::Manifest, FontError> {
    download_family(store, fam, Which::All, &|_, _| {}, &AtomicBool::new(false))
}

/// Google's static Thin says 250 and ExtraLight 275 in its OS/2 table (for Windows); they are 100 and 200.
#[test]
fn googles_thin_and_extralight_files_are_those_faces() {
    let server = Server::start();
    server.say_weight("Inter-Regular", 250);
    server.say_weight("Inter-Bold", 275);
    let fam = family_on(
        &server,
        "Testa",
        &[
            (100, false, "Inter/Inter-Regular.ttf"),
            (200, false, "Inter/Inter-Bold.ttf"),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let m = go(&store, &fam).unwrap();
    assert_eq!(
        m.files.iter().map(|f| f.weight).collect::<Vec<_>>(),
        [100, 200]
    );
}

/// A family fetched whole keeps the faces that are what they say, and leaves out one that isn't, saying
/// so in its manifest; it fails only when none is.
#[test]
fn a_face_that_isnt_the_one_named_is_left_out_and_the_rest_install() {
    let server = Server::start();
    // The "700" serves the 400 file.
    let fam = family_on(
        &server,
        "Testa",
        &[REGULAR, (700, false, "Inter/Inter-Regular.ttf")],
    );
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let m = go(&store, &fam).unwrap();
    assert_eq!(m.files.iter().map(|f| f.weight).collect::<Vec<_>>(), [400]);
    assert_eq!(m.left_out, [(700, false)]);
    assert_eq!(store.list().len(), 1);
}

/// A download asks Google for its files and nothing else; its licence comes from the app, headed by the
/// font's own copyright line.
#[test]
fn a_download_asks_google_alone_and_keeps_the_licence_the_app_ships() {
    let server = Server::start();
    let fam = family_on(&server, "Testa", &[REGULAR, BOLD]);
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let m = go(&store, &fam).unwrap();
    assert!(
        server.paths().iter().all(|p| p.starts_with("/s/")),
        "{:?}",
        server.paths()
    );
    let ofl = std::fs::read_to_string(dir.path().join("testa/v1/OFL.txt")).unwrap();
    assert!(ofl.contains("SIL OPEN FONT LICENSE Version 1.1"));
    assert!(
        ofl.starts_with("Copyright"),
        "the font's own notice first: {}",
        &ofl[..80]
    );
    assert_eq!(m.licence, "ofl");
}

/// A cancel (Google Fonts switched off) is seen before a request goes out, not only between chunks.
#[test]
fn nothing_goes_out_once_cancelled() {
    let server = Server::start();
    let fam = family_on(&server, "Testa", &[REGULAR, BOLD]);
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    let stop = AtomicBool::new(true);
    let got = download_family(&store, &fam, Which::All, &|_, _| {}, &stop);
    assert_eq!(got.unwrap_err(), FontError::Cancelled);
    assert_eq!(server.total_requests(), 0);
}

/// A finished face is kept by what was written (its SHA-256 and
/// a parse), not by the catalogue's size, and across a quit too: start-up `tidy` keeps it, and the next download fetches
/// only the rest. A face changed since is fetched again.
#[test]
fn a_finished_face_survives_a_quit_and_a_catalogue_size_that_differs() {
    let server = Server::start();
    let mut fam = family_on(&server, "Testa", &[REGULAR, BOLD]);
    // The catalogue's sizes came from HEADs, which may differ from the file sent.
    fam.files[0].bytes += 17;
    server.drop_after_bytes("Inter-Bold", 10_000);
    let dir = tempfile::tempdir().unwrap();
    let store = FontStore::open(dir.path().into()).unwrap();
    assert_eq!(go(&store, &fam).unwrap_err(), FontError::Offline);
    // A quit, then the next start.
    let store = FontStore::open(dir.path().into()).unwrap();
    store.tidy().unwrap();
    assert!(
        dir.path().join("testa/v1/Testa-400.ttf").is_file(),
        "the finished face stays"
    );
    server.heal();
    let before = server.requests("Inter-Regular");
    let m = go(&store, &fam).unwrap();
    assert_eq!(m.files.len(), 2);
    assert_eq!(
        server.requests("Inter-Regular"),
        before,
        "not fetched again"
    );
    // A finished face whose bytes changed is fetched again, never kept.
    let other = tempfile::tempdir().unwrap();
    let store = FontStore::open(other.path().into()).unwrap();
    server.drop_after_bytes("Inter-Bold", 10_000);
    assert!(go(&store, &fam).is_err());
    std::fs::write(
        other.path().join("testa/v1/Testa-400.ttf"),
        b"not that font",
    )
    .unwrap();
    store.tidy().unwrap();
    assert!(
        !other.path().join("testa/v1/Testa-400.ttf").exists(),
        "removed at start"
    );
    server.heal();
    let before = server.requests("Inter-Regular");
    go(&store, &fam).unwrap();
    assert_eq!(server.requests("Inter-Regular"), before + 1);
}
