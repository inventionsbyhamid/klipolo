//! Which face draws each stretch of Chinese, Japanese or Korean. A run is a stretch of CJK characters with
//! only common characters (spaces, digits, punctuation, emoji, full-width Latin) between them; a letter of another
//! script ends it. Kana are Japanese and Hangul Korean; the Han characters of a run holding kana are Japanese, of a run
//! holding Hangul Korean; any other run's Han follows the text language (Simplified Chinese when it isn't CJK). The
//! engine's script runs, the missing-fonts check and, through `cjk-segments-parity.json`, the webview all use this.
//!
//! Decided here, once:
//! - A run is CJK text only when it holds a Han, Bopomofo, kana or Hangul character. CJK punctuation and full-width
//!   forms join such a run's segments, but on their own (a "！" or "「…」" around Latin words) they aren't CJK text:
//!   they get no segment and need no download. Unless the text's face covers Han, they draw as `lone_marks` says.
//! - Full-width Latin letters (U+FF21–FF3A, U+FF41–FF5A) are Latin. They are set inside CJK text, so they don't end a
//!   run, but they are outside `CJK_BLOCKS`: never a segment, and never boxed by the webview's NotDef.
//! - Punctuation standing alone (`lone_marks`) draws in the text language's Noto Sans once downloaded, else Adobe
//!   NotDef's box, in the engine and the webview alike.

use std::ops::Range;

use klipolo_model::text_language::{CjkScript, cjk_script};
use serde::Serialize;
use unicode_script::{Script, UnicodeScript};

/// What the text's own face covers; covered characters draw in it and need no fallback.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, serde::Deserialize, ts_rs::TS,
)]
#[ts(export)]
pub struct CjkCover {
    pub han: bool,
    pub kana: bool,
    pub hangul: bool,
}

impl CjkCover {
    pub const NONE: CjkCover = CjkCover {
        han: false,
        kana: false,
        hangul: false,
    };
}

/// The CJK blocks (first and last code point): the one table both the engine's segments and the webview's NotDef
/// `unicode-range` come from. The four emoji inside them (〰 〽 ㊗ ㊙) are left out, so they draw in the
/// emoji face in both, and so are the full-width Latin letters and the ideographic space U+3000: a space
/// draws blank in any face (rustybuzz and WebKit both), so it is never boxed.
pub const CJK_BLOCKS: &[(u32, u32)] = &[
    (0x1100, 0x11FF), // Hangul Jamo
    (0x2E80, 0x2FDF), // CJK and Kangxi radicals
    (0x2FF0, 0x2FFF), // ideographic description
    (0x3001, 0x302F), // CJK symbols and punctuation, without the ideographic space (U+3000) …
    (0x3031, 0x303C),
    (0x303E, 0x303F),
    (0x3040, 0x30FF), // Hiragana and Katakana
    (0x3100, 0x312F), // Bopomofo
    (0x3130, 0x318F), // Hangul compatibility jamo
    (0x3190, 0x31FF), // Kanbun, Bopomofo extended, strokes, Katakana extensions
    (0x3200, 0x3296), // enclosed CJK, CJK compatibility …
    (0x3298, 0x3298),
    (0x329A, 0x33FF),
    (0x3400, 0x4DBF), // Extension A
    (0x4E00, 0x9FFF), // CJK unified ideographs
    (0xA960, 0xA97F), // Hangul Jamo extended A
    (0xAC00, 0xD7FF), // Hangul syllables, Jamo extended B
    (0xF900, 0xFAFF), // compatibility ideographs
    (0xFE30, 0xFE4F), // CJK compatibility forms
    (0xFF00, 0xFF20), // half-width and full-width forms, without the full-width Latin letters …
    (0xFF3B, 0xFF40),
    (0xFF5B, 0xFFEF),
    (0x20000, 0x3134F), // Extensions B to G
];

pub fn in_cjk_blocks(c: char) -> bool {
    let u = c as u32;
    CJK_BLOCKS.iter().any(|&(a, b)| (a..=b).contains(&u))
}

/// `CJK_BLOCKS` as a CSS `unicode-range` ("U+1100-11FF, …"), exported in `font-fallbacks.json`.
pub fn cjk_ranges_css() -> String {
    let one = |&(a, b): &(u32, u32)| {
        if a == b {
            format!("U+{a:X}")
        } else {
            format!("U+{a:X}-{b:X}")
        }
    };
    CJK_BLOCKS.iter().map(one).collect::<Vec<_>>().join(", ")
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Han,
    Kana,
    Hangul,
    /// Spaces, digits, punctuation, emoji, full-width Latin: they never end a run.
    Common,
    Other,
}

fn kind(c: char) -> Kind {
    match c.script() {
        Script::Han | Script::Bopomofo => Kind::Han,
        Script::Hiragana | Script::Katakana => Kind::Kana,
        Script::Hangul => Kind::Hangul,
        Script::Common | Script::Inherited => Kind::Common,
        _ if matches!(c, '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}') => Kind::Common,
        _ => Kind::Other,
    }
}

/// An open run: its bytes, and whether it holds kana, Hangul, or any CJK letter at all.
#[derive(Clone, Copy)]
struct Run {
    start: usize,
    end: usize,
    kana: bool,
    hangul: bool,
    letters: bool,
}

/// The CJK stretches of `text` that need a CJK fallback (byte ranges), each with the script whose face draws it, for a
/// text in `language` whose own face covers `cover`.
pub fn cjk_segments(text: &str, language: &str, cover: CjkCover) -> Vec<(Range<usize>, CjkScript)> {
    let default_han = cjk_script(language).unwrap_or(CjkScript::Sc);
    // Pass 1: the runs. A CJK character (or CJK punctuation) opens or extends one; other common characters extend an
    // open one; anything else (Latin letters…) closes it.
    let mut runs: Vec<Run> = Vec::new();
    let mut open: Option<Run> = None;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        match kind(c) {
            Kind::Other => runs.extend(open.take()),
            Kind::Common if !in_cjk_blocks(c) => {
                if let Some(r) = open.as_mut() {
                    r.end = end;
                }
            }
            k => {
                let r = open.get_or_insert(Run {
                    start: i,
                    end,
                    kana: false,
                    hangul: false,
                    letters: false,
                });
                r.end = end;
                r.kana |= k == Kind::Kana;
                r.hangul |= k == Kind::Hangul;
                r.letters |= k != Kind::Common;
            }
        }
    }
    runs.extend(open);
    // Pass 2: only characters inside the CJK blocks get a segment, the very characters the webview's NotDef range
    // covers, so ASCII spaces, digits and emoji stay with the text's own face in both. Covered characters are left
    // out too, and so are runs of punctuation alone.
    let mut out: Vec<(Range<usize>, CjkScript)> = Vec::new();
    for run in runs.into_iter().filter(|r| r.letters) {
        let han = if run.kana {
            CjkScript::Jp
        } else if run.hangul {
            CjkScript::Kr
        } else {
            default_han
        };
        let mut last: Option<CjkScript> = None;
        for (j, c) in text[run.start..run.end].char_indices() {
            let (i, e) = (run.start + j, run.start + j + c.len_utf8());
            let script = if !in_cjk_blocks(c) {
                None
            } else {
                match kind(c) {
                    Kind::Kana => (!cover.kana).then_some(CjkScript::Jp),
                    Kind::Hangul => (!cover.hangul).then_some(CjkScript::Kr),
                    Kind::Han => (!cover.han).then_some(han),
                    // CJK punctuation (。、「」ー) takes the script before it, else the run's Han script.
                    _ => (!cover.han).then(|| last.unwrap_or(han)),
                }
            };
            last = script;
            match (script, out.last_mut()) {
                (Some(s), Some((r, t))) if *t == s && r.end == i => r.end = e,
                (Some(s), _) => out.push((i..e, s)),
                (None, _) => {}
            }
        }
    }
    out
}

/// CJK punctuation standing outside every segment ("Hello！", "「Hello」", a full-width space), when the text's own face
/// doesn't cover Han (byte ranges, each with the text language's Han script). It needs no download, so it raises no
/// raises no code; but it must draw the same everywhere: once installed CJK
/// families are barred as fallbacks, the engine has no system face for it, while WebKit would reach PingFang. So the
/// engine and the webview both draw it in that script's downloaded Noto Sans, else in Adobe NotDef's box, as the
/// preview, the canvas and the export all show it. Never a system face either side picks for itself.
pub fn lone_marks(text: &str, language: &str, cover: CjkCover) -> Vec<(Range<usize>, CjkScript)> {
    if cover.han || !has_cjk(text) {
        return Vec::new();
    }
    let script = cjk_script(language).unwrap_or(CjkScript::Sc);
    let segments = cjk_segments(text, language, cover);
    let mut inside = segments.iter().peekable();
    let mut out: Vec<(Range<usize>, CjkScript)> = Vec::new();
    for (i, c) in text.char_indices() {
        while inside.peek().is_some_and(|(r, _)| r.end <= i) {
            inside.next();
        }
        let in_segment = inside.peek().is_some_and(|(r, _)| r.start <= i);
        if in_segment || !in_cjk_blocks(c) || kind(c) != Kind::Common {
            continue;
        }
        let e = i + c.len_utf8();
        match out.last_mut() {
            Some((r, _)) if r.end == i => r.end = e,
            _ => out.push((i..e, script)),
        }
    }
    out
}

/// Whether `text` has any character a CJK segment could take (a quick test before `cjk_segments`, so a text without
/// CJK allocates nothing for it).
pub fn has_cjk(text: &str) -> bool {
    text.chars().any(in_cjk_blocks)
}
