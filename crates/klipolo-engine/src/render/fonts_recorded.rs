//! The app's scan of this computer's fonts, handed to its export workers.
//! Each export process used to scan the system's fonts again before frame 0: walk the font folders, parse every face,
//! then memory-map each one and read about twenty cmap tables for its coverage (0.5 to 1.5 s on an Intel Mac with
//! 700 to 1,000 faces). Now each run of a job (`export::runner`) writes what the app's own scan found beside the job's
//! manifest (`installed-fonts.json`), and the worker builds its first font system from it: the same faces under the
//! same names, the same families with their coverage, and the same forbidden fallbacks, with no folder walked and no
//! face opened until a text draws with it.
//!
//! - **Written each run**, from the app running it: fonts installed or removed since a pause count, as `downloads` do.
//!   Before the app's own scan has finished, there's nothing to hand on: the file is removed, and the worker scans for
//!   itself, as before. A record that can't be read is the same.
//! - **The same database**: every face the scan put in the template (Klipolo's managed ones aside, which the worker
//!   registers itself), with its file, index in its collection, every family name, PostScript name, style, weight,
//!   width and whether it is monospaced: all fontdb and cosmic-text match or fall back on. Oblique faces were already
//!   made italic by the app's scan. The generic families are kept as the app's template had them.
//! - **Only what is still here**: before it is used, every face whose file is gone or isn't a
//!   regular file any more is dropped with its family (`ScanRecord::keep_present`), so a font removed or deactivated
//!   since the app's scan stops the export as missing, as the worker's own scan did, and a FIFO never reaches fontdb.
//! - **Cost**: about 0.2 KB of JSON a face, written once a run in the app (a few milliseconds for 1,000 faces) and
//!   read once by the worker. Nothing is kept in the app beyond what its scan already holds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use glyphon::fontdb::{Database, FaceInfo, Family, ID, Language, Source, Style, Weight};
use serde::{Deserialize, Serialize};

use super::installed::{Published, publish, published};
use super::managed::{is_managed, width_of};

/// The record's file in a job's folder.
const FILE: &str = "installed-fonts.json";

/// One face of the scan's database, as fontdb holds it after the scan.
#[derive(Debug, Serialize, Deserialize)]
struct SystemFace {
    path: PathBuf,
    index: u32,
    /// Every family name the font gives (the English one first), as fontdb matches on them.
    families: Vec<String>,
    post_script_name: String,
    italic: bool,
    weight: u16,
    /// The OS/2 width class (5 normal).
    stretch: u16,
    monospaced: bool,
}

/// The generic families (`serif`, `sans-serif`, `cursive`, `fantasy`, `monospace`) as the template names them.
#[derive(Debug, Serialize, Deserialize)]
struct Generics {
    serif: String,
    sans_serif: String,
    cursive: String,
    fantasy: String,
    monospace: String,
}

/// What an app's scan found, as an export worker needs it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScanRecord {
    /// The locale the template's font systems shape in.
    locale: String,
    generics: Generics,
    faces: Vec<SystemFace>,
    published: Published,
}

/// The record a worker was given (`use_recorded_scan`), until the template takes it.
static GIVEN: Mutex<Option<ScanRecord>> = Mutex::new(None);

impl ScanRecord {
    /// `db`'s faces (Klipolo's managed ones left out) with what its scan published.
    pub(crate) fn of(locale: &str, db: &Database, published: Published) -> Self {
        let faces = db
            .faces()
            .filter(|f| !is_managed(f))
            .filter_map(|face| {
                let path = match &face.source {
                    Source::File(path) | Source::SharedFile(path, _) => path.clone(),
                    Source::Binary(_) => return None,
                };
                Some(SystemFace {
                    path,
                    index: face.index,
                    families: face.families.iter().map(|(name, _)| name.clone()).collect(),
                    post_script_name: face.post_script_name.clone(),
                    italic: face.style != Style::Normal,
                    weight: face.weight.0,
                    stretch: face.stretch.to_number(),
                    monospaced: face.monospaced,
                })
            })
            .collect();
        let generic = |family: Family<'_>| db.family_name(&family).to_string();
        Self {
            locale: locale.to_string(),
            generics: Generics {
                serif: generic(Family::Serif),
                sans_serif: generic(Family::SansSerif),
                cursive: generic(Family::Cursive),
                fantasy: generic(Family::Fantasy),
                monospace: generic(Family::Monospace),
            },
            faces,
            published,
        }
    }

    /// Writes the record into the job folder `dir`.
    pub(crate) fn write(&self, dir: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(FILE), bytes)
    }

    /// Keeps only the faces whose files are still regular files here. A font removed
    /// or deactivated since the app's scan (Font Book, a font manager that quit) goes from the faces and from its
    /// family, and a family left with none goes too: a text in it is then missing, and the export stops as it did when
    /// the worker scanned, never drawn silently in the fallback. A FIFO, or a folder, under a recorded path never
    /// reaches fontdb (opening a FIFO would block). One `stat` a file: a few ms for a Mac's fonts.
    pub(crate) fn keep_present(&mut self) {
        let mut present: HashMap<PathBuf, bool> = HashMap::new();
        let mut here = |path: &Path| {
            *present
                .entry(path.to_path_buf())
                .or_insert_with(|| std::fs::metadata(path).is_ok_and(|m| m.is_file()))
        };
        self.faces.retain(|face| here(&face.path));
        for family in &mut self.published.installed {
            family.faces.retain(|face| here(&face.path));
        }
        self.published.installed.retain(|f| !f.faces.is_empty());
    }

    /// The template's locale and database, built from the record without reading a font file (its faces still here:
    /// `keep_present`), and what it publishes published (this process's installed families and forbidden fallbacks).
    pub(crate) fn into_database(mut self) -> (String, Database) {
        self.keep_present();
        let mut db = Database::new();
        for face in self.faces {
            db.push_face_info(FaceInfo {
                id: ID::dummy(),
                source: Source::File(face.path),
                index: face.index,
                families: face
                    .families
                    .into_iter()
                    .map(|name| (name, Language::English_UnitedStates))
                    .collect(),
                post_script_name: face.post_script_name,
                style: if face.italic {
                    Style::Italic
                } else {
                    Style::Normal
                },
                weight: Weight(face.weight),
                stretch: width_of(face.stretch),
                monospaced: face.monospaced,
            });
        }
        let g = self.generics;
        db.set_serif_family(g.serif);
        db.set_sans_serif_family(g.sans_serif);
        db.set_cursive_family(g.cursive);
        db.set_fantasy_family(g.fantasy);
        db.set_monospace_family(g.monospace);
        publish(self.published);
        (self.locale, db)
    }
}

/// The app's side, each run of an export job: what this process's scan found, written beside the job's manifest in
/// `dir` for its worker. Before the scan has finished (or if it can't be written), the file is removed instead, and the
/// worker scans for itself.
pub fn record_scan(dir: &Path) {
    let recorded = published().and_then(|published| {
        crate::vendor::cap::rendering::layers::with_scanned_template(|locale, db| {
            ScanRecord::of(locale, db, published)
        })
    });
    record_or_remove(dir, recorded);
}

/// Writes `recorded` into the job folder `dir`, or, with none (or when it can't be written), removes an older run's.
fn record_or_remove(dir: &Path, recorded: Option<ScanRecord>) {
    let written = recorded.map(|record| record.write(dir));
    if let Some(Ok(())) = written {
        return;
    }
    if let Some(Err(e)) = written {
        tracing::warn!(%e, "export: the fonts' record couldn't be written; its worker scans for itself");
    }
    match std::fs::remove_file(dir.join(FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(%e, "export: an old fonts' record couldn't be removed"),
    }
}

/// The worker's side, before any font system is made: the job folder `dir`'s record of the app's scan, which the first
/// font system is then built from instead of a scan. False when there is none or it can't be read: the worker scans.
pub fn use_recorded_scan(dir: &Path) -> bool {
    let Ok(bytes) = std::fs::read(dir.join(FILE)) else {
        return false;
    };
    match serde_json::from_slice::<ScanRecord>(&bytes) {
        Ok(record) => {
            *GIVEN.lock().unwrap_or_else(PoisonError::into_inner) = Some(record);
            true
        }
        Err(e) => {
            eprintln!(
                "export: the fonts' record can't be read ({e}); scanning this computer's fonts"
            );
            false
        }
    }
}

/// The record a worker was given, for the template to build from (once).
pub(crate) fn take_given() -> Option<ScanRecord> {
    GIVEN.lock().unwrap_or_else(PoisonError::into_inner).take()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use glyphon::fontdb::Database;
    use klipolo_fonts::bundled::bundled_dir;
    use klipolo_fonts::index::FontIndex;
    use klipolo_model::font::{FontRef, FontSource};

    use super::super::family_for;
    use super::super::installed::published_of;
    use super::{FILE, ScanRecord, record_or_remove, use_recorded_scan};

    /// A bundled file copied into `dir` under another name: found there, it counts as installed.
    fn installed_copy(dir: &Path, file: &str) -> PathBuf {
        let to = dir.join(format!("Installed-{}", file.replace('/', "-")));
        std::fs::copy(bundled_dir().unwrap().join(file), &to).unwrap();
        to
    }

    /// The record an app whose scan found the fonts in `dir` (and nothing else) makes.
    fn record_of(dir: &Path) -> ScanRecord {
        let mut db = Database::new();
        db.load_fonts_dir(dir);
        let published = published_of(&mut db);
        ScanRecord::of("en-US", &db, published)
    }

    fn missing(record: &ScanRecord, family: &str) -> bool {
        let index =
            FontIndex::shipped(bundled_dir()).with_installed(record.published.installed.clone());
        let font = FontRef {
            family: family.into(),
            source: FontSource::Installed,
        };
        family_for(&index, &font, 400, false).stand_in
    }

    /// A font removed or deactivated after the app's scan (Font Book, a font manager
    /// quit) is dropped from the worker's fonts with its family, so a text in it is missing and the export stops as it
    /// did when the worker scanned, never drawn silently in Inter. A path that is a FIFO now never reaches fontdb, which
    /// would block opening it.
    #[test]
    fn faces_whose_files_are_gone_or_not_files_are_dropped_and_their_family_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        installed_copy(dir.path(), "Anton/Anton-Regular.ttf");
        let bebas = installed_copy(dir.path(), "BebasNeue/BebasNeue-Regular.ttf");
        let bangers = installed_copy(dir.path(), "Bangers/Bangers-Regular.ttf");
        let mut record = record_of(dir.path());
        assert!(
            !missing(&record, "Bebas Neue"),
            "installed when the app scanned"
        );
        std::fs::remove_file(&bebas).unwrap();
        #[cfg(unix)]
        {
            std::fs::remove_file(&bangers).unwrap();
            let made = std::process::Command::new("mkfifo")
                .arg(&bangers)
                .status()
                .unwrap();
            assert!(made.success());
        }
        record.keep_present();
        assert!(missing(&record, "Bebas Neue"), "a removed font is missing");
        assert!(!missing(&record, "Anton"), "a font still here draws");
        #[cfg(unix)]
        assert!(missing(&record, "Bangers"), "a FIFO is never a font");
        assert!(record.faces.iter().all(|f| f.path.is_file()));
        assert!(
            record
                .faces
                .iter()
                .all(|f| f.path != bebas && f.path != bangers)
        );
    }

    /// Without a record, or with one that can't be read, the worker scans for itself.
    #[test]
    fn a_missing_or_damaged_record_means_the_worker_scans() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!use_recorded_scan(dir.path()), "no record");
        std::fs::write(dir.path().join(FILE), b"{ not a record").unwrap();
        assert!(!use_recorded_scan(dir.path()), "a damaged record");
    }

    /// A run started before this app's scan has finished removes an older run's record, so its worker
    /// scans rather than drawing from fonts that may have changed since.
    #[test]
    fn a_run_before_the_scan_is_in_removes_an_older_record() {
        let dir = tempfile::tempdir().unwrap();
        let fonts = tempfile::tempdir().unwrap();
        installed_copy(fonts.path(), "Anton/Anton-Regular.ttf");
        record_or_remove(dir.path(), Some(record_of(fonts.path())));
        assert!(dir.path().join(FILE).is_file(), "written");
        record_or_remove(dir.path(), None);
        assert!(!dir.path().join(FILE).exists(), "removed");
    }
}
