//! Google Fonts downloads: only when the person
//! picked a family and confirmed in place, straight from Google, into the store (`store`).
//!
//! - **One host: Google's** (`net`). Every file comes from `https://fonts.gstatic.com/s/`; the licence ships in the
//!   app (`licence`), so nothing else is ever asked. A cancel (the person's, or Google Fonts
//!   switched off) is seen before every request, retries included.
//! - **Every file is checked before it is used** (`check`). It is streamed into `<file>.part` with a 50 MB cap, parsed
//!   with ttf-parser (memory-safe), which must find a static face of the weight and style the catalogue names (Thin may
//!   say 250 and ExtraLight 275, as Google's own files do), SHA-256'd as it is written, synced and renamed into place.
//!   A family fetched whole keeps the faces that pass and leaves out one that doesn't, said in its manifest; it fails
//!   only when none passes.
//! - **A failure halfway loses nothing finished.** Each file is whole or absent, and each finished one is recorded with
//!   its SHA-256 (`partial.json`) as it lands. A lost connection is tried twice more, then said (`Offline`); a full disk
//!   stops with the finished files kept (`DiskFull`). A retry, also after a quit, keeps a finished file whose bytes
//!   still hash to what was written and still parse as that face, and fetches only the rest. The manifest is written
//!   last, so nothing half-installed is ever listed, registered or drawn.

use std::fs;
use std::io;
use std::sync::atomic::AtomicBool;

use klipolo_model::coded::CodedError;
use klipolo_platform::durable::{self, DiskReason, Durability};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::coverage::Coverage;
use crate::english::msg;
use crate::google::{GoogleFamily, GoogleFile, WHOLE_FAMILY_MAX};
use crate::licence::{licence_file, licence_text};
use crate::matching::nearest;
use crate::store::{
    FontStore, Manifest, ManifestFile, PART, face_file_name, records, verified, write_partial,
};

pub(crate) mod check;
mod net;
mod preview;

pub use check::weight_ok;
#[cfg(feature = "test-hosts")]
pub use net::test_disk_full_after;
pub use preview::fetch_preview;

/// The most one font file may be (the webview's face budget is the same).
pub const MAX_FILE_BYTES: u64 = crate::sfnt::MAX_FACE_BYTES;

/// Which faces of a family to fetch: all of it (up to 15 MB), or one face (a bigger family, a weight at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    All,
    One { weight: u16, italic: bool },
}

/// Why a download, a preview or a removal didn't happen. `code()` says it in the person's language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontError {
    /// Google couldn't be reached (after two retries on a lost connection).
    Offline,
    /// The disk is full: the finished files are kept.
    DiskFull,
    /// Another disk failure (read-only, denied): the finished files are kept.
    Disk(String),
    /// What came isn't the face asked for (or an address isn't one of Google's): deleted.
    BadFile(String),
    /// Over the 50 MB cap.
    TooBig,
    Cancelled,
    /// Google answered with this HTTP status.
    Http(u16),
    /// A removal named a family that isn't one of the store's own downloads.
    NotOurs,
}

impl FontError {
    /// Its code (`fonts.download.*`, `fonts.remove.notOurs`).
    pub fn code(&self) -> CodedError {
        match self {
            FontError::Offline => msg!("fonts.download.offline"),
            FontError::DiskFull => msg!("fonts.download.diskFull"),
            FontError::Disk(_) => msg!("fonts.download.diskError"),
            FontError::BadFile(_) => msg!("fonts.download.badFile"),
            FontError::TooBig => msg!("fonts.download.tooBig"),
            FontError::Cancelled => msg!("fonts.download.cancelled"),
            FontError::Http(status) => msg!("fonts.download.http", status = u32::from(*status)),
            FontError::NotOurs => msg!("fonts.remove.notOurs"),
        }
    }

    /// A disk error, classified: a full disk is `DiskFull`.
    pub(crate) fn disk(e: &io::Error) -> FontError {
        match durable::classify(e) {
            DiskReason::Full => FontError::DiskFull,
            _ => FontError::Disk(e.to_string()),
        }
    }
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::Disk(why) | FontError::BadFile(why) => {
                write!(f, "{}: {why}", crate::english::english(&self.code()))
            }
            _ => f.write_str(&crate::english::english(&self.code())),
        }
    }
}

impl std::error::Error for FontError {}

/// A Google face a request would download (chat's proposals name them).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FontDownload {
    pub family: String,
    pub version: String,
    /// The face asked for: its weight and style choose it in a family fetched a weight at a time.
    pub weight: u16,
    pub italic: bool,
    /// The whole family (15 MB or less), else that one face.
    pub whole: bool,
    /// What it downloads.
    pub bytes: u64,
}

impl FontDownload {
    /// What a pick of `fam` at `(weight, italic)` downloads.
    pub fn of(fam: &GoogleFamily, weight: u16, italic: bool) -> FontDownload {
        // A family fetched a face at a time names the face it has nearest the asked one.
        let (weight, italic, whole, bytes) = match which_for(fam, weight, italic) {
            Which::All => (weight, italic, true, fam.bytes()),
            Which::One { weight, italic } => (
                weight,
                italic,
                false,
                fam.files
                    .iter()
                    .find(|f| (f.weight, f.italic) == (weight, italic))
                    .map_or(0, |f| f.bytes),
            ),
        };
        FontDownload {
            family: fam.family.clone(),
            version: fam.version.clone(),
            weight,
            italic,
            whole,
            bytes,
        }
    }

    /// Whether downloading this fetches what `other` would (the same family and version, and the whole of it or the
    /// same face).
    pub fn covers(&self, other: &FontDownload) -> bool {
        self.family == other.family
            && self.version == other.version
            && (self.whole || (self.weight, self.italic) == (other.weight, other.italic))
    }
}

/// The whole family when it is 15 MB or less, else its face nearest `(weight, italic)`.
pub fn which_for(fam: &GoogleFamily, weight: u16, italic: bool) -> Which {
    if fam.bytes() <= WHOLE_FAMILY_MAX {
        return Which::All;
    }
    let faces = fam.faces();
    match nearest(&faces, weight, italic) {
        Some(i) => Which::One {
            weight: faces[i].0,
            italic: faces[i].1,
        },
        None => Which::All,
    }
}

/// Downloads `fam`'s faces (`which`) into `store`, keeping what's already there whole: each face to `.part`, checked,
/// hashed, synced, renamed into place and recorded, then its licence (from the app), then the manifest, last.
/// `progress(done, total)` hears the bytes fetched; `cancel` stops it before any request and between chunks. Call it
/// only for the person's own pick, after their consent, and on a blocking thread.
pub fn download_family(
    store: &FontStore,
    fam: &GoogleFamily,
    which: Which,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<Manifest, FontError> {
    let hosts = net::Hosts::of(fam);
    let wanted: Vec<&GoogleFile> = match which {
        Which::All => fam.files.iter().collect(),
        Which::One { weight, italic } => {
            let faces = fam.faces();
            let i =
                nearest(&faces, weight, italic).ok_or_else(|| FontError::BadFile("face".into()))?;
            vec![&fam.files[i]]
        }
    };
    // Every address first: a bad one stops it before any request.
    let urls = wanted
        .iter()
        .map(|f| hosts.file(&f.url_tail))
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(fam.licence.as_str(), "ofl" | "apache" | "ufl") {
        return Err(FontError::BadFile("licence".into()));
    }
    let folder = store.folder(&fam.family, &fam.version);
    fs::create_dir_all(&folder).map_err(|e| FontError::disk(&e))?;
    // What a download of another weight left (its manifest), kept as it is.
    let before = store.manifest(&fam.family, &fam.version);
    let mut files: Vec<ManifestFile> = before.as_ref().map(|m| m.files.clone()).unwrap_or_default();
    let mut coverage: Coverage = before.as_ref().map_or(0, |m| m.coverage);
    let mut left_out: Vec<(u16, bool)> = Vec::new();
    let mut copyright: Option<String> = None;
    // Finished by an earlier try, or before a quit: recorded with their SHA-256 as they landed.
    let earlier = records(&folder);
    let mut todo: Vec<(&GoogleFile, reqwest::Url)> = Vec::new();
    for (f, u) in wanted.into_iter().zip(urls) {
        if files
            .iter()
            .any(|m| (m.weight, m.italic) == (f.weight, f.italic))
        {
            continue;
        }
        let name = face_file_name(&fam.family, f.weight, f.italic);
        let kept = earlier
            .iter()
            .find(|r| r.file == name && (r.weight, r.italic) == (f.weight, f.italic))
            .filter(|r| verified(&folder, r))
            .and_then(|r| {
                check::check_face(&folder.join(&name), f.weight, f.italic)
                    .ok()
                    .map(|c| (r.clone(), c))
            });
        match kept {
            Some((record, checked)) => {
                coverage |= checked.coverage;
                copyright = copyright.or(checked.copyright);
                files.push(record);
            }
            None => todo.push((f, u)),
        }
    }
    let mut progress = net::Progress {
        tell: progress,
        done: 0,
        total: todo.iter().map(|(f, _)| f.bytes).sum(),
        told: 0,
    };
    (progress.tell)(0, progress.total);
    if !todo.is_empty() {
        let client = net::client(&hosts)?;
        for (f, u) in todo {
            let name = face_file_name(&fam.family, f.weight, f.italic);
            let part = folder.join(format!("{name}{PART}"));
            let (bytes, sha256) =
                net::fetch(&client, &u, &part, MAX_FILE_BYTES, &mut progress, cancel).inspect_err(
                    |_| {
                        let _ = fs::remove_file(&part);
                    },
                )?;
            let checked = match check::check_face(&part, f.weight, f.italic) {
                Ok(c) => c,
                Err(e) => {
                    let _ = fs::remove_file(&part);
                    // Fetched whole, a family keeps the faces that pass and leaves this one out, said in its manifest.
                    if matches!(which, Which::All) && matches!(e, FontError::BadFile(_)) {
                        tracing::warn!(family = %fam.family, weight = f.weight, italic = f.italic, error = %e,
                            "fonts: a face Google sent isn't the one named; it is left out");
                        left_out.push((f.weight, f.italic));
                        continue;
                    }
                    return Err(e);
                }
            };
            if let Err(e) =
                fs::rename(&part, folder.join(&name)).and_then(|()| durable::fsync_dir(&folder))
            {
                let _ = fs::remove_file(&part);
                return Err(FontError::disk(&e));
            }
            coverage |= checked.coverage;
            copyright = copyright.or(checked.copyright);
            files.push(ManifestFile {
                weight: f.weight,
                italic: f.italic,
                file: name,
                bytes,
                sha256,
            });
            // Recorded as it lands: a quit after this keeps it (`tidy` and the next try re-check it).
            write_partial(&folder, &files).map_err(|e| FontError::disk(&e))?;
        }
    }
    progress.end();
    if files.is_empty() {
        return Err(FontError::BadFile("no face was the one named".into()));
    }
    let licence = folder.join(licence_file(&fam.licence));
    if !licence.is_file() {
        let text = licence_text(&fam.licence, copyright.as_deref())
            .ok_or_else(|| FontError::BadFile("licence".into()))?;
        durable::write_atomic(&licence, text.as_bytes(), Durability::Ordered)
            .map_err(|e| FontError::disk(&e))?;
    }
    files.sort_by_key(|f| (f.weight, f.italic));
    let manifest = Manifest {
        family: fam.family.clone(),
        version: fam.version.clone(),
        licence: fam.licence.clone(),
        coverage,
        files,
        left_out,
    };
    store
        .write_manifest(&manifest)
        .map_err(|e| FontError::disk(&e))?;
    // The manifest holds them all now.
    let _ = fs::remove_file(folder.join(crate::store::PARTIAL));
    Ok(manifest)
}
