//! The languages transcription knows, by their Whisper codes, with their English names: how Rust's English (MCP
//! replies, logs, the prompt's examples) names a language a code carries ("in German", never "in de").
//! The UI names them in the person's language (`languageName`, `Intl.DisplayNames`).

/// Whisper's language codes and their English names, sorted by code (a binary search finds one).
const NAMES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("bo", "Tibetan"),
    ("br", "Breton"),
    ("bs", "Bosnian"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("eu", "Basque"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fo", "Faroese"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jw", "Javanese"),
    ("ka", "Georgian"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("la", "Latin"),
    ("lb", "Luxembourgish"),
    ("ln", "Lingala"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mg", "Malagasy"),
    ("mi", "Māori"),
    ("mk", "Macedonian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("my", "Burmese"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("nn", "Norwegian Nynorsk"),
    ("no", "Norwegian"),
    ("oc", "Occitan"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sa", "Sanskrit"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sn", "Shona"),
    ("so", "Somali"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("tg", "Tajik"),
    ("th", "Thai"),
    ("tk", "Turkmen"),
    ("tl", "Tagalog"),
    ("tr", "Turkish"),
    ("tt", "Tatar"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("yue", "Cantonese"),
    ("zh", "Chinese"),
];

/// The English name of the language with Whisper code `code` ("de" → "German"); None for a code it doesn't know.
pub fn english_name(code: &str) -> Option<&'static str> {
    NAMES
        .binary_search_by(|(c, _)| (*c).cmp(code))
        .ok()
        .map(|i| NAMES[i].1)
}

/// Whisper's own spelling of `code`, in any case ("EN" → "en"); None for a code it doesn't know.
pub fn known_code(code: &str) -> Option<&'static str> {
    NAMES
        .binary_search_by(|(c, _)| c.bytes().cmp(code.bytes().map(|b| b.to_ascii_lowercase())))
        .ok()
        .map(|i| NAMES[i].0)
}

/// Every language transcription knows, by its code, in code order.
pub fn codes() -> impl Iterator<Item = &'static str> {
    NAMES.iter().map(|(code, _)| *code)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_code_is_known_in_any_case() {
        assert_eq!(super::known_code("EN"), Some("en"));
        assert_eq!(super::known_code("yue"), Some("yue"));
        assert_eq!(super::known_code("xx"), None);
    }

    #[test]
    fn the_names_are_sorted_by_code_each_once() {
        assert!(super::NAMES.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(super::NAMES.len(), 100, "Whisper's languages");
        assert_eq!(super::codes().count(), 100);
    }
}
