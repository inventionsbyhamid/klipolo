//! Which language a transcript is in, from its words alone. Voice Transcribe hears its 25 languages (and a speaker who
//! mixes two) without being told which, but never says which it heard. So:
//! - the script most of the words' letters are in names the language where a script has one main language
//!   (Devanagari: Hindi, unless Marathi's own words stand out; Arabic letters: Arabic; Hangul: Korean; Thai ...);
//! - kana among Chinese characters make it Japanese, none make it Chinese;
//! - Latin letters are told apart by each language's commonest short words, and are English when no other language's
//!   stand out more.
//!
//! A Chinese, Japanese or Korean character counts as `CJK_WEIGHT` letters (one of them is most of a word), so "我们用
//! Final Cut" stays Chinese. The answer is a BCP 47 primary subtag ("hi", "ar", "zh"), as `Transcript::language` and
//! the caption faces (`klipolo_model::text_language`) read it. O(characters).

/// A transcript with no letters at all (no speech, only digits) is tagged this.
pub const DEFAULT: &str = "en";

/// What a Chinese, Japanese or Korean character counts for against an alphabet's letters.
const CJK_WEIGHT: usize = 2;

/// Kana this share of a text's Chinese and Japanese characters (or more) make it Japanese: Japanese writes its endings
/// and particles in kana, so even a dense line holds a fair share.
const KANA_SHARE: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Latin,
    Greek,
    Cyrillic,
    Armenian,
    Hebrew,
    Arabic,
    Devanagari,
    Bengali,
    Gurmukhi,
    Gujarati,
    Oriya,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    Sinhala,
    Thai,
    Lao,
    Myanmar,
    Georgian,
    Hangul,
    Ethiopic,
    Khmer,
    Kana,
    Han,
}

const SCRIPTS: [Script; 25] = [
    Script::Latin,
    Script::Greek,
    Script::Cyrillic,
    Script::Armenian,
    Script::Hebrew,
    Script::Arabic,
    Script::Devanagari,
    Script::Bengali,
    Script::Gurmukhi,
    Script::Gujarati,
    Script::Oriya,
    Script::Tamil,
    Script::Telugu,
    Script::Kannada,
    Script::Malayalam,
    Script::Sinhala,
    Script::Thai,
    Script::Lao,
    Script::Myanmar,
    Script::Georgian,
    Script::Hangul,
    Script::Ethiopic,
    Script::Khmer,
    Script::Kana,
    Script::Han,
];

/// The script of a letter (or a vowel sign or mark inside a word): None for digits, punctuation and spaces.
fn script(c: char) -> Option<Script> {
    use Script::*;
    let s = match c as u32 {
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF => Latin,
        0x370..=0x3FF | 0x1F00..=0x1FFF => Greek,
        0x400..=0x52F => Cyrillic,
        0x530..=0x58F => Armenian,
        0x590..=0x5FF => Hebrew,
        0x600..=0x6FF | 0x750..=0x77F | 0x8A0..=0x8FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Arabic,
        0x900..=0x97F | 0xA8E0..=0xA8FF => Devanagari,
        0x980..=0x9FF => Bengali,
        0xA00..=0xA7F => Gurmukhi,
        0xA80..=0xAFF => Gujarati,
        0xB00..=0xB7F => Oriya,
        0xB80..=0xBFF => Tamil,
        0xC00..=0xC7F => Telugu,
        0xC80..=0xCFF => Kannada,
        0xD00..=0xD7F => Malayalam,
        0xD80..=0xDFF => Sinhala,
        0xE00..=0xE7F => Thai,
        0xE80..=0xEFF => Lao,
        0x1000..=0x109F => Myanmar,
        0x10A0..=0x10FF => Georgian,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Hangul,
        0x1200..=0x137F => Ethiopic,
        0x1780..=0x17FF => Khmer,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Kana,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF => Han,
        _ => return None,
    };
    // Digits and signs inside the blocks (Devanagari's danda, Arabic's comma, Thai's digits) are no letters.
    (c.is_alphabetic() || is_mark(c)).then_some(s)
}

/// A combining mark (a virama, a vowel sign): part of its word's letters though not alphabetic itself.
fn is_mark(c: char) -> bool {
    matches!(
        c as u32,
        0x93C | 0x94D | 0x9CD | 0xA4D | 0xACD | 0xB4D | 0xBCD | 0xC4D | 0xCCD | 0xD4D | 0xE3A
    )
}

/// The language `words` (a transcript's words, in order) are in: see the module notes. `DEFAULT` when they hold no
/// letter.
pub fn of_words<'a>(words: impl IntoIterator<Item = &'a str>) -> &'static str {
    let mut counts = [0usize; SCRIPTS.len()];
    let mut latin: Vec<String> = Vec::new();
    let mut devanagari: Vec<&'a str> = Vec::new();
    for word in words {
        let mut has_latin = false;
        let mut has_devanagari = false;
        for c in word.chars() {
            let Some(s) = script(c) else { continue };
            let weight = match s {
                Script::Han | Script::Kana | Script::Hangul => CJK_WEIGHT,
                _ => 1,
            };
            counts[s as usize] += weight;
            has_latin |= s == Script::Latin;
            has_devanagari |= s == Script::Devanagari;
        }
        if has_latin {
            latin.push(bare(word));
        }
        if has_devanagari {
            devanagari.push(word);
        }
    }
    // Chinese characters and kana are one written language's, weighed together against the other scripts.
    let (kana, han) = (counts[Script::Kana as usize], counts[Script::Han as usize]);
    counts[Script::Han as usize] += kana;
    counts[Script::Kana as usize] = 0;
    let Some((top, n)) = SCRIPTS
        .iter()
        .zip(counts)
        .max_by_key(|&(s, n)| (n, std::cmp::Reverse(*s as usize)))
    else {
        return DEFAULT;
    };
    if n == 0 {
        return DEFAULT;
    }
    match top {
        Script::Latin => latin_language(&latin),
        Script::Greek => "el",
        Script::Cyrillic => "ru",
        Script::Armenian => "hy",
        Script::Hebrew => "he",
        Script::Arabic => "ar",
        Script::Devanagari => devanagari_language(&devanagari),
        Script::Bengali => "bn",
        Script::Gurmukhi => "pa",
        Script::Gujarati => "gu",
        Script::Oriya => "or",
        Script::Tamil => "ta",
        Script::Telugu => "te",
        Script::Kannada => "kn",
        Script::Malayalam => "ml",
        Script::Sinhala => "si",
        Script::Thai => "th",
        Script::Lao => "lo",
        Script::Myanmar => "my",
        Script::Georgian => "ka",
        Script::Hangul => "ko",
        Script::Ethiopic => "am",
        Script::Khmer => "km",
        Script::Han | Script::Kana => {
            if kana as f64 >= KANA_SHARE * (kana + han) as f64 {
                "ja"
            } else {
                "zh"
            }
        }
    }
}

/// A word's letters alone, lowercased ("Today," → "today").
fn bare(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphabetic() || matches!(c, '\'' | '’'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The commonest short words of the Latin-script languages Voice Transcribe hears, space separated. A word may stand in
/// more than one list ("de", "la"): each language it is in counts it.
const LATIN_WORDS: [(&str, &str); 12] = [
    (
        "en",
        "the and is you to of it that this in i what with for are my we have be so just how your was on \
         can will i'm it's don't",
    ),
    (
        "es",
        "el la los las de que y en un una es por con para como del se lo muy pero hoy más les voy cómo \
         menos yo está qué",
    ),
    (
        "fr",
        "le la les de des et est un une je vous que en pour dans pas ce il on sur avec nous c'est d'une \
         moins aujourd'hui du au vais",
    ),
    (
        "de",
        "der die das und ist ich nicht ein eine einer zu mit den wie euch es auf sie wir auch man heute \
         weniger als dem sich ihr dass",
    ),
    (
        "it",
        "il la di che e un una per non sono è con mi ti questo come della oggi vi meno gli anche ma",
    ),
    (
        "pt",
        "o a os as de que e um uma não é para com você do da em no na hoje vou como menos mais eu",
    ),
    (
        "nl",
        "de het een en is ik niet van dat je met op zijn wat voor ook jullie vandaag hoe minder dan",
    ),
    (
        "pl",
        "nie się to jest w na i z że do co jak ale tak mnie dzisiaj wam pokażę",
    ),
    (
        "tr",
        "bir ve bu da de ne için çok ben mi ile var bugün nasıl size daha",
    ),
    (
        "id",
        "yang dan di ini itu dengan untuk tidak saya ada akan ke dari hari kita cara",
    ),
    (
        "tl",
        "ang ng sa na mga ay at ko ako hindi ito siya kung paano",
    ),
    (
        "vi",
        "của và là không tôi có được này những các cho một hôm nay cách bạn",
    ),
];

/// The Latin-script language whose common words `words` (bare, lowercased) hold most of. English unless another
/// language has at least `LATIN_MIN` of its words, and more than English: so one "um" (a Portuguese word, and an
/// English hum) in "one two three, um, four" leaves it English.
fn latin_language(words: &[String]) -> &'static str {
    let mut best = ("en", 0usize);
    let english = count(words, LATIN_WORDS[0].1);
    for (language, list) in &LATIN_WORDS[1..] {
        let n = count(words, list);
        if n >= LATIN_MIN && n > english && n > best.1 {
            best = (language, n);
        }
    }
    best.0
}

/// The fewest common words that make Latin letters another language than English.
const LATIN_MIN: usize = 2;

fn count(words: &[String], list: &str) -> usize {
    words
        .iter()
        .filter(|w| list.split(' ').any(|l| l == w.as_str()))
        .count()
}

/// Marathi's commonest words, which Hindi doesn't use: Devanagari speech holding more of them than of Hindi's is
/// Marathi.
const MARATHI: &str = "आहे आणि नाही मी तुम्ही आहेत होते आम्ही कसे मध्ये आज करू";
const HINDI: &str = "है और नहीं मैं आप हैं था हम कैसे में का की के को";

fn devanagari_language(words: &[&str]) -> &'static str {
    let plain: Vec<String> = words
        .iter()
        .map(|w| {
            w.trim_matches(|c: char| !(c.is_alphabetic() || is_mark(c)))
                .to_owned()
        })
        .collect();
    let n = |list: &str| {
        (plain.iter())
            .filter(|w| list.split(' ').any(|l| l == w.as_str()))
            .count()
    };
    if n(MARATHI) > n(HINDI) { "mr" } else { "hi" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(text: &str) -> &'static str {
        of_words(text.split_whitespace())
    }

    #[test]
    fn each_script_names_its_language() {
        for (text, want) in [
            (
                "नमस्ते दोस्तों, आज मैं आपको दिखाऊंगी कि एक मिनट में वीडियो कैसे बनाते हैं।",
                "hi",
            ),
            (
                "مرحبا بكم، اليوم سأريكم كيف تعدلون الفيديو في أقل من دقيقة.",
                "ar",
            ),
            (
                "שלום לכולם, היום אני אראה לכם איך לערוך סרטון בפחות מדקה.",
                "he",
            ),
            ("สวัสดีทุกคน วันนี้ฉันจะสอนวิธีตัดต่อวิดีโอภายในหนึ่งนาที", "th"),
            ("வணக்கம் நண்பர்களே, இன்று ஒரு நிமிடத்தில் வீடியோ எடிட் செய்வது", "ta"),
            ("నమస్కారం స్నేహితులారా", "te"),
            ("ನಮಸ್ಕಾರ ಸ್ನೇಹಿತರೇ", "kn"),
            ("নমস্কার বন্ধুরা, আজ আমি দেখাব", "bn"),
            ("안녕하세요 여러분, 오늘은 영상 편집을 배워요", "ko"),
            (
                "こんにちは。今日は一分以内に動画を編集する方法を紹介します。",
                "ja",
            ),
            ("大家好，今天我来教大家如何在一分钟内剪辑视频。", "zh"),
            ("Привет всем, сегодня я покажу вам монтаж", "ru"),
            ("Γεια σας, σήμερα θα σας δείξω", "el"),
        ] {
            assert_eq!(of(text), want, "{text}");
        }
    }

    #[test]
    fn a_mixed_hindi_and_english_take_is_hindi() {
        assert_eq!(
            of("आज हम video editing सीखेंगे, और captions भी add करेंगे, बिल्कुल आसान तरीके से।"),
            "hi"
        );
        assert_eq!(
            of("So today hum log ek video edit karenge and captions add karenge"),
            "en"
        );
    }

    #[test]
    fn one_word_of_another_language_leaves_english() {
        assert_eq!(of("one two three Um, four."), "en");
        assert_eq!(of("Hola"), "en");
    }

    #[test]
    fn marathi_words_make_devanagari_marathi() {
        assert_eq!(
            of("आज मी तुम्हाला व्हिडिओ कसे बनवायचे ते दाखवणार आहे आणि ते सोपे आहे"),
            "mr"
        );
    }

    #[test]
    fn latin_languages_are_told_apart_by_their_common_words() {
        for (text, want) in [
            (
                "Today I want to show you how to cut a video in under a minute.",
                "en",
            ),
            (
                "Hoy les voy a enseñar cómo editar un video en menos de un minuto.",
                "es",
            ),
            (
                "Aujourd'hui, je vais vous montrer comment monter une vidéo en moins d'une minute.",
                "fr",
            ),
            (
                "Heute zeige ich euch, wie man ein Video in weniger als einer Minute schneidet.",
                "de",
            ),
            (
                "Oggi vi mostro come montare un video in meno di un minuto.",
                "it",
            ),
            (
                "Hoje eu vou mostrar como editar um vídeo em menos de um minuto.",
                "pt",
            ),
            ("Hôm nay tôi sẽ chỉ cho bạn cách chỉnh sửa video.", "vi"),
        ] {
            assert_eq!(of(text), want, "{text}");
        }
    }

    #[test]
    fn a_few_cjk_characters_outweigh_an_english_name() {
        assert_eq!(of("我们用 Final Cut 剪辑"), "zh");
    }

    #[test]
    fn no_letters_is_the_default() {
        assert_eq!(of(""), DEFAULT);
        assert_eq!(of("123 4.5 —"), DEFAULT);
        assert_eq!(of_words([]), DEFAULT);
    }
}
