//! A downloaded Google family as a font index holds it: what the download store's manifest
//! records of its family, exact version, faces and coverage (moved out of `index.rs`),
//! and the index's lookups of them beyond the exact version: the CJK fallbacks and the menu's older downloads.

use klipolo_model::font::{FontRef, FontSource};
use serde::{Deserialize, Serialize};

use klipolo_model::text_language::CjkScript;

use crate::alias::alias;
use crate::cjk::CjkCover;
use crate::index::FontIndex;
use crate::resolve::FaceFile;

/// A downloaded Google family as an index needs it: what its manifest records of its family, exact version,
/// faces and coverage. An export job records these (`ExportJob::downloads`), so its worker registers the same faces.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
    pub family: String,
    pub version: String,
    pub faces: Vec<DownloadedFace>,
    /// The Chinese, Japanese and Korean its faces draw themselves: a downloaded Noto Sans JP picked as a text's
    /// font draws its own kanji and kana.
    pub cover: CjkCover,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadedFace {
    pub weight: u16,
    pub italic: bool,
    pub file: FaceFile,
}

impl Downloaded {
    /// The `FontRef` a text names it by.
    pub fn font(&self) -> FontRef {
        FontRef {
            family: self.family.clone(),
            source: FontSource::Google {
                version: self.version.clone(),
            },
        }
    }

    /// Its alias (`kl:g:<slug>:<version>`), the family its faces are registered under.
    pub fn alias(&self) -> String {
        alias(&self.font())
    }
}

/// A Google version's number (`v31` → 31; 0 for anything else).
pub(crate) fn version_number(version: &str) -> u32 {
    version
        .get(1..)
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
}

impl FontIndex {
    /// The downloaded Noto Sans of `script` (SC, TC, JP or KR), the newest if several versions are here: the CJK
    /// fallback `resolve_fallback` answers, in the engine and the webview alike.
    pub fn cjk(&self, script: CjkScript) -> Option<&Downloaded> {
        let family = crate::fallback::cjk_family(script);
        self.downloads()
            .iter()
            .filter(|d| d.family == family)
            .max_by_key(|d| version_number(&d.version))
    }

    /// `family` downloaded here: at `current` (the catalogue's version) when that one is, else the newest downloaded
    /// (the menu's row for a family downloaded only at an older version is that download).
    pub fn download_of(&self, family: &str, current: &str) -> Option<&Downloaded> {
        self.downloaded(family, current).or_else(|| {
            self.downloads()
                .iter()
                .filter(|d| d.family == family)
                .max_by_key(|d| version_number(&d.version))
        })
    }
}
