import concurrent.futures
import email.parser
import email.policy
import io
import json
import logging
import math
import secrets
import time
import uuid
import wave
from contextlib import contextmanager
from dataclasses import dataclass, replace

import anyio
import httpx
import pytest
import respx
from fastapi.testclient import TestClient

from relay import db, proxy
from relay.app import create_app
from relay.devices import hash_token

META = "https://api.meta.ai/v1"
SPARK_USAGE = {
    "input_tokens": 10000,
    "input_tokens_details": {"cached_tokens": 4000},
    "output_tokens": 500,
    "output_tokens_details": {"reasoning_tokens": 100},
    "total_tokens": 10500,
}
SPARK_COST = 6000 * 1.25e-6 + 4000 * 0.15e-6 + 500 * 4.25e-6
BUDGET = {
    "error": "budget",
    "message": "This device has used its Klipolo credit. Add your own Meta key in Settings.",
}


@dataclass
class Dev:
    device_id: str
    token: str

    @property
    def headers(self):
        return {"Authorization": f"Bearer {self.token}"}


@pytest.fixture
def conn(settings):
    connection = db.connect(settings.db_path)
    yield connection
    connection.close()


@pytest.fixture
def meta():
    with respx.mock(base_url=META, assert_all_called=False) as router:
        yield router


@contextmanager
def relay_client(settings, **changes):
    with TestClient(create_app(replace(settings, **changes))) as test_client:
        yield test_client


def with_sam(settings):
    return replace(settings, allowed_models=settings.allowed_models | {"sam-3.1"})


@pytest.fixture
def sam_client(settings):
    """A relay with SAM switched on, which it is not by default."""
    with TestClient(create_app(with_sam(settings))) as test_client:
        yield test_client


def add_device(conn, *, code="testcode", active=True, spent=0.0):
    """A registered device. Any starting spend gets a matching ledger row, so
    the ledger total and spent_usd agree from the start."""
    if code is not None:
        conn.execute("INSERT OR IGNORE INTO codes (code, active) VALUES (?, ?)", (code, int(active)))
    token = secrets.token_urlsafe(32)
    device_id = str(uuid.uuid4())
    conn.execute(
        "INSERT INTO devices (device_id, token_hash, code, created_at, spent_usd) VALUES (?, ?, ?, ?, ?)",
        (device_id, hash_token(token), code, "2026-10-30T00:00:00+00:00", spent),
    )
    if spent:
        conn.execute(
            "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd) VALUES (?, ?, ?, ?, ?, ?)",
            (device_id, "2026-10-30T00:00:00+00:00", "responses", "muse-spark-1.3", "{}", spent),
        )
    return Dev(device_id, token)


def ledger(conn, device=None):
    if device is None:
        rows = conn.execute("SELECT * FROM ledger ORDER BY id").fetchall()
    else:
        rows = conn.execute("SELECT * FROM ledger WHERE device_id = ? ORDER BY id", (device.device_id,)).fetchall()
    return [dict(row) for row in rows]


def spent(conn, device):
    return conn.execute("SELECT spent_usd FROM devices WHERE device_id = ?", (device.device_id,)).fetchone()[0]


def assert_ledger_matches_spend(conn, device):
    total = sum(row["cost_usd"] for row in ledger(conn, device))
    assert spent(conn, device) == pytest.approx(total)


def spark_body(usage=SPARK_USAGE, model="muse-spark-1.3"):
    return {
        "id": "resp_1",
        "object": "response",
        "status": "completed",
        "model": model,
        "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hi"}]}],
        "usage": usage,
    }


def spark_request(model="muse-spark-1.3", **extra):
    return {"model": model, "input": "Say hi.", **extra}


def reserved_usage(sent, *, media=False, max_output_tokens=8192):
    """The reservation for a Spark call whose forwarded body was `sent`: one
    input token per byte, the whole output limit, and $0.75 for media."""
    usage = {"input_tokens": len(sent), "output_tokens": max_output_tokens, "reserved": True}
    if media:
        usage["media_usd"] = 0.75
    return usage


def reserved_cost(sent, *, media=False, max_output_tokens=8192):
    return len(sent) * 1.25e-6 + max_output_tokens * 4.25e-6 + (0.75 if media else 0)


def forwarded(body):
    """The bytes the relay sends Meta for a Spark `body` without max_output_tokens."""
    return json.dumps({**body, "max_output_tokens": 8192}, separators=(",", ":")).encode()


# What a plain spark_request() reserves.
SPARK_RESERVATION = reserved_cost(forwarded(spark_request()))


# The gate: token, relay AI, configuration.


def test_unknown_token_is_401(client, meta):
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(), headers={"Authorization": "Bearer nope"})

    assert response.status_code == 401
    assert response.json()["error"] == "unauthorized"
    assert not route.called


@pytest.mark.parametrize("code", [None, "retired"], ids=["no-code", "inactive-code"])
def test_device_without_relay_ai_is_403(client, conn, meta, code):
    device = add_device(conn, code=code, active=code is None)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "no_relay_ai"
    assert not route.called


def test_deactivating_a_code_stops_the_next_call(client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(200, json=spark_body())
    assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 200

    conn.execute("UPDATE codes SET active = 0 WHERE code = 'testcode'")
    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "no_relay_ai"


def test_missing_meta_key_is_503_not_configured(settings, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses")

    with TestClient(create_app(replace(settings, meta_api_key=""))) as unconfigured:
        response = unconfigured.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 503
    assert response.json() == {"error": "not_configured", "message": "Relay not configured."}
    assert not route.called


# Models.


def test_spark_call_is_forwarded_with_the_relay_key_and_metered(client, conn, meta, settings):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body(), headers={"x-request-id": "req-1"})

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    assert response.json() == spark_body()
    assert response.headers["x-request-id"] == "req-1"
    sent = route.calls.last.request
    assert sent.headers["authorization"] == f"Bearer {settings.meta_api_key}"
    assert json.loads(sent.content) == {**spark_request(), "max_output_tokens": 8192}
    [row] = ledger(conn, device)
    assert row["endpoint"] == "responses"
    assert row["model"] == "muse-spark-1.3"
    assert json.loads(row["usage_json"]) == SPARK_USAGE
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert spent(conn, device) == pytest.approx(SPARK_COST)


def test_contributor_model_is_403(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post(
        "/meta/v1/responses", json=spark_request("muse-spark-1.3-contributor"), headers=device.headers
    )

    assert response.status_code == 403
    assert response.json()["error"] == "model_not_allowed"
    assert not route.called
    assert ledger(conn) == []


def test_wonder_is_403(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request("muse-wonder-1.0"), headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "model_not_allowed"
    assert not route.called


def test_sam_is_403_by_default(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses")
    body = {"model": "sam-3.1", "stream": True, "input": [{"role": "user", "content": [{"type": "input_text", "text": "person"}, {"type": "input_video", "file_id": "file-1"}]}]}

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "model_not_allowed"
    assert not route.called


def test_wonder_and_contributors_stay_refused_even_if_listed(settings, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(200, json=spark_body())
    listed = frozenset({"muse-wonder-1.0", "muse-spark-1.3-contributor", "muse-spark-1.3"})

    with relay_client(settings, allowed_models=listed) as client:
        statuses = {
            model: client.post("/meta/v1/responses", json=spark_request(model), headers=device.headers).status_code
            for model in sorted(listed)
        }

    assert statuses == {"muse-spark-1.3": 200, "muse-spark-1.3-contributor": 403, "muse-wonder-1.0": 403}


def test_model_left_out_of_the_allowlist_is_403(settings, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses")

    with relay_client(settings, allowed_models=frozenset({"muse-voice-transcribe-1.0"})) as client:
        response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "model_not_allowed"
    assert not route.called


@pytest.mark.parametrize(
    "body",
    [
        {"input": "no model"},
        {"model": ["muse-spark-1.3"], "input": "x"},
        {"model": "muse-voice-transcribe-1.0", "input": "x"},
        {"model": "MUSE-SPARK-1.3", "input": "x"},
    ],
    ids=["missing", "not-a-string", "transcribe-model", "wrong-case"],
)
def test_responses_model_must_be_allowlisted(client, conn, meta, body):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "model_not_allowed"
    assert not route.called


@pytest.mark.parametrize(
    ("extra", "message"),
    [
        ({"background": True}, "Background responses are not supported through the relay."),
        ({"tools": [{"type": "web_search"}]}, "Only function tools are supported through the relay."),
        (
            {"tools": [{"type": "function", "name": "cut"}, {"type": "image_generation"}]},
            "Only function tools are supported through the relay.",
        ),
        ({"service_tier": "priority"}, "Only the default service tier is supported through the relay."),
    ],
    ids=["background", "hosted-tool", "mixed-tools", "priority-tier"],
)
def test_options_the_relay_cannot_meter_are_400(client, conn, meta, extra, message):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(**extra), headers=device.headers)

    assert response.status_code == 400
    assert response.json() == {"error": "unsupported", "message": message}
    assert not route.called


@pytest.mark.parametrize(
    ("extra", "named"),
    [
        ({"prompt": {"id": "pmpt_1", "variables": {"clip": {"type": "input_file", "file_id": "file-1"}}}}, "prompt"),
        ({"truncation": "auto"}, "truncation"),
        ({"include": ["reasoning.encrypted_content"], "prompt_cache_key": "k"}, "include, prompt_cache_key"),
        # Stored context the relay can't bound; Klipolo uses neither.
        ({"previous_response_id": "resp_0"}, "previous_response_id"),
        ({"conversation": "conv_1"}, "conversation"),
    ],
    ids=["prompt", "truncation", "two-fields", "previous-response", "conversation"],
)
def test_fields_the_relay_does_not_know_are_400(client, conn, meta, extra, named):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(**extra), headers=device.headers)

    assert response.status_code == 400
    assert response.json() == {"error": "unsupported", "message": f"The relay does not accept these fields: {named}."}
    assert not route.called


def test_every_known_field_is_forwarded(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())
    body = spark_request(
        instructions="Be brief.",
        tools=[{"type": "function", "name": "cut", "parameters": {"type": "object", "properties": {}}}],
        tool_choice="auto",
        parallel_tool_calls=True,
        text={"format": {"type": "json_schema", "name": "cut", "schema": {"type": "object"}, "strict": True}},
        reasoning={"effort": "low"},
        max_output_tokens=4000,
        temperature=1.0,
        top_p=1.0,
        stream=False,
        store=False,
        metadata={"clip": "a"},
        user="device-1",
        safety_identifier="device-1",
        background=False,
        service_tier="auto",
    )

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    assert json.loads(route.calls.last.request.content) == body


def test_function_tools_are_forwarded(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())
    body = spark_request(
        tools=[{"type": "function", "name": "cut", "parameters": {}}], service_tier="auto", max_output_tokens=2000
    )

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    assert json.loads(route.calls.last.request.content) == body


@pytest.mark.parametrize(("sent", "forwarded"), [(None, 8192), (1, 1), (16384, 16384)])
def test_spark_output_is_bounded_by_max_output_tokens(client, conn, meta, sent, forwarded):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())
    body = spark_request() if sent is None else spark_request(max_output_tokens=sent)

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    assert json.loads(route.calls.last.request.content)["max_output_tokens"] == forwarded


@pytest.mark.parametrize("value", [16385, 0, -1, 1.5, "100", True])
def test_max_output_tokens_over_16384_or_not_a_count_is_400(client, conn, meta, value):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(max_output_tokens=value), headers=device.headers)

    assert response.status_code == 400
    assert response.json() == {
        "error": "invalid_request",
        "message": "max_output_tokens must be a whole number from 1 to 16384.",
    }
    assert not route.called


@pytest.mark.parametrize(
    "content",
    [
        b'{"model": "muse-spark-1.3", "model": "muse-wonder-1.0", "input": "x"}',
        b'["muse-spark-1.3"]',
        b"not json",
        b'{"model": "muse-spark-1.3", "temperature": NaN}',
    ],
    ids=["duplicate-key", "not-an-object", "not-json", "nan"],
)
def test_bad_json_is_400(client, conn, meta, content):
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post(
        "/meta/v1/responses", content=content, headers={**device.headers, "content-type": "application/json"}
    )

    assert response.status_code == 400
    assert response.json()["error"] == "invalid_request"
    assert not route.called


def nested(levels):
    """A Spark request whose JSON is `levels` deep, counting the outer object."""
    return ('{"model":"muse-spark-1.3","input":' + "[" * (levels - 1) + "]" * (levels - 1) + "}").encode()


@pytest.mark.parametrize(("levels", "status"), [(64, 200), (65, 400), (50_000, 400)])
def test_json_nested_deeper_than_64_levels_is_400(client, conn, meta, levels, status):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())

    response = client.post(
        "/meta/v1/responses", content=nested(levels), headers={**device.headers, "content-type": "application/json"}
    )

    assert response.status_code == status
    assert route.called is (status == 200)
    if status == 400:
        assert response.json()["error"] == "invalid_request"


def test_transcribe_request_part_nested_too_deeply_is_400(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/asr/transcribe")
    spec = '{"model":"muse-voice-transcribe-1.0","keywords":' + "[" * 70 + "]" * 70 + "}"
    files = {"request": (None, spec, "application/json"), "audio": ("a.wav", b"RIFF", "audio/wav")}

    response = client.post("/meta/v1/asr/transcribe", files=files, headers=device.headers)

    assert response.status_code == 400
    assert not route.called


TOO_COSTLY = "This request could cost more than a device's whole Klipolo credit. Make it smaller."


@pytest.mark.parametrize(
    ("size", "status", "message"),
    [
        (1024 * 1024, 200, None),
        # At one token per byte, about 1.57 MB of text could cost more than $2.
        (1_600_000, 413, TOO_COSTLY),
        (2 * 1024 * 1024, 413, "The request is too large."),
    ],
    ids=["1-mib", "worst-case-over-the-cap", "over-2-mib"],
)
def test_json_bodies_are_capped_at_2_mib_and_by_their_worst_case(client, conn, meta, size, status, message):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())

    response = client.post("/meta/v1/responses", json=spark_request(input="x" * size), headers=device.headers)

    assert response.status_code == status
    assert route.called is (status == 200)
    if message is not None:
        assert response.json() == {"error": "too_large", "message": message}


def test_json_body_over_the_limit_is_413(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "MAX_JSON_BYTES", 100)
    device = add_device(conn)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(input="x" * 200), headers=device.headers)

    assert response.status_code == 413
    assert response.json()["error"] == "too_large"
    assert not route.called


# Caps.


def test_device_over_cap_is_402(client, conn, meta):
    device = add_device(conn, spent=2.00)
    route = meta.post("/responses")

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 402
    assert response.json() == BUDGET
    assert not route.called
    assert spent(conn, device) == 2.00


@pytest.mark.parametrize(("room", "status"), [(0.001, 200), (-0.001, 402)], ids=["fits", "does-not-fit"])
def test_admission_needs_room_for_the_reservation(client, conn, meta, room, status):
    start = 2.00 - SPARK_RESERVATION - room
    device = add_device(conn, spent=start)
    route = meta.post("/responses").respond(200, json=spark_body())

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == status
    assert route.called is (status == 200)
    if status == 200:
        assert spent(conn, device) == pytest.approx(start + SPARK_COST)
    assert_ledger_matches_spend(conn, device)


def test_a_media_call_needs_room_for_75_cents_more(client, conn, meta):
    device = add_device(conn, spent=1.30)
    route = meta.post("/responses").respond(200, json=spark_body())
    video = spark_request(input=[{"role": "user", "content": [{"type": "input_video", "file_id": "file-1"}]}])

    assert client.post("/meta/v1/responses", json=video, headers=device.headers).status_code == 402
    assert not route.called
    assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 200


def test_a_call_that_could_cost_more_than_the_device_cap_is_413(settings, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())
    video = spark_request(input=[user({"type": "input_video", "file_id": "file-1"})])

    with relay_client(settings, device_cap_usd=0.50) as client:
        # A fresh device: "used its credit" would be wrong here.
        too_big = client.post("/meta/v1/responses", json=video, headers=device.headers)
        small = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert too_big.status_code == 413
    assert too_big.json() == {
        "error": "too_large",
        "message": "This request could cost more than a device's whole Klipolo credit. Make it smaller.",
    }
    assert small.status_code == 200
    assert route.call_count == 1


def test_over_cap_device_is_refused_before_its_body_is_read(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "MAX_JSON_BYTES", 100)
    device = add_device(conn, spent=2.00)

    # Too large to read, but the budget answer comes first.
    response = client.post("/meta/v1/responses", json=spark_request(input="x" * 500), headers=device.headers)

    assert response.status_code == 402
    assert response.json() == BUDGET


def test_global_over_cap_is_402(client, conn, meta):
    # Twenty devices at the device cap reach the $40 global cap together.
    for _ in range(20):
        add_device(conn, spent=2.00)
    fresh = add_device(conn)
    other_code = add_device(conn, code="press")
    route = meta.post("/responses")

    for device in (fresh, other_code):
        response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)
        assert response.status_code == 402
        assert response.json() == BUDGET
    assert not route.called


def test_global_cap_counts_the_whole_ledger_and_the_reservation(client, conn, meta):
    add_device(conn, spent=40.00 - SPARK_RESERVATION - 0.001)
    fresh = add_device(conn)
    meta.post("/responses").respond(200, json=spark_body())

    assert client.post("/meta/v1/responses", json=spark_request(), headers=fresh.headers).status_code == 200
    # The first call's $0.0102 leaves less room than a reservation, so the next
    # call from anyone is refused.
    assert client.post("/meta/v1/responses", json=spark_request(), headers=fresh.headers).status_code == 402


# Meta errors.


@pytest.mark.parametrize(
    ("status", "error"),
    [
        (503, {"code": "service_overloaded", "message": "The backend is temporarily overloaded. Please retry."}),
        (429, {"code": "rate_limit_exceeded", "message": "Too many requests."}),
        (400, {"code": "invalid_value", "message": "Unsupported video format."}),
    ],
)
def test_meta_503_passes_through_and_charges_nothing(client, conn, meta, status, error):
    device = add_device(conn)
    meta.post("/responses").respond(
        status, json={"error": {**error, "param": None, "type": "server_error"}}, headers={"retry-after": "7"}
    )

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == status
    assert response.json() == {"error": "meta_error", "message": error["message"]}
    assert response.headers["retry-after"] == "7"
    assert ledger(conn) == []
    assert spent(conn, device) == 0


def test_meta_error_without_a_json_body_gets_a_plain_reason(client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(502, text="<html>Bad gateway</html>")

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 502
    assert response.json() == {"error": "meta_error", "message": "Meta returned an error."}


def test_long_meta_reason_is_cut_short(client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(400, json={"error": {"message": "word " * 500}})

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 400
    assert len(response.json()["message"]) <= 300


def test_meta_rejecting_the_relay_key_is_502_not_401(client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(401, json={"error": {"message": "Incorrect API key provided."}})

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    # A 401 would tell the app its own device token is bad.
    assert response.status_code == 502
    assert response.json() == {
        "error": "meta_auth",
        "message": "The relay could not use its Meta key. Try again later.",
    }
    assert ledger(conn) == []


@pytest.mark.parametrize(
    "error",
    [httpx.ConnectError("no route"), httpx.LocalProtocolError("invalid header")],
    ids=["connect-error", "local-protocol-error"],
)
def test_meta_unreachable_is_502_and_charges_nothing(client, conn, meta, error):
    device = add_device(conn)
    meta.post("/responses").mock(side_effect=error)

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 502
    assert response.json() == {"error": "meta_unreachable", "message": "Could not reach Meta. Try again."}
    assert ledger(conn) == []


def test_meta_timeout_after_the_request_was_sent_charges_the_reservation(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses").mock(side_effect=httpx.ReadTimeout("slow"))

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 504
    assert response.json() == {"error": "meta_timeout", "message": "Meta took too long to answer. Try again."}
    [row] = ledger(conn, device)
    sent = route.calls.last.request.content
    assert json.loads(row["usage_json"]) == reserved_usage(sent)
    assert row["cost_usd"] == pytest.approx(reserved_cost(sent))
    assert_ledger_matches_spend(conn, device)


VIDEO_BY_FILE = {"type": "input_video", "file_id": "file-1"}


@pytest.mark.parametrize(
    "body",
    [
        spark_request(input=[{"role": "user", "content": [{"type": "input_text", "text": "moments"}, VIDEO_BY_FILE]}]),
        spark_request(input=[{"role": "user", "content": [{"type": "input_file", "file_url": "https://x/a.mp4"}]}]),
        spark_request(input=[{"role": "user", "content": [{"type": "input_image", "file_id": "file-2"}]}]),
        {"model": "sam-3.1", "input": [{"role": "user", "content": [{"type": "input_text", "text": "person"}]}]},
    ],
    ids=["input-video", "input-file", "file-id", "sam"],
)
def test_timeout_on_a_media_call_charges_the_media_reservation(sam_client, conn, meta, body):
    device = add_device(conn)
    route = meta.post("/responses").mock(side_effect=httpx.ReadTimeout("slow"))

    response = sam_client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 504
    [row] = ledger(conn, device)
    usage = json.loads(row["usage_json"])
    assert usage["reserved"] is True
    assert usage["media_usd"] == 0.75
    if body["model"] == "sam-3.1":
        assert row["cost_usd"] == pytest.approx(0.75)
    else:
        assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content, media=True))
    assert_ledger_matches_spend(conn, device)


def user(*parts):
    return {"role": "user", "content": list(parts)}


URL_IMAGE = {"type": "input_image", "image_url": "https://example.com/frame.jpg"}
INLINE_IMAGE = {"type": "input_image", "image_url": "data:image/png;base64,iVBORw0KGgo="}
MEDIA_REQUESTS = {
    "500-url-images": spark_request(input=[user(*[URL_IMAGE] * 500)]),
    "200-item-references": spark_request(input=[{"type": "item_reference", "id": f"msg_{i}"} for i in range(200)]),
    "file-in-instructions": spark_request(instructions=[user({"type": "input_file", "file_id": "file-1"})]),
    "image-by-file-id": spark_request(input=[user({"type": "input_image", "file_id": "file-1"})]),
    "inline-audio": spark_request(input=[user({"type": "input_audio", "input_audio": {"data": "AAAA", "format": "wav"}})]),
    "file-as-an-item": spark_request(input=[{"type": "input_file", "file_url": "https://example.com/a.mp4"}]),
    "tool-output-with-an-image": spark_request(
        input=[{"type": "function_call_output", "call_id": "c1", "output": [URL_IMAGE]}]
    ),
    "unknown-part": spark_request(input=[user({"type": "input_sticker", "text": "hi"})]),
    "unknown-item": spark_request(input=[{"type": "mcp_call", "id": "call_1", "name": "fetch"}]),
    "url-in-a-tool": spark_request(
        tools=[{"type": "function", "name": "f", "parameters": {"type": "object"}, "logo_url": "https://example.com/l.png"}]
    ),
    "inline-image": spark_request(input=[user({"type": "input_text", "text": "What is this?"}, INLINE_IMAGE)]),
    "reasoning-by-id": spark_request(input=[{"type": "reasoning", "id": "rs_1"}]),
    "reasoning-with-empty-summary": spark_request(input=[{"type": "reasoning", "id": "rs_1", "summary": []}]),
    "list-typed-part": spark_request(input=[user({"type": ["input_text"], "text": "hi"})]),
    "list-typed-item": spark_request(input=[{"type": ["message"], "role": "user", "content": "hi"}]),
}
TEXT_REQUESTS = {
    "string-input": spark_request(),
    "text-history": spark_request(
        instructions="Be brief.",
        input=[
            user({"type": "input_text", "text": "Cut the intro."}),
            {"role": "assistant", "content": [{"type": "output_text", "text": "Which part?"}]},
            {"role": "user", "content": "The first 13 seconds."},
            {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "Plan."}]},
            {"type": "function_call", "call_id": "c1", "name": "cut", "arguments": "{\"end\": 13}"},
            {"type": "function_call_output", "call_id": "c1", "output": "done"},
        ],
    ),
    "reasoning-carried-inline": spark_request(
        input=[{"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "gAAAAABo"}]
    ),
    "nullable-tool-property": spark_request(
        tools=[
            {
                "type": "function",
                "name": "follow",
                "strict": True,
                "parameters": {
                    "type": "object",
                    "properties": {"who": {"type": ["string", "null"]}},
                    "required": ["who"],
                    "additionalProperties": False,
                },
            }
        ]
    ),
    "nullable-schema-property": spark_request(
        text={
            "format": {
                "type": "json_schema",
                "name": "moment",
                "strict": True,
                "schema": {"type": "object", "properties": {"who": {"type": ["string", "null"]}}, "required": ["who"]},
            }
        }
    ),
    "schema-and-tools": spark_request(
        tools=[{"type": "function", "name": "cut", "parameters": {"type": "object", "properties": {"end": {"type": "number"}}}}],
        text={"format": {"type": "json_schema", "name": "m", "schema": {"type": "object", "properties": {"kind": {"type": "string"}}}}},
    ),
}


@pytest.mark.parametrize(
    ("body", "media"),
    [*((body, True) for body in MEDIA_REQUESTS.values()), *((body, False) for body in TEXT_REQUESTS.values())],
    ids=[*MEDIA_REQUESTS, *TEXT_REQUESTS],
)
def test_only_text_only_requests_skip_the_media_reservation(client, conn, meta, body, media):
    device = add_device(conn)
    # No usage in the answer, so the reservation itself is charged.
    route = meta.post("/responses").respond(200, json={"status": "completed"})

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    sent = route.calls.last.request.content
    assert json.loads(row["usage_json"]) == reserved_usage(sent, media=media)
    assert row["cost_usd"] == pytest.approx(reserved_cost(sent, media=media))


@pytest.mark.parametrize(
    ("fps", "status"),
    [(2, 200), (1, 200), (0.5, 200), (2.5, 400), (30, 400), (0, 400), (-1, 400), ("2", 400), (True, 400), (None, 400)],
)
def test_input_video_fps_is_at_most_2(client, conn, meta, fps, status):
    # The media amount in a reservation assumes at most 2 frames a second.
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())
    body = spark_request(input=[user({"type": "input_video", "file_id": "file-1", "fps": fps})])

    response = client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == status
    assert route.called is (status == 200)
    if status == 400:
        assert response.json() == {
            "error": "invalid_request",
            "message": "input_video fps must be a number above 0 and at most 2.",
        }


def test_success_without_usage_charges_the_reservation(client, conn, meta):
    device = add_device(conn)
    body = spark_body()
    del body["usage"]
    route = meta.post("/responses").respond(200, json=body)

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    sent = route.calls.last.request.content
    assert json.loads(row["usage_json"]) == reserved_usage(sent)
    assert row["cost_usd"] == pytest.approx(reserved_cost(sent))


def test_success_with_garbled_usage_charges_the_reservation(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body(usage={"input_tokens": "lots", "output_tokens": 1}))

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content))


DEEP = "[" * 100_000 + "]" * 100_000


@pytest.mark.parametrize(
    "usage",
    [{"input_tokens": 10000}, {"output_tokens": 500}, {"total_tokens": 10500}, {}],
    ids=["no-output", "no-input", "total-only", "empty"],
)
def test_usage_missing_a_counter_charges_the_reservation(client, conn, meta, usage):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body(usage=usage))

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    sent = route.calls.last.request.content
    assert json.loads(row["usage_json"]) == reserved_usage(sent)
    assert row["cost_usd"] == pytest.approx(reserved_cost(sent))


def test_sam_video_usage_without_frames_charges_the_reservation(sam_client, conn, meta):
    device = add_device(conn)
    done = {"type": "response.completed", "response": {"status": "completed", "usage": {}}}
    stream_from_meta(meta, MetaStream([sse(CREATED), sse(done)]))
    body = {"model": "sam-3.1", "stream": True, "input": [{"role": "user", "content": [{"type": "input_text", "text": "person"}, {"type": "input_video", "file_id": "file-1"}]}]}

    response = sam_client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"])["reserved"] is True
    assert row["cost_usd"] == pytest.approx(0.75 + 50 * 0.20 / 1000)


def test_usage_that_breaks_the_parser_charges_the_reservation(client, conn, meta):
    device = add_device(conn)
    body = '{"usage":{"input_tokens":1,"output_tokens":1},"output":' + DEEP + "}"
    route = meta.post("/responses").respond(200, content=body.encode(), headers={"content-type": "application/json"})

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    assert response.content == body.encode()
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content))
    assert client.app.state.in_flight.count() == 0


def test_a_failure_while_pricing_usage_charges_the_reservation(client, conn, meta, monkeypatch):
    def broken(*args, **kwargs):
        raise RuntimeError("unexpected")

    monkeypatch.setattr(proxy, "_numbers_only", broken)
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content))


def test_usage_after_a_huge_output_is_found_in_the_tail(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "FULL_PARSE_BYTES", 1000)
    device = add_device(conn)
    body = spark_body()
    body["output"][0]["content"][0]["text"] = 'say "usage": {"input_tokens": 0} ' * 200
    meta.post("/responses").respond(200, json=body)

    response = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    assert response.status_code == 200
    assert response.json() == body
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)


def test_every_usage_in_a_huge_body_is_priced_and_the_highest_charged(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "FULL_PARSE_BYTES", 1000)
    device = add_device(conn)
    body = {"output": "x" * 2000, "extra": {"usage": BIG_USAGE}, "usage": SPARK_USAGE}
    meta.post("/responses").respond(200, json=body)

    client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(BIG_COST)


def test_huge_body_without_usage_charges_the_reservation(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "FULL_PARSE_BYTES", 1000)
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json={"output": "x" * 2000})

    client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content))


# SAM.


@pytest.mark.parametrize("stream", [None, False])
def test_sam_video_must_stream(sam_client, conn, meta, stream):
    device = add_device(conn)
    route = meta.post("/responses")
    body = {
        "model": "sam-3.1",
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "person"}, {"type": "input_video", "file_id": "file-1"}]}],
    }
    if stream is not None:
        body["stream"] = stream

    response = sam_client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 400
    assert response.json() == {"error": "unsupported", "message": "SAM video calls must stream through the relay."}
    assert not route.called


def test_sam_images_are_charged_per_image_in_the_request(sam_client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(200, json={"status": "completed", "usage": {}})
    image = {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}
    body = {"model": "sam-3.1", "input": [{"role": "user", "content": [{"type": "input_text", "text": "face"}, image, image]}]}

    response = sam_client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"]) == {"images_in_request": 2}
    assert row["cost_usd"] == pytest.approx(2 * 2.50 / 1000)


# Transcription.


def transcribe_files(model="muse-voice-transcribe-1.0", audio=b"RIFF" + b"\0" * 1000):
    spec = {"mode": "DIARIZATION", "model": model, "audioEncoding": "WAV", "languageBias": ["English"]}
    return {"request": (None, json.dumps(spec), "application/json"), "audio": ("clip.wav", audio, "audio/wav")}, spec


def parse_multipart(request):
    message = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(
        b"Content-Type: " + request.headers["content-type"].encode() + b"\r\n\r\n" + request.content
    )
    return {part.get_param("name", header="content-disposition"): part for part in message.iter_parts()}


def test_transcribe_is_forwarded_and_metered_from_audio_duration(client, conn, meta, settings):
    device = add_device(conn)
    route = meta.post("/asr/transcribe").respond(
        200, json={"sessionId": "s1", "transcript": "Hi.", "audioDurationMs": 155000, "turns": []}
    )
    files, spec = transcribe_files()

    response = client.post("/meta/v1/asr/transcribe?sessionId=s1", files=files, headers=device.headers)

    assert response.status_code == 200
    assert response.json()["transcript"] == "Hi."
    sent = route.calls.last.request
    assert sent.url.params["sessionId"] == "s1"
    assert sent.headers["authorization"] == f"Bearer {settings.meta_api_key}"
    parts = parse_multipart(sent)
    assert set(parts) == {"request", "audio"}
    assert json.loads(parts["request"].get_payload(decode=True)) == spec
    assert parts["audio"].get_payload(decode=True) == files["audio"][1]
    assert parts["audio"].get_content_type() == "audio/wav"
    [row] = ledger(conn, device)
    assert row["endpoint"] == "asr/transcribe"
    assert row["model"] == "muse-voice-transcribe-1.0"
    assert json.loads(row["usage_json"]) == {"audioDurationMs": 155000}
    assert row["cost_usd"] == pytest.approx(0.18 * 155 / 3600)


def wav(seconds, rate=16000):
    out = io.BytesIO()
    with wave.open(out, "wb") as file:
        file.setnchannels(1)
        file.setsampwidth(2)
        file.setframerate(rate)
        file.writeframes(b"\0\0" * int(seconds * rate))
    return out.getvalue()


def with_byte_rate(audio, rate):
    # The canonical header keeps the byte rate at bytes 28 to 31.
    return audio[:28] + rate.to_bytes(4, "little") + audio[32:]


@pytest.mark.parametrize(
    ("audio", "seconds"),
    [
        (wav(3), 3),
        # Reserved at 32,000 bytes a second, the lowest rate Meta takes.
        (wav(2, rate=24000), 3),
        (with_byte_rate(wav(3), 0xFFFFFFFF), 3),
        (with_byte_rate(wav(3), 0), 3),
        (b"RIFF" + b"\0" * 1000, 600),
        (b"not audio at all", 600),
    ],
    ids=["16-khz", "24-khz", "huge-byte-rate", "zero-byte-rate", "bad-header", "not-wav"],
)
def test_transcribe_without_a_duration_charges_the_reservation_from_the_wav_header(client, conn, meta, audio, seconds):
    device = add_device(conn)
    meta.post("/asr/transcribe").respond(200, json={"transcript": "Hi.", "turns": []})
    files, _ = transcribe_files(audio=audio)

    response = client.post("/meta/v1/asr/transcribe", files=files, headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"]) == {"audioDurationMs": seconds * 1000, "reserved": True}
    assert row["cost_usd"] == pytest.approx(0.18 * seconds / 3600)


@pytest.mark.parametrize("model", ["muse-spark-1.3", "muse-voice-transcribe-1.0-contributor", None])
def test_transcribe_model_must_be_allowlisted(client, conn, meta, model):
    device = add_device(conn)
    route = meta.post("/asr/transcribe")
    files, _ = transcribe_files(model=model)

    response = client.post("/meta/v1/asr/transcribe", files=files, headers=device.headers)

    assert response.status_code == 403
    assert response.json()["error"] == "model_not_allowed"
    assert not route.called


@pytest.mark.parametrize(
    "files",
    [
        {"audio": ("clip.wav", b"RIFF", "audio/wav")},
        {"request": (None, '{"model": "muse-voice-transcribe-1.0"}', "application/json")},
        {
            "request": (None, '{"model": "muse-voice-transcribe-1.0"}', "application/json"),
            "audio": ("clip.wav", b"RIFF", "audio/wav"),
            "extra": (None, "x", "text/plain"),
        },
    ],
    ids=["no-request", "no-audio", "extra-part"],
)
def test_transcribe_needs_exactly_a_request_and_an_audio_part(client, conn, meta, files):
    device = add_device(conn)
    route = meta.post("/asr/transcribe")

    response = client.post("/meta/v1/asr/transcribe", files=files, headers=device.headers)

    assert response.status_code == 400
    assert response.json()["error"] == "invalid_request"
    assert not route.called


def test_transcribe_that_is_not_multipart_is_400(client, conn, meta):
    device = add_device(conn)

    response = client.post("/meta/v1/asr/transcribe", json={"model": "muse-voice-transcribe-1.0"}, headers=device.headers)

    assert response.status_code == 400
    assert response.json()["error"] == "invalid_request"


def test_transcribe_audio_over_the_limit_is_413(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "MAX_AUDIO_BYTES", 500)
    device = add_device(conn)
    route = meta.post("/asr/transcribe")
    files, _ = transcribe_files(audio=b"\0" * 600)

    response = client.post("/meta/v1/asr/transcribe", files=files, headers=device.headers)

    assert response.status_code == 413
    assert response.json()["error"] == "too_large"
    assert not route.called


# Files.


def test_file_upload_streams_through_uncharged(client, conn, meta):
    device = add_device(conn)
    uploaded = {"id": "file-123", "object": "file", "bytes": 4, "purpose": "user_data"}
    route = meta.post("/files").respond(200, json=uploaded)
    files = [("purpose", (None, "user_data")), ("file", ("clip.mp4", b"\0\0\0\x18ftyp" * 100, "video/mp4"))]

    response = client.post("/meta/v1/files", files=files, headers=device.headers)

    assert response.status_code == 200
    assert response.json() == uploaded
    sent = route.calls.last.request
    assert sent.headers["content-type"].startswith("multipart/form-data; boundary=")
    parts = parse_multipart(sent)
    assert parts["purpose"].get_payload(decode=True) == b"user_data"
    assert parts["file"].get_payload(decode=True) == b"\0\0\0\x18ftyp" * 100
    assert sent.headers["content-length"] == str(len(sent.content))
    assert "transfer-encoding" not in sent.headers
    assert ledger(conn) == []


def test_file_upload_declared_over_1_gib_is_413(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/files")

    response = client.post(
        "/meta/v1/files",
        content=b"--b--\r\n",
        headers={
            **device.headers,
            "content-type": "multipart/form-data; boundary=b",
            "content-length": str(2 * 1024**3),
        },
    )

    assert response.status_code == 413
    assert response.json() == {"error": "too_large", "message": "The file is larger than 1 GiB."}
    assert not route.called


def test_chunked_file_upload_over_the_limit_is_413(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "MAX_UPLOAD_BYTES", 1000)
    monkeypatch.setattr(proxy, "UPLOAD_OVERHEAD_BYTES", 0)
    device = add_device(conn)
    meta.post("/files").respond(200, json={"id": "file-1"})

    def chunks():
        for _ in range(20):
            yield b"x" * 100

    response = client.post(
        "/meta/v1/files",
        content=chunks(),
        headers={**device.headers, "content-type": "multipart/form-data; boundary=b"},
    )

    assert response.status_code == 413
    assert response.json()["error"] == "too_large"


def test_file_upload_that_is_not_multipart_is_400(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/files")

    response = client.post("/meta/v1/files", content=b"raw", headers={**device.headers, "content-type": "video/mp4"})

    assert response.status_code == 400
    assert not route.called


def test_file_upload_is_refused_over_the_cap(client, conn, meta):
    device = add_device(conn, spent=2.00)
    route = meta.post("/files")

    response = client.post("/meta/v1/files", files={"file": ("a.mp4", b"x", "video/mp4")}, headers=device.headers)

    assert response.status_code == 402
    assert not route.called


@pytest.mark.parametrize("method", ["GET", "DELETE"])
def test_file_read_and_delete_pass_through_uncharged_even_over_the_cap(client, conn, meta, method):
    device = add_device(conn, spent=2.00)
    route = meta.route(method=method, path="/files/file-1812852566410237").respond(
        200, json={"id": "file-1812852566410237", "deleted": method == "DELETE"}
    )

    response = client.request(method, "/meta/v1/files/file-1812852566410237", headers=device.headers)

    assert response.status_code == 200
    assert response.json()["id"] == "file-1812852566410237"
    assert route.called
    assert len(ledger(conn)) == 1  # only the starting spend


def test_file_id_with_odd_characters_is_404(client, conn, meta):
    device = add_device(conn)

    response = client.get("/meta/v1/files/..%2Ef", headers=device.headers)

    assert response.status_code == 404
    assert not meta.calls


# Routing and headers.


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("GET", "/meta/v1/responses"),
        ("GET", "/meta/v1/responses/resp_1"),
        ("POST", "/meta/v1/responses/input_tokens"),
        ("GET", "/meta/v1/files"),
        ("POST", "/meta/v1/files/file-1"),
        ("GET", "/meta/v1/files/file-1/content"),
        ("POST", "/meta/v1/chat/completions"),
        ("GET", "/meta/v1/models"),
        ("POST", "/meta/v1/images/generations"),
        ("PUT", "/meta/v1/asr/transcribe"),
        ("POST", "/meta/v1/"),
        ("POST", "/meta/v2/responses"),
    ],
)
def test_other_paths_and_methods_are_404(client, conn, meta, method, path):
    device = add_device(conn)

    response = client.request(method, path, headers=device.headers)

    assert response.status_code == 404
    assert response.json() == {"error": "not_found", "message": "Not found."}
    assert not meta.calls


def test_client_credentials_and_hop_headers_are_not_forwarded(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body(), headers={"set-cookie": "meta=1"})

    response = client.post(
        "/meta/v1/responses",
        json=spark_request(),
        headers={
            **device.headers,
            "Cookie": "session=abc",
            "X-Api-Key": "client-key",
            "Proxy-Authorization": "Basic eA==",
            "X-Forwarded-For": "203.0.113.9",
            "Idempotency-Key": "idem-1",
        },
    )

    assert response.status_code == 200
    sent = route.calls.last.request.headers
    assert device.token not in str(sent.raw)
    for name in ("cookie", "x-api-key", "proxy-authorization", "x-forwarded-for"):
        assert name not in sent
    assert sent["idempotency-key"] == "idem-1"
    assert "set-cookie" not in response.headers


def test_session_id_passes_through_to_transcribe(client, conn, meta):
    device = add_device(conn)
    route = meta.post("/asr/transcribe").respond(200, json={"audioDurationMs": 1000})

    response = client.post(
        "/meta/v1/asr/transcribe?sessionId=klipolo-a1_b.2", files=transcribe_files()[0], headers=device.headers
    )

    assert response.status_code == 200
    assert route.calls.last.request.url.query == b"sessionId=klipolo-a1_b.2"


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("POST", "/meta/v1/responses?include=reasoning.encrypted_content"),
        ("POST", "/meta/v1/responses?sessionId=s1"),
        ("POST", "/meta/v1/files?purpose=user_data"),
        ("GET", "/meta/v1/files/file-1?x=1"),
        ("DELETE", "/meta/v1/files/file-1?x=1"),
        ("POST", "/meta/v1/asr/transcribe?other=1"),
        ("POST", "/meta/v1/asr/transcribe?sessionId=a&sessionId=b"),
        ("POST", "/meta/v1/asr/transcribe?sessionId=%C3%A9"),
        ("POST", "/meta/v1/asr/transcribe?sessionId"),
        ("POST", "/meta/v1/asr/transcribe?sessionId=" + "a" * 200),
    ],
)
def test_other_query_parameters_are_400(client, conn, meta, method, path):
    device = add_device(conn)

    response = client.request(method, path, headers=device.headers)

    assert response.status_code == 400
    assert response.json() == {"error": "invalid_request", "message": "This query parameter is not supported through the relay."}
    assert not meta.calls


# Concurrency.


BIG_USAGE = {"input_tokens": 100_000, "output_tokens": 500}
BIG_COST = 100_000 * 1.25e-6 + 500 * 4.25e-6


@pytest.mark.parametrize(
    ("usage", "cost"), [(SPARK_USAGE, SPARK_COST), (BIG_USAGE, BIG_COST)], ids=["within-reservation", "over-reservation"]
)
def test_racing_requests_stay_within_the_cap_and_their_reservations(client, conn, meta, settings, usage, cost):
    # Room for two reservations: of four calls at once, two get in.
    start = settings.device_cap_usd - 2.5 * SPARK_RESERVATION
    device = add_device(conn, spent=start)
    state = hold_meta_calls(meta, usage=usage)

    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        calls = [
            pool.submit(client.post, "/meta/v1/responses", json=spark_request(), headers=device.headers)
            for _ in range(4)
        ]
        wait_for(lambda: state["arrived"] == 2 and sum(call.done() for call in calls) == 2)
        state["release"] = True
        statuses = sorted(call.result().status_code for call in calls)

    assert statuses == [200, 200, 402, 402]
    total = spent(conn, device)
    assert total == pytest.approx(start + 2 * cost)
    # Never past the cap, except by what Meta charged beyond a reservation.
    shortfall = 2 * max(0.0, cost - SPARK_RESERVATION)
    assert total <= settings.device_cap_usd + shortfall + 1e-9
    assert_ledger_matches_spend(conn, device)
    assert client.app.state.in_flight.reserved(device.device_id) == (0.0, 0.0)


def test_reservations_in_flight_count_against_the_global_cap(client, conn, meta, settings):
    add_device(conn, spent=settings.global_cap_usd - 1.5 * SPARK_RESERVATION)
    first, second = add_device(conn), add_device(conn)
    state = hold_meta_calls(meta)

    with concurrent.futures.ThreadPoolExecutor(1) as pool:
        held = pool.submit(client.post, "/meta/v1/responses", json=spark_request(), headers=first.headers)
        wait_for(lambda: state["arrived"] == 1)
        # The first call's reservation leaves no room for a second, from any device.
        refused = client.post("/meta/v1/responses", json=spark_request(), headers=second.headers)
        state["release"] = True

    assert held.result().status_code == 200
    assert refused.status_code == 402
    assert client.app.state.in_flight.reserved(first.device_id) == (0.0, 0.0)


def hold_meta_calls(meta, path="/responses", usage=SPARK_USAGE):
    """Make Meta calls wait until `state["release"]` is set."""
    state = {"arrived": 0, "release": False}

    async def held(request):
        state["arrived"] += 1
        with anyio.fail_after(5):
            while not state["release"]:
                await anyio.sleep(0.01)
        return httpx.Response(200, json=spark_body(usage))

    meta.post(path).mock(side_effect=held)
    return state


def wait_for(predicate, timeout=5.0):
    deadline = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < deadline, "timed out"
        time.sleep(0.01)


def test_more_than_four_calls_at_once_from_one_device_is_429(client, conn, meta):
    device = add_device(conn)
    state = hold_meta_calls(meta)

    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        held = [
            pool.submit(client.post, "/meta/v1/responses", json=spark_request(), headers=device.headers)
            for _ in range(4)
        ]
        wait_for(lambda: state["arrived"] == 4)
        fifth = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)
        state["release"] = True
        statuses = [future.result().status_code for future in held]

    assert fifth.status_code == 429
    assert fifth.json()["error"] == "busy"
    assert statuses == [200] * 4
    assert len(ledger(conn, device)) == 4
    # The slots are free again.
    assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 200


def test_calls_in_flight_are_capped_across_devices(client, conn, meta):
    client.app.state.in_flight = proxy.InFlight(per_device=4, total=2)
    first, second, third = (add_device(conn) for _ in range(3))
    state = hold_meta_calls(meta)

    with concurrent.futures.ThreadPoolExecutor(2) as pool:
        held = [
            pool.submit(client.post, "/meta/v1/responses", json=spark_request(), headers=device.headers)
            for device in (first, second)
        ]
        wait_for(lambda: state["arrived"] == 2)
        refused = client.post("/meta/v1/responses", json=spark_request(), headers=third.headers)
        state["release"] = True
        statuses = [future.result().status_code for future in held]

    assert refused.status_code == 429
    assert refused.json()["error"] == "busy"
    assert statuses == [200, 200]


def test_refused_calls_free_their_slot(client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(503, json={"error": {"message": "busy"}})

    for _ in range(6):
        assert client.post("/meta/v1/responses", json=spark_request("muse-wonder-1.0"), headers=device.headers).status_code == 403
        assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 503

    assert client.app.state.in_flight.count() == 0


# Charging.


def test_charges_fail_closed_until_one_can_be_written(client, conn, meta, monkeypatch):
    import sqlite3

    monkeypatch.setattr(proxy, "CHARGE_RETRY_DELAY_S", 0)
    real_record_charge = proxy.record_charge
    broken = {"on": True}

    def record_charge(*args, **kwargs):
        if broken["on"]:
            raise sqlite3.OperationalError("disk I/O error")
        return real_record_charge(*args, **kwargs)

    monkeypatch.setattr(proxy, "record_charge", record_charge)
    device = add_device(conn)
    route = meta.post("/responses").respond(200, json=spark_body())
    meta.get("/files/file-1").respond(200, json={"id": "file-1"})

    # Meta answered, so the answer goes back, but its charge is not written.
    assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 200
    assert ledger(conn) == []

    refused = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)
    assert refused.status_code == 503
    assert refused.json() == {
        "error": "charging_unavailable",
        "message": "The relay can't record charges right now. Try again in a minute.",
    }
    assert route.call_count == 1
    # Free calls still work.
    assert client.get("/meta/v1/files/file-1", headers=device.headers).status_code == 200

    broken["on"] = False
    assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 200
    # The held charge is written first, then the new one.
    assert [row["cost_usd"] for row in ledger(conn, device)] == [pytest.approx(SPARK_COST)] * 2
    assert_ledger_matches_spend(conn, device)


def test_admission_tries_a_held_charge_once_and_quickly(client, conn, meta, settings, monkeypatch):
    import sqlite3

    # Slow enough that three full attempts with pauses would take seconds.
    monkeypatch.setattr(db, "_BUSY_TIMEOUT_S", 2.0)
    device = add_device(conn)
    meta.post("/responses").respond(200, json=spark_body())
    held = proxy.Charge(device.device_id, "responses", "muse-spark-1.3", dict(SPARK_USAGE), SPARK_COST)
    client.app.state.in_flight.unwritten.append(held)
    writer = sqlite3.connect(settings.db_path, isolation_level=None)
    writer.execute("BEGIN IMMEDIATE")
    try:
        started = time.monotonic()
        refused = client.post("/meta/v1/responses", json=spark_request(), headers=device.headers)
        elapsed = time.monotonic() - started
    finally:
        writer.execute("ROLLBACK")
        writer.close()

    assert refused.status_code == 503
    assert refused.json()["error"] == "charging_unavailable"
    assert elapsed < 2.0
    # Once the database takes writes again, the held charge goes in first.
    assert client.post("/meta/v1/responses", json=spark_request(), headers=device.headers).status_code == 200
    assert [row["cost_usd"] for row in ledger(conn, device)] == [pytest.approx(SPARK_COST)] * 2


# Secrets.


def test_key_never_appears_in_responses_or_logs(client, conn, meta, settings, caplog):
    caplog.set_level(logging.DEBUG)
    key = settings.meta_api_key
    device = add_device(conn)
    meta.post("/responses").mock(
        side_effect=[
            httpx.Response(200, json=spark_body()),
            httpx.Response(400, json={"error": {"message": f"Bad request for key {key}"}}),
            httpx.Response(401, json={"error": {"message": f"Incorrect API key provided: {key}"}}),
            httpx.Response(403, text=f"forbidden {key}"),
        ]
    )
    meta.post("/asr/transcribe").respond(200, json={"audioDurationMs": 1000})

    responses = [
        client.post("/meta/v1/responses", json=spark_request(), headers=device.headers) for _ in range(4)
    ]
    responses.append(client.post("/meta/v1/asr/transcribe", files=transcribe_files()[0], headers=device.headers))

    assert [response.status_code for response in responses] == [200, 400, 502, 403, 200]
    assert responses[1].json()["message"] == "Bad request for key [redacted]"
    for response in responses:
        assert key not in response.text
        assert key not in str(response.headers)
    assert key not in caplog.text
    for record in caplog.records:
        assert key not in record.getMessage()


# Streaming.


class MetaStream(httpx.AsyncByteStream):
    """Meta's side of a streamed response. It yields its chunks, can wait on a
    gate before chunk number `gate_at` (then pause `pause` seconds and run
    `after_gate`), then ends, hangs or breaks. It records how many chunks the relay read and
    whether the relay closed it."""

    def __init__(self, chunks, *, then=None, gate=None, gate_at=1, pause=0, after_gate=None, close_error=None):
        self.chunks = chunks
        self.then = then
        self.gate = gate
        self.gate_at = gate_at
        self.pause = pause
        self.after_gate = after_gate
        self.close_error = close_error
        self.served = 0
        self.closed = False

    async def __aiter__(self):
        for chunk in self.chunks:
            if self.served == self.gate_at and self.gate is not None:
                await self.gate.wait()
                await anyio.sleep(self.pause)
                if self.after_gate is not None:
                    self.after_gate()
            self.served += 1
            yield chunk
        if self.then == "hang":
            await anyio.sleep_forever()
        elif isinstance(self.then, BaseException):
            raise self.then

    async def aclose(self):
        self.closed = True
        if self.close_error is not None:
            raise self.close_error


def sse(*events, crlf=False):
    end = "\r\n" if crlf else "\n"
    return "".join(f"event: {e['type']}{end}data: {json.dumps(e)}{end}{end}" for e in events).encode()


CREATED = {"type": "response.created", "sequence_number": 0, "response": {"id": "resp_1", "status": "in_progress", "usage": None}}
DELTA = {"type": "response.output_text.delta", "sequence_number": 1, "delta": "Hi"}


def completed(usage=SPARK_USAGE):
    return {"type": "response.completed", "sequence_number": 2, "response": spark_body(usage)}


def stream_from_meta(meta, stream):
    return meta.post("/responses").mock(
        return_value=httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=stream)
    )


def test_streamed_response_is_metered_from_final_event(client, conn, meta):
    device = add_device(conn)
    raw = sse(CREATED, DELTA, crlf=True) + sse(completed()) + b"data: [DONE]\n\n"
    # Chunk edges fall inside lines and inside the final event's JSON.
    chunks = [raw[i : i + 37] for i in range(0, len(raw), 37)]
    stream_from_meta(meta, MetaStream(chunks))

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.status_code == 200
    assert response.headers["content-type"] == "text/event-stream"
    assert response.content == raw
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"]) == SPARK_USAGE
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert_ledger_matches_spend(conn, device)
    assert client.app.state.in_flight.count() == 0


def test_event_data_split_over_several_lines_is_read(client, conn, meta):
    device = add_device(conn)
    body = json.dumps(completed(), indent=1).splitlines()
    raw = sse(CREATED) + "".join(f"data: {line}\n" for line in body).encode() + b"\n"
    stream_from_meta(meta, MetaStream([raw]))

    client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)


def test_last_event_without_a_closing_blank_line_is_still_read(client, conn, meta):
    device = add_device(conn)
    stream_from_meta(meta, MetaStream([sse(CREATED), f"data: {json.dumps(completed())}".encode()]))

    client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)


def test_streamed_sam_call_is_charged_from_frames(sam_client, conn, meta):
    device = add_device(conn)
    done = {"type": "response.completed", "response": {"status": "completed", "usage": {"video_frames_processed": 160}}}
    stream_from_meta(meta, MetaStream([sse(CREATED), b'data: {"type":"response.output_text.delta","delta":"<0f>"}\n\n', sse(done)]))
    body = {"model": "sam-3.1", "stream": True, "input": [{"role": "user", "content": [{"type": "input_text", "text": "person"}, {"type": "input_video", "file_id": "file-1"}]}]}

    response = sam_client.post("/meta/v1/responses", json=body, headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert row["model"] == "sam-3.1"
    assert row["cost_usd"] == pytest.approx(0.032)


def test_stream_that_ends_without_usage_charges_the_reservation(client, conn, meta):
    device = add_device(conn)
    route = stream_from_meta(meta, MetaStream([sse(CREATED, DELTA)]))

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    sent = route.calls.last.request.content
    assert json.loads(row["usage_json"]) == reserved_usage(sent)
    assert row["cost_usd"] == pytest.approx(reserved_cost(sent))


@pytest.mark.parametrize(("seen", "cost"), [(BIG_USAGE, BIG_COST), (SPARK_USAGE, None)], ids=["seen-is-higher", "reservation-is-higher"])
def test_stream_without_a_final_event_charges_at_least_the_reservation(client, conn, meta, seen, cost):
    device = add_device(conn)
    in_progress = {"type": "response.in_progress", "response": {"status": "in_progress", "usage": seen}}
    route = stream_from_meta(meta, MetaStream([sse(CREATED, in_progress, DELTA)]))

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    expected = cost if cost is not None else reserved_cost(route.calls.last.request.content)
    assert row["cost_usd"] == pytest.approx(expected)


@pytest.mark.parametrize("final_type", ["response.completed", "response.incomplete", "response.failed"])
def test_any_final_event_settles_the_charge(client, conn, meta, final_type):
    device = add_device(conn)
    final = {"type": final_type, "response": {"status": final_type.split(".")[1], "usage": SPARK_USAGE}}
    stream_from_meta(meta, MetaStream([sse(CREATED, final)]))

    client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)


def test_huge_final_event_without_an_event_line_settles_the_charge(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "FULL_PARSE_BYTES", 2000)
    monkeypatch.setattr(proxy, "USAGE_TAIL_BYTES", 500)
    device = add_device(conn)
    final = completed()
    final["response"]["output"][0]["content"][0]["text"] = "x" * 3000
    # Data lines only: the event's type is read from the start of its JSON.
    raw = f"data: {json.dumps(CREATED)}\n\ndata: {json.dumps(final)}\n\n".encode()
    stream_from_meta(meta, MetaStream([raw[i : i + 300] for i in range(0, len(raw), 300)]))

    client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)


def test_every_usage_in_a_huge_final_event_is_priced_and_the_highest_charged(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "FULL_PARSE_BYTES", 2000)
    monkeypatch.setattr(proxy, "USAGE_TAIL_BYTES", 500)
    device = add_device(conn)
    final = completed()
    final["response"]["output"][0]["content"][0]["text"] = "x" * 3000
    # The response's own usage comes last, after a larger one.
    del final["response"]["usage"]
    final["response"]["extra"] = {"usage": BIG_USAGE}
    final["response"]["usage"] = SPARK_USAGE
    stream_from_meta(meta, MetaStream([sse(CREATED, final)]))

    client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(BIG_COST)


def test_stream_broken_off_by_meta_still_charges(client, conn, meta):
    device = add_device(conn)
    upstream = MetaStream([sse(CREATED), sse(DELTA)], then=httpx.ReadError("connection reset"))
    stream_from_meta(meta, upstream)

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.content == sse(CREATED) + sse(DELTA)
    assert upstream.closed
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"])["reserved"] is True
    assert client.app.state.in_flight.count() == 0


def test_huge_final_event_has_its_usage_found_in_its_tail(client, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "FULL_PARSE_BYTES", 2000)
    monkeypatch.setattr(proxy, "USAGE_TAIL_BYTES", 500)
    device = add_device(conn)
    final = completed()
    final["response"]["output"][0]["content"][0]["text"] = '"usage": {"input_tokens": 0} ' * 500
    raw = sse(CREATED, final)
    stream_from_meta(meta, MetaStream([raw[i : i + 300] for i in range(0, len(raw), 300)]))

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.content == raw
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)


def test_stream_event_that_breaks_the_parser_charges_the_reservation(client, conn, meta):
    device = add_device(conn)
    final = '{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1},"output":' + DEEP + "}}"
    route = stream_from_meta(meta, MetaStream([sse(CREATED), f"data: {final}\n\n".encode()]))

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.status_code == 200
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content))


def test_meta_error_on_a_streamed_call_passes_through_and_charges_nothing(client, conn, meta):
    device = add_device(conn)
    meta.post("/responses").respond(503, json={"error": {"message": "The backend is temporarily overloaded. Please retry."}})

    response = client.post("/meta/v1/responses", json=spark_request(stream=True), headers=device.headers)

    assert response.status_code == 503
    assert response.json()["error"] == "meta_error"
    assert ledger(conn) == []


@pytest.fixture
def anyio_backend():
    return "asyncio"


@pytest.fixture
async def app(settings):
    application = create_app(settings)
    async with application.router.lifespan_context(application):
        yield application


@pytest.fixture
async def sam_app(settings):
    application = create_app(with_sam(settings))
    async with application.router.lifespan_context(application):
        yield application


class AsgiCall:
    """One request driven straight through the ASGI app, so a test can watch
    each chunk arrive and hang up whenever it likes. Once the client has hung
    up, sends are dropped, as uvicorn does."""

    def __init__(self, app, path, body, headers, query_string=b"", stall_after=None):
        self.app = app
        self.path = path
        self.query_string = query_string
        self.body = body
        # Send only this many body bytes, then go quiet.
        self.stall_after = stall_after
        self.headers = {"content-type": "application/json", **headers, "content-length": str(len(body))}
        self.status = None
        self.chunks = []
        self.hang_up = anyio.Event()
        self.on_chunk = None

    async def run(self):
        body_sent = False

        async def receive():
            nonlocal body_sent
            if not body_sent:
                body_sent = True
                if self.stall_after is not None:
                    return {"type": "http.request", "body": self.body[: self.stall_after], "more_body": True}
                return {"type": "http.request", "body": self.body, "more_body": False}
            if self.stall_after is not None:
                await anyio.sleep_forever()
            await self.hang_up.wait()
            return {"type": "http.disconnect"}

        async def send(message):
            if self.hang_up.is_set():
                return
            if message["type"] == "http.response.start":
                self.status = message["status"]
            elif message["type"] == "http.response.body" and message.get("body"):
                self.chunks.append(message["body"])
                if self.on_chunk is not None:
                    await self.on_chunk(len(self.chunks))

        scope = {
            "type": "http",
            "asgi": {"version": "3.0", "spec_version": "2.3"},
            "http_version": "1.1",
            "method": "POST",
            "scheme": "http",
            "path": self.path,
            "raw_path": self.path.encode(),
            "root_path": "",
            "query_string": self.query_string,
            "headers": [(k.lower().encode(), v.encode()) for k, v in self.headers.items()],
            "client": ("127.0.0.1", 50000),
            "server": ("relay", 80),
        }
        with anyio.fail_after(5):
            await self.app(scope, receive, send)


# How long Meta stays silent after a client hangs up: long enough for the
# relay to notice the hang-up before Meta goes on.
MOMENT = 0.05


def hang_up_after(call, chunks):
    async def on_chunk(count):
        if count == chunks:
            call.hang_up.set()

    return on_chunk


def streamed(body):
    return json.dumps({**body, "stream": True}).encode()


VIDEO_REQUEST = spark_request(input=[{"role": "user", "content": [{"type": "input_text", "text": "moments"}, VIDEO_BY_FILE]}])
SAM_REQUEST = {"model": "sam-3.1", "input": [{"role": "user", "content": [{"type": "input_text", "text": "person"}, VIDEO_BY_FILE]}]}


@pytest.mark.anyio
async def test_stream_passes_bytes_through_as_they_arrive(app, conn, meta):
    device = add_device(conn)
    first_chunk_seen = anyio.Event()
    # Meta sends its second chunk only after the client has the first.
    upstream = MetaStream([sse(CREATED), sse(DELTA), sse(completed())], gate=first_chunk_seen)
    stream_from_meta(meta, upstream)
    call = AsgiCall(app, "/meta/v1/responses", streamed(spark_request()), device.headers)

    async def on_chunk(count):
        first_chunk_seen.set()

    call.on_chunk = on_chunk
    await call.run()

    assert b"".join(call.chunks) == sse(CREATED, DELTA, completed())
    assert ledger(conn, device)[0]["cost_usd"] == pytest.approx(SPARK_COST)


@pytest.mark.anyio
async def test_client_disconnect_mid_stream_drains_upstream_and_charges_final_usage(app, conn, meta):
    device = add_device(conn)
    partial = {"input_tokens": 10000, "output_tokens": 1}
    in_progress = {"type": "response.in_progress", "sequence_number": 1, "response": {"status": "in_progress", "usage": partial}}
    late_delta = {"type": "response.output_text.delta", "sequence_number": 3, "delta": " there"}
    call = AsgiCall(app, "/meta/v1/responses", streamed(spark_request()), device.headers)
    slots_while_draining = []
    # The client hangs up after three chunks, while Meta is silent; Meta then
    # carries on and finishes.
    upstream = MetaStream(
        [sse(CREATED), sse(in_progress), sse(DELTA), sse(late_delta), sse(completed())],
        gate=call.hang_up,
        gate_at=3,
        pause=MOMENT,
        after_gate=lambda: slots_while_draining.append(app.state.in_flight.count(device.device_id)),
    )
    stream_from_meta(meta, upstream)
    call.on_chunk = hang_up_after(call, 3)

    await call.run()

    # Nothing reached the client after it left, but Meta's stream was read to
    # its end, with the device's slot held all along.
    assert b"".join(call.chunks) == sse(CREATED, in_progress, DELTA)
    assert upstream.served == 5
    assert upstream.closed
    assert slots_while_draining == [1]
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"]) == SPARK_USAGE
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert_ledger_matches_spend(conn, device)
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
async def test_abandoned_streams_hold_their_slots_until_drained(app, conn, meta):
    device = add_device(conn)
    meta_done = anyio.Event()
    streams = []

    def answer(request):
        stream = MetaStream([sse(CREATED), sse(completed())], gate=meta_done)
        streams.append(stream)
        return httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=stream)

    meta.post("/responses").mock(side_effect=answer)
    body = streamed(spark_request())
    calls = [AsgiCall(app, "/meta/v1/responses", body, device.headers) for _ in range(4)]
    for call in calls:
        call.on_chunk = hang_up_after(call, 1)

    async with anyio.create_task_group() as group:
        for call in calls:
            group.start_soon(call.run)
        for call in calls:
            await call.hang_up.wait()
        # All four clients are gone but Meta has not finished, so a fifth call
        # from the device finds no free slot.
        fifth = AsgiCall(app, "/meta/v1/responses", body, device.headers)
        await fifth.run()
        meta_done.set()

    assert fifth.status == 429
    assert json.loads(b"".join(fifth.chunks))["error"] == "busy"
    assert [stream.served for stream in streams] == [2, 2, 2, 2]
    rows = ledger(conn, device)
    assert [row["cost_usd"] for row in rows] == [pytest.approx(SPARK_COST)] * 4
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
async def test_drain_that_fails_charges_the_reservation(app, conn, meta):
    device = add_device(conn)
    call = AsgiCall(app, "/meta/v1/responses", streamed(spark_request()), device.headers)
    upstream = MetaStream(
        [sse(CREATED), sse(DELTA), sse(DELTA)],
        gate=call.hang_up,
        gate_at=2,
        pause=MOMENT,
        then=httpx.ReadTimeout("Meta went quiet"),
    )
    route = stream_from_meta(meta, upstream)
    call.on_chunk = hang_up_after(call, 2)

    await call.run()

    assert upstream.served == 3
    assert upstream.closed
    [row] = ledger(conn, device)
    sent = route.calls.last.request.content
    assert json.loads(row["usage_json"]) == reserved_usage(sent)
    assert row["cost_usd"] == pytest.approx(reserved_cost(sent))
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
@pytest.mark.parametrize("body", [VIDEO_REQUEST, SAM_REQUEST], ids=["spark-video", "sam"])
async def test_drain_that_fails_for_media_charges_the_media_reservation(sam_app, conn, meta, body):
    app = sam_app
    device = add_device(conn)
    call = AsgiCall(app, "/meta/v1/responses", streamed(body), device.headers)
    upstream = MetaStream(
        [sse(CREATED), sse(DELTA), sse(DELTA)],
        gate=call.hang_up,
        gate_at=2,
        pause=MOMENT,
        then=httpx.RemoteProtocolError("peer closed connection"),
    )
    route = stream_from_meta(meta, upstream)
    call.on_chunk = hang_up_after(call, 2)

    await call.run()

    assert upstream.served == 3
    [row] = ledger(conn, device)
    usage = json.loads(row["usage_json"])
    assert usage["reserved"] is True
    assert usage["media_usd"] == 0.75
    if body["model"] == "sam-3.1":
        assert row["cost_usd"] == pytest.approx(0.75 + 50 * 0.20 / 1000)
    else:
        assert row["cost_usd"] == pytest.approx(reserved_cost(route.calls.last.request.content, media=True))
    assert_ledger_matches_spend(conn, device)


@pytest.mark.anyio
@pytest.mark.parametrize(
    ("seen", "cost"),
    [({"input_tokens": 1_000_000, "output_tokens": 0}, 1.25), ({"input_tokens": 10, "output_tokens": 0}, None)],
    ids=["usage-seen-is-higher", "reservation-is-higher"],
)
async def test_drain_failure_charges_the_higher_of_usage_seen_and_the_reservation(app, conn, meta, seen, cost):
    device = add_device(conn)
    in_progress = {"type": "response.in_progress", "response": {"status": "in_progress", "usage": seen}}
    call = AsgiCall(app, "/meta/v1/responses", streamed(VIDEO_REQUEST), device.headers)
    upstream = MetaStream(
        [sse(CREATED), sse(in_progress), sse(DELTA)],
        gate=call.hang_up,
        gate_at=2,
        pause=MOMENT,
        then=httpx.ReadError("reset"),
    )
    route = stream_from_meta(meta, upstream)
    call.on_chunk = hang_up_after(call, 2)

    await call.run()

    assert upstream.served == 3
    [row] = ledger(conn, device)
    if cost is not None:
        assert row["cost_usd"] == pytest.approx(cost)
        assert json.loads(row["usage_json"]) == seen
    else:
        sent = route.calls.last.request.content
        assert row["cost_usd"] == pytest.approx(reserved_cost(sent, media=True))
        assert json.loads(row["usage_json"]) == reserved_usage(sent, media=True)


@pytest.mark.anyio
async def test_send_failure_counts_as_a_disconnect_and_the_stream_is_drained(app, conn, meta):
    device = add_device(conn)
    upstream = MetaStream([sse(CREATED), sse(DELTA), sse(completed())])
    stream_from_meta(meta, upstream)
    call = AsgiCall(app, "/meta/v1/responses", streamed(spark_request()), device.headers)

    async def on_chunk(count):
        if count == 2:
            raise OSError("client went away")

    call.on_chunk = on_chunk
    await call.run()

    assert len(call.chunks) == 2
    assert upstream.served == 3
    assert upstream.closed
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
@pytest.mark.parametrize("body", [spark_request(), VIDEO_REQUEST], ids=["text", "video"])
async def test_stream_is_charged_even_when_closing_meta_fails(app, conn, meta, body):
    device = add_device(conn)
    upstream = MetaStream([sse(CREATED), sse(completed())], close_error=OSError("socket already gone"))
    stream_from_meta(meta, upstream)
    call = AsgiCall(app, "/meta/v1/responses", streamed(body), device.headers)

    await call.run()

    assert upstream.closed
    # The stream finished, so the real usage stands, below the reservation.
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
async def test_client_disconnect_during_a_non_streamed_call_still_charges_real_usage(app, conn, meta):
    device = add_device(conn)
    call = AsgiCall(app, "/meta/v1/responses", json.dumps(spark_request()).encode(), device.headers)
    answered = []

    async def answer_after_the_client_left(request):
        # The client gives up while Meta is still working.
        call.hang_up.set()
        await anyio.sleep(MOMENT)
        answered.append(True)
        return httpx.Response(200, json=spark_body())

    meta.post("/responses").mock(side_effect=answer_after_the_client_left)

    await call.run()

    # The relay waited for Meta, charged the real usage, and sent nothing.
    assert answered == [True]
    assert call.status is None
    [row] = ledger(conn, device)
    assert json.loads(row["usage_json"]) == SPARK_USAGE
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
async def test_raw_non_ascii_query_is_400(app, conn, meta):
    device = add_device(conn)
    call = AsgiCall(app, "/meta/v1/responses", json.dumps(spark_request()).encode(), device.headers, query_string="x=é".encode())

    await call.run()

    assert call.status == 400
    assert json.loads(b"".join(call.chunks))["error"] == "invalid_request"
    assert not meta.calls


@pytest.mark.anyio
@pytest.mark.parametrize(
    ("path", "content_type", "deadline"),
    [
        ("/meta/v1/responses", "application/json", "JSON_BODY_DEADLINE_S"),
        ("/meta/v1/asr/transcribe", "multipart/form-data; boundary=b", "JSON_BODY_DEADLINE_S"),
        ("/meta/v1/files", "multipart/form-data; boundary=b", "UPLOAD_DEADLINE_S"),
    ],
    ids=["responses", "transcribe", "upload"],
)
async def test_body_that_arrives_too_slowly_is_408(app, conn, meta, monkeypatch, path, content_type, deadline):
    monkeypatch.setattr(proxy, deadline, 0.2)
    device = add_device(conn)
    meta.post("/files").respond(200, json={"id": "file-1"})
    call = AsgiCall(app, path, b"x" * 1000, {**device.headers, "content-type": content_type}, stall_after=10)

    await call.run()

    assert call.status == 408
    assert json.loads(b"".join(call.chunks)) == {
        "error": "request_timeout",
        "message": "The request took too long to arrive. Try again.",
    }
    assert ledger(conn) == []
    assert app.state.in_flight.count() == 0


@pytest.mark.anyio
async def test_a_client_that_stops_reading_is_dropped_and_the_stream_still_drained(app, conn, meta, monkeypatch):
    monkeypatch.setattr(proxy, "SEND_TIMEOUT_S", 0.2)
    device = add_device(conn)
    upstream = MetaStream([sse(CREATED), sse(DELTA), sse(DELTA), sse(completed())])
    stream_from_meta(meta, upstream)
    call = AsgiCall(app, "/meta/v1/responses", streamed(spark_request()), device.headers)

    async def on_chunk(count):
        if count == 2:
            # The client's receive window is full: this send never finishes.
            await anyio.sleep_forever()

    call.on_chunk = on_chunk
    await call.run()

    assert len(call.chunks) == 2
    assert upstream.served == 4
    assert upstream.closed
    [row] = ledger(conn, device)
    assert row["cost_usd"] == pytest.approx(SPARK_COST)
    assert app.state.in_flight.count() == 0
    assert app.state.in_flight.reserved(device.device_id) == (0.0, 0.0)
