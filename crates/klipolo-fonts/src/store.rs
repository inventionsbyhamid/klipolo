//! The download store: Google families downloaded here, each at its exact version, in
//! `<root>/<slug>/<version>/` (the app's root is `<app data>/fonts/google`):
//! - its faces, `<Family>-<weight><i?>.ttf`, each validated and hashed before it was renamed into place (`download`);
//! - its licence (`OFL.txt`, `LICENSE.txt` or `UFL.txt`, from the app's own copies: `licence`), kept beside them;
//! - `partial.json`, each finished face with its SHA-256, recorded as it lands, so a quit or crash keeps it;
//! - `manifest.json`, written last and atomically: what the folder holds. A folder without a valid one is incomplete
//!   and never used. At the next start `tidy` removes the `.part` and temporary files a quit or crash left, and any
//!   face that no longer hashes to what was written; the finished faces stay, and the next download resumes from them
//!   (a quit keeps finished files).
//!
//! Nothing here is ever deleted by itself: a family goes only when the person removes it in Settings (`remove`), by its
//! id (family and version), which must name one of this store's own manifests. The folder removed is the one that
//! manifest was found in, checked to lie inside the root once both are canonical: IPC never passes a path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use klipolo_model::font::{FontRef, FontSource};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::alias::alias;
use crate::cjk::CjkCover;
use crate::coverage::{Coverage, HAN, HANGUL, KANA};
use crate::download::FontError;
use crate::index::{Downloaded, DownloadedFace};
use crate::resolve::FaceFile;

/// The manifest's name in a family's folder.
pub const MANIFEST: &str = "manifest.json";
mod files;
mod tidy;

use files::{dirs, read_manifest};
pub use files::{face_file_name, folder_name};
pub(crate) use files::{plain_name, records, verified, write_partial};

/// What a file being downloaded is called until it is whole and valid.
pub const PART: &str = ".part";
/// The finished faces of a download not finished yet.
pub const PARTIAL: &str = "partial.json";

/// A downloaded family, by what names it: the family and its exact version (never a path).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DownloadedId {
    pub family: String,
    pub version: String,
}

/// A downloaded family as Settings lists it: its id and what its files take on the disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DownloadedFamily {
    pub id: DownloadedId,
    pub bytes: u64,
    /// Its faces' (weight, italic): a family fetched a weight at a time lists the ones here.
    pub faces: Vec<(u16, bool)>,
}

/// What a family's folder holds, written last (`manifest.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub family: String,
    pub version: String,
    /// `ofl`, `apache` or `ufl`.
    pub licence: String,
    /// The scripts its faces draw, computed from them once when the manifest is written (`coverage_of`): `cjk_cover`
    /// reads it, so a downloaded Noto Sans JP picked as a text's font draws its own kanji and kana.
    pub coverage: Coverage,
    pub files: Vec<ManifestFile>,
    /// Faces Google sent that weren't the one named, left out of a family fetched whole (said in the log).
    #[serde(default)]
    pub left_out: Vec<(u16, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub weight: u16,
    pub italic: bool,
    /// Its name in the folder.
    pub file: String,
    pub bytes: u64,
    /// Lower-case hex.
    pub sha256: String,
}

/// One downloaded face as the engine registers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedFaceSpec {
    pub alias: String,
    pub weight: u16,
    pub italic: bool,
    pub path: PathBuf,
}

impl Manifest {
    pub fn id(&self) -> DownloadedId {
        DownloadedId {
            family: self.family.clone(),
            version: self.version.clone(),
        }
    }

    pub fn font(&self) -> FontRef {
        FontRef {
            family: self.family.clone(),
            source: FontSource::Google {
                version: self.version.clone(),
            },
        }
    }

    /// The CJK its faces draw themselves.
    pub fn cjk_cover(&self) -> CjkCover {
        CjkCover {
            han: self.coverage & HAN != 0,
            kana: self.coverage & KANA != 0,
            hangul: self.coverage & HANGUL != 0,
        }
    }

    /// Its files together.
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    /// The family as an index holds it, its files in `folder`.
    pub fn downloaded(&self, folder: &Path) -> Downloaded {
        Downloaded {
            family: self.family.clone(),
            version: self.version.clone(),
            faces: self
                .files
                .iter()
                .map(|f| DownloadedFace {
                    weight: f.weight,
                    italic: f.italic,
                    file: FaceFile {
                        path: folder.join(&f.file),
                        index: 0,
                    },
                })
                .collect(),
            cover: self.cjk_cover(),
        }
    }
}

/// The store, rooted at a folder of its own.
#[derive(Clone, Debug)]
pub struct FontStore {
    root: PathBuf,
}

impl FontStore {
    /// The store at `root`, made if it isn't there yet.
    pub fn open(root: PathBuf) -> io::Result<FontStore> {
        fs::create_dir_all(&root)?;
        Ok(FontStore { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where `family` at `version` is kept (the version is checked by `font_ok` before it names a folder).
    pub(crate) fn folder(&self, family: &str, version: &str) -> PathBuf {
        self.root.join(folder_name(family)).join(version)
    }

    /// Every complete family, with the folder its manifest is in: a manifest that can't be read, names another family
    /// than its folder's, or lists a file that isn't there at its size, counts as incomplete and isn't used.
    fn entries(&self) -> Vec<(PathBuf, Manifest)> {
        let mut out = Vec::new();
        for family in dirs(&self.root) {
            for folder in dirs(&family) {
                if let Some(m) = read_manifest(&folder)
                    && self.folder(&m.family, &m.version) == folder
                {
                    out.push((folder, m));
                }
            }
        }
        out.sort_by(|(_, a), (_, b)| (&a.family, &a.version).cmp(&(&b.family, &b.version)));
        out
    }

    /// The downloaded families' manifests.
    pub fn list(&self) -> Vec<Manifest> {
        self.entries().into_iter().map(|(_, m)| m).collect()
    }

    /// The manifest of `family` at `version`, if it is complete here.
    pub fn manifest(&self, family: &str, version: &str) -> Option<Manifest> {
        let folder = self.folder(family, version);
        read_manifest(&folder).filter(|m| m.family == family && m.version == version)
    }

    /// The downloaded families as an index holds them (`FontIndex::with_downloaded`, `render::fonts::register`).
    pub fn downloaded(&self) -> Vec<Downloaded> {
        self.entries()
            .iter()
            .map(|(folder, m)| m.downloaded(folder))
            .collect()
    }

    /// Settings' list: each family's id, size and faces.
    pub fn families(&self) -> Vec<DownloadedFamily> {
        self.list()
            .into_iter()
            .map(|m| DownloadedFamily {
                bytes: m.bytes(),
                faces: m.files.iter().map(|f| (f.weight, f.italic)).collect(),
                id: m.id(),
            })
            .collect()
    }

    /// Every downloaded face, under the alias the engine registers it by (`kl:g:<slug>:<version>`).
    pub fn faces(&self) -> Vec<ManagedFaceSpec> {
        self.entries()
            .iter()
            .flat_map(|(folder, m)| {
                let alias = alias(&m.font());
                m.files.iter().map(move |f| ManagedFaceSpec {
                    alias: alias.clone(),
                    weight: f.weight,
                    italic: f.italic,
                    path: folder.join(&f.file),
                })
            })
            .collect()
    }

    /// Removes the family `id` names, which must be one of this store's own manifests. Its folder is the one
    /// that manifest was found in, never one made from the id's text, and it must lie inside the root once both are
    /// canonical; anything else is `NotOurs`, and nothing is touched.
    pub fn remove(&self, id: &DownloadedId) -> Result<(), FontError> {
        let Some((folder, _)) = self
            .entries()
            .into_iter()
            .find(|(_, m)| m.family == id.family && m.version == id.version)
        else {
            return Err(FontError::NotOurs);
        };
        let root = self.root.canonicalize().map_err(|_| FontError::NotOurs)?;
        let folder = folder.canonicalize().map_err(|_| FontError::NotOurs)?;
        if folder == root || !folder.starts_with(&root) {
            return Err(FontError::NotOurs);
        }
        fs::remove_dir_all(&folder).map_err(|e| FontError::disk(&e))?;
        if let Some(family) = folder.parent()
            && family != root
        {
            let _ = fs::remove_dir(family);
        }
        Ok(())
    }

    /// Writes `m` into its folder, atomically: the last step of a download.
    pub(crate) fn write_manifest(&self, m: &Manifest) -> io::Result<()> {
        let folder = self.folder(&m.family, &m.version);
        let bytes = serde_json::to_vec_pretty(m).map_err(io::Error::other)?;
        klipolo_platform::durable::write_atomic(
            &folder.join(MANIFEST),
            &bytes,
            klipolo_platform::durable::Durability::Ordered,
        )
    }
}
