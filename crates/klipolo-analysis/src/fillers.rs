//! Filler sounds ("um", "uh") among a transcript's words, from the speech crates' one list per language
//! (`klipolo_speech_wire::timing::fillers`): only sounds that are never words, so "like" stays a word.

use klipolo_speech_wire::timing::fillers;

/// Whether `text` (a word as heard, in any case and with its punctuation: "Um,") is one of `language`'s filler sounds.
/// Its letters alone are compared, lowercased, so "Uh-huh" ("uhhuh") is a word. A virama (the sign that joins
/// Devanagari's "म्म") counts as a letter, so "हम्म" (a hum) is never "हम" ("we").
pub fn is_filler(language: &str, text: &str) -> bool {
    let heard = bare(text);
    !heard.is_empty() && fillers(language).iter().any(|f| bare(f) == heard)
}

fn bare(text: &str) -> String {
    text.chars()
        .filter(|&c| c.is_alphabetic() || c == '\u{94D}')
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_fillers_match_in_any_case_with_punctuation() {
        for w in [
            "um", "Um,", "UMM.", "uh", "Uh...", "uhh", "uhm", "er,", "Erm", "ah", "hm", "Hmm?",
            "mm", "mhm", "\"um\"",
        ] {
            assert!(is_filler("en", w), "{w}");
        }
    }

    #[test]
    fn words_and_empty_text_are_not_fillers() {
        for w in [
            "", "...", "-", "like", "umbrella", "huh", "her", "Uh-huh", "a", "I",
        ] {
            assert!(!is_filler("en", w), "{w}");
        }
    }

    #[test]
    fn each_language_has_its_own_fillers() {
        // "er" is a German word, so it is no German filler; "äh" is one.
        assert!(!is_filler("de", "er"));
        assert!(is_filler("de", "Äh,"));
        assert!(is_filler("en-US", "um"));
    }

    #[test]
    fn hindi_hums_are_fillers_and_its_words_are_not() {
        for w in ["हम्म", "उम्म,", "अं...", "Hmm", "um"] {
            assert!(is_filler("hi", w), "{w}");
        }
        for w in ["हम", "अ", "उन", "और", "है"] {
            assert!(!is_filler("hi", w), "{w}");
        }
        // English hums are no filler in a language whose list doesn't hold them ("um" is Portuguese for "a").
        assert!(!is_filler("pt", "um"));
    }
}
