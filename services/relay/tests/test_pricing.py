import math

import pytest

from relay.pricing import cost_usd, estimate_usage


def test_spark_cost_counts_cached_tokens_at_cached_price():
    usage = {"input_tokens": 10000, "input_tokens_details": {"cached_tokens": 4000}, "output_tokens": 500}

    cost = cost_usd("responses", "muse-spark-1.3", usage)

    assert cost == pytest.approx(6000 * 1.25e-6 + 4000 * 0.15e-6 + 500 * 4.25e-6)


def test_spark_1_2_has_the_same_price():
    usage = {"input_tokens": 1000, "output_tokens": 100}

    assert cost_usd("responses", "muse-spark-1.2", usage) == pytest.approx(
        cost_usd("responses", "muse-spark-1.3", usage)
    )


def test_spark_without_cache_details_charges_all_input_at_full_price():
    usage = {"input_tokens": 42291, "output_tokens": 517, "output_tokens_details": {"reasoning_tokens": 454}}

    cost = cost_usd("responses", "muse-spark-1.3", usage)

    # Reasoning tokens are already inside output_tokens.
    assert cost == pytest.approx(42291 * 1.25e-6 + 517 * 4.25e-6)


def test_spark_cached_tokens_never_exceed_input_tokens():
    usage = {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 5000}, "output_tokens": 0}

    assert cost_usd("responses", "muse-spark-1.3", usage) == pytest.approx(100 * 0.15e-6)


def test_sam_cost_from_video_frames():
    assert cost_usd("responses", "sam-3.1", {"video_frames_processed": 160}) == pytest.approx(0.032)


def test_sam_bills_at_least_50_frames_per_video():
    assert cost_usd("responses", "sam-3.1", {"video_frames_processed": 20}) == pytest.approx(0.01)


def test_sam_images_are_charged_per_image_in_the_request():
    usage = {"images_in_request": 3}

    assert cost_usd("responses", "sam-3.1", usage) == pytest.approx(3 * 2.50 / 1000)


def test_asr_cost_from_duration():
    cost = cost_usd("asr/transcribe", "muse-voice-transcribe-1.0", {"audioDurationMs": 155000})

    assert cost == pytest.approx(0.18 * 155 / 3600)


def test_file_calls_are_free():
    assert cost_usd("files", None, {}) == 0.0


@pytest.mark.parametrize(
    ("endpoint", "model"),
    [
        ("responses", "muse-wonder-1.0"),
        ("responses", "muse-spark-1.3-contributor"),
        ("responses", "muse-voice-transcribe-1.0"),
        ("asr/transcribe", "muse-spark-1.3"),
        ("responses", None),
        ("chat/completions", "muse-spark-1.3"),
    ],
)
def test_unpriced_endpoint_or_model_raises(endpoint, model):
    with pytest.raises(ValueError):
        cost_usd(endpoint, model, {"input_tokens": 1})


@pytest.mark.parametrize(
    "usage",
    [
        {"input_tokens": "1000", "output_tokens": 10},
        {"input_tokens": True, "output_tokens": 10},
        {"input_tokens": -5, "output_tokens": 10},
        {"input_tokens": math.nan, "output_tokens": 10},
        {"input_tokens": 10, "input_tokens_details": [1], "output_tokens": 10},
        ["input_tokens", 10],
    ],
    ids=["string", "bool", "negative", "nan", "details-not-object", "not-object"],
)
def test_malformed_usage_raises(usage):
    with pytest.raises(ValueError):
        cost_usd("responses", "muse-spark-1.3", usage)


def test_spark_estimate_is_request_bytes_over_four_at_the_input_price():
    usage = estimate_usage("responses", "muse-spark-1.3", request_bytes=4001)

    assert usage["estimated"] is True
    assert usage["input_tokens"] == 1001
    assert cost_usd("responses", "muse-spark-1.3", usage) == pytest.approx(1001 * 1.25e-6)


def test_sam_estimate_charges_the_minimum_frames_for_a_video_and_each_image():
    with_video = estimate_usage("responses", "sam-3.1", request_bytes=300, has_video=True, images=2)
    images_only = estimate_usage("responses", "sam-3.1", request_bytes=300, images=2)

    assert cost_usd("responses", "sam-3.1", with_video) == pytest.approx(50 * 0.20 / 1000 + 2 * 2.50 / 1000)
    assert cost_usd("responses", "sam-3.1", images_only) == pytest.approx(2 * 2.50 / 1000)


def test_transcribe_estimate_assumes_16_khz_16_bit_mono():
    # 32,000 bytes per second is the lowest byte rate Meta accepts, so this is
    # the longest the audio could be.
    usage = estimate_usage("asr/transcribe", "muse-voice-transcribe-1.0", request_bytes=0, audio_bytes=32000 * 155)

    assert usage["audioDurationMs"] == 155000
    assert cost_usd("asr/transcribe", "muse-voice-transcribe-1.0", usage) == pytest.approx(0.18 * 155 / 3600)


def test_media_estimate_is_at_least_the_floor():
    small = estimate_usage("responses", "muse-spark-1.3", request_bytes=300, media=True)
    large = estimate_usage("responses", "muse-spark-1.3", request_bytes=400_000, media=True)
    sam = estimate_usage("responses", "sam-3.1", request_bytes=300, has_video=True, media=True)

    assert small["floor_usd"] == 0.10
    assert cost_usd("responses", "muse-spark-1.3", small) == pytest.approx(0.10)
    # Above the floor the byte estimate stands.
    assert cost_usd("responses", "muse-spark-1.3", large) == pytest.approx(100_000 * 1.25e-6)
    assert cost_usd("responses", "sam-3.1", sam) == pytest.approx(0.10)


def test_text_estimate_has_no_floor():
    usage = estimate_usage("responses", "muse-spark-1.3", request_bytes=300)

    assert "floor_usd" not in usage
    assert cost_usd("responses", "muse-spark-1.3", usage) == pytest.approx(75 * 1.25e-6)


def test_floor_applies_only_to_estimates():
    assert cost_usd("responses", "muse-spark-1.3", {"input_tokens": 10, "floor_usd": 0.10}) == pytest.approx(10 * 1.25e-6)
