"""ASGI app factory. Run with: uvicorn --factory relay.app:create_app"""

from fastapi import FastAPI

from relay.config import Settings, load_settings
from relay.errors import install_error_handlers


def create_app(settings: Settings | None = None) -> FastAPI:
    settings = settings or load_settings()
    app = FastAPI(title="Klipolo relay", docs_url=None, redoc_url=None, openapi_url=None)
    app.state.settings = settings
    install_error_handlers(app)

    @app.get("/healthz")
    def healthz() -> dict[str, bool]:
        return {"ok": True}

    return app
