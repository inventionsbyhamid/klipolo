//! Speech in a project: what was said in each source file, its loudness, and the person's corrections.
//!
//! A `Transcript` belongs to one source file, in that file's sound time (a picture file's time less its sound lead,
//! `Asset::lead_us`; a sound file's own time), so cuts, trims, speed changes and reorders never change it. It is
//! immutable and lives outside the project, keyed by the file's content. The person's corrections are
//! project data (`Project::word_fixes`, `Op::FixWords`) in the same sound time, so they follow the words through every
//! edit and undo. `map` puts words on the timeline for one revision; `silence` finds pauses from loudness alone. The
//! caption pages the engine draws are made from them in `crate::captions`.

mod fill;
pub mod map;
pub mod silence;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::Id;
use crate::time::{RangeUs, Us};

pub use map::{
    run_spans, spans, spoken, spoken_from, voice_runs, word_cut, SpeechSpan, Spoken, SpokenWord,
    VoiceRun,
};

/// A source file's content key (`klipolo_speech::key::speech_key`): its length and a hash of three sampled MiB.
pub type SpeechKey = String;
/// One loudness value per 100 ms (the PCM cache's energy windows).
pub const LEVEL_WINDOW_US: Us = 100_000;
/// A gap between words this long or longer is a pause.
pub const PAUSE_US: Us = 700_000;
/// How much a word cut reaches past the words it removes, at most (`word_cut`).
pub const CUT_PAD_US: Us = 120_000;
/// A word fix's longest text, in characters.
pub const FIX_MAX_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WordKind {
    Word,
    /// "um", "uh", "äh" … (best-effort: a transcriber often drops them).
    Filler,
    /// A gap of `PAUSE_US` or more between words; its text is empty.
    Pause,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Word {
    /// As heard, with its punctuation ("Notion."); empty for a pause.
    pub text: String,
    /// In the source file's sound time.
    pub start_us: Us,
    pub end_us: Us,
    /// 0 to 1, the model's.
    pub confidence: f32,
    pub kind: WordKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Transcript {
    pub key: SpeechKey,
    /// The language's ISO 639-1 style code ("en", "de", "ja").
    pub language: String,
    /// The speech model's pin id.
    pub model: String,
    /// How far into the source the words reach: its length once complete, less while a job is still running.
    pub covered_us: Us,
    pub complete: bool,
    /// Sorted by start, never overlapping, inside `0..=MAX_TIME_US` (the speech store checks a transcript it reads).
    pub words: Vec<Word>,
}

/// A person's correction of the words heard in `[start_us, end_us)` of `asset_id` (its sound time): captions, the
/// Script view and chat's snippets read `text` instead (a text override on what the transcript heard).
// `Eq`, so the project's shared list compares by pointer first (`Arc`'s `==`): an edit that leaves it alone diffs it in
// O(1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WordFix {
    pub asset_id: Id,
    pub start_us: Us,
    pub end_us: Us,
    pub text: String,
}

/// What `Op::FixWords` and validation accept as a fix's text: one line of 1 to `FIX_MAX_CHARS` characters with
/// something to see (`fix_text_blank`). No control characters (`\n`, `\r`, `\t`, U+0085 …), no line or paragraph
/// separators (U+2028, U+2029) and no bidi embedding, override or isolate marks: a caption would break its line on
/// them, or reorder the words around it. An agent through MCP can send any of them.
pub fn fix_text_ok(text: &str) -> bool {
    let n = text.chars().count();
    (1..=FIX_MAX_CHARS).contains(&n)
        && !fix_text_blank(text)
        && !text.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                )
        })
}

/// Characters that take two columns of a caption line: Han, kana, Hangul and full-width forms.
pub fn is_wide(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x3FFFD
    )
}

/// A combining mark that belongs to the letter before it: a vowel sign, virama, nukta or tone mark of the Indic, Thai,
/// Lao, Myanmar and Khmer scripts, Hebrew points, Arabic vowel marks, or a combining accent. Not alphabetic to Rust
/// (Tamil's pulli in "வணக்கம்", Thai's tone mark in "นี้", Hindi's nukta in "साफ़"), but trimming one off a word's edge
/// changes the word. The scripts' own punctuation (the danda "।", Arabic's comma) is no mark.
pub fn is_mark(c: char) -> bool {
    match c as u32 {
        0x0300..=0x036F | 0x0483..=0x0489 | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF => {
            true
        }
        // Hebrew points, less the maqaf, paseq and sof pasuq.
        0x0591..=0x05C7 => !matches!(c as u32, 0x05BE | 0x05C0 | 0x05C3 | 0x05C6),
        0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 => true,
        0x06E7..=0x06E8 | 0x06EA..=0x06ED | 0x08D3..=0x08FF => true,
        // Devanagari to Sinhala: everything not a letter or digit, less the dandas and the abbreviation sign.
        0x0900..=0x0DFF => {
            !c.is_alphanumeric() && !matches!(c as u32, 0x0964 | 0x0965 | 0x0970 | 0x0DF4)
        }
        // Thai and Lao: their vowel signs and tone marks, not their punctuation (ฯ, ๏, ๚, ๛).
        0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECE => {
            true
        }
        0x102B..=0x103E | 0x1056..=0x1059 | 0x105E..=0x1060 | 0x1062..=0x1064 | 0x1067..=0x106D => {
            true
        }
        0x1071..=0x1074 | 0x1082..=0x108D | 0x108F | 0x109A..=0x109D => true,
        0x17B4..=0x17D3 | 0x17DD => true,
        _ => false,
    }
}

/// Characters of scripts written without spaces between words (Han, kana and their punctuation and full-width forms:
/// Chinese and Japanese): caption words that meet on two of them join without a space. Not Hangul: Korean separates
/// its words with spaces.
pub fn joins_unspaced(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x303E
            | 0x3041..=0x312F
            | 0x3190..=0x31FF
            | 0x3300..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF9F
            | 0x20000..=0x3FFFD
    )
}

/// The byte ranges of `text`'s words, as captions show them: its runs between whitespace, with each Chinese or Japanese
/// character (Han, kana) a word of its own (words of `joins_unspaced` characters meet without a space, so they read as
/// written). A character's punctuation ("，", "。", a closing bracket), Japanese's small kana, long-vowel and repeat
/// marks stay with the character before it; an opening bracket goes with the one after it; letters or digits of
/// another script start a word of their own ("用Final" is "用", "Final"). Text with none of those characters is split
/// on whitespace alone. O(characters).
pub fn word_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    // The word being read holds a letter or digit (not only an opening bracket), and its last character was Chinese
    // or Japanese (a character or its punctuation).
    let (mut has_letter, mut after_unspaced) = (false, false);
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push(s..i);
            }
            (has_letter, after_unspaced) = (false, false);
            continue;
        }
        let unspaced = joins_unspaced(c);
        let breaks = has_letter
            && if unspaced {
                (c.is_alphabetic() && !attaches_back(c)) || (OPENING.contains(&c) && after_unspaced)
            } else {
                after_unspaced && (c.is_alphanumeric() || OPENING.contains(&c))
            };
        if breaks {
            if let Some(s) = start.replace(i) {
                out.push(s..i);
            }
            has_letter = false;
        }
        start.get_or_insert(i);
        has_letter |= c.is_alphanumeric();
        after_unspaced = unspaced;
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out
}

/// `text`'s words (`word_ranges`) with one space between each: a Chinese or Japanese sentence as one word per
/// character. Text with no Chinese or Japanese character comes back as it was.
pub fn spaced_words(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(joins_unspaced) {
        return std::borrow::Cow::Borrowed(text);
    }
    let words: Vec<&str> = word_ranges(text).into_iter().map(|r| &text[r]).collect();
    std::borrow::Cow::Owned(words.join(" "))
}

/// Opening brackets and quotes: they go with the character after them.
const OPENING: [char; 12] = [
    '「', '『', '（', '【', '〈', '《', '〔', '〖', '“', '‘', '［', '｛',
];

/// Japanese's small kana, long-vowel mark and repeat marks: part of the sound of the character before them.
fn attaches_back(c: char) -> bool {
    matches!(
        c as u32,
        0x3041 | 0x3043 | 0x3045 | 0x3047 | 0x3049 | 0x3063 | 0x3083 | 0x3085 | 0x3087 | 0x308E | 0x3095 | 0x3096
            | 0x30A1 | 0x30A3 | 0x30A5 | 0x30A7 | 0x30A9 | 0x30C3 | 0x30E3 | 0x30E5 | 0x30E7 | 0x30EE | 0x30F5
            | 0x30F6 | 0x31F0..=0x31FF | 0x30FC | 0x309D | 0x309E | 0x30FD | 0x30FE | 0x3005 | 0xFF67..=0xFF70
            | 0xFF9E | 0xFF9F
    )
}

/// `text` without Unicode's tag characters (U+E0000–E007F): they draw nothing, so a correction holding them could carry
/// text the person never sees but an agent reads. A correction is stored without them.
pub fn without_tags(text: &str) -> std::borrow::Cow<'_, str> {
    let tag = |c: char| matches!(c, '\u{E0000}'..='\u{E007F}');
    if text.contains(tag) {
        text.chars().filter(|&c| !tag(c)).collect::<String>().into()
    } else {
        text.into()
    }
}

/// A fix's text with characters that show nothing only (spaces, zero-width and other format marks, variation
/// selectors, fillers, tags): a caption word nobody could see, refused with its own message.
pub fn fix_text_blank(text: &str) -> bool {
    !text.is_empty() && text.chars().all(invisible)
}

/// A character that draws nothing: white space, a control character or an invisible format character.
fn invisible(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{2800}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E007F}'
                | '\u{E0100}'..='\u{E01EF}'
        )
}

/// A voice source's loudness, dBFS per 100 ms window from its start.
#[derive(Clone, Debug, PartialEq)]
pub struct Levels {
    pub db: Arc<[f32]>,
}

/// What the compiler, the captions and the Script view know about the open project's speech, by asset id. Cheap to
/// clone: everything is shared.
#[derive(Clone, Debug, Default)]
pub struct Speech {
    pub transcripts: HashMap<Id, Arc<Transcript>>,
    pub levels: HashMap<Id, Arc<Levels>>,
    /// Topic searches the app resolved for this request before compiling: the call's text → timeline ranges,
    /// best first.
    pub topics: HashMap<String, Vec<RangeUs>>,
}
