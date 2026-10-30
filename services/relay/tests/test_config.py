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
