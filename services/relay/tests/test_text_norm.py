import gc
import logging
import random
import time
import unicodedata
from collections import defaultdict

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


def test_a_word_the_romanizer_fails_on_is_kept_with_nothing_alignable(monkeypatch, caplog):
    class Broken:
        def romanize_string(self, word):
            raise ValueError(f"cannot romanize {word}")

    monkeypatch.setattr(text_norm, "load_romanizer", lambda: Broken())
    assert normalize("ok नमस्ते") == [("ok", "ok"), ("नमस्ते", "")]
    assert "नमस्ते" not in caplog.text
    assert "could not romanize 6 characters of a word: ValueError" in caplog.text


def test_a_romanizer_that_cannot_load_never_fails_the_turn(monkeypatch):
    def fail():
        raise OSError("tables missing")

    monkeypatch.setattr(text_norm, "load_romanizer", fail)
    assert normalize("ok مرحبا") == [("ok", "ok"), ("مرحبا", "")]


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


# The romanizer's limits.


def random_words(count, seed=7):
    rng = random.Random(seed)
    blocks = [
        (0x0900, 0x097F), (0x0600, 0x06FF), (0x0590, 0x05FF), (0x0E00, 0x0E7F), (0x4E00, 0x9FFF), (0x3040, 0x30FF),
        (0xAC00, 0xD7A3), (0x0400, 0x04FF), (0x0980, 0x09FF), (0x0B80, 0x0BFF), (0x1200, 0x137F), (0x2150, 0x218F),
    ]  # fmt: skip
    words = []
    for _ in range(count):
        low, high = rng.choice(blocks)
        words.append("".join(chr(rng.randint(low, high)) for _ in range(rng.randint(1, 12))))
    return words


def table_sizes(romanizer):
    return {name: len(table) for name, table in vars(romanizer).items() if isinstance(table, dict)}


def test_romanizing_never_adds_to_uromans_tables():
    romanizer = text_norm.load_romanizer()
    before = table_sizes(romanizer)
    assert before["dict_bool"] > 10_000 and before["rom_rules"] > 10_000
    for word in random_words(1000):
        text_norm.romanize(word)
    assert table_sizes(romanizer) == before


class PieceRecorder:
    def __init__(self):
        self.pieces = []

    def romanize_string(self, piece):
        self.pieces.append(piece)
        return "a" * len(piece)


def test_a_long_word_is_romanized_in_pieces_that_keep_marks_with_their_letter(monkeypatch):
    recorder = PieceRecorder()
    monkeypatch.setattr(text_norm, "load_romanizer", lambda: recorder)
    word = "कि" * 50_000  # a letter and its vowel sign, 100,000 characters
    assert normalize(word) == [(word, "a" * len(word))]
    assert "".join(recorder.pieces) == word
    assert max(map(len, recorder.pieces)) <= text_norm.ROMANIZE_PIECE + 8
    assert not [piece for piece in recorder.pieces if unicodedata.category(piece[0]).startswith("M")]


def test_a_word_of_marks_alone_is_still_cut_into_short_pieces(monkeypatch):
    recorder = PieceRecorder()
    monkeypatch.setattr(text_norm, "load_romanizer", lambda: recorder)
    text_norm.romanize("क" + "ि" * 10_000)
    assert max(map(len, recorder.pieces)) <= text_norm.ROMANIZE_PIECE + 8


def test_a_long_word_takes_time_in_proportion_to_its_length():
    text_norm.load_romanizer()
    rng = random.Random(3)
    word = "".join(chr(rng.randint(0x0915, 0x0939)) for _ in range(20_000))  # Devanagari consonants
    started = time.monotonic()
    assert text_norm.romanize(word)
    # About 3.5 s on the server's 2013 Xeon; one 20,000 character string in
    # one call would take minutes.
    assert time.monotonic() - started < 15


@pytest.fixture
def no_romanizer(monkeypatch):
    """The romanizer as if never loaded; the real one comes back afterwards."""
    monkeypatch.setattr(text_norm, "_romanizer", None)
    monkeypatch.setattr(text_norm, "_failed_at", None)


def test_a_failed_load_is_not_tried_again_for_a_while(no_romanizer, monkeypatch):
    loads = []

    class Broken:
        def __init__(self):
            loads.append(1)
            gc.disable()  # as uroman does while it loads
            raise FileNotFoundError("data")

    monkeypatch.setattr(text_norm.uroman, "Uroman", Broken)
    with pytest.raises(FileNotFoundError):
        text_norm.load_romanizer()
    assert gc.isenabled()
    with pytest.raises(text_norm.RomanizerUnavailable):
        text_norm.load_romanizer()
    assert normalize("नमस्ते") == [("नमस्ते", "")]
    assert len(loads) == 1
    monkeypatch.setattr(text_norm, "_failed_at", time.monotonic() - text_norm.RETRY_LOAD_AFTER_S - 1)
    with pytest.raises(FileNotFoundError):
        text_norm.load_romanizer()
    assert len(loads) == 2


class FakeUroman:
    answer = "namaste"

    def __init__(self):
        self.dict_bool = defaultdict(bool, {"known": True})
        self.stats = defaultdict(int)

    def romanize_string(self, word):
        self.dict_bool[("s-prefix", word)]
        self.stats[word] += 1
        return self.answer


def test_a_romanizer_that_gets_its_check_word_wrong_is_not_loaded(no_romanizer, monkeypatch):
    class Wrong(FakeUroman):
        answer = "nmst"

    monkeypatch.setattr(text_norm.uroman, "Uroman", Wrong)
    with pytest.raises(text_norm.RomanizerUnavailable, match="check word"):
        text_norm.load_romanizer()
    assert text_norm._romanizer is None and text_norm._failed_at is not None


def test_a_check_word_failure_on_a_request_is_logged_without_text(no_romanizer, monkeypatch, caplog):
    class Wrong(FakeUroman):
        answer = "nmst"

    monkeypatch.setattr(text_norm.uroman, "Uroman", Wrong)
    with caplog.at_level(logging.ERROR, logger="relay.text_norm"):
        assert normalize("मेरा नाम") == [("मेरा", ""), ("नाम", "")]
    assert "romanizer could not load (RomanizerUnavailable)" in caplog.text
    assert [r.levelno for r in caplog.records] == [logging.ERROR]
    assert "मेरा" not in caplog.text and "nmst" not in caplog.text


def test_a_loaded_romanizer_is_frozen_and_its_load_time_logged(no_romanizer, monkeypatch, caplog):
    monkeypatch.setattr(text_norm.uroman, "Uroman", FakeUroman)
    with caplog.at_level(logging.INFO, logger="relay.text_norm"):
        romanizer = text_norm.load_romanizer()
    assert "romanizer loaded in" in caplog.text
    assert text_norm.load_romanizer() is romanizer
    assert romanizer.dict_bool["known"] is True and romanizer.dict_bool["unknown"] is False
    assert text_norm.romanize("नमस्ते") == "namaste"
    assert len(romanizer.dict_bool) == 1 and len(romanizer.stats) == 0
