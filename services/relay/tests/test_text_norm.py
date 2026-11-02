import pytest

from relay import text_norm
from relay.text_norm import ALIGNABLE, normalize


def forms(text):
    return [form for _, form in normalize(text)]


def test_normalize_spells_numbers():
    assert normalize("3 cats") == [("3", "three"), ("cats", "cats")]
    assert " ".join(forms("3 cats")) == "three cats"


def test_normalize_keeps_original_text_for_display():
    pairs = normalize("Hello, World! It's 3.")
    assert [original for original, _ in pairs] == ["Hello,", "World!", "It's", "3."]
    assert [form for _, form in pairs] == ["hello", "world", "it's", "three"]


def test_every_word_is_kept_even_with_nothing_alignable():
    assert normalize("well - um ... 🙂 ok") == [
        ("well", "well"),
        ("-", ""),
        ("um", "um"),
        ("...", ""),
        ("🙂", ""),
        ("ok", "ok"),
    ]


def test_empty_and_blank_text_give_no_words():
    assert normalize("") == []
    assert normalize("  \n\t ") == []


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("0", "zero"),
        ("7", "seven"),
        ("13", "thirteen"),
        ("20", "twenty"),
        ("42", "forty two"),
        ("100", "one hundred"),
        ("101", "one hundred one"),
        ("999", "nine hundred ninety nine"),
        ("1,000", "one thousand"),
        ("2,500,000", "two million five hundred thousand"),
        ("1000000", "one million"),
        ("12345", "twelve thousand three hundred forty five"),
        ("3.5", "three point five"),
        ("0.25", "zero point two five"),
        ("50%", "fifty percent"),
        ("$5", "five dollars"),
        ("$1", "one dollar"),
        ("1st", "first"),
        ("2nd", "second"),
        ("3rd", "third"),
        ("12th", "twelfth"),
        ("21st", "twenty first"),
        ("40th", "fortieth"),
        ("1969", "nineteen sixty nine"),
        ("1900", "nineteen hundred"),
        ("1905", "nineteen oh five"),
        ("2005", "two thousand five"),
        ("2025", "twenty twenty five"),
        ("1990s", "nineteen nineties"),
        ("80s", "eighties"),
        ("10:30", "ten thirty"),
        ("10:05", "ten oh five"),
        ("10:00", "ten"),
        ("007", "zero zero seven"),
        ("3D", "three d"),
        ("MP4", "mp four"),
        ("COVID-19", "covid nineteen"),
    ],
)
def test_numbers_are_spelled_as_spoken(text, expected):
    assert normalize(text) == [(text, expected)]


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("Hello", "hello"),
        ("it’s", "it's"),
        ("'quoted'", "quoted"),
        ("\"Hi\"", "hi"),
        ("long-term", "long term"),
        ("and/or", "and or"),
        ("R&D", "r and d"),
        ("&", "and"),
        ("café", "cafe"),
        ("Straße", "strasse"),
        ("naïve", "naive"),
        ("'", ""),
        ("\u2014", ""),
    ],
)
def test_words_are_lowercase_letters_and_apostrophes(text, expected):
    assert normalize(text) == [(text, expected)]


def test_very_long_digit_runs_are_read_digit_by_digit():
    assert normalize("1234567890123456") == [
        ("1234567890123456", "one two three four five six seven eight nine zero one two three four five six")
    ]


def test_forms_only_use_alignable_characters():
    text = (
        "Hi! 3 cats, 1,000 dogs & 2nd place: 50% at 10:30 on 3/4 \u2014 naïve “quotes” ½ ² "
        "नमस्ते, क्या हाल है? مرحبا، كيف حالك؟ שלום! Привет. 日本語 สวัสดี ٣ ३"
    )
    for _, form in normalize(text):
        assert set(form) <= ALIGNABLE | {" "}
        assert form == form.strip()
        assert "  " not in form


@pytest.mark.parametrize(
    "text",
    ["1" * 5000 + ":05", "$" + "9" * 5000, "$" + "9" * 5000 + ".50", "9" * 5000 + "th", "1" * 5000 + "s"],
    ids=["time", "dollars", "dollars-cents", "ordinal", "plural"],
)
def test_huge_digit_runs_are_read_digit_by_digit_without_error(text):
    ((original, form),) = normalize(text)
    assert original == text
    words = form.split()
    assert len(words) >= 5000
    assert set(form) <= ALIGNABLE | {" "}


def test_only_one_or_two_digit_hours_are_times():
    assert normalize("123:05") == [("123:05", "one hundred twenty three zero five")]
    assert normalize("9:05") == [("9:05", "nine oh five")]


# Other scripts.


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("नमस्ते", "namaste"),
        ("क्या", "kyaa"),
        ("हिंदी", "himdii"),
        ("है।", "hai"),
        ("मैं", "maim"),
        ("مرحبا", "mrhba"),
        ("العربية", "al'rbya"),
        ("שלום", "shlvm"),
        ("Привет", "privet"),
        ("Ελλάδα", "ellada"),
        ("สวัสดี", "swatdii"),
        ("日本", "riben"),
        ("こんにちは", "konnichiha"),
        ("안녕하세요", "annyeonghaseyo"),
        ("বাংলা", "baanglaa"),
        ("தமிழ்", "tamil"),
        ("AI-आधारित", "ai aadhaarit"),
        ("२०२४", "twenty twenty four"),
    ],
)
def test_words_in_other_scripts_are_romanized(text, expected):
    assert normalize(text) == [(text, expected)]


def test_a_hindi_turn_keeps_one_aligner_form_per_word():
    text = "नमस्ते, आज हम एक नया वीडियो बना रहे हैं। यह बहुत आसान है।"
    pairs = normalize(text)
    assert [original for original, _ in pairs] == text.split()
    assert all(form and set(form) <= ALIGNABLE | {" "} for _, form in pairs)


def test_mixed_turns_romanize_only_the_words_in_other_scripts(monkeypatch):
    seen = []
    romanize = text_norm.romanize

    def spy(word):
        seen.append(word)
        return romanize(word)

    monkeypatch.setattr(text_norm, "romanize", spy)
    assert forms("Today हम video बना रहे हैं, 3 बार") == [
        "today", "ham", "video", "banaa", "rahe", "haim", "three", "baar"
    ]  # fmt: skip
    assert seen == ["हम", "बना", "रहे", "हैं,", "बार"]


@pytest.mark.parametrize(
    "text",
    [
        "Hello, World! It's 3.",
        "café Straße naïve Søren Łódź ﬁne Ａbc ª ½ ² 1990s $5 R&D \u2014 it’s 'quoted'",
    ],
)
def test_latin_words_never_reach_the_romanizer(monkeypatch, text):
    expected = normalize(text)

    def fail(word):
        raise AssertionError(f"romanized {word!r}")

    monkeypatch.setattr(text_norm, "romanize", fail)
    assert normalize(text) == expected


def test_a_word_the_romanizer_fails_on_is_kept_with_nothing_alignable(monkeypatch):
    class Broken:
        def romanize_string(self, word):
            raise ValueError("no")

    monkeypatch.setattr(text_norm, "load_romanizer", lambda: Broken())
    assert normalize("ok नमस्ते") == [("ok", "ok"), ("नमस्ते", "")]


def test_chinese_and_japanese_sent_one_character_per_word_romanize_each_alone():
    assert forms("大 家 好， 今 天 我 来 教 大 家。") == [
        "da", "jia", "hao", "jin", "tian", "wo", "lai", "jiao", "da", "jia"
    ]  # fmt: skip
    pairs = normalize("きょ う は 「動 画」 で す 。")
    assert [original for original, _ in pairs] == ["きょ", "う", "は", "「動", "画」", "で", "す", "。"]
    assert all(form and set(form) <= ALIGNABLE for _, form in pairs[:-1])
    # Punctuation alone has nothing to align, like "—" in English.
    assert pairs[-1] == ("。", "")


def test_a_long_unspaced_thai_run_is_one_word():
    ((original, form),) = normalize("สวัสดีครับวันนี้เราจะมาทำวิดีโอกันนะครับ")
    assert form and set(form) <= ALIGNABLE | {" "}
