"""Word times for transcript turns, from Meta's open MMS forced aligner.

Meta's speech-to-text gives times per turn only. Each turn's text is aligned
to its own slice of the audio (the turn plus 0.25 s on each side, clamped to
the audio) with torchaudio's MMS_FA pipeline, which gives every word a start,
an end and a score from 0 to 1. Words the transcript invented score near 0.

Planning (normalizing the text, slicing, clamping, skipping turns that cannot
be aligned) and assembling the words is plain Python, so it is tested without
torch. torch and the model load once, on first use or at a startup warm-up.
"""

import logging
import threading
import warnings
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Any, Protocol

from relay.text_norm import normalize

logger = logging.getLogger(__name__)

SAMPLE_RATE = 16000
PAD_MS = 250
# wav2vec2's feature extractor: (kernel, stride) of each conv layer. One
# emission frame per 320 samples (20 ms), each seeing 400 samples.
_CONV_LAYERS = ((10, 5), (3, 2), (3, 2), (3, 2), (3, 2), (2, 2), (2, 2))
FRAME_SAMPLES = 320
FRAME_MS = FRAME_SAMPLES * 1000 // SAMPLE_RATE
RECEPTIVE_FIELD = 400
# Self-attention memory grows with the square of the input, so long slices
# are run through the model in windows of about 30 s with 2 s of context on
# each side, and the emissions joined. Shorter slices take one pass.
WINDOW_FRAMES = 1500
CONTEXT_FRAMES = 100

NO_TEXT = "The turn has no text."
ENDS_BEFORE_START = "The turn ends before it starts."
PAST_END = "The turn starts after the audio ends."
NOTHING_TO_ALIGN = "The turn has no words that can be aligned."
TOO_SHORT = "The turn is too short for its text."
FAILED = "The turn could not be aligned."

# One aligned word: first frame, frame after the last, score from 0 to 1.
# Frames count from the start of the turn's slice.
Span = tuple[int, int, float]


@dataclass(frozen=True)
class Turn:
    start_ms: int
    end_ms: int
    text: str


@dataclass(frozen=True)
class PlannedTurn:
    index: int
    start: int  # first sample of the slice
    end: int  # one past the last sample
    words: tuple[tuple[str, str], ...]  # (original, aligner form) for every word

    @property
    def tokens(self) -> list[str]:
        """One aligner word per word that has something alignable."""
        return [form.replace(" ", "") for _, form in self.words if form]

    @property
    def seconds(self) -> float:
        return (self.end - self.start) / SAMPLE_RATE


@dataclass(frozen=True)
class Skipped:
    turn: int
    reason: str

    def as_json(self) -> dict[str, Any]:
        return {"turn": self.turn, "reason": self.reason}


@dataclass
class Alignment:
    words: list[dict[str, Any]] = field(default_factory=list)
    skipped: list[Skipped] = field(default_factory=list)
    seconds: float = 0.0  # audio actually aligned


@dataclass(frozen=True)
class Window:
    start: int  # samples fed to the model
    end: int
    keep_from: int  # emission frames of this window that are kept
    keep_to: int


class Aligner(Protocol):
    def load(self) -> None: ...

    def align(self, pcm: bytes, tokens: list[str]) -> list[Span]: ...


def plan(turns: Sequence[Turn], n_samples: int) -> tuple[list[PlannedTurn], list[Skipped]]:
    """The slice and words of each turn, or why it is skipped."""
    planned: list[PlannedTurn] = []
    skipped: list[Skipped] = []
    duration_ms = n_samples * 1000 // SAMPLE_RATE
    for index, turn in enumerate(turns):
        words = tuple(normalize(turn.text))
        start = max(0, turn.start_ms - PAD_MS) * SAMPLE_RATE // 1000
        end = min(n_samples, max(0, turn.end_ms + PAD_MS) * SAMPLE_RATE // 1000)
        tokens = [form.replace(" ", "") for _, form in words if form]
        if not words:
            reason = NO_TEXT
        elif turn.end_ms < turn.start_ms:
            reason = ENDS_BEFORE_START
        elif turn.start_ms >= duration_ms:
            reason = PAST_END
        elif not tokens:
            reason = NOTHING_TO_ALIGN
        elif end <= start or emission_frames(end - start) < frames_needed(tokens):
            reason = TOO_SHORT
        else:
            planned.append(PlannedTurn(index, start, end, words))
            continue
        skipped.append(Skipped(index, reason))
    return planned, skipped


def emission_frames(n_samples: int) -> int:
    """How many emission frames the model gives for `n_samples` of audio."""
    n = n_samples
    for kernel, stride in _CONV_LAYERS:
        if n < kernel:
            return 0
        n = (n - kernel) // stride + 1
    return n


def frames_needed(tokens: Sequence[str]) -> int:
    """CTC needs a frame per letter, plus a blank between repeated letters."""
    letters = "".join(tokens)
    repeats = sum(a == b for a, b in zip(letters, letters[1:]))
    return len(letters) + repeats


def emission_windows(
    n_samples: int, window: int = WINDOW_FRAMES, context: int = CONTEXT_FRAMES
) -> list[Window]:
    """Pieces of a slice to run through the model, so that the kept frames of
    all windows, in order, are exactly the frames of the whole slice."""
    total = emission_frames(n_samples)
    if total <= window + 2 * context:
        return [Window(0, n_samples, 0, total)]
    windows = []
    for first in range(0, total, window):
        last = min(first + window, total)
        computed_from = max(0, first - context)
        computed_to = min(total, last + context)
        start = computed_from * FRAME_SAMPLES
        end = min(n_samples, (computed_to - 1) * FRAME_SAMPLES + RECEPTIVE_FIELD)
        windows.append(Window(start, end, first - computed_from, last - computed_from))
    return windows


def assemble(turn: PlannedTurn, spans: Sequence[Span]) -> list[dict[str, Any]]:
    """Words in ms from the start of the audio. Words with nothing alignable
    score 0 and share the gap between their aligned neighbours."""
    origin_ms = turn.start * 1000 // SAMPLE_RATE
    timed: list[tuple[int, int, float] | None] = []
    aligned = iter(spans)
    for _, form in turn.words:
        if form:
            first, after, score = next(aligned)
            timed.append((origin_ms + first * FRAME_MS, origin_ms + after * FRAME_MS, score))
        else:
            timed.append(None)

    words = []
    i = 0
    while i < len(timed):
        if timed[i] is not None:
            start_ms, end_ms, score = timed[i]
            words.append(_word(turn, i, start_ms, end_ms, score))
            i += 1
            continue
        run_end = i
        while run_end < len(timed) and timed[run_end] is None:
            run_end += 1
        before = timed[i - 1][1] if i > 0 else None
        after = timed[run_end][0] if run_end < len(timed) else None
        low = before if before is not None else after
        high = after if after is not None else before
        high = max(low, high)
        count = run_end - i
        for k in range(count):
            start_ms = low + round((high - low) * k / count)
            end_ms = low + round((high - low) * (k + 1) / count)
            words.append(_word(turn, i + k, start_ms, end_ms, 0.0))
        i = run_end
    return words


def _word(turn: PlannedTurn, i: int, start_ms: int, end_ms: int, score: float) -> dict[str, Any]:
    return {
        "text": turn.words[i][0],
        "startMs": start_ms,
        "endMs": end_ms,
        "score": round(score, 3),
        "turn": turn.index,
    }


def align_turns(
    aligner: Aligner, read_pcm: Callable[[int, int], bytes], planned: Sequence[PlannedTurn]
) -> Alignment:
    """Align each planned turn to its slice. A turn the model fails on is
    skipped with a reason; the others still come back."""
    result = Alignment()
    for turn in planned:
        try:
            spans = aligner.align(read_pcm(turn.start, turn.end), turn.tokens)
            if len(spans) != len(turn.tokens):
                raise ValueError(f"{len(spans)} spans for {len(turn.tokens)} words")
        except Exception:
            # The text is never logged.
            logger.exception("could not align turn %d (%.1f s)", turn.index, turn.seconds)
            result.skipped.append(Skipped(turn.index, FAILED))
            continue
        result.words.extend(assemble(turn, spans))
        result.seconds += turn.seconds
    return result


class MmsAligner:
    """torchaudio's MMS_FA model, loaded once. Safe to call from several
    threads at a time: the model only runs inference."""

    def __init__(
        self, threads: int, window_frames: int = WINDOW_FRAMES, context_frames: int = CONTEXT_FRAMES
    ) -> None:
        self.threads = threads
        self.window_frames = window_frames
        self.context_frames = context_frames
        self._lock = threading.Lock()
        self._loaded: tuple[Any, ...] | None = None

    def load(self) -> None:
        with self._lock:
            if self._loaded is not None:
                return
            import torch
            import torchaudio

            # torchaudio is pinned to 2.8 for forced_align; its notice that
            # 2.9 removes it would otherwise land in the log on first use.
            warnings.filterwarnings("ignore", message=r".*forced_align has been deprecated", category=UserWarning)
            torch.set_num_threads(self.threads)
            bundle = torchaudio.pipelines.MMS_FA
            model = bundle.get_model(with_star=False)
            model.eval()
            self._loaded = (torch, model, bundle.get_tokenizer(), bundle.get_aligner())
            logger.info("alignment model loaded, %d threads", self.threads)

    def align(self, pcm: bytes, tokens: list[str]) -> list[Span]:
        self.load()
        torch, model, tokenizer, aligner = self._loaded
        waveform = torch.frombuffer(bytearray(pcm), dtype=torch.int16).to(torch.float32) / 32768.0
        waveform = waveform.unsqueeze(0)
        with torch.inference_mode():
            pieces = []
            for window in emission_windows(waveform.shape[1], self.window_frames, self.context_frames):
                emission, _ = model(waveform[:, window.start : window.end])
                pieces.append(emission[0, window.keep_from : window.keep_to])
            emission = torch.cat(pieces)
            word_spans = aligner(emission, tokenizer(tokens))
        spans = []
        for token_spans in word_spans:
            frames = sum(span.end - span.start for span in token_spans)
            score = sum(span.score * (span.end - span.start) for span in token_spans) / max(1, frames)
            spans.append((token_spans[0].start, token_spans[-1].end, float(score)))
        return spans
