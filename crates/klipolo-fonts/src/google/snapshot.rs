//! The snapshot's compact rows, as `scripts/build-font-catalogue.mjs` writes them (the fixture is the format's
//! reference), checked and mapped into `GoogleFamily`s.

use klipolo_model::font::{FontRef, FontSource, font_ok};
use serde::Deserialize;

use super::{GoogleFamily, GoogleFile};
use crate::bundled::{BUNDLED, Category};
use crate::coverage::{
    ARABIC, CYRILLIC, Coverage, DEVANAGARI, GREEK, HAN, HANGUL, HEBREW, KANA, LATIN, LATIN_EXT,
    THAI, VIETNAMESE,
};

/// The snapshot as `build-font-catalogue.mjs` writes it (the fixture is the format's reference).
#[derive(Deserialize)]
pub(super) struct Snapshot {
    pub(super) date: String,
    pub(super) sample: bool,
    pub(super) families: Vec<Row>,
}

/// One family, compact: name, slug, category, popularity and trending ranks, version, subsets, licence, files as
/// `[weight, italic, tail, bytes]`, and the menu file's tail.
#[derive(Deserialize)]
pub(super) struct Row {
    f: String,
    slug: String,
    c: Category,
    p: u32,
    t: u32,
    v: String,
    s: Vec<String>,
    l: String,
    files: Vec<(u16, bool, String, u64)>,
    menu: String,
}

/// `row` checked against the rules in `google`'s comment, and mapped.
pub(super) fn family(row: Row) -> Result<GoogleFamily, String> {
    let named = |what: &str| format!("the font catalogue's {} ({}) {what}", row.f, row.v);
    let font = FontRef {
        family: row.f.clone(),
        source: FontSource::Google {
            version: row.v.clone(),
        },
    };
    // Bidi controls could make one name read as another in the menu.
    if !font_ok(&font) || row.f.chars().any(bidi_control) {
        return Err(named("isn't a family name and version a project can hold"));
    }
    if row.c == Category::Fallback {
        return Err(named("has no menu category"));
    }
    if !matches!(row.l.as_str(), "ofl" | "apache" | "ufl") {
        return Err(named("has no known licence folder"));
    }
    if row.files.is_empty() {
        return Err(named("has no files"));
    }
    let mut faces: Vec<(u16, bool)> = row.files.iter().map(|&(w, i, _, _)| (w, i)).collect();
    faces.sort_unstable();
    if faces.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(named("lists a face twice"));
    }
    if let Some((w, _, tail, _)) = row
        .files
        .iter()
        .find(|(w, _, tail, _)| !(1..=1000).contains(w) || !tail_ok(tail))
    {
        return Err(named(&format!(
            "has a file that isn't a plain tail at a weight: {w} {tail}"
        )));
    }
    let bundled = BUNDLED
        .iter()
        .any(|b| b.category != Category::Fallback && b.family == row.f);
    Ok(GoogleFamily {
        coverage: subsets_coverage(&row.s),
        bundled,
        #[cfg(feature = "test-hosts")]
        test_base: None,
        family: row.f,
        slug: row.slug,
        category: row.c,
        popularity: row.p,
        trending: row.t,
        version: row.v,
        subsets: row.s,
        licence: row.l,
        files: row
            .files
            .into_iter()
            .map(|(weight, italic, url_tail, bytes)| GoogleFile {
                weight,
                italic,
                url_tail,
                bytes,
            })
            .collect(),
        // The menu file is only a preview's: one Google gives in another shape is left out, not the family.
        menu: if tail_ok(&row.menu) {
            row.menu
        } else {
            String::new()
        },
    })
}

/// The marks and embeddings that change a line's direction (Unicode's Bidi_Control): ALM, LRM, RLM, LRE to RLO, and
/// LRI to PDI.
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A plain path under `GSTATIC`: letters, digits, `/`, `.`, `_` and `-`, never absolute, never climbing (`..`), so a
/// download can only ever ask that host for a file.
fn tail_ok(tail: &str) -> bool {
    !tail.is_empty()
        && !tail.starts_with('/')
        && !tail.contains("..")
        && tail
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
}

/// The scripts Google's subsets cover, as the coverage bits a text's letters need (`coverage::needs`): "Covers my
/// text" keeps a family whose subsets include every script the text uses. A Japanese family covers kanji and kana, a
/// Chinese one Han, a Korean one Hangul; subsets for symbols and the like add nothing.
pub fn subsets_coverage(subsets: &[String]) -> Coverage {
    subsets.iter().fold(0, |bits, s| {
        bits | match s.as_str() {
            "latin" => LATIN,
            "latin-ext" => LATIN_EXT,
            "cyrillic" => CYRILLIC,
            "greek" => GREEK,
            "vietnamese" => VIETNAMESE,
            "arabic" => ARABIC,
            "hebrew" => HEBREW,
            "devanagari" => DEVANAGARI,
            "thai" => THAI,
            "japanese" => HAN | KANA,
            "chinese-simplified" | "chinese-traditional" | "chinese-hongkong" => HAN,
            "korean" => HANGUL,
            _ => 0,
        }
    })
}
