//! The speech router's vocabulary: which family of models hears a language, and which engine runs that
//! family. Pure: the driver asks it per row, with the person's language or the detected one, and decides what is
//! installed.
//!
//! - **Parakeet** (NVIDIA Parakeet TDT 0.6B v3, transcribe.cpp in klipolo-asr): its 25 European languages.
//! - **Nemotron** (Nemotron 3.5 ASR, transcribe.cpp): Vietnamese, Japanese and Chinese, which Parakeet doesn't take.
//! - **Whisper** (whisper.cpp): every other language, and any language nobody could tell.
//!
//! Each family is the default for every language it takes (Parakeet for all 25).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The program that runs a speech model. `Whisper` is whisper.cpp in Klipolo's own worker; `Transcribe` is
/// transcribe.cpp in `klipolo-asr`, whose ggml never meets whisper.cpp's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Engine {
    #[default]
    Whisper,
    Transcribe,
}

/// A family of speech models, by the languages it hears well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Family {
    Parakeet,
    Nemotron,
    Whisper,
}

impl Family {
    /// The program that runs this family's models.
    pub fn engine(self) -> Engine {
        match self {
            Self::Parakeet | Self::Nemotron => Engine::Transcribe,
            Self::Whisper => Engine::Whisper,
        }
    }
}

/// Parakeet TDT 0.6B v3's languages, by their base codes, sorted.
pub const PARAKEET_LANGUAGES: [&str; 25] = [
    "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv", "mt",
    "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
];

/// The languages Nemotron takes that Parakeet doesn't.
pub const NEMOTRON_LANGUAGES: [&str; 3] = ["vi", "ja", "zh"];

/// The languages klipolo-asr's language finder chooses among when a request names none (its bench): Klipolo's nine.
/// Unrestricted, VoxLingua107 heard the synthetic German as Welsh; among these it heard German. The driver names
/// Whisper's own languages instead: with Parakeet hearing all 25 European languages, a Polish or Korean
/// file must be named as itself, never as the nearest of nine, and the interface language's prior covers a close call.
pub const FINDER_LANGUAGES: [&str; 9] = ["en", "de", "fr", "es", "pt", "ru", "vi", "ja", "zh"];

/// `tag`'s language without its region or script, in its own spelling: "pt-BR" is "pt", "zh-Hans" is "zh", "EN" is
/// "en" (a code transcription doesn't know comes back as written).
pub fn base_language(tag: &str) -> &str {
    let base = tag.split(['-', '_']).next().unwrap_or_default();
    crate::languages::known_code(base).unwrap_or(base)
}

/// The family that hears `language` (any tag; "" when nobody could tell) best.
pub fn family_for(language: &str) -> Family {
    let base = base_language(language);
    if PARAKEET_LANGUAGES.contains(&base) {
        Family::Parakeet
    } else if NEMOTRON_LANGUAGES.contains(&base) {
        Family::Nemotron
    } else {
        Family::Whisper
    }
}
