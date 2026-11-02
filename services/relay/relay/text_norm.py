"""Transcript text in the form the MMS forced aligner reads.

`normalize(text)` splits the text on whitespace and maps each original word
to its aligner form: lowercase a-z and apostrophes, numbers spelled out in
English as they are usually spoken, punctuation dropped. One form can hold
several aligner words separated by spaces ("42" becomes "forty two"). A word
with nothing alignable maps to "" and is still kept, so the caller can give
it interpolated times and a zero score while showing the original text.

A word with letters of a script other than Latin (Devanagari, Arabic, Hebrew,
Cyrillic, Thai, CJK and so on) is first romanized with uroman, the romanizer
the MMS aligner was trained with ("नमस्ते" becomes "namaste", "مرحبا"
becomes "mrhba"), and then goes through the same steps. Words in Latin
letters never reach uroman, so their forms do not depend on it. Digits in
other scripts come back as 0-9 and are spelled in English like any number.

uroman takes about 10 s to load its tables, so it loads once: at the startup
warm-up (`load_romanizer`) or on the first word that needs it.
"""

import logging
import re
import threading
import unicodedata

import regex
import uroman

logger = logging.getLogger(__name__)

ALIGNABLE = frozenset("abcdefghijklmnopqrstuvwxyz'")

# A letter (or digit) of any script but Latin. Common characters (0-9,
# punctuation, symbols) and Inherited ones (combining accents) never call for
# romanizing on their own.
_OTHER_SCRIPT = regex.compile(r"[^\p{Script=Latin}\p{Script=Common}\p{Script=Inherited}\p{Script=Unknown}]")

_APOSTROPHES = str.maketrans({"’": "'", "‘": "'", "ʼ": "'", "`": "'", "´": "'"})
_SYMBOLS = {"&": " and ", "%": " percent ", "+": " plus ", "@": " at "}
_SYMBOL = re.compile("[" + re.escape("".join(_SYMBOLS)) + "]")
_NOT_ALIGNABLE = re.compile(r"[^a-z']+")

_ONES = (
    "zero one two three four five six seven eight nine ten eleven twelve thirteen "
    "fourteen fifteen sixteen seventeen eighteen nineteen"
).split()
_TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()
_SCALES = ((10**12, "trillion"), (10**9, "billion"), (10**6, "million"), (10**3, "thousand"))
_ORDINALS = {
    "one": "first",
    "two": "second",
    "three": "third",
    "five": "fifth",
    "eight": "eighth",
    "nine": "ninth",
    "twelve": "twelfth",
}
# Longer digit runs (phone or serial numbers) are read digit by digit.
_MAX_CARDINAL_DIGITS = 15

_NUMBER = re.compile(
    r"""
    (?P<currency>\$)?
    (?:
        # A time only with a one or two digit hour, so no long digit run
        # is ever turned into an int.
        (?<![0-9])(?P<hour>[0-9]{1,2}):(?P<minutes>[0-9]{2})(?![0-9])
      | (?P<number>[0-9]{1,3}(?:,[0-9]{3})+|[0-9]+)(?:\.(?P<decimals>[0-9]+))?
    )
    (?:(?P<suffix>st|nd|rd|th|s)(?![a-z]))?
    """,
    re.VERBOSE,
)


_romanizer: uroman.Uroman | None = None
# uroman keeps caches it writes while romanizing, so one word at a time.
_romanizer_lock = threading.Lock()


def normalize(text: str) -> list[tuple[str, str]]:
    """Each whitespace-separated word of `text` with its aligner form."""
    return [(word, _normalize_word(word)) for word in text.split()]


def load_romanizer() -> uroman.Uroman:
    """uroman with its tables loaded, once (about 10 s)."""
    global _romanizer
    with _romanizer_lock:
        if _romanizer is None:
            _romanizer = uroman.Uroman()
        return _romanizer


def romanize(word: str) -> str:
    """`word` in Latin letters, or the word itself if uroman fails on it."""
    romanizer = load_romanizer()
    try:
        with _romanizer_lock:
            return romanizer.romanize_string(word)
    except Exception:
        # The text is never logged.
        logger.exception("could not romanize a word of %d characters", len(word))
        return word


def _normalize_word(word: str) -> str:
    if _OTHER_SCRIPT.search(word):
        word = romanize(word)
    word = unicodedata.normalize("NFKD", word.translate(_APOSTROPHES))
    word = "".join(char for char in word if not unicodedata.combining(char)).casefold()
    word = _NUMBER.sub(lambda match: f" {_spell_number(match)} ", word)
    word = _SYMBOL.sub(lambda match: _SYMBOLS[match.group()], word)
    parts = (part.strip("'") for part in _NOT_ALIGNABLE.sub(" ", word).split())
    return " ".join(part for part in parts if part)


def _spell_number(match: re.Match[str]) -> str:
    digits = (match["number"] or match["hour"]).replace(",", "")
    suffix = match["suffix"]
    if match["minutes"] is not None:
        minutes = int(match["minutes"])
        words = _cardinal(int(digits))
        if minutes:
            words += (" oh " if minutes < 10 else " ") + _cardinal(minutes)
    elif (digits.startswith("0") and len(digits) > 1) or len(digits) > _MAX_CARDINAL_DIGITS:
        words = _digit_by_digit(digits)
    elif match["decimals"] is not None and match["currency"] and len(match["decimals"]) == 2:
        words = f"{_cardinal(int(digits))} {_dollars(digits)}"
        if int(match["decimals"]):
            words += f" {_cardinal(int(match['decimals']))}"
        return words
    elif match["decimals"] is not None:
        words = f"{_cardinal(int(digits))} point {_digit_by_digit(match['decimals'])}"
    elif "," not in match["number"] and len(digits) == 4 and suffix != "th" and _is_year(int(digits)):
        words = _year(int(digits))
    else:
        words = _cardinal(int(digits))

    if suffix in ("st", "nd", "rd", "th"):
        words = _last_word(words, _ordinal)
    elif suffix == "s":
        words = _last_word(words, _plural)
    if match["currency"]:
        words += f" {_dollars(digits)}"
    return words


def _cardinal(number: int) -> str:
    if number < 20:
        return _ONES[number]
    if number < 100:
        tens, ones = divmod(number, 10)
        return _TENS[tens] + (f" {_ONES[ones]}" if ones else "")
    if number < 1000:
        hundreds, rest = divmod(number, 100)
        return f"{_ONES[hundreds]} hundred" + (f" {_cardinal(rest)}" if rest else "")
    for scale, name in _SCALES:
        if number >= scale:
            count, rest = divmod(number, scale)
            return f"{_cardinal(count)} {name}" + (f" {_cardinal(rest)}" if rest else "")
    raise AssertionError("unreachable")


def _digit_by_digit(digits: str) -> str:
    return " ".join(_ONES[int(digit)] for digit in digits)


def _is_year(number: int) -> bool:
    return 1100 <= number <= 1999 or 2010 <= number <= 2099


def _year(number: int) -> str:
    century, rest = divmod(number, 100)
    if rest == 0:
        return f"{_cardinal(century)} hundred"
    if rest < 10:
        return f"{_cardinal(century)} oh {_ONES[rest]}"
    return f"{_cardinal(century)} {_cardinal(rest)}"


def _dollars(digits: str) -> str:
    return "dollar" if digits == "1" else "dollars"


def _last_word(words: str, change) -> str:
    head, _, last = words.rpartition(" ")
    return f"{head} {change(last)}" if head else change(last)


def _ordinal(word: str) -> str:
    if word in _ORDINALS:
        return _ORDINALS[word]
    if word.endswith("y"):
        return word[:-1] + "ieth"
    return word + "th"


def _plural(word: str) -> str:
    if word.endswith("y"):
        return word[:-1] + "ies"
    if word.endswith("x"):
        return word + "es"
    return word + "s"
