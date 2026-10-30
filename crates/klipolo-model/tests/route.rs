//! The router's vocabulary: which family of speech models takes a language, and the engine that runs it.

use klipolo_model::speech::route::{
    base_language, family_for, Engine, Family, FINDER_LANGUAGES, NEMOTRON_LANGUAGES,
    PARAKEET_LANGUAGES,
};

#[test]
fn parakeet_takes_its_25_languages() {
    assert_eq!(PARAKEET_LANGUAGES.len(), 25);
    for l in [
        "en", "de", "fr", "es", "pt", "ru", "uk", "pl", "nl", "it", "sv", "el", "mt",
    ] {
        assert_eq!(family_for(l), Family::Parakeet, "{l}");
    }
}

#[test]
fn vietnamese_japanese_and_chinese_go_to_nemotron() {
    assert_eq!(NEMOTRON_LANGUAGES, ["vi", "ja", "zh"]);
    for l in NEMOTRON_LANGUAGES {
        assert_eq!(family_for(l), Family::Nemotron);
    }
}

#[test]
fn every_other_language_goes_to_whisper() {
    for l in ["ko", "ar", "hi", "tr", "yue", "id", "th", "he", ""] {
        assert_eq!(family_for(l), Family::Whisper, "{l:?}");
    }
}

#[test]
fn region_and_script_tags_route_by_their_language_and_the_default_engine_is_whisper() {
    assert_eq!(
        (
            base_language("pt-BR"),
            base_language("zh-Hans"),
            base_language("EN")
        ),
        ("pt", "zh", "en")
    );
    assert_eq!(family_for("pt-BR"), Family::Parakeet);
    assert_eq!(Engine::default(), Engine::Whisper);
}

#[test]
fn each_family_runs_on_its_engine() {
    assert_eq!(Family::Parakeet.engine(), Engine::Transcribe);
    assert_eq!(Family::Nemotron.engine(), Engine::Transcribe);
    assert_eq!(Family::Whisper.engine(), Engine::Whisper);
}

#[test]
fn the_language_finder_chooses_among_klipolo_s_nine_languages() {
    // Unrestricted, VoxLingua107 heard the synthetic German as Welsh; among these nine it heard German.
    assert_eq!(
        FINDER_LANGUAGES,
        ["en", "de", "fr", "es", "pt", "ru", "vi", "ja", "zh"]
    );
}

#[test]
fn the_names_go_to_the_ui_in_camel_case() {
    assert_eq!(
        serde_json::to_string(&Engine::Transcribe).unwrap(),
        "\"transcribe\""
    );
    assert_eq!(
        serde_json::to_string(&Family::Parakeet).unwrap(),
        "\"parakeet\""
    );
}
