import pytest
from fastapi.testclient import TestClient

from relay.app import create_app
from relay.config import load_settings

FAKE_META_KEY = "test-meta-key-not-real"


@pytest.fixture
def settings(tmp_path):
    return load_settings(
        {
            "RELAY_DB_PATH": str(tmp_path / "data" / "relay.db"),
            "META_API_KEY": FAKE_META_KEY,
        }
    )


@pytest.fixture
def client(settings):
    with TestClient(create_app(settings)) as test_client:
        yield test_client
