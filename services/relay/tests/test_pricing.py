import math

import pytest

from relay.pricing import cost_usd, reservation_usage


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


def test_muse_image_costs_a_cent_per_image_returned():
    assert cost_usd("images/generations", "muse-image-1.0", {"images": 1}) == pytest.approx(0.01)
    assert cost_usd("images/generations", "muse-image-1.0", {"images": 2}) == pytest.approx(0.02)
    assert cost_usd("images/generations", "muse-image-1.0", {"images": 0}) == 0.0


def test_muse_image_reserves_two_images_whatever_it_asks_for():
    usage = reservation_usage("images/generations", "muse-image-1.0", request_bytes=300, images=1)

    assert usage == {"images": 2, "reserved": True}
    assert cost_usd("images/generations", "muse-image-1.0", usage) == pytest.approx(0.02)


def test_file_calls_are_free():
    assert cost_usd("files", None, {}) == 0.0


@pytest.mark.parametrize(
    ("endpoint", "model"),
    [
        ("responses", "muse-wonder-1.0"),
        ("responses", "muse-spark-1.3-contributor"),
        ("responses", "muse-voice-transcribe-1.0"),
        ("asr/transcribe", "muse-spark-1.3"),
        ("images/generations", "muse-spark-1.3"),
        ("responses", "muse-image-1.0"),
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


def test_spark_reservation_is_one_token_per_request_byte_plus_the_whole_output_limit():
    # Adversarial text can come close to one token per byte.
    usage = reservation_usage("responses", "muse-spark-1.3", request_bytes=4001, max_output_tokens=8192)

    assert usage == {"input_tokens": 4001, "output_tokens": 8192, "reserved": True}
    assert cost_usd("responses", "muse-spark-1.3", usage) == pytest.approx(4001 * 1.25e-6 + 8192 * 4.25e-6)


def test_spark_reservation_adds_75_cents_for_media():
    usage = reservation_usage("responses", "muse-spark-1.3", request_bytes=400, max_output_tokens=100, media=True)

    assert usage["media_usd"] == 0.75
    assert cost_usd("responses", "muse-spark-1.3", usage) == pytest.approx(400 * 1.25e-6 + 100 * 4.25e-6 + 0.75)


def test_sam_reservation_is_the_videos_counted_frames_and_each_image():
    # A video's frames are counted from its upload, so they bound the cost: no media amount on top.
    with_video = reservation_usage("responses", "sam-3.1", request_bytes=300, has_video=True, video_frames=15_000, images=2)
    short = reservation_usage("responses", "sam-3.1", request_bytes=300, has_video=True, video_frames=20)
    images_only = reservation_usage("responses", "sam-3.1", request_bytes=300, images=2)

    assert cost_usd("responses", "sam-3.1", with_video) == pytest.approx(15_000 * 0.20 / 1000 + 2 * 2.50 / 1000)
    assert cost_usd("responses", "sam-3.1", short) == pytest.approx(50 * 0.20 / 1000)
    assert cost_usd("responses", "sam-3.1", images_only) == pytest.approx(0.75 + 2 * 2.50 / 1000)


def test_transcribe_reservation_is_the_audio_length():
    usage = reservation_usage("asr/transcribe", "muse-voice-transcribe-1.0", audio_seconds=155)

    assert usage == {"audioDurationMs": 155000, "reserved": True}
    assert cost_usd("asr/transcribe", "muse-voice-transcribe-1.0", usage) == pytest.approx(0.18 * 155 / 3600)


def test_media_amount_counts_only_in_a_reservation():
    usage = {"input_tokens": 10, "output_tokens": 0, "media_usd": 0.75}

    assert cost_usd("responses", "muse-spark-1.3", usage) == pytest.approx(10 * 1.25e-6)


@pytest.mark.parametrize(
    ("endpoint", "model", "usage"),
    [
        ("responses", "muse-spark-1.3", {"input_tokens": 10000}),
        ("responses", "muse-spark-1.3", {"output_tokens": 500}),
        ("responses", "muse-spark-1.3", {"total_tokens": 10500}),
        ("responses", "muse-spark-1.2", {}),
        ("asr/transcribe", "muse-voice-transcribe-1.0", {}),
        ("images/generations", "muse-image-1.0", {"input_tokens": 40}),
    ],
    ids=["no-output", "no-input", "total-only", "empty", "no-duration", "no-image-count"],
)
def test_usage_without_its_counters_raises(endpoint, model, usage):
    # Missing counters would otherwise price as zero.
    with pytest.raises(ValueError):
        cost_usd(endpoint, model, usage)
