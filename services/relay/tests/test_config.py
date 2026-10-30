from pathlib import Path

import pytest

from relay.config import load_settings


def test_defaults_match_the_documented_caps():
    settings = load_settings({})

    assert settings.db_path == Path("data/relay.db")
    assert settings.device_cap_usd == 2.00
    assert settings.global_cap_usd == 40.00
    assert settings.align_minutes_per_device_per_day == 120
    assert settings.align_concurrency == 2
    assert settings.align_threads == 8
    assert settings.meta_api_key == ""
    assert settings.allowed_models == {"muse-spark-1.3", "muse-spark-1.2", "muse-voice-transcribe-1.0"}


def test_environment_overrides_defaults():
    settings = load_settings(
        {
            "RELAY_DB_PATH": "/srv/relay/relay.db",
            "DEVICE_CAP_USD": "1.5",
            "GLOBAL_CAP_USD": "10",
            "ALIGN_MINUTES_PER_DEVICE_PER_DAY": "30",
            "ALIGN_CONCURRENCY": "1",
            "ALIGN_THREADS": "4",
            "META_API_KEY": "fake-key",
        }
    )

    assert settings.db_path == Path("/srv/relay/relay.db")
    assert settings.device_cap_usd == 1.5
    assert settings.global_cap_usd == 10.0
    assert settings.align_minutes_per_device_per_day == 30
    assert settings.align_concurrency == 1
    assert settings.align_threads == 4
    assert settings.meta_api_key == "fake-key"


def test_meta_key_is_left_out_of_repr():
    settings = load_settings({"META_API_KEY": "fake-secret-value"})

    assert "fake-secret-value" not in repr(settings)


@pytest.mark.parametrize(
    ("name", "value"),
    [
        ("DEVICE_CAP_USD", "two"),
        ("GLOBAL_CAP_USD", "-1"),
        ("ALIGN_MINUTES_PER_DEVICE_PER_DAY", "-5"),
        ("ALIGN_CONCURRENCY", "0"),
        ("ALIGN_THREADS", "1.5"),
    ],
)
def test_bad_values_fail_with_the_variable_name(name, value):
    with pytest.raises(ValueError, match=name):
        load_settings({name: value})


@pytest.mark.parametrize(
    ("name", "value"),
    [
        ("DEVICE_CAP_USD", "nan"),
        ("DEVICE_CAP_USD", "inf"),
        ("GLOBAL_CAP_USD", "NaN"),
        ("GLOBAL_CAP_USD", "Infinity"),
        ("ALIGN_MINUTES_PER_DEVICE_PER_DAY", "inf"),
    ],
)
def test_non_finite_values_fail_with_the_variable_name(name, value):
    with pytest.raises(ValueError, match=name):
        load_settings({name: value})


def test_allowed_models_are_read_from_a_comma_separated_list():
    settings = load_settings({"ALLOWED_MODELS": " sam-3.1, muse-spark-1.3 ,,"})

    assert settings.allowed_models == {"sam-3.1", "muse-spark-1.3"}


@pytest.mark.parametrize(("raw", "key"), [("  fake-key\n", "fake-key"), (" \n", "")])
def test_meta_key_is_stripped(raw, key):
    # A stray newline from an env file would break the Authorization header;
    # a blank key means the relay is not configured.
    assert load_settings({"META_API_KEY": raw}).meta_api_key == key
