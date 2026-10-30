def test_healthz_reports_ok(client):
    response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"ok": True}


def test_unknown_route_uses_error_shape(client):
    response = client.get("/no-such-route")

    assert response.status_code == 404
    assert response.json() == {"error": "not_found", "message": "Not found."}
