"""Request dependencies shared by the routers."""

import sqlite3
from collections.abc import Iterator

from fastapi import Request

from relay import db
from relay.config import Settings


def get_settings(request: Request) -> Settings:
    return request.app.state.settings


def get_db(request: Request) -> Iterator[sqlite3.Connection]:
    """One connection per request, closed when the request is done."""
    conn = db.connect(get_settings(request).db_path)
    try:
        yield conn
    finally:
        conn.close()
