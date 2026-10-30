"""Meta Model API list prices and the cost of one call from its usage.

`cost_usd` prices the usage numbers Meta returns. When a call's usage is never
seen (Meta's stream broke off, a timeout, a success without usage),
`estimate_usage` builds a conservative stand-in from the request so the call
is still charged.
"""

import math
from collections.abc import Mapping
from typing import Any

SPARK_MODELS = frozenset({"muse-spark-1.3", "muse-spark-1.2"})
SAM_MODEL = "sam-3.1"
TRANSCRIBE_MODEL = "muse-voice-transcribe-1.0"

# The only models the relay forwards, by endpoint. Everything else (Muse
# Wonder, the *-contributor tiers that train on inputs) is refused.
MODELS_BY_ENDPOINT: dict[str, frozenset[str]] = {
    "responses": SPARK_MODELS | {SAM_MODEL},
    "asr/transcribe": frozenset({TRANSCRIBE_MODEL}),
}

# Spark's output per call is capped so the most a call can cost is known up
# front: the relay sets max_output_tokens when it is missing and refuses more
# than the maximum.
DEFAULT_MAX_OUTPUT_TOKENS = 8192
MAX_OUTPUT_TOKENS = 16384

SPARK_INPUT_USD = 1.25 / 1_000_000
SPARK_CACHED_INPUT_USD = 0.15 / 1_000_000
SPARK_OUTPUT_USD = 4.25 / 1_000_000
SAM_FRAME_USD = 0.20 / 1000
SAM_IMAGE_USD = 2.50 / 1000
# Meta bills at least 50 frames per SAM video request.
SAM_MIN_FRAMES = 50
TRANSCRIBE_HOUR_USD = 0.18
# The least an estimated call with media costs. Video sent by reference makes a
# tiny request, so its byte estimate says almost nothing about its cost.
MEDIA_ESTIMATE_FLOOR_USD = 0.10

# Meta's SAM usage names video frames but no image count is documented, so the
# relay counts the images in the request and adds them under this key.
SAM_IMAGES_KEY = "images_in_request"

# 16 kHz, 16-bit, mono WAV: the lowest byte rate Meta's transcription accepts,
# so dividing by it gives the longest the audio could be.
_WAV_MIN_BYTES_PER_SECOND = 32_000


def cost_usd(endpoint: str, model: str | None, usage: Mapping[str, Any]) -> float:
    """Price one call. Raises ValueError for an unpriced endpoint or model, or
    for usage that is not a mapping of non-negative numbers. An estimate
    (`"estimated": true`) costs at least its `floor_usd`."""
    if endpoint == "files":
        return 0.0
    if model not in MODELS_BY_ENDPOINT.get(endpoint, ()):
        raise ValueError(f"no price for {model!r} on {endpoint!r}")
    if not isinstance(usage, Mapping):
        raise ValueError("usage must be an object")
    cost = _cost(model, usage)
    if usage.get("estimated") is True:
        cost = max(cost, _count(usage, "floor_usd"))
    return cost


def _cost(model: str, usage: Mapping[str, Any]) -> float:
    if model in SPARK_MODELS:
        input_tokens = _count(usage, "input_tokens")
        details = usage.get("input_tokens_details") or {}
        if not isinstance(details, Mapping):
            raise ValueError("input_tokens_details must be an object")
        cached = min(_count(details, "cached_tokens"), input_tokens)
        output_tokens = _count(usage, "output_tokens")
        return (
            (input_tokens - cached) * SPARK_INPUT_USD
            + cached * SPARK_CACHED_INPUT_USD
            + output_tokens * SPARK_OUTPUT_USD
        )
    if model == SAM_MODEL:
        frames = _count(usage, "video_frames_processed")
        if frames > 0:
            frames = max(frames, SAM_MIN_FRAMES)
        return frames * SAM_FRAME_USD + _count(usage, SAM_IMAGES_KEY) * SAM_IMAGE_USD
    # Voice transcription is billed by audio length.
    return _count(usage, "audioDurationMs") / 3_600_000 * TRANSCRIBE_HOUR_USD


def estimate_usage(
    endpoint: str,
    model: str | None,
    *,
    request_bytes: int,
    audio_bytes: int = 0,
    images: int = 0,
    has_video: bool = False,
    media: bool = False,
) -> dict[str, Any]:
    """A conservative usage stand-in for a call whose real usage was not seen.

    Spark: input tokens are about request bytes / 4, priced as input. SAM:
    the 50-frame video minimum plus each image in the request. Transcription:
    the audio's longest possible length. Both byte and frame estimates are far
    too low for video sent by file_id or URL (Meta counts about 131 tokens per
    sampled frame), so a call with `media` costs at least
    MEDIA_ESTIMATE_FLOOR_USD.
    """
    if model in SPARK_MODELS:
        usage: dict[str, Any] = {"input_tokens": math.ceil(request_bytes / 4), "output_tokens": 0}
    elif model == SAM_MODEL:
        usage = {"video_frames_processed": SAM_MIN_FRAMES if has_video else 0, SAM_IMAGES_KEY: images}
    elif model == TRANSCRIBE_MODEL:
        seconds = audio_bytes / _WAV_MIN_BYTES_PER_SECOND
        usage = {"audioDurationMs": math.ceil(seconds * 1000)}
    else:
        usage = {}
    usage["estimated"] = True
    if media:
        usage["floor_usd"] = MEDIA_ESTIMATE_FLOOR_USD
    return usage


def _count(usage: Mapping[str, Any], key: str) -> float:
    value = usage.get(key)
    if value is None:
        return 0.0
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise ValueError(f"usage {key} must be a number")
    if not math.isfinite(value) or value < 0:
        raise ValueError(f"usage {key} must be a finite, non-negative number")
    return float(value)
