"""Word times for transcript turns, from Meta's open MMS forced aligner.

Meta's speech-to-text gives times per turn only. Each turn's text is aligned
to its own slice of the audio (the turn plus 0.25 s on each side, clamped to
the audio) with torchaudio's MMS_FA pipeline, which gives every word a start,
an end and a score from 0 to 1. Words the transcript invented score near 0.

Planning (normalizing the text, slicing, clamping, skipping turns that cannot
be aligned) and assembling the words is plain Python, so it is tested without
torch. torch and the model load once, on first use or at a startup warm-up.

`POST /v1/align` is open to every registered device, with or without relay
AI. It takes multipart `audio` (WAV, 16 kHz mono 16-bit, at most 10 minutes)
and `turns` (JSON, at most 300 `{"startMs", "endMs", "text"}`) and returns
`{"words": [...], "skipped": [{"turn", "reason"}]}`. Each device gets
ALIGN_MINUTES_PER_DEVICE_PER_DAY minutes of aligned audio per UTC day. The
turns' slices may add up to little more than the audio (400 otherwise), and
they are taken from the day's minutes before any work starts (429 when they
do not fit); slices that were not aligned are given back afterwards. The
audio sits in a temp file only while the request runs; nothing is stored.
"""

import asyncio
import json
import logging
import math
import os
import sqlite3
import struct
import tempfile
import threading
import time
import warnings
import wave
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from typing import Any, BinaryIO, Protocol

import anyio
from fastapi import APIRouter, Request
from pydantic import BaseModel, TypeAdapter, ValidationError
from python_multipart.exceptions import MultipartParseError
from python_multipart.multipart import MultipartParser, MultipartState, parse_options_header
from starlette.concurrency import run_in_threadpool
from starlette.requests import ClientDisconnect

from relay import db
from relay.config import Settings
from relay.deps import AppSettings, Conn
from relay.devices import Device
from relay.errors import RelayError, error_response
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
# CTC needs at least one frame per letter, but forced alignment's memory
# grows with frames x letters, so a turn may hold at most 0.6 letters per
# frame (30 a second; fast speech is about 20). More text than that is
# skipped as too short for its audio.
MAX_LETTERS_PER_FRAME = 0.6

MAX_AUDIO_SECONDS = 600
MAX_TURNS = 300
# Ten minutes of PCM plus room for the WAV header and any extra chunks.
MAX_AUDIO_BYTES = MAX_AUDIO_SECONDS * SAMPLE_RATE * 2 + 64 * 1024
# 128 KB of turns is far more text than 10 minutes of audio can hold (at
# most 0.6 letters per frame, about 18,000 letters), and parsing it takes a
# fraction of a second.
MAX_TURNS_BYTES = 128 * 1024
MAX_BODY_BYTES = MAX_AUDIO_BYTES + MAX_TURNS_BYTES + 64 * 1024
# The turns' padded slices may add up to the audio's length plus this much
# per turn: the 0.5 s of padding and up to 0.5 s of overlap between turns.
SLACK_PER_TURN_S = 2 * PAD_MS / 1000 + 0.5
# Requests in the relay at once: per device, and per alignment worker
# (ALIGN_CONCURRENCY) including those waiting for one.
MAX_REQUESTS_PER_DEVICE = 2
REQUESTS_PER_WORKER = 4

router = APIRouter()

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
        elif end <= start or frames_needed(tokens) > MAX_LETTERS_PER_FRAME * emission_frames(end - start):
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
    aligner: Aligner,
    read_pcm: Callable[[int, int], bytes],
    planned: Sequence[PlannedTurn],
    stop: Callable[[], bool] = lambda: False,
) -> Alignment:
    """Align each planned turn to its slice. A turn the model fails on is
    skipped with a reason; the others still come back. `stop` is checked
    before each turn, so work ends early once nobody waits for it."""
    result = Alignment()
    for turn in planned:
        if stop():
            break
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


# Math functions MKL implements for torch on CPU (ATen's vml).
_MKL_MATH = (
    "exp", "expm1", "log", "log1p", "log10", "log2", "sqrt", "rsqrt", "tanh", "erf", "erfc", "erfinv",
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "lgamma", "trunc",
)  # fmt: skip


def warm_up_math(torch: Any) -> None:
    """Run each MKL math function once, on one element, so on one thread.

    MKL picks its CPU code the first time a function runs. On CPUs without
    AVX2, such as a 2013 Xeon E5-2680 v2, first calls that race from
    several threads can execute an AVX2 instruction and kill the process with
    SIGILL. Measured there: torch.exp on 8 threads crashed 8 of 10 fresh
    processes; after this warm-up, none.
    """
    values = torch.full((1,), 0.5)
    for name in _MKL_MATH:
        getattr(torch, name)(values)


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
            # Everything that runs for the first time runs on one thread: see
            # warm_up_math.
            torch.set_num_threads(1)
            warm_up_math(torch)
            bundle = torchaudio.pipelines.MMS_FA
            model = bundle.get_model(with_star=False)
            model.eval()
            tokenizer, aligner = bundle.get_tokenizer(), bundle.get_aligner()
            # One tiny alignment, so every kernel on the path is first used here.
            with torch.inference_mode():
                emission, _ = model(torch.randn(1, SAMPLE_RATE) * 0.01)
                aligner(emission[0], tokenizer(["a"]))
            torch.set_num_threads(self.threads)
            self._loaded = (torch, model, tokenizer, aligner)
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


def warm_up(aligner: Aligner) -> None:
    """Load the model in the background, so the server answers at once and
    the first alignment does not wait for it."""

    def load() -> None:
        try:
            aligner.load()
        except Exception:
            logger.exception("alignment model could not load at startup; will retry on first use")

    threading.Thread(target=load, name="align-warmup", daemon=True).start()


# The endpoint.

_BAD_FORM = "Send multipart/form-data with one audio part and one turns part."
_NOT_WAV = "Send the audio as a WAV file: 16 kHz, mono, 16-bit PCM."
_BAD_TURNS = "Send turns as a JSON list of objects with startMs, endMs and text."
_TOO_LARGE = "The upload is too large. Send at most 10 minutes of 16 kHz mono 16-bit audio."
_COVERS_TOO_MUCH = (
    "The turns add up to more than the audio. Each turn should cover its own part of the audio, with little overlap."
)


class _TurnIn(BaseModel):
    startMs: int
    endMs: int
    text: str


_TURN = TypeAdapter(_TurnIn)


class AlignerUnavailable(Exception):
    pass


@dataclass(frozen=True)
class Reservation:
    day: str
    seconds: float


@router.post("/v1/align")
async def align_words(request: Request, device: Device, conn: Conn, settings: AppSettings) -> Any:
    check_quota(device, settings)
    device_id = device["device_id"]
    slot = _acquire_slot(request, device_id)
    path = None
    reservation = None
    result = Alignment()
    gone = threading.Event()
    watcher = None
    try:
        boundary = _boundary(request)
        fd, path = tempfile.mkstemp(prefix="align-", suffix=".wav")
        with os.fdopen(fd, "wb") as audio_file:
            turns_raw = await _read_form(request, boundary, audio_file)
        watcher = asyncio.create_task(_watch_for_hang_up(request, gone))
        turns = parse_turns(turns_raw)
        n_samples = await run_in_threadpool(wav_samples, path)
        planned, skipped = plan(turns, n_samples)
        seconds = check_coverage(planned, n_samples)
        if planned:
            reservation = await run_in_threadpool(reserve_seconds, conn, device_id, seconds, settings)
            started = time.monotonic()
            async with request.app.state.align_gate:
                try:
                    if not gone.is_set():
                        result = await anyio.to_thread.run_sync(
                            _align_file, request.app.state.aligner, path, planned, gone.is_set
                        )
                except AlignerUnavailable:
                    raise RelayError(
                        503, "align_unavailable", "Word timing is not available right now. Try again later."
                    ) from None
            logger.info(
                "aligned %.1f s of audio in %d turns for device %s in %.1f s",
                result.seconds,
                len(planned),
                device_id[:8],
                time.monotonic() - started,
            )
        if gone.is_set():
            # Nobody reads this; the unaligned turns are given back below.
            return error_response(400, "client_disconnected", "The request was cut off.")
        return {
            "words": result.words,
            "skipped": [s.as_json() for s in sorted(skipped + result.skipped, key=lambda s: s.turn)],
        }
    except ClientDisconnect:
        return error_response(400, "client_disconnected", "The request was cut off.")
    finally:
        if watcher is not None:
            watcher.cancel()
        if reservation is not None and reservation.seconds - result.seconds > 1e-6:
            with anyio.CancelScope(shield=True):
                await run_in_threadpool(
                    refund_seconds, conn, device_id, reservation, reservation.seconds - result.seconds
                )
        if path is not None:
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass
        slot.release()


def _acquire_slot(request: Request, device_id: str) -> Any:
    try:
        return request.app.state.align_in_flight.acquire(device_id)
    except RelayError as exc:
        # An alignment takes tens of seconds; asking again in 1 s is no use.
        raise RelayError(exc.status_code, exc.error, exc.message, headers={"Retry-After": "30"}) from None


def check_coverage(planned: Sequence[PlannedTurn], n_samples: int) -> float:
    """The seconds of audio the planned turns run through the model, or 400
    when that is far more than the audio sent. This bounds the work, and the
    minutes charged, by the length of the upload."""
    seconds = sum(turn.seconds for turn in planned)
    if seconds > n_samples / SAMPLE_RATE + SLACK_PER_TURN_S * len(planned):
        raise RelayError(400, "invalid_request", _COVERS_TOO_MUCH)
    return seconds


def check_quota(device: Any, settings: Settings) -> None:
    """429 before the upload is read when today's minutes are used up."""
    used = device["align_seconds_day"] if device["align_day"] == _today() else 0.0
    if used >= settings.align_minutes_per_device_per_day * 60:
        raise _quota_error(settings, used)


def reserve_seconds(
    conn: sqlite3.Connection, device_id: str, seconds: float, settings: Settings
) -> Reservation:
    """Take the planned seconds from today's minutes before any work, in one
    transaction, so two requests in flight cannot both pass the limit. 429
    when they do not fit."""
    day = _today()
    with db.transaction(conn):
        row = conn.execute(
            "SELECT align_seconds_day, align_day FROM devices WHERE device_id = ?", (device_id,)
        ).fetchone()
        used = row["align_seconds_day"] if row["align_day"] == day else 0.0
        if used + seconds > settings.align_minutes_per_device_per_day * 60 + 1e-6:
            raise _quota_error(settings, used, seconds)
        conn.execute(
            "UPDATE devices SET align_seconds_day = ?, align_day = ? WHERE device_id = ?",
            (used + seconds, day, device_id),
        )
    return Reservation(day, seconds)


def refund_seconds(conn: sqlite3.Connection, device_id: str, reservation: Reservation, seconds: float) -> None:
    """Give back reserved seconds that were not aligned: skipped or failed
    turns, a model that would not load, a client that hung up. A failed
    write is logged; the device is then charged too much, never too little."""
    try:
        with db.transaction(conn):
            conn.execute(
                "UPDATE devices SET align_seconds_day = MAX(0, align_seconds_day - ?)"
                " WHERE device_id = ? AND align_day = ?",
                (seconds, device_id, reservation.day),
            )
    except sqlite3.Error:
        logger.exception("could not refund %.1f s of alignment to device %s", seconds, device_id[:8])


def _quota_error(settings: Settings, used: float, needed: float | None = None) -> RelayError:
    minutes = settings.align_minutes_per_device_per_day
    left = minutes * 60 - used
    if needed is None or left <= 0:
        message = f"This device has used today's {minutes:.10g} minutes of word timing. It resets at midnight UTC."
    else:
        message = (
            f"This request needs {_amount(needed, math.ceil)} of word timing and this device has "
            f"{_amount(left, math.floor)} left today. Send less audio or try again after midnight UTC."
        )
    now = datetime.now(UTC)
    midnight = datetime.combine(now.date() + timedelta(days=1), datetime.min.time(), UTC)
    return RelayError(
        429,
        "align_quota",
        message,
        headers={"Retry-After": str(max(1, int((midnight - now).total_seconds())))},
    )


def _amount(seconds: float, rounding: Callable[[float], int]) -> str:
    if seconds < 90:
        count, unit = rounding(seconds), "second"
    else:
        count, unit = rounding(seconds / 60), "minute"
    return f"{count} {unit}" + ("" if count == 1 else "s")


def parse_turns(raw: bytes) -> list[Turn]:
    try:
        data = json.loads(raw)
    except (ValueError, RecursionError):
        # RecursionError: lists nested thousands deep.
        raise RelayError(400, "invalid_request", _BAD_TURNS) from None
    if not isinstance(data, list):
        raise RelayError(400, "invalid_request", _BAD_TURNS)
    if len(data) > MAX_TURNS:
        raise RelayError(400, "invalid_request", f"Send at most {MAX_TURNS} turns.")
    turns = []
    for index, item in enumerate(data):
        try:
            turn = _TURN.validate_python(item)
        except ValidationError:
            raise RelayError(
                400,
                "invalid_request",
                f"Turn {index} is not valid. Each turn needs startMs and endMs as whole numbers and text as a string.",
            ) from None
        turns.append(Turn(turn.startMs, turn.endMs, turn.text))
    return turns


def wav_samples(path: str) -> int:
    """How many samples the WAV file holds, or 400 with the reason it can't
    be used."""
    try:
        with wave.open(path, "rb") as audio:
            if audio.getcomptype() != "NONE":
                raise RelayError(400, "invalid_request", _NOT_WAV)
            if audio.getnchannels() != 1:
                raise RelayError(400, "invalid_request", "The audio must be mono.")
            if audio.getsampwidth() != 2:
                raise RelayError(400, "invalid_request", "The audio must be 16-bit PCM.")
            if audio.getframerate() != SAMPLE_RATE:
                raise RelayError(400, "invalid_request", "The audio must be 16 kHz.")
            # Count the samples actually sent, not the header's size: a
            # streaming writer leaves it at 0xFFFFFFFF, and a header can
            # claim more than the file holds.
            limit = MAX_AUDIO_SECONDS * SAMPLE_RATE
            n_samples = len(audio.readframes(limit + 1)) // 2
    except (wave.Error, EOFError, RuntimeError, struct.error):
        # RuntimeError: a chunk whose size runs past the end of the file.
        raise RelayError(400, "invalid_request", _NOT_WAV) from None
    if n_samples > limit:
        raise RelayError(400, "invalid_request", "The audio is longer than 10 minutes.")
    if n_samples == 0:
        raise RelayError(400, "invalid_request", "The audio is empty.")
    return n_samples


async def _watch_for_hang_up(request: Request, gone: threading.Event) -> None:
    """With the body read, the next message is the client hanging up."""
    while True:
        message = await request.receive()
        if message["type"] == "http.disconnect":
            gone.set()
            return


def _align_file(
    aligner: Aligner, path: str, planned: Sequence[PlannedTurn], stop: Callable[[], bool]
) -> Alignment:
    """The CPU work, in a worker thread."""
    try:
        aligner.load()
    except Exception as exc:
        logger.exception("alignment model could not load")
        raise AlignerUnavailable from exc
    with wave.open(path, "rb") as audio:

        def read_pcm(start: int, end: int) -> bytes:
            audio.setpos(start)
            return audio.readframes(end - start)

        return align_turns(aligner, read_pcm, planned, stop)


def _today() -> str:
    return datetime.now(UTC).date().isoformat()


def _boundary(request: Request) -> bytes:
    kind, options = parse_options_header(request.headers.get("content-type", ""))
    boundary = options.get(b"boundary")
    if kind != b"multipart/form-data" or not boundary:
        raise RelayError(400, "invalid_request", _BAD_FORM)
    declared = request.headers.get("content-length")
    if declared is not None:
        if not declared.isdigit():
            raise RelayError(400, "invalid_request", "The Content-Length header is not valid.")
        if int(declared) > MAX_BODY_BYTES:
            raise RelayError(413, "too_large", _TOO_LARGE)
    return boundary


async def _read_form(request: Request, boundary: bytes, audio_file: BinaryIO) -> bytes:
    """Stream the body: the audio part into `audio_file`, the turns part into
    memory. Exactly one of each, within the size limits."""
    names: list[str] = []
    headers: dict[bytes, bytes] = {}
    header_name, header_value = bytearray(), bytearray()
    turns = bytearray()
    audio_bytes = 0

    def on_part_begin() -> None:
        if len(names) == 2:
            raise RelayError(400, "invalid_request", _BAD_FORM)
        names.append("")
        headers.clear()

    def on_header_end() -> None:
        headers[bytes(header_name).lower()] = bytes(header_value)
        header_name.clear()
        header_value.clear()

    def on_headers_finished() -> None:
        _, disposition = parse_options_header(headers.get(b"content-disposition", b""))
        names[-1] = disposition.get(b"name", b"").decode("utf-8", "replace")
        if names.count(names[-1]) > 1:
            raise RelayError(400, "invalid_request", _BAD_FORM)

    def on_part_data(data: bytes, start: int, end: int) -> None:
        nonlocal audio_bytes
        if names[-1] == "audio":
            audio_bytes += end - start
            if audio_bytes > MAX_AUDIO_BYTES:
                raise RelayError(413, "too_large", _TOO_LARGE)
            audio_file.write(data[start:end])
        elif names[-1] == "turns":
            turns.extend(data[start:end])
            if len(turns) > MAX_TURNS_BYTES:
                raise RelayError(413, "too_large", "The turns are larger than 128 KB.")

    parser = MultipartParser(
        boundary,
        {
            "on_part_begin": on_part_begin,
            "on_header_field": lambda data, start, end: header_name.extend(data[start:end]),
            "on_header_value": lambda data, start, end: header_value.extend(data[start:end]),
            "on_header_end": on_header_end,
            "on_headers_finished": on_headers_finished,
            "on_part_data": on_part_data,
        },
    )
    received = 0
    try:
        async for chunk in request.stream():
            received += len(chunk)
            if received > MAX_BODY_BYTES:
                raise RelayError(413, "too_large", _TOO_LARGE)
            parser.write(chunk)
        parser.finalize()
    except (MultipartParseError, ValueError):
        raise RelayError(400, "invalid_request", _BAD_FORM) from None
    if parser.state != MultipartState.END or sorted(names) != ["audio", "turns"]:
        raise RelayError(400, "invalid_request", _BAD_FORM)
    return bytes(turns)
