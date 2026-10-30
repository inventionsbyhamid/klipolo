use klipolo_fonts::cjk::{CjkCover, cjk_segments, lone_marks};
use klipolo_model::text_language::CjkScript::{self, *};

fn scripts<'a>(text: &'a str, language: &str) -> Vec<(&'a str, CjkScript)> {
    cjk_segments(text, language, CjkCover::NONE)
        .into_iter()
        .map(|(r, s)| (&text[r], s))
        .collect()
}

#[test]
fn kana_make_their_runs_han_japanese_and_hangul_korean() {
    // A Japanese title in an English project needs only the Japanese face.
    assert_eq!(scripts("今日は予算", "en"), [("今日は予算", Jp)]);
    assert_eq!(
        scripts("韓國語 한국어", "en"),
        [("韓國語", Kr), ("한국어", Kr)]
    );
    // Han with neither follows the project's language, Simplified Chinese when it isn't CJK.
    assert_eq!(scripts("東京", "en"), [("東京", Sc)]);
    assert_eq!(scripts("東京", "ja"), [("東京", Jp)]);
    assert_eq!(scripts("預算", "zh-Hant"), [("預算", Tc)]);
    // Runs are separate: Latin between them ends one.
    assert_eq!(
        scripts("Hi 東京 and タワー", "en"),
        [("東京", Sc), ("タワー", Jp)]
    );
    // CJK punctuation inside a run joins its segment.
    assert_eq!(scripts("今日、東京へ。", "en"), [("今日、東京へ。", Jp)]);
    // Digits, ASCII spaces and emoji keep the text's own face (outside the CJK blocks), but don't end the run.
    assert_eq!(scripts("東京2024年", "en"), [("東京", Sc), ("年", Sc)]);
    assert_eq!(scripts("今日😀は", "en"), [("今日", Jp), ("は", Jp)]);
    // The webview's NotDef range is the same table.
    assert!(klipolo_fonts::cjk::cjk_ranges_css().starts_with("U+1100-11FF, U+2E80-2FDF"));
}

/// CJK punctuation alone isn't CJK text (an English title with one "！" needs no download), and
/// full-width Latin letters are Latin set inside CJK text: they don't end a run, and never take a CJK face.
#[test]
fn punctuation_alone_needs_no_cjk_face_and_full_width_latin_stays_latin() {
    assert_eq!(scripts("Hello！", "en"), []);
    assert_eq!(scripts("「Hello」", "en"), []);
    assert_eq!(scripts("「東京」", "en"), [("「東京」", Sc)]);
    assert_eq!(scripts("番組ＮＨＫの", "en"), [("番組", Jp), ("の", Jp)]);
    assert!(!klipolo_fonts::cjk::in_cjk_blocks('Ａ'));
    assert!(klipolo_fonts::cjk::in_cjk_blocks('！'));
}

/// Punctuation standing alone draws the same in the engine and the webview, in the text
/// language's Noto Sans once downloaded, else Adobe NotDef's box; a face that covers Han draws it itself.
#[test]
fn punctuation_standing_alone_takes_the_text_languages_face() {
    let marks = |text: &str, language: &str, cover: CjkCover| -> Vec<(String, CjkScript)> {
        lone_marks(text, language, cover)
            .into_iter()
            .map(|(r, s)| (text[r].to_string(), s))
            .collect()
    };
    assert_eq!(
        marks("Hello！", "en", CjkCover::NONE),
        [("！".to_string(), Sc)]
    );
    assert_eq!(
        marks("「Hello」", "ja", CjkCover::NONE),
        [("「".to_string(), Jp), ("」".to_string(), Jp)]
    );
    // Punctuation inside CJK text is the segment's, never a lone mark.
    assert_eq!(
        marks("「東京」 Hi！", "en", CjkCover::NONE),
        [("！".to_string(), Sc)]
    );
    let han = CjkCover {
        han: true,
        kana: false,
        hangul: false,
    };
    assert!(marks("Hello！", "en", han).is_empty());
    assert!(marks("Plain text", "en", CjkCover::NONE).is_empty());
    // A full-width space is never a mark, nor boxed: it draws blank in the text's own face.
    assert!(marks("Hello\u{3000}world", "zh-Hans", CjkCover::NONE).is_empty());
    assert!(!klipolo_fonts::cjk::in_cjk_blocks('\u{3000}'));
}

#[test]
fn a_face_that_covers_the_characters_needs_no_fallback() {
    assert!(
        cjk_segments(
            "今日は",
            "en",
            CjkCover {
                han: true,
                kana: true,
                hangul: false
            }
        )
        .is_empty()
    );
    // Only what it covers: a Han-only face leaves the kana to the Japanese fallback.
    let han_only = CjkCover {
        han: true,
        kana: false,
        hangul: false,
    };
    let text = "今日は";
    let left: Vec<_> = cjk_segments(text, "en", han_only)
        .into_iter()
        .map(|(r, s)| (&text[r], s))
        .collect();
    assert_eq!(left, [("は", Jp)]);
}

/// Keys in sorted order, so the file reads the same whether or not klipolo-ai's `preserve_order` builds alongside.
fn sorted(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted).collect())
        }
        other => other,
    }
}

/// `apps/desktop/src/lib/fonts/cjk-segments-parity.json`: the TS mirror must give these answers. Rust is the
/// truth; the file is written here with `UPDATE_GOLDEN=1` and checked otherwise (the parity fixtures' pattern). Byte
/// ranges are UTF-8 offsets: the TS mirror converts them to string indices in its test.
///
/// The rows: CJK punctuation standing alone gets no segment, and a text whose own face covers CJK
/// a picked Hiragino covers Han and kana) leaves only what it lacks to the fallbacks. Each row also carries its `marks`
/// (`lone_marks`): punctuation standing alone, which both draw in the text language's Noto Sans, else NotDef's box.
#[test]
fn export_the_parity_fixture_for_the_webview() {
    const NONE: CjkCover = CjkCover::NONE;
    let han_kana = CjkCover {
        han: true,
        kana: true,
        hangul: false,
    };
    let han = CjkCover {
        han: true,
        kana: false,
        hangul: false,
    };
    let hangul = CjkCover {
        han: false,
        kana: false,
        hangul: true,
    };
    let cases = [
        ("今日は予算", "en", NONE),
        ("韓國語 한국어", "en", NONE),
        ("東京", "en", NONE),
        ("東京", "ja", NONE),
        ("預算", "zh-Hant", NONE),
        ("预算", "ko", NONE),
        ("Hi 東京 and タワー", "en", NONE),
        ("今日、東京へ。", "en", NONE),
        ("東京2024年", "en", NONE),
        ("今日😀は", "en", NONE),
        ("ㄅㄆㄇ", "zh-Hant", NONE),
        ("「予算」", "ja", NONE),
        ("Plain text", "ja", NONE),
        ("Hello！", "en", NONE),
        ("番組ＮＨＫの", "en", NONE),
        // Punctuation alone: no segment, whatever the language.
        ("「Hello」", "ja", NONE),
        ("Hello\u{3000}world、ok", "zh-Hans", NONE),
        // The text's own face covers some CJK.
        ("今日は", "en", han_kana),
        ("今日、東京へ。", "ja", han_kana),
        ("今日は", "en", han),
        ("韓國語 한국어", "en", hangul),
        // Punctuation alone takes the text language's face, unless the face covers Han.
        ("「東京」 Hi！", "ko", NONE),
        ("Hello！", "en", han),
    ];
    let rows: Vec<_> = cases
        .iter()
        .map(|(t, l, cover)| {
            let segments: Vec<_> = cjk_segments(t, l, *cover)
                .into_iter()
                .map(|(r, s)| serde_json::json!([r.start, r.end, s]))
                .collect();
            let marks: Vec<_> = lone_marks(t, l, *cover)
                .into_iter()
                .map(|(r, s)| serde_json::json!([r.start, r.end, s]))
                .collect();
            serde_json::json!({ "text": t, "language": l, "cover": cover, "segments": segments, "marks": marks })
        })
        .collect();
    let json =
        serde_json::to_string_pretty(&sorted(serde_json::Value::Array(rows))).unwrap() + "\n";
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/lib/fonts/cjk-segments-parity.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &json).unwrap();
    }
    let stored = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing: run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(stored, json, "the CJK rule changed: {}", path.display());
}
