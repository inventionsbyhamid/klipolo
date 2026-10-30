import sqlite3
import threading

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
    assert columns(conn, "codes") == ["code", "active", "note"]
    assert columns(conn, "ledger") == [
        "id",
        "device_id",
        "at",
        "endpoint",
        "model",
        "usage_json",
        "cost_usd",
    ]


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
