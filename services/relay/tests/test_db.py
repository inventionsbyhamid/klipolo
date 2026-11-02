import sqlite3
import threading
import time

import pytest

from relay import db


@pytest.fixture
def conn(tmp_path):
    connection = db.connect(tmp_path / "relay.db")
    yield connection
    connection.close()


def columns(conn, table):
    return [row["name"] for row in conn.execute(f"PRAGMA table_info({table})")]


def test_connect_creates_the_planned_tables(conn):
    assert columns(conn, "devices") == [
        "device_id",
        "token_hash",
        "code",
        "created_at",
        "spent_usd",
        "align_seconds_day",
        "align_day",
    ]
    assert columns(conn, "codes") == ["code", "active", "note", "sam_off", "cap_usd", "max_devices"]
    assert columns(conn, "ledger") == [
        "id",
        "device_id",
        "at",
        "endpoint",
        "model",
        "usage_json",
        "cost_usd",
        "code",
    ]
    assert columns(conn, "code_devices") == ["code", "device_id"]
    assert conn.execute("PRAGMA user_version").fetchone()[0] == 3


def version_2_database(path):
    """A database as the relay made it before per-code limits: schema version 2, with a code, two devices on it
    (one has spent), a device with no code and their charges."""
    conn = sqlite3.connect(path, isolation_level=None)
    for statement in db._SCHEMA:
        conn.execute(statement)
    conn.execute("ALTER TABLE codes ADD COLUMN sam_off INTEGER NOT NULL DEFAULT 0")
    conn.execute("PRAGMA user_version = 2")
    conn.execute("INSERT INTO codes (code, note) VALUES ('judges', 'for the judges')")
    for device_id, code, spent in [("d1", "judges", 0.75), ("d2", "judges", 0.0), ("d3", None, 0.25)]:
        conn.execute(
            "INSERT INTO devices (device_id, token_hash, code, created_at, spent_usd) VALUES (?, ?, ?, 'now', ?)",
            (device_id, f"h{device_id}", code, spent),
        )
    for device_id, cost in [("d1", 0.5), ("d1", 0.25), ("d3", 0.25)]:
        conn.execute(
            "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd)"
            " VALUES (?, 'now', 'responses', 'muse-spark-1.3', '{}', ?)",
            (device_id, cost),
        )
    conn.close()


def test_a_version_2_database_moves_to_version_3_keeping_everything(tmp_path):
    path = tmp_path / "relay.db"
    version_2_database(path)

    conn = db.connect(path)

    assert conn.execute("PRAGMA user_version").fetchone()[0] == 3
    # No limits on any code until the owner sets them.
    [code] = conn.execute("SELECT * FROM codes").fetchall()
    assert dict(code) == {
        "code": "judges",
        "active": 1,
        "note": "for the judges",
        "sam_off": 0,
        "cap_usd": None,
        "max_devices": None,
    }
    # Each charge so far belongs to its device's code, and each device on a code counts as registered with it.
    assert [tuple(row) for row in conn.execute("SELECT device_id, code, cost_usd FROM ledger ORDER BY id")] == [
        ("d1", "judges", 0.5),
        ("d1", "judges", 0.25),
        ("d3", None, 0.25),
    ]
    assert [tuple(row) for row in conn.execute("SELECT code, device_id FROM code_devices ORDER BY device_id")] == [
        ("judges", "d1"),
        ("judges", "d2"),
    ]
    assert [row["spent_usd"] for row in conn.execute("SELECT spent_usd FROM devices ORDER BY device_id")] == [
        0.75,
        0.0,
        0.25,
    ]
    conn.close()
    # Opening it again changes nothing.
    db.connect(path).close()
    again = sqlite3.connect(path)
    assert again.execute("SELECT count(*) FROM code_devices").fetchone()[0] == 2
    again.close()


def test_a_version_2_database_opened_by_many_at_once_moves_once(tmp_path):
    # The server's requests and the admin CLI may open the live database at the same moment.
    failures = []

    def open_and_query(path):
        try:
            conn = db.connect(path)
            conn.execute("SELECT count(*) FROM code_devices").fetchone()
            conn.close()
        except sqlite3.Error as exc:
            failures.append(repr(exc))

    for trial in range(10):
        path = tmp_path / f"trial-{trial}.db"
        version_2_database(path)
        threads = [threading.Thread(target=open_and_query, args=(path,)) for _ in range(8)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        conn = sqlite3.connect(path)
        assert conn.execute("SELECT count(*) FROM code_devices").fetchone()[0] == 2
        conn.close()

    assert failures == []


def test_rows_an_earlier_relay_writes_on_version_3_get_their_code_on_the_next_open(tmp_path):
    # An earlier relay (a rollback, or the old container still running during a deploy) writes charges without their
    # code and registrations without the table of devices on each code.
    path = tmp_path / "relay.db"
    db.connect(path).close()
    old = sqlite3.connect(path, isolation_level=None)
    old.execute("INSERT INTO codes (code) VALUES ('judges')")
    for device_id, code in [("d1", "judges"), ("d2", None)]:
        old.execute(
            "INSERT INTO devices (device_id, token_hash, code, created_at) VALUES (?, ?, ?, 'now')",
            (device_id, f"h{device_id}", code),
        )
        old.execute(
            "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd)"
            " VALUES (?, 'now', 'responses', 'muse-spark-1.3', '{}', 0.25)",
            (device_id,),
        )
    old.close()

    conn = db.connect(path)

    assert [tuple(row) for row in conn.execute("SELECT device_id, code FROM ledger ORDER BY id")] == [
        ("d1", "judges"),
        ("d2", None),
    ]
    assert [tuple(row) for row in conn.execute("SELECT code, device_id FROM code_devices")] == [("judges", "d1")]
    conn.close()


def test_opening_a_database_with_nothing_to_fill_in_writes_nothing(tmp_path, monkeypatch):
    # Every request opens the database: with nothing to fill in, opening it must not wait for the write lock.
    path = tmp_path / "relay.db"
    conn = db.connect(path)
    conn.execute("INSERT INTO codes (code) VALUES ('judges')")
    conn.execute("INSERT INTO devices (device_id, token_hash, code, created_at) VALUES ('d1', 'h1', NULL, 'now')")
    conn.execute(
        "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd, code)"
        " VALUES ('d1', 'now', 'responses', 'muse-spark-1.3', '{}', 0.25, NULL)"
    )
    monkeypatch.setattr(db, "_BUSY_TIMEOUT_S", 0.2)
    conn.execute("BEGIN IMMEDIATE")
    try:
        db.connect(path).close()
    finally:
        conn.execute("ROLLBACK")
        conn.close()


def test_an_open_that_cant_get_the_write_lock_soon_leaves_the_filling_in_to_the_next(tmp_path, monkeypatch):
    # A device an earlier relay registered, while another connection holds the write lock: the open goes on.
    path = tmp_path / "relay.db"
    conn = db.connect(path)
    conn.execute("INSERT INTO codes (code) VALUES ('judges')")
    conn.execute("INSERT INTO devices (device_id, token_hash, code, created_at) VALUES ('d1', 'h1', 'judges', 'now')")
    monkeypatch.setattr(db, "_BUSY_TIMEOUT_S", 5.0)
    conn.execute("BEGIN IMMEDIATE")
    try:
        started = time.monotonic()
        opened = db.connect(path)
        elapsed = time.monotonic() - started
        assert opened.execute("PRAGMA busy_timeout").fetchone()[0] == 5000
        opened.close()
    finally:
        conn.execute("ROLLBACK")

    assert elapsed < 1.0
    assert conn.execute("SELECT count(*) FROM code_devices").fetchone()[0] == 0
    db.connect(path).close()
    assert conn.execute("SELECT count(*) FROM code_devices").fetchone()[0] == 1
    conn.close()


def test_the_checks_on_every_open_use_the_ledgers_code_index(conn):
    plan = " ".join(row["detail"] for row in conn.execute(f"EXPLAIN QUERY PLAN {db.LEDGER_TO_FILL}"))

    assert "INDEX ledger_code" in plan, plan


def test_connect_uses_wal_mode(conn):
    assert conn.execute("PRAGMA journal_mode").fetchone()[0] == "wal"


def test_connect_creates_the_parent_directory(tmp_path):
    path = tmp_path / "nested" / "data" / "relay.db"

    db.connect(path).close()

    assert path.exists()


def test_connect_closes_the_connection_when_setup_fails(tmp_path, monkeypatch):
    path = tmp_path / "relay.db"
    path.write_bytes(b"this is not a database" * 100)
    opened = []
    real_connect = sqlite3.connect

    def spy(*args, **kwargs):
        connection = real_connect(*args, **kwargs)
        opened.append(connection)
        return connection

    monkeypatch.setattr(sqlite3, "connect", spy)

    with pytest.raises(sqlite3.DatabaseError, match="not a database"):
        db.connect(path)

    assert len(opened) == 1
    with pytest.raises(sqlite3.ProgrammingError, match="closed database"):
        opened[0].execute("SELECT 1")


def test_reconnecting_keeps_existing_rows(tmp_path):
    path = tmp_path / "relay.db"
    first = db.connect(path)
    first.execute("INSERT INTO codes (code, note) VALUES ('testcode', 'for tests')")
    first.close()

    second = db.connect(path)
    row = second.execute("SELECT code, active, note FROM codes").fetchone()
    second.close()

    assert (row["code"], row["active"], row["note"]) == ("testcode", 1, "for tests")


def test_new_device_starts_with_no_spend(conn):
    conn.execute(
        "INSERT INTO devices (device_id, token_hash, created_at) VALUES ('d1', 'h1', 'now')"
    )

    row = conn.execute("SELECT spent_usd, align_seconds_day, code FROM devices").fetchone()

    assert (row["spent_usd"], row["align_seconds_day"], row["code"]) == (0, 0, None)


def test_ledger_rows_need_a_known_device(conn):
    with pytest.raises(sqlite3.IntegrityError):
        conn.execute(
            "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd)"
            " VALUES ('ghost', 'now', 'responses', 'muse-spark-1.3', '{}', 0.01)"
        )


def test_transaction_commits_on_success(conn):
    with db.transaction(conn):
        conn.execute("INSERT INTO codes (code) VALUES ('kept')")

    assert conn.execute("SELECT count(*) FROM codes").fetchone()[0] == 1


def test_transaction_rolls_back_on_error(conn):
    with pytest.raises(RuntimeError):
        with db.transaction(conn):
            conn.execute("INSERT INTO codes (code) VALUES ('dropped')")
            raise RuntimeError("boom")

    assert conn.execute("SELECT count(*) FROM codes").fetchone()[0] == 0


def test_transaction_keeps_the_error_when_sqlite_already_rolled_back(conn):
    # OR ROLLBACK makes SQLite end the transaction itself before raising.
    with pytest.raises(sqlite3.IntegrityError):
        with db.transaction(conn):
            conn.execute("INSERT INTO codes (code) VALUES ('dup')")
            conn.execute("INSERT OR ROLLBACK INTO codes (code) VALUES ('dup')")

    assert not conn.in_transaction
    assert conn.execute("SELECT count(*) FROM codes").fetchone()[0] == 0


def test_transaction_rolls_back_when_commit_fails(conn):
    # A deferred foreign key fails at COMMIT, which leaves the transaction open.
    with pytest.raises(sqlite3.IntegrityError):
        with db.transaction(conn):
            conn.execute("PRAGMA defer_foreign_keys = ON")
            conn.execute(
                "INSERT INTO ledger (device_id, at, endpoint, usage_json, cost_usd)"
                " VALUES ('ghost', 'now', 'responses', '{}', 0.01)"
            )

    assert not conn.in_transaction
    assert conn.execute("SELECT count(*) FROM ledger").fetchone()[0] == 0


def test_concurrent_first_connects_all_succeed(tmp_path):
    failures = []

    def open_and_query(path):
        try:
            conn = db.connect(path)
            conn.execute("SELECT count(*) FROM devices").fetchone()
            conn.close()
        except sqlite3.Error as exc:
            failures.append(repr(exc))

    for trial in range(30):
        path = tmp_path / f"trial-{trial}" / "relay.db"
        threads = [threading.Thread(target=open_and_query, args=(path,)) for _ in range(8)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()

    assert failures == []
