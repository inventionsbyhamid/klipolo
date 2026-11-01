//! Noto Sans SC, TC, JP and KR: the only faces Chinese, Japanese and Korean text falls back to, downloaded a weight at a
//! time (each is over 15 MB as a family). Settings lists each script's upright weights with their sizes and which are
//! here; a downloaded one is registered with the engine as that script's fallback.

use klipolo_fonts::fallback::cjk_family;
use klipolo_fonts::index::FontIndex;
use klipolo_model::text_language::CjkScript;
use serde::Serialize;
use ts_rs::TS;

/// One script's Noto Sans as Settings offers it: its faces' sizes, and which are here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CjkFonts {
    pub script: CjkScript,
    pub family: String,
    pub faces: Vec<CjkFace>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CjkFace {
    pub weight: u16,
    /// What its download takes (0 when the catalogue doesn't list it: a weight downloaded before).
    pub bytes: u64,
    pub downloaded: bool,
}

/// The four Noto Sans: the catalogue's upright faces, and the weights downloaded here.
pub fn cjk_fonts(index: &FontIndex) -> Vec<CjkFonts> {
    CjkScript::ALL
        .into_iter()
        .map(|script| {
            let family = cjk_family(script);
            let here: Vec<u16> = index
                .cjk(script)
                .map(|d| d.faces.iter().map(|f| f.weight).collect())
                .unwrap_or_default();
            let mut faces: Vec<CjkFace> = index
                .catalogue()
                .and_then(|c| c.find(&family.to_lowercase()))
                .map(|g| {
                    g.files
                        .iter()
                        .filter(|f| !f.italic)
                        .map(|f| CjkFace {
                            weight: f.weight,
                            bytes: f.bytes,
                            downloaded: here.contains(&f.weight),
                        })
                        .collect()
                })
                .unwrap_or_default();
            for &w in &here {
                if !faces.iter().any(|f| f.weight == w) {
                    faces.push(CjkFace {
                        weight: w,
                        bytes: 0,
                        downloaded: true,
                    });
                }
            }
            faces.sort_by_key(|f| f.weight);
            CjkFonts {
                script,
                family: family.to_owned(),
                faces,
            }
        })
        .collect()
}
