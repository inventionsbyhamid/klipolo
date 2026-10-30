//! Which scripts a face covers, as bits. An installed family's coverage is computed once,
//! at scan time, from a few sample characters per script (about twenty cmap lookups a face), so "Covers my text", the
//! CJK cover of a picked installed face and the forbidden fallback list read bits, never font files.

use std::path::Path;
use std::sync::OnceLock;

use unicode_script::{Script, UnicodeScript};

use crate::bundled::BUNDLED;

/// One bit per script group a text can need.
pub type Coverage = u32;

pub const LATIN: Coverage = 1 << 0;
pub const LATIN_EXT: Coverage = 1 << 1;
pub const CYRILLIC: Coverage = 1 << 2;
pub const GREEK: Coverage = 1 << 3;
pub const VIETNAMESE: Coverage = 1 << 4;
pub const ARABIC: Coverage = 1 << 5;
pub const HEBREW: Coverage = 1 << 6;
pub const DEVANAGARI: Coverage = 1 << 7;
pub const THAI: Coverage = 1 << 8;
pub const HAN: Coverage = 1 << 9;
pub const KANA: Coverage = 1 << 10;
pub const HANGUL: Coverage = 1 << 11;
/// Han, kana or Hangul: a family covering any of them is never a fallback.
pub const CJK: Coverage = HAN | KANA | HANGUL;

/// Each group's sample characters: a face covers the group when its cmap maps all of them.
const SAMPLES: [(Coverage, &[char]); 12] = [
    (LATIN, &['A', 'z']),
    (LATIN_EXT, &['ł', 'ő']),
    (CYRILLIC, &['Ж', 'я']),
    (GREEK, &['Ω', 'λ']),
    (VIETNAMESE, &['ế', 'ơ']),
    (ARABIC, &['ب']),
    (HEBREW, &['א']),
    (DEVANAGARI, &['क']),
    (THAI, &['ก']),
    (HAN, &['中', '今']),
    (KANA, &['あ', 'カ']),
    (HANGUL, &['한']),
];

/// The groups `face`'s cmap has the samples of. Glyph 0 is `.notdef`, which covers nothing.
pub fn coverage_of(face: &ttf_parser::Face) -> Coverage {
    let has = |c: char| face.glyph_index(c).is_some_and(|g| g.0 != 0);
    SAMPLES
        .iter()
        .filter(|(_, samples)| samples.iter().all(|&c| has(c)))
        .fold(0, |bits, (bit, _)| bits | bit)
}

/// What the scan records of a face from its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceFacts {
    pub coverage: Coverage,
    pub variable: bool,
    /// Its own size (`sfnt::face_len`): a collection face's tables, else the file's.
    pub bytes: u64,
}

/// What the scan records of a face from its bytes (`data`, face `index` in a collection). None for bytes that aren't a
/// font.
pub fn face_facts(data: &[u8], index: u32) -> Option<FaceFacts> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    Some(FaceFacts {
        coverage: coverage_of(&face),
        variable: face.is_variable(),
        bytes: crate::sfnt::face_len(data, index).unwrap_or(data.len() as u64),
    })
}

/// `coverage_of` the face at `index` in the file at `path`; 0 for a file that can't be read or parsed.
pub fn coverage_of_file(path: &Path, index: u32) -> Coverage {
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    ttf_parser::Face::parse(&bytes, index).map_or(0, |face| coverage_of(&face))
}

/// The groups `text`'s letters need. Common characters (spaces, digits, punctuation, emoji) need none.
pub fn needs(text: &str) -> Coverage {
    text.chars().fold(0, |bits, c| bits | need_of(c))
}

fn need_of(c: char) -> Coverage {
    match c.script() {
        Script::Latin => latin_group(c),
        Script::Cyrillic => CYRILLIC,
        Script::Greek => GREEK,
        Script::Arabic => ARABIC,
        Script::Hebrew => HEBREW,
        Script::Devanagari => DEVANAGARI,
        Script::Thai => THAI,
        Script::Han | Script::Bopomofo => HAN,
        Script::Hiragana | Script::Katakana => KANA,
        Script::Hangul => HANGUL,
        _ => 0,
    }
}

/// Latin-1 is Latin; Vietnamese's own letters (ơ, ư and the Latin Extended Additional block) are Vietnamese; the rest
/// of Latin (ł, ő, ş…) is Latin-ext. Full-width Latin is set in CJK faces: it asks nothing of a Latin one.
fn latin_group(c: char) -> Coverage {
    match c as u32 {
        0..=0xFF => LATIN,
        0x1A0 | 0x1A1 | 0x1AF | 0x1B0 | 0x1EA0..=0x1EF9 => VIETNAMESE,
        0xFF00..=0xFFEF => 0,
        _ => LATIN_EXT,
    }
}

/// The coverage of the bundled family whose face is `BUNDLED[i]` (all of its faces together), read once from the
/// bundled folder `dir` (the files are pinned by SHA-256, so any install's folder gives the same answer). 0 when the
/// folder is missing (a broken install).
pub fn bundled_coverage(dir: Option<&Path>, i: usize) -> Coverage {
    static FACES: OnceLock<Vec<Coverage>> = OnceLock::new();
    let Some(dir) = dir else {
        return 0;
    };
    let faces = FACES.get_or_init(|| {
        BUNDLED
            .iter()
            .map(|b| coverage_of_file(&dir.join(b.file), 0))
            .collect()
    });
    let family = BUNDLED[i].family;
    BUNDLED
        .iter()
        .zip(faces)
        .filter(|(b, _)| b.family == family)
        .fold(0, |bits, (_, c)| bits | c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_needs_its_scripts_and_common_characters_need_nothing() {
        assert_eq!(needs("Hello, 2024! 😀"), LATIN);
        assert_eq!(needs("Łódź"), LATIN | LATIN_EXT);
        assert_eq!(needs("Привет"), CYRILLIC);
        assert_eq!(needs("Tiếng Việt"), LATIN | VIETNAMESE);
        assert_eq!(needs("今日は 한국"), HAN | KANA | HANGUL);
        assert_eq!(needs("สวัสดี"), THAI);
        assert_eq!(needs("！、。"), 0);
    }

    #[test]
    fn bundled_inter_covers_latin_and_cyrillic_and_no_cjk() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts");
        let inter = bundled_coverage(Some(&dir), 0);
        assert_eq!(
            inter & (LATIN | LATIN_EXT | CYRILLIC | GREEK),
            LATIN | LATIN_EXT | CYRILLIC | GREEK
        );
        assert_eq!(inter & CJK, 0);
        assert_eq!(bundled_coverage(None, 0), 0);
    }
}
