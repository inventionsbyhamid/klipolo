"""Settings read from the environment.

META_API_KEY is held for the proxy and is never logged or shown in repr.
ALLOWED_MODELS is the comma-separated list of Meta models the proxy forwards
(Spark 1.3 and 1.2, Voice Transcribe, Muse Image and SAM 3.1 by default).
Models the relay never forwards (Muse Wonder, the *-contributor tiers) stay
refused even when listed.
"""

import math
import os
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_ALLOWED_MODELS = "muse-spark-1.3,muse-spark-1.2,muse-voice-transcribe-1.0,muse-image-1.0,sam-3.1"


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
    # SAM's own cap on what it may cost in a day (UTC), across every device.
    sam_daily_cap_usd: float = 5.0
    # Where SAM's uploads and copies are made: on the data volume, beside the database, never word timing's /tmp.
    sam_work_dir: Path = Path("./data/sam-work")
    # Load the alignment model at startup instead of on the first request.
    align_warmup: bool = False


_FLAGS = {"1": True, "true": True, "yes": True, "on": True, "0": False, "false": False, "no": False, "off": False, "": False}


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


def _flag(env: Mapping[str, str], name: str, default: str) -> bool:
    raw = env.get(name, default)
    try:
        return _FLAGS[raw.strip().lower()]
    except KeyError:
        raise ValueError(f"{name} must be 1 or 0, got {raw!r}") from None


def load_settings(env: Mapping[str, str] | None = None) -> Settings:
    env = os.environ if env is None else env
    return Settings(
        db_path=Path(env.get("RELAY_DB_PATH", "./data/relay.db")),
        device_cap_usd=_number(env, "DEVICE_CAP_USD", "2.00", float, 0),
        global_cap_usd=_number(env, "GLOBAL_CAP_USD", "40.00", float, 0),
        sam_daily_cap_usd=_number(env, "SAM_DAILY_CAP_USD", "5.00", float, 0),
        sam_work_dir=Path(env.get("SAM_WORK_DIR", "") or Path(env.get("RELAY_DB_PATH", "./data/relay.db")).parent / "sam-work"),
        align_minutes_per_device_per_day=_number(
            env, "ALIGN_MINUTES_PER_DEVICE_PER_DAY", "120", float, 0
        ),
        align_concurrency=_number(env, "ALIGN_CONCURRENCY", "2", int, 1),
        # 2 alignments x 3 threads leaves CPU for the proxy's event loop.
        align_threads=_number(env, "ALIGN_THREADS", "3", int, 1),
        meta_api_key=env.get("META_API_KEY", "").strip(),
        allowed_models=frozenset(
            name.strip() for name in env.get("ALLOWED_MODELS", DEFAULT_ALLOWED_MODELS).split(",") if name.strip()
        ),
        align_warmup=_flag(env, "ALIGN_WARMUP", "0"),
    )
