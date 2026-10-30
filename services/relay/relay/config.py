"""Settings read from the environment.

META_API_KEY is held for the proxy and is never logged or shown in repr.
"""

import os
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(frozen=True)
class Settings:
    db_path: Path
    device_cap_usd: float
    global_cap_usd: float
    align_minutes_per_device_per_day: float
    align_concurrency: int
    align_threads: int
    meta_api_key: str = field(default="", repr=False)


def _number(env: Mapping[str, str], name: str, default: str, kind: type, minimum: float) -> float:
    raw = env.get(name, default)
    try:
        value = kind(raw)
    except ValueError:
        raise ValueError(f"{name} must be a {kind.__name__}, got {raw!r}") from None
    if value < minimum:
        raise ValueError(f"{name} must be at least {minimum}, got {raw!r}")
    return value


def load_settings(env: Mapping[str, str] | None = None) -> Settings:
    env = os.environ if env is None else env
    return Settings(
        db_path=Path(env.get("RELAY_DB_PATH", "./data/relay.db")),
        device_cap_usd=_number(env, "DEVICE_CAP_USD", "2.00", float, 0),
        global_cap_usd=_number(env, "GLOBAL_CAP_USD", "40.00", float, 0),
        align_minutes_per_device_per_day=_number(
            env, "ALIGN_MINUTES_PER_DEVICE_PER_DAY", "120", float, 0
        ),
        align_concurrency=_number(env, "ALIGN_CONCURRENCY", "2", int, 1),
        align_threads=_number(env, "ALIGN_THREADS", "8", int, 1),
        meta_api_key=env.get("META_API_KEY", ""),
    )
