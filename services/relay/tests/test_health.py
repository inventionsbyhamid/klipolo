import io
import logging
import sys

from fastapi.testclient import TestClient

from relay.app import create_app


def test_healthz_reports_ok(client):
    response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"ok": True}


def test_unknown_route_uses_error_shape(client):
    response = client.get("/no-such-route")

    assert response.status_code == 404
    assert response.json() == {"error": "not_found", "message": "Not found."}


def test_startup_creates_the_database(settings):
    assert not settings.db_path.exists()

    with TestClient(create_app(settings)):
        assert settings.db_path.exists()


def test_unexpected_error_is_500_with_error_shape(settings):
    app = create_app(settings)

    @app.get("/boom")
    def boom():
        raise RuntimeError("internal detail that must not leak")

    with TestClient(app, raise_server_exceptions=False) as client:
        response = client.get("/boom")

    assert response.status_code == 500
    assert response.json() == {
        "error": "server_error",
        "message": "Something went wrong. Try again.",
    }
    assert "internal detail" not in response.text


def test_relay_info_lines_go_to_stderr_once_when_nothing_else_set_up_logging(settings, monkeypatch):
    # As under uvicorn: the root logger has no handlers. The relay's INFO
    # lines go out once each, however often the app is made; other
    # libraries' INFO lines (httpx logs every Meta URL) stay off.
    root, relay_logger = logging.getLogger(), logging.getLogger("relay")
    saved = root.handlers[:]
    for handler in saved:
        root.removeHandler(handler)
    stream = io.StringIO()
    monkeypatch.setattr(sys, "stderr", stream)
    try:
        create_app(settings)
        create_app(settings)
        logging.getLogger("relay.proxy").info("Charged device abcd1234 $0.010000 for responses muse-spark-1.3")
        logging.getLogger("httpx").info("HTTP Request: POST https://api.meta.ai/v1/responses")
    finally:
        for handler in relay_logger.handlers[:]:
            relay_logger.removeHandler(handler)
        for handler in saved:
            root.addHandler(handler)

    lines = stream.getvalue().splitlines()
    assert len(lines) == 1
    assert lines[0].endswith("INFO relay.proxy: Charged device abcd1234 $0.010000 for responses muse-spark-1.3")
