//! This computer's fonts, as the engine's one system scan finds them. The scan (the font template, on the
//! font thread) loads the system's folders and the files a font manager activated, then publishes them here once:
//! grouped by family (a family's other widths as their own entries), each face with its file, weight, style, width,
//! size and whether it is variable, each family with the scripts it covers. The engine's index takes them, the forbidden fallback list takes the CJK ones, and the app, which waits
//! on `subscribe_scanned`, rebuilds its own index from `installed_families()`: the same list, never a second scan.
//! An export worker publishes the app's own list instead, from its job's record of it (`Published`).
//!
//! Cost: one memory-map and about twenty cmap lookups per face, once per process, on the font thread (a Mac's 700 to
//! 1,000 system faces). Memory: one record per face (its path and a few numbers), kept for the process.

use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, OnceLock};

use glyphon::fontdb::{Database, FaceInfo, ID, Source, Style};
use klipolo_fonts::coverage::face_facts;
use klipolo_fonts::index::{FontIndex, InstalledFace, InstalledFamily};
use klipolo_fonts::installed::{ScannedFace, group_faces};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::GENERATION;
use super::managed::is_managed;
use crate::render::font_fallback::{cjk_forbidden, forbid_installed_cjk};

/// The families the scan published: none until it has run.
static INSTALLED: OnceLock<Arc<[InstalledFamily]>> = OnceLock::new();
/// True once the scan has published (the app's menu and missing-fonts check wait on it, with a ceiling).
static SCANNED: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::channel(false).0);
/// The installed CJK families' names the scan put on the forbidden fallback list (hidden ones included): an export job
/// records them with the families.
static FORBIDDEN: OnceLock<Arc<[String]>> = OnceLock::new();

/// This computer's families (hidden ones left out, sorted by name); empty until the scan has run.
pub fn installed_families() -> Arc<[InstalledFamily]> {
    INSTALLED.get().cloned().unwrap_or_else(|| Arc::from([]))
}

/// Turns true once the scan has published this computer's families.
pub fn subscribe_scanned() -> watch::Receiver<bool> {
    SCANNED.subscribe()
}

/// Every family in `db` but Klipolo's own, hidden ones included (they matter to the forbidden list), with their faces
/// (file, weight, style, width, size) and coverage, grouped by `klipolo_fonts::installed::group_faces` (a family's other
/// widths as their own entries, each weight and style once). A face loaded from memory has no file to send the webview,
/// and is left out.
pub(crate) fn group_installed(db: &Database) -> Vec<InstalledFamily> {
    let scanned = db
        .faces()
        .filter(|f| !is_managed(f))
        .filter_map(|face| {
            let (name, _) = face.families.first()?;
            let path = match &face.source {
                Source::File(path) | Source::SharedFile(path, _) => path.clone(),
                Source::Binary(_) => return None,
            };
            let facts = db.with_face_data(face.id, face_facts).flatten();
            Some(ScannedFace {
                family: name.clone(),
                face: InstalledFace {
                    weight: face.weight.0,
                    italic: face.style != Style::Normal,
                    stretch: face.stretch.to_number(),
                    path,
                    index: face.index,
                    variable: facts.is_some_and(|f| f.variable),
                    bytes: facts.map_or(0, |f| f.bytes),
                },
                coverage: facts.map_or(0, |f| f.coverage),
            })
        })
        .collect();
    group_faces(scanned)
}

/// Oblique faces are re-registered as italic (Helvetica's "Oblique" faces): cosmic-text matches the asked style
/// exactly, and the drawer asks italic, as CSS treats oblique as italic's stand-in. Before anything holds their ids.
fn oblique_as_italic(db: &mut Database) {
    let oblique: Vec<FaceInfo> = db
        .faces()
        .filter(|f| f.style == Style::Oblique)
        .cloned()
        .collect();
    for face in oblique {
        db.remove_face(face.id);
        db.push_face_info(FaceInfo {
            id: ID::dummy(),
            style: Style::Italic,
            ..face
        });
    }
}

/// What a scan publishes: this computer's families as the index and the menu list them (hidden ones left out, sorted
/// by name), and the installed CJK families' names for the forbidden fallback list (hidden ones included).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Published {
    pub(crate) installed: Vec<InstalledFamily>,
    pub(crate) forbidden: Vec<String>,
}

/// What a scan of `db` publishes: its oblique faces become italic first (before anything holds their ids).
pub(crate) fn published_of(db: &mut Database) -> Published {
    oblique_as_italic(db);
    let all = group_installed(db);
    Published {
        forbidden: cjk_forbidden(&all),
        installed: FontIndex::default()
            .with_installed(all)
            .installed_list()
            .to_vec(),
    }
}

/// Publishes `published`, once a process: its families become the engine's installed families, their CJK ones go on
/// the forbidden list, the fonts' generation moves on (texts are drawn again with them), and `subscribe_scanned` says
/// so.
pub(crate) fn publish(published: Published) {
    forbid_installed_cjk(&published.forbidden);
    let _ = FORBIDDEN.set(published.forbidden.into());
    if INSTALLED.set(published.installed.into()).is_ok() {
        GENERATION.fetch_add(1, Ordering::SeqCst);
    }
    SCANNED.send_replace(true);
}

/// The scan's last step, once a process: what `db` publishes (`published_of`), published.
pub(crate) fn publish_installed(db: &mut Database) {
    publish(published_of(db));
}

/// What this process published, once it has (an export job records it: `render::fonts::record_scan`).
pub(crate) fn published() -> Option<Published> {
    Some(Published {
        installed: INSTALLED.get()?.to_vec(),
        forbidden: FORBIDDEN.get()?.to_vec(),
    })
}
