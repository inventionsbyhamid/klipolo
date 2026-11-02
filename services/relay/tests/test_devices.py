import hashlib
import uuid

import pytest

from relay import db

INVALID_CODE = {
    "error": "invalid_code",
    "message": "This access code is not valid. Check the code or add your own Meta key in Settings.",
}
SHARED_CREDIT = {
    "error": "budget",
    "message": "Klipolo's shared AI credit is used up for now. Add your own Meta key in Settings.",
}


@pytest.fixture
def conn(settings):
    connection = db.connect(settings.db_path)
    yield connection
    connection.close()


def add_code(conn, code, active=True):
    conn.execute("INSERT INTO codes (code, active) VALUES (?, ?)", (code, int(active)))


def register(client, device_id=None, access_code=None):
    body = {"device_id": device_id or str(uuid.uuid4())}
    if access_code is not None:
        body["access_code"] = access_code
    return client.post("/v1/devices", json=body)


def bearer(token):
    return {"Authorization": f"Bearer {token}"}


def test_register_without_code_has_no_relay_ai(client):
    response = register(client)

    assert response.status_code == 200
    body = response.json()
    assert body["relay_ai"] is False
    assert body["budget_usd"] == 0
    assert body["spent_usd"] == 0
    assert isinstance(body["token"], str)
    assert len(body["token"]) >= 32


def test_register_with_active_code_gets_relay_ai_and_budget(client, conn):
    add_code(conn, "testcode")

    response = register(client, access_code="testcode")

    assert response.status_code == 200
    body = response.json()
    assert body["relay_ai"] is True
    assert body["budget_usd"] == 2.00
    assert body["spent_usd"] == 0


@pytest.mark.parametrize(("code", "stored_as"), [("retired", "inactive"), ("never-issued", None)])
def test_register_with_inactive_or_unknown_code_is_403(client, conn, code, stored_as):
    if stored_as == "inactive":
        add_code(conn, code, active=False)

    response = register(client, access_code=code)

    assert response.status_code == 403
    assert response.json() == INVALID_CODE
    assert conn.execute("SELECT count(*) FROM devices").fetchone()[0] == 0


def test_reregister_keeps_spend_and_rotates_token(client, conn):
    add_code(conn, "testcode")
    device_id = str(uuid.uuid4())
    first = register(client, device_id, "testcode").json()
    conn.execute("UPDATE devices SET spent_usd = 0.75 WHERE device_id = ?", (device_id,))

    second = register(client, device_id, "testcode").json()

    assert second["token"] != first["token"]
    assert second["spent_usd"] == 0.75
    assert second["relay_ai"] is True
    assert client.get("/v1/devices/me", headers=bearer(first["token"])).status_code == 401
    assert client.get("/v1/devices/me", headers=bearer(second["token"])).status_code == 200
    assert conn.execute("SELECT count(*) FROM devices").fetchone()[0] == 1


@pytest.mark.parametrize(("code", "stored_as"), [("retired", "inactive"), ("never-issued", None)])
def test_reregister_with_inactive_or_unknown_code_is_403_and_keeps_the_device(
    client, conn, code, stored_as
):
    add_code(conn, "testcode")
    if stored_as == "inactive":
        add_code(conn, code, active=False)
    device_id = str(uuid.uuid4())
    token = register(client, device_id, "testcode").json()["token"]
    before = conn.execute("SELECT * FROM devices WHERE device_id = ?", (device_id,)).fetchone()

    response = register(client, device_id, code)

    assert response.status_code == 403
    assert response.json() == INVALID_CODE
    after = conn.execute("SELECT * FROM devices WHERE device_id = ?", (device_id,)).fetchone()
    assert dict(after) == dict(before)
    assert after["code"] == "testcode"
    me = client.get("/v1/devices/me", headers=bearer(token))
    assert me.status_code == 200
    assert me.json()["relay_ai"] is True


def devices_on(conn, code):
    return conn.execute("SELECT count(*) FROM devices WHERE code = ?", (code,)).fetchone()[0]


def test_a_code_takes_any_number_of_devices_until_the_owner_sets_a_limit(client, conn):
    add_code(conn, "testcode")

    responses = [register(client, access_code="testcode") for _ in range(30)]

    assert {response.status_code for response in responses} == {200}
    assert devices_on(conn, "testcode") == 30


def test_a_code_at_its_device_limit_refuses_new_devices_but_not_its_own(client, conn):
    add_code(conn, "testcode")
    add_code(conn, "other")
    conn.execute("UPDATE codes SET max_devices = 2 WHERE code = 'testcode'")
    first, second = str(uuid.uuid4()), str(uuid.uuid4())
    assert register(client, first, "testcode").status_code == 200
    assert register(client, second, "testcode").status_code == 200

    refused = register(client, access_code="testcode")

    assert refused.status_code == 402
    assert refused.json() == SHARED_CREDIT
    assert devices_on(conn, "testcode") == 2
    # Its own devices register again (a new token after a 401), and other codes are not affected.
    again = register(client, first, "testcode")
    assert again.status_code == 200
    assert again.json()["relay_ai"] is True
    assert register(client, access_code="other").status_code == 200


def test_a_device_that_leaves_a_code_frees_no_room_on_it(client, conn):
    add_code(conn, "testcode")
    conn.execute("UPDATE codes SET max_devices = 1 WHERE code = 'testcode'")
    device_id = str(uuid.uuid4())
    register(client, device_id, "testcode")
    assert register(client, device_id).json()["relay_ai"] is False

    refused = register(client, access_code="testcode")

    assert refused.status_code == 402
    assert refused.json() == SHARED_CREDIT
    # The device that registered with it before may come back.
    assert register(client, device_id, "testcode").json()["relay_ai"] is True


@pytest.mark.parametrize(
    "headers",
    [
        {},
        bearer("not-a-real-token"),
        {"Authorization": "Basic dXNlcjpwYXNz"},
        {"Authorization": "Bearer "},
    ],
    ids=["missing", "unknown", "wrong-scheme", "empty"],
)
def test_unknown_token_is_401(client, headers):
    register(client)

    response = client.get("/v1/devices/me", headers=headers)

    assert response.status_code == 401
    assert response.json() == {
        "error": "unauthorized",
        "message": "This device is not registered. Register it again.",
    }
    assert response.headers["www-authenticate"] == "Bearer"


def test_me_reports_the_device_status(client, conn):
    add_code(conn, "testcode")
    token = register(client, access_code="testcode").json()["token"]

    response = client.get("/v1/devices/me", headers=bearer(token))

    assert response.status_code == 200
    assert response.json() == {"relay_ai": True, "budget_usd": 2.00, "spent_usd": 0}


def test_relay_ai_follows_the_code_active_flag(client, conn):
    add_code(conn, "testcode")
    token = register(client, access_code="testcode").json()["token"]

    conn.execute("UPDATE codes SET active = 0 WHERE code = 'testcode'")
    body = client.get("/v1/devices/me", headers=bearer(token)).json()

    assert body["relay_ai"] is False
    assert body["budget_usd"] == 0


def test_reregister_without_code_turns_relay_ai_off(client, conn):
    add_code(conn, "testcode")
    device_id = str(uuid.uuid4())
    register(client, device_id, "testcode")

    body = register(client, device_id).json()

    assert body["relay_ai"] is False
    assert body["budget_usd"] == 0


@pytest.mark.parametrize("typed", ["  testcode  ", "testcode\n"])
def test_code_is_trimmed(client, conn, typed):
    add_code(conn, "testcode")

    body = register(client, access_code=typed).json()

    assert body["relay_ai"] is True


def test_blank_code_counts_as_no_code(client):
    response = register(client, access_code="   ")

    assert response.status_code == 200
    assert response.json()["relay_ai"] is False


def test_token_is_stored_only_as_sha256(client, conn):
    device_id = str(uuid.uuid4())
    token = register(client, device_id).json()["token"]

    row = conn.execute(
        "SELECT token_hash FROM devices WHERE device_id = ?", (device_id,)
    ).fetchone()

    assert row["token_hash"] == hashlib.sha256(token.encode()).hexdigest()
    assert token not in "\n".join(conn.iterdump())


def test_device_id_is_stored_in_canonical_form(client, conn):
    device_id = uuid.uuid4()

    register(client, str(device_id).upper())
    register(client, str(device_id))

    rows = conn.execute("SELECT device_id FROM devices").fetchall()
    assert [row["device_id"] for row in rows] == [str(device_id)]


@pytest.mark.parametrize(
    ("body", "field"),
    [
        ({"device_id": "not-a-uuid"}, "device_id"),
        ({}, "device_id"),
        ({"device_id": str(uuid.uuid4()), "access_code": "x" * 200}, "access_code"),
    ],
    ids=["not-uuid", "missing", "code-too-long"],
)
def test_bad_registration_is_422_with_error_shape(client, body, field):
    response = client.post("/v1/devices", json=body)

    assert response.status_code == 422
    assert response.json() == {
        "error": "invalid_request",
        "message": f"Check these fields: {field}.",
    }
