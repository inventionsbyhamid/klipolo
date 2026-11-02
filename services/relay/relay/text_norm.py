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
warm-up (`load_romanizer`) or on the first word that needs it. A failed load
is tried again only after `RETRY_LOAD_AFTER_S`; until then those words have
nothing alignable. Once loaded, its tables are frozen so that romanizing never
adds to them (uroman stores an entry for every key it looks up), and words are
romanized `ROMANIZE_PIECE` characters at a time (its work on one string grows
with the square of the string's length).
"""

import gc
import logging
import re
import threading
import time
import unicodedata
from collections import defaultdict
from typing import Any, Iterator

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


# Words longer than this are romanized a piece at a time. A piece may run a
# few marks longer, so a letter is never cut from its vowel sign or accents.
ROMANIZE_PIECE = 64
_MAX_MARKS_AT_A_CUT = 8
RETRY_LOAD_AFTER_S = 300.0
# What a loaded uroman must give for this word. With a data file missing it
# loads anyway and only prints to stderr.
_CHECK_WORD, _CHECK_ROMAN = "नमस्ते", "namaste"
# uroman's own caches: with frozen tables they would only grow.
_CACHES = ("stats", "abugida_cache", "hangul_rom", "float2fraction", "rom_cache")

_romanizer: uroman.Uroman | None = None
_failed_at: float | None = None  # time.monotonic() of the last failed load
# Loading, and uroman's state while romanizing, one thread at a time.
_romanizer_lock = threading.Lock()


class RomanizerUnavailable(Exception):
    pass


class _Frozen(dict):
    """A uroman table after loading: a missing key gets a fresh default, as
    from the defaultdict it replaces, but is not stored."""

    def __init__(self, table: defaultdict) -> None:
        super().__init__(table)
        self.default_factory = table.default_factory

    def __missing__(self, key: Any) -> Any:
        return self.default_factory()


class _Forgetful(dict):
    """A uroman cache or count that keeps nothing: every lookup misses."""

    def __missing__(self, key: Any) -> int:
        return 0

    def __setitem__(self, key: Any, value: Any) -> None:
        pass


def normalize(text: str) -> list[tuple[str, str]]:
    """Each whitespace-separated word of `text` with its aligner form."""
    return [(word, _normalize_word(word)) for word in text.split()]


def load_romanizer() -> uroman.Uroman:
    """uroman with its tables loaded and frozen, once (about 10 s). Raises
    RomanizerUnavailable within RETRY_LOAD_AFTER_S of a failed load."""
    global _romanizer, _failed_at
    with _romanizer_lock:
        if _romanizer is not None:
            return _romanizer
        if _failed_at is not None and time.monotonic() - _failed_at < RETRY_LOAD_AFTER_S:
            raise RomanizerUnavailable("The romanizer failed to load a short while ago.")
        started = time.monotonic()
        try:
            romanizer = _load()
        except Exception:
            _failed_at = time.monotonic()
            raise
        _romanizer, _failed_at = romanizer, None
        logger.info("romanizer loaded in %.1f s", time.monotonic() - started)
        return romanizer


def _load() -> uroman.Uroman:
    try:
        romanizer = uroman.Uroman()
    finally:
        # uroman turns the garbage collector off while it loads and back on
        # only when the load succeeds.
        gc.enable()
    for name, table in list(vars(romanizer).items()):
        if name in _CACHES and isinstance(table, dict):
            setattr(romanizer, name, _Forgetful())
        elif isinstance(table, defaultdict):
            setattr(romanizer, name, _Frozen(table))
    if (rom := romanizer.romanize_string(_CHECK_WORD)) != _CHECK_ROMAN:
        raise RomanizerUnavailable(f"uroman gave {rom!r} for its check word, not {_CHECK_ROMAN!r}: are its data files there?")
    return romanizer


def romanize(word: str) -> str:
    """`word` in Latin letters. A piece uroman fails on (or all of it, while
    uroman cannot load) stays as it is: its letters in other scripts are then
    dropped, and a word left with none gets interpolated times."""
    try:
        romanizer = load_romanizer()
    except RomanizerUnavailable:
        return word  # logged when the load failed
    except Exception:
        logger.exception("romanizer could not load; trying again in %d s", RETRY_LOAD_AFTER_S)
        return word
    pieces = []
    for piece in _pieces(word):
        try:
            # The lock is taken per piece, so a long word never holds it long.
            with _romanizer_lock:
                pieces.append(romanizer.romanize_string(piece))
        except Exception as exc:
            # The text is never logged, not even in the error's message.
            logger.error("could not romanize %d characters of a word: %s", len(piece), type(exc).__name__)
            pieces.append(piece)
    return "".join(pieces)


def _pieces(word: str) -> Iterator[str]:
    """`word` in pieces of ROMANIZE_PIECE characters, each cut moved past any
    marks (at most _MAX_MARKS_AT_A_CUT) so they stay with their letter."""
    start = 0
    while start < len(word):
        end = min(len(word), start + ROMANIZE_PIECE)
        limit = min(len(word), end + _MAX_MARKS_AT_A_CUT)
        while end < limit and unicodedata.category(word[end]).startswith("M"):
            end += 1
        yield word[start:end]
        start = end


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
