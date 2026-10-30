"""ASGI app factory. Run with: uvicorn --factory relay.app:create_app"""

import asyncio
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI

from relay import align, db, devices, proxy
from relay.config import Settings, load_settings
from relay.errors import install_error_handlers


def create_app(settings: Settings | None = None) -> FastAPI:
    settings = settings or load_settings()

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        # Create the database and schema at boot so a bad volume fails fast.
        db.connect(settings.db_path).close()
        # Alignments run in worker threads, at most ALIGN_CONCURRENCY at once.
        app.state.align_gate = asyncio.Semaphore(settings.align_concurrency)
        if settings.align_warmup:
            align.warm_up(app.state.aligner)
        async with proxy.meta_client() as client:
            app.state.meta_client = client
            yield

    app = FastAPI(
        title="Klipolo relay",
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
        lifespan=lifespan,
    )
    app.state.settings = settings
    app.state.in_flight = proxy.InFlight()
    app.state.aligner = align.MmsAligner(settings.align_threads)
    app.state.align_in_flight = proxy.InFlight(
        per_device=align.MAX_REQUESTS_PER_DEVICE,
        total=align.REQUESTS_PER_WORKER * settings.align_concurrency,
    )
    install_error_handlers(app)
    app.include_router(devices.router)
    app.include_router(proxy.router)
    app.include_router(align.router)

    @app.get("/healthz")
    def healthz() -> dict[str, bool]:
        return {"ok": True}

    return app
