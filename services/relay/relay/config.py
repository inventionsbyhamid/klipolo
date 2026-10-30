"""Settings read from the environment.

META_API_KEY is held for the proxy and is never logged or shown in repr.
ALLOWED_MODELS is the comma-separated list of Meta models the proxy forwards;
SAM is off unless listed. Models the relay never forwards (Muse Wonder, the
*-contributor tiers) stay refused even when listed.
"""

import math
import os
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_ALLOWED_MODELS = "muse-spark-1.3,muse-spark-1.2,muse-voice-transcribe-1.0"


@dataclass(frozen=True)
class Settings:
    db_path: Path
    device_cap_usd: float
    global_cap_usd: float
    align_minutes_per_device_per_day: float
    align_concurrency: int
    align_threads: int
    meta_api_key: str = field(default="", repr=False)
    allowed_models: frozenset[str] = frozenset(DEFAULT_ALLOWED_MODELS.split(","))


def _number(env: Mapping[str, str], name: str, default: str, kind: type, minimum: float) -> float:
    raw = env.get(name, default)
    try:
        value = kind(raw)
    except ValueError:
        raise ValueError(f"{name} must be a {kind.__name__}, got {raw!r}") from None
    if not math.isfinite(value):
        raise ValueError(f"{name} must be a finite number, got {raw!r}")
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
        meta_api_key=env.get("META_API_KEY", "").strip(),
        allowed_models=frozenset(
            name.strip() for name in env.get("ALLOWED_MODELS", DEFAULT_ALLOWED_MODELS).split(",") if name.strip()
        ),
    )
