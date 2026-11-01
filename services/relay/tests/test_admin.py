import os
import subprocess
import sys
import uuid

import pytest

from relay import admin, db

INVALID_CODE = {
    "error": "invalid_code",
    "message": "This access code is not valid. Check the code or add your own Meta key in Settings.",
}


@pytest.fixture(autouse=True)
def clean_env(monkeypatch):
    # The CLI reads its config from the environment; keep the shell's out.
    for name in ("RELAY_DB_PATH", "DEVICE_CAP_USD", "GLOBAL_CAP_USD"):
        monkeypatch.delenv(name, raising=False)


@pytest.fixture
def db_path(settings):
    # The server creates the database at boot; the CLI only opens it.
    db.connect(settings.db_path).close()
    return settings.db_path


@pytest.fixture
def cli(db_path, capsys):
    def run(*args):
        status = admin.main(["--db", str(db_path), *args])
        out, err = capsys.readouterr()
        return status, out, err

    return run


@pytest.fixture
def conn(db_path):
    connection = db.connect(db_path)
    yield connection
    connection.close()


def register(client, access_code=None, device_id=None):
    body = {"device_id": device_id or str(uuid.uuid4())}
    if access_code is not None:
        body["access_code"] = access_code
    return client.post("/v1/devices", json=body)


def me(client, token):
    return client.get("/v1/devices/me", headers={"Authorization": f"Bearer {token}"}).json()


def add_device(conn, code=None):
    device_id = str(uuid.uuid4())
    conn.execute(
        "INSERT INTO devices (device_id, token_hash, code, created_at) VALUES (?, ?, ?, 'now')",
        (device_id, device_id, code),
    )
    return device_id


def charge(conn, device_id, cost, at, endpoint="responses", model="muse-spark-1.3"):
    conn.execute(
        "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd)"
        " VALUES (?, ?, ?, ?, '{}', ?)",
        (device_id, at, endpoint, model, cost),
    )


def sections(out):
    """Blocks of output separated by blank lines, each as a list of lines."""
    return [block.splitlines() for block in out.strip().split("\n\n")]


def table(out):
    """Rows of a printed table, each split into words, header first."""
    return [line.split() for line in out.splitlines() if line.strip()]


def test_add_then_list_shows_active_code(cli):
    status, out, err = cli("add-code", "testcode", "--note", "for tests")

    assert (status, err) == (0, "")
    assert out == 'Added code "testcode".\n'

    status, out, _ = cli("list-codes")

    assert status == 0
    assert table(out) == [
        ["CODE", "STATUS", "DEVICES", "NOTE"],
        ["testcode", "active", "0", "for", "tests"],
    ]


def test_deactivated_code_rejects_new_registrations_and_stops_relay_for_its_devices(
    client, cli
):
    cli("add-code", "testcode")
    registered = register(client, "testcode").json()
    assert registered["relay_ai"] is True

    status, out, err = cli("deactivate-code", "testcode")

    assert (status, err) == (0, "")
    assert out == (
        'Deactivated code "testcode". New registrations with it get 403. '
        "Relay AI is off for its 1 device from the next request.\n"
    )
    assert me(client, registered["token"]) == {
        "relay_ai": False,
        "budget_usd": 0.0,
        "spent_usd": 0.0,
    }
    refused = register(client, "testcode")
    assert refused.status_code == 403
    assert refused.json() == INVALID_CODE
    assert table(cli("list-codes")[1])[1][:3] == ["testcode", "inactive", "1"]


def test_add_code_on_an_existing_code_reactivates_it_and_updates_the_note(client, cli):
    cli("add-code", "testcode", "--note", "old note")
    token = register(client, "testcode").json()["token"]
    cli("deactivate-code", "testcode")

    status, out, _ = cli("add-code", "testcode", "--note", "new note")

    assert status == 0
    assert out == 'Code "testcode" already exists. Reactivated it. Updated its note.\n'
    assert me(client, token)["relay_ai"] is True
    assert register(client, "testcode").status_code == 200
    assert table(cli("list-codes")[1])[1] == ["testcode", "active", "2", "new", "note"]


def test_add_code_again_without_a_note_keeps_the_note(cli):
    cli("add-code", "testcode", "--note", "for tests")

    status, out, _ = cli("add-code", "testcode")

    assert status == 0
    assert out == 'Code "testcode" already exists. It was already active.\n'
    assert table(cli("list-codes")[1])[1] == ["testcode", "active", "0", "for", "tests"]


def test_deactivate_unknown_code_fails_and_changes_nothing(cli):
    cli("add-code", "testcode")

    status, out, err = cli("deactivate-code", "nope")

    assert status == 1
    assert out == ""
    assert err == 'No code "nope". Codes are case-sensitive. Run list-codes to see them.\n'
    assert table(cli("list-codes")[1])[1][:2] == ["testcode", "active"]


def test_deactivate_inactive_code_says_so(cli):
    cli("add-code", "testcode")
    first = cli("deactivate-code", "testcode")[1]

    status, out, _ = cli("deactivate-code", "testcode")

    assert first.endswith("get 403. No devices use it.\n")

    assert status == 0
    assert out == 'Code "testcode" was already inactive.\n'


def test_codes_are_case_sensitive(client, cli):
    cli("add-code", "TestCode")

    assert cli("deactivate-code", "testcode")[0] == 1
    assert register(client, "testcode").status_code == 403
    assert register(client, "TestCode").status_code == 200


def test_add_code_trims_the_code_like_registration_does(client, cli):
    status, out, _ = cli("add-code", "  testcode  ")

    assert (status, out) == (0, 'Added code "testcode".\n')
    assert register(client, "testcode").status_code == 200


@pytest.mark.parametrize("code", ["", "   ", "x" * 129], ids=["empty", "blank", "too-long"])
def test_add_code_rejects_a_code_no_device_could_send(cli, code):
    with pytest.raises(SystemExit) as exit_info:
        cli("add-code", code)

    assert exit_info.value.code == 2
    assert cli("list-codes")[1] == "No codes yet. Add one with add-code.\n"


def test_list_codes_when_there_are_none(cli):
    assert cli("list-codes") == (0, "No codes yet. Add one with add-code.\n", "")


@pytest.mark.parametrize("before", [True, False], ids=["before-command", "after-command"])
def test_db_option_works_before_or_after_the_command(db_path, capsys, before):
    db_args = ["--db", str(db_path)]
    argv = [*db_args, "add-code", "testcode"] if before else ["add-code", "testcode", *db_args]

    assert admin.main(argv) == 0
    assert 'Added code "testcode"' in capsys.readouterr().out


def test_db_path_comes_from_relay_db_path(db_path, conn, monkeypatch, capsys):
    monkeypatch.setenv("RELAY_DB_PATH", str(db_path))

    assert admin.main(["add-code", "testcode"]) == 0
    assert conn.execute("SELECT code FROM codes").fetchone()["code"] == "testcode"


def test_missing_database_is_an_error_and_is_not_created(tmp_path, capsys):
    path = tmp_path / "data" / "relay.db"

    status = admin.main(["--db", str(path), "list-codes"])

    assert status == 1
    assert capsys.readouterr().err == (
        f"No database at {path}. Check RELAY_DB_PATH or --db, "
        "or start the server once to create it.\n"
    )
    assert not path.exists()


def test_runs_as_a_module(db_path, conn):
    env = {**os.environ, "RELAY_DB_PATH": str(db_path)}

    result = subprocess.run(
        [sys.executable, "-m", "relay.admin", "add-code", "testcode"],
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert (result.returncode, result.stdout, result.stderr) == (0, 'Added code "testcode".\n', "")
    assert conn.execute("SELECT active FROM codes WHERE code = 'testcode'").fetchone()[0] == 1


def test_spend_sums_ledger(cli, conn):
    cli("add-code", "testcode")
    big = add_device(conn, "testcode")
    small = add_device(conn)
    charge(conn, big, 0.25, "2026-10-30T12:00:01+00:00")
    charge(conn, small, 0.125, "2026-10-30T12:00:02+00:00", endpoint="align", model=None)
    charge(conn, big, 0.5, "2026-10-30T12:00:03+00:00")

    status, out, err = cli("spend")

    assert (status, err) == (0, "")
    total, per_device, latest = sections(out)
    assert total == [
        "Total: $0.8750 across 3 requests. Caps: $2.00 per device, $40.00 overall."
    ]
    assert per_device[0] == "Per device, highest spend first:"
    assert table("\n".join(per_device[1:])) == [
        ["DEVICE", "CODE", "REQUESTS", "SPENT"],
        [big[:8], "testcode", "2", "$0.7500"],
        [small[:8], "-", "1", "$0.1250"],
    ]
    assert latest[0] == "Latest requests, newest first (up to 20):"
    assert table("\n".join(latest[1:])) == [
        ["AT", "DEVICE", "ENDPOINT", "MODEL", "COST"],
        ["2026-10-30T12:00:03+00:00", big[:8], "responses", "muse-spark-1.3", "$0.5000"],
        ["2026-10-30T12:00:02+00:00", small[:8], "align", "-", "$0.1250"],
        ["2026-10-30T12:00:01+00:00", big[:8], "responses", "muse-spark-1.3", "$0.2500"],
    ]


def test_spend_shows_only_the_last_20_ledger_rows(cli, conn):
    device = add_device(conn)
    for second in range(25):
        charge(conn, device, 0.01, f"2026-10-30T12:00:{second:02d}+00:00")

    total, _, latest = sections(cli("spend")[1])

    assert total[0].startswith("Total: $0.2500 across 25 requests.")
    assert [line.split()[0] for line in latest[2:]] == [
        f"2026-10-30T12:00:{second:02d}+00:00" for second in range(24, 4, -1)
    ]


def test_spend_with_an_empty_ledger(cli):
    assert cli("spend") == (
        0,
        "Total: $0.0000 across 0 requests. Caps: $2.00 per device, $40.00 overall.\n"
        "\n"
        "No requests yet.\n",
        "",
    )


def test_spend_shows_the_caps_from_the_environment(cli, monkeypatch):
    monkeypatch.setenv("DEVICE_CAP_USD", "0.50")
    monkeypatch.setenv("GLOBAL_CAP_USD", "25")

    assert "Caps: $0.50 per device, $25.00 overall." in cli("spend")[1]


def test_enable_sam_turns_video_tracking_back_on_for_a_code(cli, conn):
    cli("add-code", "testcode")
    conn.execute("UPDATE codes SET sam_off = 1 WHERE code = 'testcode'")
    # list-codes shows which code the relay turned video tracking off for.
    assert table(cli("list-codes")[1])[1][:4] == ["testcode", "active,", "video", "off"]

    status, out, err = cli("enable-sam", "testcode")

    assert (status, err) == (0, "")
    assert out == 'Video tracking is on again for code "testcode".\n'
    assert conn.execute("SELECT sam_off FROM codes WHERE code = 'testcode'").fetchone()[0] == 0
    assert cli("enable-sam", "nope")[0] == 1
