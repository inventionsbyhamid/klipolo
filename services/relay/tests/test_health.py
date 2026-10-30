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
