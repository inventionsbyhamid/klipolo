//! Which language a project's video text is in, for the faces of Chinese, Japanese and Korean: the
//! project's caption language, else the main language of its transcripts (the caller passes them), else the
//! interface's locale. One answer for the engine, the export and the webview: Rust works it out and sends it.

use crate::project::Project;
use crate::speech::{Speech, WordKind};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CjkScript {
    /// Simplified Chinese.
    Sc,
    /// Traditional Chinese.
    Tc,
    Jp,
    Kr,
}

impl CjkScript {
    pub const ALL: [CjkScript; 4] = [CjkScript::Sc, CjkScript::Tc, CjkScript::Jp, CjkScript::Kr];

    /// Its place in `ALL` (a bit in a set of scripts).
    pub fn index(self) -> usize {
        self as usize
    }

    /// Its lower-case name, as JSON and the codes' `script` parameter say it.
    pub fn as_str(self) -> &'static str {
        match self {
            CjkScript::Sc => "sc",
            CjkScript::Tc => "tc",
            CjkScript::Jp => "jp",
            CjkScript::Kr => "kr",
        }
    }
}

/// `zh`, `zh-CN`, `zh-Hans` → Sc; `zh-TW`, `zh-HK`, `zh-Hant` → Tc; `ja` → Jp; `ko` → Kr.
pub fn cjk_script(tag: &str) -> Option<CjkScript> {
    let t = tag.to_ascii_lowercase();
    if t.starts_with("ja") {
        Some(CjkScript::Jp)
    } else if t.starts_with("ko") {
        Some(CjkScript::Kr)
    } else if t.starts_with("zh") {
        Some(
            if t.contains("hant") || t.contains("-tw") || t.contains("-hk") || t.contains("-mo") {
                CjkScript::Tc
            } else {
                CjkScript::Sc
            },
        )
    } else {
        None
    }
}

/// The language tag (primary subtag lower-cased, a script or region kept for Chinese).
pub fn text_language(p: &Project, speech: Option<&Speech>, ui_locale: &str) -> String {
    if let Some(l) = &p.captions.language {
        return l.clone();
    }
    if let Some(l) = speech.and_then(|s| spoken_language(p, s)) {
        return l;
    }
    let tag = ui_locale.split(['.', '@']).next().unwrap_or("en");
    if tag.to_ascii_lowercase().starts_with("zh") {
        tag.to_owned()
    } else {
        match tag.split(['-', '_']).next() {
            Some(primary) if !primary.is_empty() => primary.to_ascii_lowercase(),
            _ => "en".into(),
        }
    }
}

/// The language most of the words heard in `p`'s files were in; None when no file of `p` has a word in a known
/// language. Only finished transcripts count: an export freezes those alone, so the preview draws the
/// same words in the same CJK faces as the export, even while a file is still being transcribed. Only `p`'s own files
/// count (`speech` may still hold a file the project no longer has), pauses are no words, and a tie goes to the
/// language first in alphabetical order, so the answer never depends on the map's order. O(words + transcripts ×
/// assets).
fn spoken_language(p: &Project, speech: &Speech) -> Option<String> {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (asset, t) in &speech.transcripts {
        if !t.complete || t.language.is_empty() || p.asset(asset).is_none() {
            continue;
        }
        let words = t.words.iter().filter(|w| w.kind != WordKind::Pause).count();
        match counts.iter_mut().find(|(l, _)| *l == t.language) {
            Some((_, n)) => *n += words,
            None => counts.push((&t.language, words)),
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .max_by(|(la, a), (lb, b)| a.cmp(b).then_with(|| lb.cmp(la)))
        .map(|(l, _)| l.to_ascii_lowercase())
}
