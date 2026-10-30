"""Device registration and bearer token auth.

A device registers with its own UUID and, optionally, an access code. It gets
an opaque token; only the token's SHA-256 is stored. Relay AI access is not
stored on the device: it is read from the code's `active` flag on every
request, so deactivating a code takes effect at once.
"""

import hashlib
import secrets
import sqlite3
from datetime import UTC, datetime
from typing import Annotated, Any
from uuid import UUID

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel, Field

from relay import db
from relay.config import Settings
from relay.deps import AppSettings, Conn
from relay.errors import RelayError

router = APIRouter()


class Registration(BaseModel):
    device_id: UUID
    access_code: str | None = Field(default=None, max_length=128)


def hash_token(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def has_relay_ai(conn: sqlite3.Connection, device: sqlite3.Row) -> bool:
    if device["code"] is None:
        return False
    row = conn.execute("SELECT active FROM codes WHERE code = ?", (device["code"],)).fetchone()
    return bool(row and row["active"])


def device_status(
    conn: sqlite3.Connection, device: sqlite3.Row, settings: Settings
) -> dict[str, Any]:
    relay_ai = has_relay_ai(conn, device)
    return {
        "relay_ai": relay_ai,
        "budget_usd": settings.device_cap_usd if relay_ai else 0.0,
        "spent_usd": float(device["spent_usd"]),
    }


def current_device(request: Request, conn: Conn) -> sqlite3.Row:
    """Resolve `Authorization: Bearer <token>` to the device row, or 401."""
    scheme, _, token = request.headers.get("authorization", "").partition(" ")
    token = token.strip()
    if scheme.lower() == "bearer" and token:
        device = conn.execute(
            "SELECT * FROM devices WHERE token_hash = ?", (hash_token(token),)
        ).fetchone()
        if device is not None:
            return device
    raise RelayError(
        401,
        "unauthorized",
        "This device is not registered. Register it again.",
        headers={"WWW-Authenticate": "Bearer"},
    )


Device = Annotated[sqlite3.Row, Depends(current_device)]


@router.post("/v1/devices")
def register_device(body: Registration, conn: Conn, settings: AppSettings) -> dict[str, Any]:
    """Register or re-register a device. Re-registering rotates the token,
    replaces the stored code with the one sent (none clears it) and keeps
    the spend."""
    device_id = str(body.device_id)
    code = (body.access_code or "").strip() or None
    token = secrets.token_urlsafe(32)
    with db.transaction(conn):
        if code is not None:
            row = conn.execute("SELECT active FROM codes WHERE code = ?", (code,)).fetchone()
            if row is None or not row["active"]:
                raise RelayError(
                    403,
                    "invalid_code",
                    "This access code is not valid. "
                    "Check the code or add your own Meta key in Settings.",
                )
        conn.execute(
            """
            INSERT INTO devices (device_id, token_hash, code, created_at)
            VALUES (?, ?, ?, ?)
            ON CONFLICT (device_id) DO UPDATE
            SET token_hash = excluded.token_hash, code = excluded.code
            """,
            (device_id, hash_token(token), code, datetime.now(UTC).isoformat(timespec="seconds")),
        )
        device = conn.execute(
            "SELECT * FROM devices WHERE device_id = ?", (device_id,)
        ).fetchone()
        status = device_status(conn, device, settings)
    return {"token": token, **status}


@router.get("/v1/devices/me")
def read_device(device: Device, conn: Conn, settings: AppSettings) -> dict[str, Any]:
    return device_status(conn, device, settings)
