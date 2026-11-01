"""Meta Model API list prices and the cost of one call from its usage.

`cost_usd` prices the usage numbers Meta returns. `reservation_usage` is the
most a call could cost, worked out from the request before it goes out. The
relay holds that amount against the caps while the call runs, and charges it
when the call's real usage can't be read (Meta's stream broke off, a timeout,
a success without usable usage).
"""

import math
from collections.abc import Mapping
from typing import Any

SPARK_MODELS = frozenset({"muse-spark-1.3", "muse-spark-1.2"})
SAM_MODEL = "sam-3.1"
TRANSCRIBE_MODEL = "muse-voice-transcribe-1.0"
IMAGE_MODEL = "muse-image-1.0"

# The only models the relay forwards, by endpoint. Everything else (Muse
# Wonder, the *-contributor tiers that train on inputs) is refused.
MODELS_BY_ENDPOINT: dict[str, frozenset[str]] = {
    "responses": SPARK_MODELS | {SAM_MODEL},
    "asr/transcribe": frozenset({TRANSCRIBE_MODEL}),
    "images/generations": frozenset({IMAGE_MODEL}),
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
# Muse Image, per image returned. A call asks for at most MAX_IMAGES_PER_CALL,
# and reserves that many whatever it asks for.
IMAGE_USD = 0.01
MAX_IMAGES_PER_CALL = 2
# Added to the reservation of any call that isn't text only: the request is
# small, but the media or stored items behind it are not.
MEDIA_RESERVE_USD = 0.75
# Reserved for transcription audio whose WAV header can't be read: Meta takes
# at most 10 minutes per request.
UNREADABLE_AUDIO_SECONDS = 600

# Meta's SAM usage names video frames but no image count is documented, so the
# relay counts the images in the request and adds them under this key.
SAM_IMAGES_KEY = "images_in_request"
# Muse Image's price is per image returned: the relay counts them in the answer
# and records the count under this key.
IMAGES_KEY = "images"


def cost_usd(endpoint: str, model: str | None, usage: Mapping[str, Any]) -> float:
    """Price one call. Raises ValueError for an unpriced endpoint or model, or
    for usage that is not a mapping of non-negative numbers or lacks the
    counters the price needs (missing counters would price as zero). A
    reservation (`"reserved": true`) also adds its `media_usd`."""
    if endpoint == "files":
        return 0.0
    if model not in MODELS_BY_ENDPOINT.get(endpoint, ()):
        raise ValueError(f"no price for {model!r} on {endpoint!r}")
    if not isinstance(usage, Mapping):
        raise ValueError("usage must be an object")
    cost = _cost(model, usage)
    if usage.get("reserved") is True:
        cost += _count(usage, "media_usd")
    return cost


def _cost(model: str, usage: Mapping[str, Any]) -> float:
    if model in SPARK_MODELS:
        input_tokens = _count(usage, "input_tokens", required=True)
        details = usage.get("input_tokens_details") or {}
        if not isinstance(details, Mapping):
            raise ValueError("input_tokens_details must be an object")
        cached = min(_count(details, "cached_tokens"), input_tokens)
        output_tokens = _count(usage, "output_tokens", required=True)
        return (
            (input_tokens - cached) * SPARK_INPUT_USD
            + cached * SPARK_CACHED_INPUT_USD
            + output_tokens * SPARK_OUTPUT_USD
        )
    if model == IMAGE_MODEL:
        return _count(usage, IMAGES_KEY, required=True) * IMAGE_USD
    if model == SAM_MODEL:
        frames = _count(usage, "video_frames_processed")
        if frames > 0:
            frames = max(frames, SAM_MIN_FRAMES)
        return frames * SAM_FRAME_USD + _count(usage, SAM_IMAGES_KEY) * SAM_IMAGE_USD
    # Voice transcription is billed by audio length.
    return _count(usage, "audioDurationMs", required=True) / 3_600_000 * TRANSCRIBE_HOUR_USD


def reservation_usage(
    endpoint: str,
    model: str | None,
    *,
    request_bytes: int = 0,
    max_output_tokens: int = 0,
    media: bool = False,
    audio_seconds: float = 0.0,
    images: int = 0,
    has_video: bool = False,
    video_frames: int = 0,
) -> dict[str, Any]:
    """The most a call could cost, as usage that `cost_usd` prices.

    Spark: one input token per request byte (adversarial text can tokenize
    close to that), plus the whole output limit, plus MEDIA_RESERVE_USD
    unless the call is text only.
    Transcription: the audio's length. SAM: its videos' `video_frames` (each
    at least the 50-frame minimum, counted from the uploads, so they bound the
    cost) and each image; with no video, each image plus MEDIA_RESERVE_USD.
    Muse Image: the most images a call may ask for.
    """
    if model in SPARK_MODELS:
        usage: dict[str, Any] = {"input_tokens": request_bytes, "output_tokens": max_output_tokens}
    elif model == SAM_MODEL:
        frames = max(video_frames, SAM_MIN_FRAMES) if has_video else 0
        usage = {"video_frames_processed": frames, SAM_IMAGES_KEY: images}
        media = not has_video
    elif model == TRANSCRIBE_MODEL:
        usage = {"audioDurationMs": math.ceil(audio_seconds * 1000)}
        media = False
    elif model == IMAGE_MODEL:
        usage = {IMAGES_KEY: MAX_IMAGES_PER_CALL}
        media = False
    else:
        usage = {}
    if media:
        usage["media_usd"] = MEDIA_RESERVE_USD
    usage["reserved"] = True
    return usage


def _count(usage: Mapping[str, Any], key: str, required: bool = False) -> float:
    value = usage.get(key)
    if value is None:
        if required:
            raise ValueError(f"usage has no {key}")
        return 0.0
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise ValueError(f"usage {key} must be a number")
    if not math.isfinite(value) or value < 0:
        raise ValueError(f"usage {key} must be a finite, non-negative number")
    return float(value)
