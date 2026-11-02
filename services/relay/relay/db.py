"""SQLite storage for devices, access codes and the cost ledger.

Connections run in autocommit mode with WAL, so the server and the admin CLI
can share the live database. Group writes that must be atomic in
`transaction(conn)`, which takes the write lock up front (BEGIN IMMEDIATE).
"""

import sqlite3
import time
from collections.abc import Iterator
from contextlib import contextmanager
from os import PathLike
from pathlib import Path

SCHEMA_VERSION = 3
_BUSY_TIMEOUT_S = 10.0

_SCHEMA = (
    """
    CREATE TABLE codes (
        code TEXT PRIMARY KEY,
        active INTEGER NOT NULL DEFAULT 1,
        note TEXT NOT NULL DEFAULT ''
    )
    """,
    """
    CREATE TABLE devices (
        device_id TEXT PRIMARY KEY,
        token_hash TEXT NOT NULL UNIQUE,
        code TEXT REFERENCES codes (code),
        created_at TEXT NOT NULL,
        spent_usd REAL NOT NULL DEFAULT 0,
        align_seconds_day REAL NOT NULL DEFAULT 0,
        align_day TEXT
    )
    """,
    """
    CREATE TABLE ledger (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        device_id TEXT NOT NULL REFERENCES devices (device_id),
        at TEXT NOT NULL,
        endpoint TEXT NOT NULL,
        model TEXT,
        usage_json TEXT NOT NULL,
        cost_usd REAL NOT NULL
    )
    """,
    "CREATE INDEX ledger_device_id ON ledger (device_id)",
)


def connect(path: str | PathLike[str]) -> sqlite3.Connection:
    """Open the database, creating the file and schema when missing."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(
        path, timeout=_BUSY_TIMEOUT_S, isolation_level=None, check_same_thread=False
    )
    try:
        conn.row_factory = sqlite3.Row
        _enable_wal(conn)
        conn.execute("PRAGMA synchronous = NORMAL")
        conn.execute("PRAGMA foreign_keys = ON")
        _migrate(conn)
    except BaseException:
        conn.close()
        raise
    return conn


@contextmanager
def transaction(conn: sqlite3.Connection) -> Iterator[sqlite3.Connection]:
    """Run a block as one write transaction; roll back if it or COMMIT raises."""
    conn.execute("BEGIN IMMEDIATE")
    try:
        yield conn
        conn.execute("COMMIT")
    except BaseException:
        # Some errors (OR ROLLBACK, a full disk, I/O errors) make SQLite roll
        # back on its own; a second ROLLBACK would raise and hide the error.
        if conn.in_transaction:
            conn.execute("ROLLBACK")
        raise


def _enable_wal(conn: sqlite3.Connection) -> None:
    # Switching a new file to WAL can fail with "locked" right away, without
    # waiting on the busy timeout, while another connection opens the same
    # file. The mode is stored in the file, so retry until one switch lands.
    deadline = time.monotonic() + _BUSY_TIMEOUT_S
    while True:
        try:
            if conn.execute("PRAGMA journal_mode = WAL").fetchone()[0] == "wal":
                return
        except sqlite3.OperationalError as exc:
            if exc.sqlite_errorcode & 0xFF not in (sqlite3.SQLITE_BUSY, sqlite3.SQLITE_LOCKED):
                raise
        if time.monotonic() >= deadline:
            raise sqlite3.OperationalError("could not switch the database to WAL mode")
        time.sleep(0.01)


def _user_version(conn: sqlite3.Connection) -> int:
    return conn.execute("PRAGMA user_version").fetchone()[0]


def _migrate(conn: sqlite3.Connection) -> None:
    if _user_version(conn) >= SCHEMA_VERSION:
        return
    with transaction(conn):
        if _user_version(conn) < 1:
            for statement in _SCHEMA:
                conn.execute(statement)
        if _user_version(conn) < 2:
            # SAM turned off for a code by the relay's breaker, until the owner turns it back on.
            conn.execute("ALTER TABLE codes ADD COLUMN sam_off INTEGER NOT NULL DEFAULT 0")
        if _user_version(conn) < 3:
            _add_code_limits(conn)
        conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")


def _add_code_limits(conn: sqlite3.Connection) -> None:
    """Version 3: limits per access code, none until the owner sets them (`admin set-code-cap`). Only adds columns
    and a table, so a relay from before still runs on the database."""
    # The most the code's devices may spend together, and how many devices may register with it. NULL: no limit.
    conn.execute("ALTER TABLE codes ADD COLUMN cap_usd REAL")
    conn.execute("ALTER TABLE codes ADD COLUMN max_devices INTEGER")
    # The code a charge was made under, so a device that leaves its code takes none of the code's spend along.
    # Charges from before belong to the code their device has now.
    conn.execute("ALTER TABLE ledger ADD COLUMN code TEXT")
    conn.execute("UPDATE ledger SET code = (SELECT code FROM devices WHERE devices.device_id = ledger.device_id)")
    conn.execute("CREATE INDEX ledger_code ON ledger (code)")
    # Every device that ever registered with a code, which its device limit counts: leaving the code frees no room.
    conn.execute(
        """
        CREATE TABLE code_devices (
            code TEXT NOT NULL REFERENCES codes (code),
            device_id TEXT NOT NULL REFERENCES devices (device_id),
            PRIMARY KEY (code, device_id)
        )
        """
    )
    conn.execute("INSERT INTO code_devices (code, device_id) SELECT code, device_id FROM devices WHERE code IS NOT NULL")
