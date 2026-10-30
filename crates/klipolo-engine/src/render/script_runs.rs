//! Text in Chinese, Japanese or Korean asks for the face of its language explicitly, so kanji in a Japanese
//! title draw in the Japanese face and not a Chinese one. Which script each CJK stretch takes is decided once, by
//! `klipolo_fonts::cjk::cjk_segments`, which the webview mirrors through a parity fixture and the
//! missing-fonts check calls directly. This file only maps its segments, and the punctuation standing alone
//! (`lone_marks`), to families.

use std::ops::Range;
use std::sync::Arc;

use klipolo_fonts::cjk::{CjkCover, cjk_segments, lone_marks};
use klipolo_fonts::fallback::{NOTDEF, cjk_alias};
use klipolo_model::text_language::CjkScript;

use crate::render::fonts;

/// A text's runs: byte ranges covering it, each with the family it asks for.
pub type Runs = Vec<(Range<usize>, Arc<str>)>;

/// The family punctuation standing alone asks for: its script's Noto Sans once downloaded, else Adobe NotDef,
/// whose box the webview draws too (`markStack`). Never left to cosmic-text's last stage, whose system faces the
/// webview can't know.
fn mark_family(script: CjkScript) -> &'static str {
    if fonts::cjk_registered(script) {
        cjk_alias(script)
    } else {
        NOTDEF
    }
}

/// `text` cut into runs, each with the family to ask: the CJK segments their script's fallback alias, punctuation
/// standing alone `mark_family`, the rest `primary`. Characters `primary` covers (`cover`, e.g. a picked Hiragino) stay
/// with `primary`.
pub fn runs(text: &str, primary: &str, language: &str, cover: CjkCover) -> Runs {
    let primary: Arc<str> = Arc::from(primary);
    let mut stretches: Runs = cjk_segments(text, language, cover)
        .into_iter()
        .map(|(range, script)| (range, Arc::from(cjk_alias(script))))
        .chain(
            lone_marks(text, language, cover)
                .into_iter()
                .map(|(range, script)| (range, Arc::from(mark_family(script)))),
        )
        .collect();
    stretches.sort_unstable_by_key(|(range, _)| range.start);
    let mut out: Runs = Vec::with_capacity(stretches.len() * 2 + 1);
    let mut at = 0;
    for (range, family) in stretches {
        if range.start > at {
            out.push((at..range.start, primary.clone()));
        }
        at = range.end;
        out.push((range, family));
    }
    if at < text.len() || out.is_empty() {
        out.push((at..text.len(), primary));
    }
    out
}
