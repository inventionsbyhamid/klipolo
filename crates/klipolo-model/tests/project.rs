//! Format 5: a text names its font and where it comes from, and the text language the CJK faces follow.

use std::sync::Arc;

use klipolo_model::font::{font_ok, FontRef, FontSource};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::text_language::{cjk_script, text_language, CjkScript};

#[test]
fn a_text_style_names_its_font_and_where_it_comes_from() {
    let style = klipolo_model::timeline::TextStyle::default();
    let json = serde_json::to_value(&style).unwrap();
    assert_eq!(
        json["font"],
        serde_json::json!({ "family": "Inter", "source": { "type": "bundled" } })
    );
    assert_eq!(json["italic"], false);
    let google = FontRef {
        family: "Poppins".into(),
        source: FontSource::Google {
            version: "v22".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&google).unwrap(),
        serde_json::json!({ "family": "Poppins", "source": { "type": "google", "version": "v22" } })
    );
}

/// Format 5 added text fonts; the caption settings made 6: no migration.
#[test]
fn a_format_4_or_5_project_opens_as_made_with_an_older_version() {
    for version in [4, 5] {
        let old = serde_json::json!({ "version": version, "id": "p", "name": "old" });
        let err =
            klipolo_model::load::load_project(&serde_json::to_string(&old).unwrap()).unwrap_err();
        assert!(matches!(
            err,
            klipolo_model::error::ModelError::OlderVersion
        ));
    }
}

#[test]
fn a_font_family_must_be_one_line_of_at_most_100_characters() {
    assert!(font_ok(&FontRef::inter()));
    assert!(!font_ok(&FontRef {
        family: "".into(),
        source: FontSource::Installed
    }));
    assert!(!font_ok(&FontRef {
        family: "A\nB".into(),
        source: FontSource::Installed
    }));
    assert!(!font_ok(&FontRef {
        family: "x".repeat(101),
        source: FontSource::Installed
    }));
    assert!(!font_ok(&FontRef {
        family: "Inter".into(),
        source: FontSource::Google {
            version: "31".into()
        }
    }));
}

#[test]
fn the_text_language_is_the_ui_locale_until_captions_or_transcripts_say_otherwise() {
    let p = klipolo_model::project::Project::new("p");
    assert_eq!(text_language(&p, None, "ja-JP"), "ja");
    assert_eq!(text_language(&p, None, "zh-Hant-TW"), "zh-Hant-TW");
    assert_eq!(text_language(&p, None, "en_US.UTF-8"), "en");
    assert_eq!(cjk_script("zh-TW"), Some(CjkScript::Tc));
    assert_eq!(cjk_script("zh-Hans"), Some(CjkScript::Sc));
    assert_eq!(cjk_script("ko"), Some(CjkScript::Kr));
    assert_eq!(cjk_script("en"), None);
}

/// Without a caption language, the transcripts say it: the language most of the project's words were heard in.
/// Pauses aren't words, a file the project no longer has doesn't count, a tie never depends on the map's order, and the
/// captions' own language still wins.
#[test]
fn the_transcripts_language_is_the_one_most_of_the_projects_words_were_heard_in() {
    let file = |id: &str| Asset {
        id: id.into(),
        kind: AssetKind::Audio,
        path: format!("/media/{id}.wav"),
        name: format!("{id}.wav"),
        duration_us: 60_000_000,
        width: 0,
        height: 0,
        fps: 0.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    };
    let mut p = Project::new("p");
    p.assets = vec![file("a"), file("b")];
    let words = |n: i64, kind: WordKind| -> Vec<Word> {
        (0..n)
            .map(|i| Word {
                text: if kind == WordKind::Pause { "" } else { "w" }.into(),
                start_us: i * 1_000_000,
                end_us: i * 1_000_000 + 500_000,
                confidence: 1.0,
                kind,
            })
            .collect()
    };
    let heard = |language: &str, words: Vec<Word>| {
        Arc::new(Transcript {
            key: "k".into(),
            language: language.into(),
            model: "m".into(),
            covered_us: 0,
            complete: true,
            words,
        })
    };
    let mut speech = Speech::default();
    speech
        .transcripts
        .insert("a".into(), heard("ja", words(3, WordKind::Word)));
    speech.transcripts.insert(
        "b".into(),
        heard(
            "en",
            [words(2, WordKind::Filler), words(5, WordKind::Pause)].concat(),
        ),
    );
    assert_eq!(
        text_language(&p, Some(&speech), "en"),
        "ja",
        "3 words against 2: pauses aren't words"
    );
    speech
        .transcripts
        .insert("gone".into(), heard("en", words(10, WordKind::Word)));
    assert_eq!(
        text_language(&p, Some(&speech), "en"),
        "ja",
        "only the project's files"
    );
    speech
        .transcripts
        .insert("b".into(), heard("en", words(3, WordKind::Word)));
    assert_eq!(
        text_language(&p, Some(&speech), "ja"),
        "en",
        "a tie: the first alphabetically"
    );
    assert_eq!(
        text_language(&p, Some(&Speech::default()), "ko"),
        "ko",
        "no words: the interface's"
    );
    // A transcript still being made doesn't count until it is whole: an export freezes finished
    // transcripts only, so the preview's CJK faces follow the same words the export draws.
    let mut partial = (*heard("ja", words(9, WordKind::Word))).clone();
    partial.complete = false;
    speech.transcripts.insert("a".into(), Arc::new(partial));
    assert_eq!(
        text_language(&p, Some(&speech), "ja"),
        "en",
        "the partial Japanese transcript waits"
    );
    p.captions.language = Some("zh-Hant".into());
    assert_eq!(text_language(&p, Some(&speech), "en"), "zh-Hant");
}
