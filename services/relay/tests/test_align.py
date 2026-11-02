"""Alignment planning and assembly (fast, no torch) and the real MMS model
(marked slow: needs torch and the model, so it runs in the container)."""

import json
import os
import statistics
import subprocess
import sys
import time
import wave
from pathlib import Path

import pytest

from relay import align
from relay.config import load_settings
from relay.align import (
    FAILED,
    NO_TEXT,
    NOTHING_TO_ALIGN,
    PAST_END,
    ENDS_BEFORE_START,
    TOO_SHORT,
    PlannedTurn,
    Turn,
    align_turns,
    assemble,
    emission_frames,
    emission_windows,
    plan,
)

FIXTURES = Path(__file__).parent / "fixtures"
RATE = 16000


def seconds(n):
    return int(n * RATE)


# Planning.


def test_empty_turn_is_skipped_with_reason():
    planned, skipped = plan(
        [Turn(0, 1000, ""), Turn(0, 1000, "  \n "), Turn(0, 1000, "hello")], seconds(5)
    )
    assert [(s.turn, s.reason) for s in skipped] == [(0, NO_TEXT), (1, NO_TEXT)]
    assert NO_TEXT == "The turn has no text."
    assert [p.index for p in planned] == [2]


def test_turn_past_audio_end_is_clamped():
    planned, skipped = plan([Turn(1000, 99_000, "hello there")], seconds(5))
    assert skipped == []
    (turn,) = planned
    assert turn.start == seconds(0.75)
    assert turn.end == seconds(5)


def test_turn_is_padded_by_a_quarter_second_on_each_side():
    (turn,), _ = plan([Turn(2000, 3000, "hello")], seconds(10))
    assert (turn.start, turn.end) == (seconds(1.75), seconds(3.25))
    assert turn.seconds == pytest.approx(1.5)


def test_turn_at_audio_start_is_clamped_to_zero():
    planned, skipped = plan([Turn(100, 1000, "hello"), Turn(-500, 1000, "hello")], seconds(10))
    assert skipped == []
    assert [(p.start, p.end) for p in planned] == [(0, seconds(1.25)), (0, seconds(1.25))]


def test_turn_starting_after_the_audio_ends_is_skipped():
    _, skipped = plan([Turn(5000, 6000, "hello"), Turn(7000, 8000, "hello")], seconds(5))
    assert [(s.turn, s.reason) for s in skipped] == [(0, PAST_END), (1, PAST_END)]
    assert PAST_END == "The turn starts after the audio ends."


def test_turn_ending_before_it_starts_is_skipped():
    _, skipped = plan([Turn(3000, 2000, "hello")], seconds(5))
    assert [(s.turn, s.reason) for s in skipped] == [(0, ENDS_BEFORE_START)]
    assert ENDS_BEFORE_START == "The turn ends before it starts."


def test_turn_without_alignable_words_is_skipped():
    _, skipped = plan([Turn(0, 1000, "... — !"), Turn(0, 1000, "🙂 ♪")], seconds(5))
    assert [(s.turn, s.reason) for s in skipped] == [(0, NOTHING_TO_ALIGN), (1, NOTHING_TO_ALIGN)]


def test_turn_too_short_for_its_text_is_skipped():
    # 0.5 s of audio is 24 frames; this text needs far more.
    long_text = "the quick brown fox jumps over the lazy dog " * 3
    _, skipped = plan([Turn(1000, 1000, long_text)], seconds(5))
    assert [(s.turn, s.reason) for s in skipped] == [(0, TOO_SHORT)]
    # A zero-length turn still gets its padding and can hold a short word.
    planned, skipped = plan([Turn(1000, 1000, "hi")], seconds(5))
    assert skipped == [] and planned[0].seconds == pytest.approx(0.5)


def test_turn_with_more_letters_than_its_audio_can_hold_is_skipped():
    # A 1 s turn is a 1.5 s slice: 74 frames, room for 0.6 x 74 = 44.4 letters.
    assert emission_frames(seconds(1.5)) == 74
    fits = ["abcdefghij"] * 4 + ["abcd"]
    too_many = ["abcdefghij"] * 4 + ["abcde"]
    planned, skipped = plan([Turn(2000, 3000, " ".join(fits)), Turn(2000, 3000, " ".join(too_many))], seconds(10))
    assert [p.index for p in planned] == [0]
    assert [(s.turn, s.reason) for s in skipped] == [(1, TOO_SHORT)]
    assert align.MAX_LETTERS_PER_FRAME == 0.6


def test_repeated_letters_need_a_frame_between_them():
    # "aa" needs 3 frames (a, blank, a); "ab" needs 2.
    assert align.frames_needed(["aa"]) == 3
    assert align.frames_needed(["ab"]) == 2
    assert align.frames_needed(["a", "a"]) == 3


def test_plan_keeps_every_word_and_its_aligner_form():
    (turn,), _ = plan([Turn(0, 2000, "Hi, it's 3 o'clock — now!")], seconds(5))
    assert turn.words == (
        ("Hi,", "hi"),
        ("it's", "it's"),
        ("3", "three"),
        ("o'clock", "o'clock"),
        ("—", ""),
        ("now!", "now"),
    )
    assert turn.tokens == ["hi", "it's", "three", "o'clock", "now"]


def test_number_forms_become_one_aligner_word():
    (turn,), _ = plan([Turn(0, 3000, "42 cats")], seconds(5))
    assert turn.tokens == ["fortytwo", "cats"]


def test_turns_in_other_scripts_get_one_aligner_word_per_word():
    (hindi, arabic), skipped = plan(
        [Turn(0, 2000, "नमस्ते, आज हम वीडियो बना रहे हैं।"), Turn(2000, 4000, "مرحبا، هذا سهل جدا.")], seconds(5)
    )
    assert skipped == []
    assert hindi.tokens == ["namaste", "aaj", "ham", "viiddiyo", "banaa", "rahe", "haim"]
    assert arabic.tokens == ["mrhba", "htha", "shl", "jda"]


# Emission frames and windows.


def test_emission_frames_match_the_wav2vec2_feature_extractor():
    for n in range(0, 5000):
        assert emission_frames(n) == (0 if n < 400 else (n - 400) // 320 + 1)
    assert emission_frames(seconds(1)) == 49


def test_short_slices_are_one_window():
    assert emission_windows(seconds(20)) == [align.Window(0, seconds(20), 0, emission_frames(seconds(20)))]


@pytest.mark.parametrize("n", [seconds(34.5), seconds(90), seconds(600), seconds(61.3) + 123])
def test_long_slices_are_split_into_windows_that_cover_every_frame(n):
    windows = emission_windows(n, window=1500, context=100)
    assert len(windows) > 1
    covered = []
    for w in windows:
        assert 0 <= w.start < w.end <= n
        # Each window holds at most window + 2 * context frames.
        assert emission_frames(w.end - w.start) <= 1500 + 200
        assert 0 <= w.keep_from < w.keep_to <= emission_frames(w.end - w.start)
        first = w.start // 320
        assert w.start % 320 == 0
        covered.extend(range(first + w.keep_from, first + w.keep_to))
    assert covered == list(range(emission_frames(n)))


# Assembly.


def test_assemble_converts_frames_to_ms_from_the_audio_start():
    turn = PlannedTurn(3, seconds(2), seconds(4), (("Hello,", "hello"), ("world", "world")))
    words = assemble(turn, [(10, 20, 0.91234), (25, 40, 0.5)])
    assert words == [
        {"text": "Hello,", "startMs": 2200, "endMs": 2400, "score": 0.912, "turn": 3},
        {"text": "world", "startMs": 2500, "endMs": 2800, "score": 0.5, "turn": 3},
    ]


def test_words_with_nothing_alignable_get_score_zero_and_interpolated_times():
    turn = PlannedTurn(
        0,
        0,
        seconds(4),
        (("—", ""), ("well", "well"), ("...", ""), ("-", ""), ("ok", "ok"), ("!", "")),
    )
    words = assemble(turn, [(10, 20, 0.9), (50, 60, 0.8)])
    assert [(w["text"], w["startMs"], w["endMs"], w["score"]) for w in words] == [
        ("—", 200, 200, 0.0),
        ("well", 200, 400, 0.9),
        ("...", 400, 700, 0.0),
        ("-", 700, 1000, 0.0),
        ("ok", 1000, 1200, 0.8),
        ("!", 1200, 1200, 0.0),
    ]


class FakeAligner:
    """Spreads each turn's words evenly over its slice."""

    def __init__(self, fail_on=()):
        self.fail_on = set(fail_on)
        self.calls = []

    def load(self):
        pass

    def align(self, pcm, tokens):
        self.calls.append((len(pcm), list(tokens)))
        if any(token in self.fail_on for token in tokens):
            raise RuntimeError("boom")
        frames = emission_frames(len(pcm) // 2)
        step = frames // len(tokens)
        return [(i * step, i * step + max(1, step // 2), 0.9) for i in range(len(tokens))]


def test_align_turns_reads_each_slice_and_counts_aligned_seconds():
    planned, _ = plan([Turn(1000, 2000, "one two"), Turn(3000, 5000, "three")], seconds(10))
    reads = []

    def read_pcm(start, end):
        reads.append((start, end))
        return bytes(2 * (end - start))

    fake = FakeAligner()
    result = align_turns(fake, read_pcm, planned)
    assert reads == [(seconds(0.75), seconds(2.25)), (seconds(2.75), seconds(5.25))]
    assert [call[1] for call in fake.calls] == [["one", "two"], ["three"]]
    assert [w["text"] for w in result.words] == ["one", "two", "three"]
    assert [w["turn"] for w in result.words] == [0, 0, 1]
    assert result.skipped == []
    assert result.seconds == pytest.approx(1.5 + 2.5)


def test_a_turn_the_model_cannot_align_is_skipped_and_not_counted():
    planned, _ = plan([Turn(1000, 2000, "fine"), Turn(3000, 5000, "boom here")], seconds(10))
    result = align_turns(FakeAligner(fail_on={"boom"}), lambda s, e: bytes(2 * (e - s)), planned)
    assert [w["text"] for w in result.words] == ["fine"]
    assert [(s.turn, s.reason) for s in result.skipped] == [(1, FAILED)]
    assert result.seconds == pytest.approx(1.5)


def test_align_turns_stops_between_turns_when_asked():
    planned, _ = plan([Turn(1000, 2000, "one"), Turn(3000, 4000, "two")], seconds(10))
    fake = FakeAligner()
    result = align_turns(fake, lambda s, e: bytes(2 * (e - s)), planned, stop=lambda: len(fake.calls) == 1)
    assert [w["text"] for w in result.words] == ["one"]
    assert result.seconds == pytest.approx(1.5)


def test_a_wrong_number_of_spans_skips_the_turn():
    class Short(FakeAligner):
        def align(self, pcm, tokens):
            return super().align(pcm, tokens)[:-1]

    planned, _ = plan([Turn(1000, 2000, "one two")], seconds(10))
    result = align_turns(Short(), lambda s, e: bytes(2 * (e - s)), planned)
    assert result.words == [] and [(s.turn, s.reason) for s in result.skipped] == [(0, FAILED)]


# The real model (slow).


@pytest.fixture(scope="module")
def mms():
    pytest.importorskip("torchaudio")
    aligner = align.MmsAligner(threads=load_settings().align_threads)
    started = time.monotonic()
    aligner.load()
    print(f"\nmodel loaded in {time.monotonic() - started:.1f} s")
    return aligner


def fixture_case():
    case = json.loads((FIXTURES / "iss_answer.json").read_text())
    with wave.open(str(FIXTURES / case["audio"])) as audio:
        pcm = audio.readframes(audio.getnframes())
    turns = [Turn(t["startMs"], t["endMs"], t["text"]) for t in case["turns"]]
    return pcm, turns, case


def align_pcm(aligner, pcm, turns):
    planned, skipped = plan(turns, len(pcm) // 2)
    result = align_turns(aligner, lambda start, end: pcm[2 * start : 2 * end], planned)
    result.skipped = sorted(skipped + result.skipped, key=lambda s: s.turn)
    return result


@pytest.mark.slow
def test_fixture_words_start_within_150ms_of_reference(mms):
    pcm, turns, case = fixture_case()
    started, cpu = time.monotonic(), time.process_time()
    result = align_pcm(mms, pcm, turns)
    wall, cpu = time.monotonic() - started, time.process_time() - cpu
    assert result.skipped == []
    words, reference = result.words, case["reference"]
    assert [(w["text"], w["turn"]) for w in words] == [(r["text"], r["turn"]) for r in reference]

    # Words under 0.05 are the transcript's invented repeats: they have no
    # audio of their own, so their times are not meaningful.
    checked = [(w, r) for w, r in zip(words, reference) if r["score"] >= 0.05]
    offsets = [w["startMs"] - r["startMs"] for w, r in checked]
    print(
        f"\n{len(checked)} of {len(words)} words checked; start offsets ms: "
        f"max abs {max(map(abs, offsets))}, mean abs {sum(map(abs, offsets)) / len(offsets):.1f}; "
        f"{result.seconds:.1f} s aligned, wall {wall:.1f} s, process CPU {cpu:.1f} s, "
        f"{cpu / result.seconds * 60:.0f} CPU s per audio minute"
    )
    far = [(w["text"], w["startMs"], r["startMs"]) for w, r in checked if abs(w["startMs"] - r["startMs"]) > 150]
    assert far == []
    assert len(checked) >= 0.9 * len(words)
    low = [w["text"] for w, r in zip(words, reference) if r["score"] < 0.05]
    print(f"scores of the invented repeats {low}: {[w['score'] for w, r in zip(words, reference) if r['score'] < 0.05]}")


def language_case(name):
    case = json.loads((FIXTURES / name).read_text(encoding="utf-8"))
    with wave.open(str(FIXTURES / case["audio"])) as audio:
        pcm = audio.readframes(audio.getnframes())
    turns = [Turn(t["startMs"], t["endMs"], t["text"]) for t in case["turns"]]
    return pcm, turns, case


@pytest.mark.slow
@pytest.mark.parametrize("name", ["fleurs_hi_pyramids.json", "fleurs_ar_pyramids.json"])
def test_turns_in_other_scripts_time_every_word(mms, name):
    pcm, turns, case = language_case(name)
    result = align_pcm(mms, pcm, turns)
    assert result.skipped == []
    words, reference = result.words, case["reference"]
    assert [w["text"] for w in words] == turns[0].text.split()
    scores = [w["score"] for w in words]
    print(f"\n{case['language']}: {len(words)} words, median score {statistics.median(scores):.3f}, min {min(scores):.3f}")
    # The app drops words under 0.05 and times a whole turn roughly when its
    # median is under 0.5 (MIN_SCORE and MATCH_MEDIAN in klipolo-analysis).
    # A word with nothing alignable would score 0 here.
    assert min(scores) >= 0.3
    assert statistics.median(scores) >= 0.8
    starts = [w["startMs"] for w in words]
    assert starts == sorted(starts)
    assert all(w["startMs"] < w["endMs"] <= turns[0].end_ms for w in words)
    far = [(w["text"], w["startMs"], r["startMs"]) for w, r in zip(words, reference) if abs(w["startMs"] - r["startMs"]) > 150]
    assert far == []


@pytest.mark.slow
def test_long_turns_aligned_in_windows_match_one_pass(mms):
    pcm, turns, case = fixture_case()
    turn = Turn(0, 25_800, " ".join(t.text for t in turns))
    (planned,), _ = plan([turn], len(pcm) // 2)
    piece = pcm[2 * planned.start : 2 * planned.end]
    one_pass = mms.align(piece, planned.tokens)
    # 8 s windows with 2 s of context, so this 25.8 s turn takes 4 windows.
    windows = (mms.window_frames, mms.context_frames)
    mms.window_frames, mms.context_frames = 400, 100
    try:
        assert len(emission_windows(len(piece) // 2, 400, 100)) == 4
        windowed = mms.align(piece, planned.tokens)
    finally:
        mms.window_frames, mms.context_frames = windows
    assert len(windowed) == len(one_pass) == len(planned.tokens)
    gaps = [abs(a[0] - b[0]) for a, b in zip(one_pass, windowed)]
    print(f"\nwindowed vs one pass start gaps in frames: max {max(gaps)}, over 5: {sum(g > 5 for g in gaps)}")
    # Frames are 20 ms: within 5 frames (100 ms) for all but a few words.
    assert sum(gap > 5 for gap in gaps) <= 2, gaps


@pytest.mark.slow
def test_model_uses_the_configured_threads(mms):
    import torch

    assert torch.get_num_threads() == mms.threads


# The endpoint with the real model (slow).


@pytest.fixture
def mms_client(client, mms):
    client.app.state.aligner = mms
    token = client.post("/v1/devices", json={"device_id": "6b0e2f8e-7d61-4c39-9a51-3d7f0c6c9e11"}).json()["token"]
    client.headers["Authorization"] = f"Bearer {token}"
    return client


def post_align(client, turns):
    _, _, case = fixture_case()
    audio = (FIXTURES / case["audio"]).read_bytes()
    return client.post(
        "/v1/align",
        files={"audio": ("audio.wav", audio, "audio/wav"), "turns": (None, json.dumps(turns))},
    )


@pytest.mark.slow
def test_unspoken_text_gets_low_scores_not_500(mms_client):
    _, _, case = fixture_case()
    spoken = case["turns"][0]
    unspoken = {**spoken, "text": "purple elephants juggle seven frozen bananas beneath a quiet volcano"}
    response = post_align(mms_client, [spoken, unspoken])
    assert response.status_code == 200, response.text
    body = response.json()
    scores = {turn: sorted(w["score"] for w in body["words"] if w["turn"] == turn) for turn in (0, 1)}
    if not scores[1]:
        # Skipped with a reason is also fine.
        assert [s["turn"] for s in body["skipped"]] == [1]
        return
    median = {turn: values[len(values) // 2] for turn, values in scores.items()}
    mean = {turn: sum(values) / len(values) for turn, values in scores.items()}
    print(f"\nspoken: median {median[0]:.3f} mean {mean[0]:.3f}; unspoken: median {median[1]:.3f} mean {mean[1]:.3f} {scores[1]}")
    # Real speech-to-text turns score a median of 0.74 to 0.97. Words that
    # were never said can still find a few frames that sound alike, so single
    # scores vary, but the turn as a whole scores low.
    assert median[0] > 0.7
    assert median[1] < 0.5
    assert mean[1] < mean[0] - 0.25


@pytest.mark.slow
def test_odd_text_and_turns_never_give_a_500(mms_client):
    turns = [
        {"startMs": 0, "endMs": 400, "text": "the quick brown fox jumps over the lazy dog " * 4},
        {"startMs": 1000, "endMs": 3000, "text": "3 ½ cafés — 日本 R&D 1990s!!"},
        {"startMs": 2000, "endMs": 1000, "text": "backwards"},
        {"startMs": 30000, "endMs": 40000, "text": "after the end"},
        {"startMs": 24000, "endMs": 90000, "text": "challenges as a as a team"},
        {"startMs": 0, "endMs": 0, "text": ""},
    ]
    response = post_align(mms_client, turns)
    assert response.status_code == 200, response.text
    body = response.json()
    assert [(s["turn"], s["reason"]) for s in body["skipped"]] == [
        (0, TOO_SHORT),
        (2, align.ENDS_BEFORE_START),
        (3, PAST_END),
        (5, NO_TEXT),
    ]
    assert {w["turn"] for w in body["words"]} == {1, 4}
    assert [w["score"] for w in body["words"] if w["text"] == "—"] == [0.0]
    team = [w for w in body["words"] if w["turn"] == 4]
    print(f"\nclamped turn: {[(w['text'], w['startMs'], w['score']) for w in team]}")
    # The turn runs past the audio; its words are still found where they are
    # spoken ("challenges" at 24.08 s in the reference) and end by 25.8 s.
    assert abs(team[0]["startMs"] - 24_080) <= 150
    assert team[-1]["endMs"] <= 25_800


# MKL picks its CPU code the first time each math function runs. On a CPU
# without AVX2 (a 2013 Xeon E5-2680 v2), first calls racing from several
# threads die with SIGILL and take the server down. Measured cold: 8 of 10
# processes running torch.exp on 8 threads crashed; after the alignment
# aligner's own scores.exp() crashed 1 run in 5 even after the model loaded.


def run_python(code, env=None):
    return subprocess.run(
        [sys.executable, "-X", "faulthandler", "-c", code],
        cwd=Path(__file__).parents[1],
        capture_output=True,
        env=env,
    )


@pytest.mark.slow
def test_warmed_up_math_survives_many_threads():
    pytest.importorskip("torch")
    code = (
        "import torch\n"
        "from relay.align import warm_up_math\n"
        "torch.set_num_threads(8)\n"
        "warm_up_math(torch)\n"
        "for _ in range(20):\n"
        "    (torch.rand(600_000) * -15).exp()\n"
        "    (torch.rand(29_999) * -15).exp()\n"
    )
    for _ in range(10):
        result = run_python(code)
        assert result.returncode == 0, (result.returncode, result.stderr[-800:])


@pytest.mark.slow
def test_a_freshly_loaded_model_aligns_a_long_turn_on_many_threads():
    pytest.importorskip("torchaudio")
    code = (
        "import torch\n"
        "from relay.align import MmsAligner\n"
        "aligner = MmsAligner(threads=8)\n"
        "aligner.load()\n"
        "_, _, tokenizer, align = aligner._loaded\n"
        "emission = torch.randn(29_999, 28).log_softmax(-1)\n"
        "with torch.inference_mode():\n"
        "    align(emission, tokenizer(['abcdefghij'] * 1799))\n"
    )
    for _ in range(8):
        result = run_python(code)
        assert result.returncode == 0, (result.returncode, result.stderr[-800:])


@pytest.mark.slow
def test_cold_math_survives_many_threads_when_mkl_is_held_to_avx():
    # The image sets MKL_ENABLE_INSTRUCTIONS=AVX as a second guard: MKL then
    # never picks AVX2 code, warm-up or not. Measured on that Xeon without the
    # warm-up: 5 of 10 processes crashed unset, 0 of 40 with AVX.
    pytest.importorskip("torch")
    code = (
        "import torch\n"
        "torch.set_num_threads(8)\n"
        "for _ in range(20):\n"
        "    (torch.rand(600_000) * -15).exp()\n"
        "    (torch.rand(29_999) * -15).exp()\n"
    )
    env = {**os.environ, "MKL_ENABLE_INSTRUCTIONS": "AVX"}
    for _ in range(10):
        result = run_python(code, env)
        assert result.returncode == 0, (result.returncode, result.stderr[-800:])
