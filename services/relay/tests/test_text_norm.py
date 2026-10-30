import pytest

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
    assert normalize("well - um ... 日本 ok") == [
        ("well", "well"),
        ("-", ""),
        ("um", "um"),
        ("...", ""),
        ("日本", ""),
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
    text = "Hi! 3 cats, 1,000 dogs & 2nd place: 50% at 10:30 on 3/4 \u2014 naïve “quotes” ½ ²"
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
