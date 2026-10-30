"""Metered proxy to Meta's Model API.

`/meta/v1/{path}` forwards to `https://api.meta.ai/v1/{path}` with the relay's
own key, for an allowlist of endpoints and models:

    POST   responses        JSON; Spark 1.3/1.2 or SAM 3.1
    POST   files            multipart upload, streamed, at most 1 GiB
    GET    files/{id}
    DELETE files/{id}
    POST   asr/transcribe   multipart `request` (JSON) and `audio` parts

Anything else is 404. Every call is gated per request: a known device token
(401), relay AI for the device's code right now (403), a configured key (503).
Billable calls and uploads are then refused with 402 once the device or the
whole relay has reached its cap. After Meta answers, the call is charged from
Meta's own `usage`: the ledger row and the device's `spent_usd` are written
in one transaction. Failed calls (non-2xx) are not charged.

A client hanging up never makes a call cheaper than finishing it. A
non-streamed call still waits for Meta's answer and charges its usage. A
streamed call is read to its end, the bytes dropped, and charged from its
final event, with the device's in-flight slot held until then. Only when
Meta's side fails after the request reached it (a timeout, a broken stream)
is the call charged an estimate: the higher of any usage seen and the request
estimate, which is at least $0.10 for calls with media (video, files or
earlier context by reference, or SAM).

Nothing from a proxied request is stored except the ledger row, and request
or response headers and bodies are never logged.
"""

import json
import logging
import math
import re
import sqlite3
import time
from collections.abc import AsyncIterator, Awaitable, Callable, Iterator, Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Annotated, Any

import anyio
import httpx
from fastapi import APIRouter, Depends, Request, Response
from python_multipart.exceptions import MultipartParseError
from python_multipart.multipart import MultipartParser, MultipartState, parse_options_header
from starlette.concurrency import run_in_threadpool
from starlette.requests import ClientDisconnect

from relay import db
from relay.config import Settings
from relay.deps import AppSettings, Conn
from relay.devices import Device, has_relay_ai
from relay.errors import RelayError, error_response
from relay.pricing import MODELS_BY_ENDPOINT, SAM_IMAGES_KEY, SAM_MODEL, cost_usd, estimate_usage

logger = logging.getLogger(__name__)

router = APIRouter()

META_BASE_URL = "https://api.meta.ai/v1"
# Video calls take tens of seconds before the first byte.
UPSTREAM_TIMEOUT = httpx.Timeout(connect=10.0, read=300.0, write=300.0, pool=10.0)

# Inline media is at most 50 MB, which is about 67 MB as base64.
MAX_JSON_BYTES = 72 * 1024 * 1024
MAX_UPLOAD_BYTES = 1024**3
# Multipart framing and the small form fields around an upload.
UPLOAD_OVERHEAD_BYTES = 64 * 1024
MAX_AUDIO_BYTES = 32 * 1024 * 1024
MAX_TRANSCRIBE_SPEC_BYTES = 64 * 1024
MAX_ERROR_BYTES = 64 * 1024
# Larger bodies are not parsed whole: `usage` follows the output, so it is
# looked for in the last USAGE_TAIL_BYTES instead.
FULL_PARSE_BYTES = 8 * 1024 * 1024
USAGE_TAIL_BYTES = 64 * 1024

MAX_IN_FLIGHT_PER_DEVICE = 4
MAX_IN_FLIGHT_TOTAL = 32

# Request headers worth passing on. The client's Authorization, cookies, any
# other credential and all hop-by-hop or forwarding headers stay here.
_REQUEST_HEADERS = ("accept", "idempotency-key")
_RESPONSE_HEADERS = ("content-type", "retry-after", "x-request-id")

_FILE_ID = re.compile(r"[A-Za-z0-9_-]{1,128}")
_SAFE_FILENAME = re.compile(r"[\w. -]{1,100}")
_SAFE_MEDIA_TYPE = re.compile(r"[\w.+-]+/[\w.+-]+")
_USAGE_KEY = re.compile(r'"usage"\s*:\s*(?=\{)')
_DURATION_KEY = re.compile(r'"audioDurationMs"\s*:\s*')

BUDGET_MESSAGE = "This device has used its Klipolo credit. Add your own Meta key in Settings."


def meta_client() -> httpx.AsyncClient:
    # trust_env=False: no proxy variables and no .netrc credentials.
    return httpx.AsyncClient(
        timeout=UPSTREAM_TIMEOUT,
        limits=httpx.Limits(max_connections=64, max_keepalive_connections=16),
        follow_redirects=False,
        trust_env=False,
    )


class InFlight:
    """Counts proxied calls in progress, per device and in total.

    Caps are checked before a call and charged after it, so each call in
    flight can carry spend past a cap by its own cost. Bounding the calls in
    flight bounds that overshoot. Counts live in this process and are only
    touched from the event loop: run the server with a single worker.
    """

    def __init__(self, per_device: int = MAX_IN_FLIGHT_PER_DEVICE, total: int = MAX_IN_FLIGHT_TOTAL) -> None:
        self.per_device = per_device
        self.total = total
        self._by_device: dict[str, int] = {}
        self._count = 0

    def acquire(self, device_id: str) -> "Slot":
        if self._by_device.get(device_id, 0) >= self.per_device:
            raise RelayError(
                429,
                "busy",
                "Too many requests at once from this device. Wait for one to finish.",
                headers={"Retry-After": "1"},
            )
        if self._count >= self.total:
            raise RelayError(429, "busy", "The relay is busy. Try again in a moment.", headers={"Retry-After": "1"})
        self._by_device[device_id] = self._by_device.get(device_id, 0) + 1
        self._count += 1
        return Slot(self, device_id)

    def count(self, device_id: str | None = None) -> int:
        return self._count if device_id is None else self._by_device.get(device_id, 0)

    def _release(self, device_id: str) -> None:
        left = self._by_device[device_id] - 1
        if left:
            self._by_device[device_id] = left
        else:
            del self._by_device[device_id]
        self._count -= 1


class Slot:
    def __init__(self, owner: InFlight, device_id: str) -> None:
        self._owner = owner
        self._device_id = device_id
        self._released = False

    def release(self) -> None:
        if not self._released:
            self._released = True
            self._owner._release(self._device_id)


def relay_device(device: Device, conn: Conn, settings: AppSettings) -> sqlite3.Row:
    """Gate for every proxied call, checked on each request so a deactivated
    code stops at once: a known token (401), relay AI (403), a key (503)."""
    if not has_relay_ai(conn, device):
        raise RelayError(
            403,
            "no_relay_ai",
            "Klipolo AI is off for this device. Add an access code or your own Meta key in Settings.",
        )
    if not settings.meta_api_key:
        raise RelayError(503, "not_configured", "Relay not configured.")
    return device


RelayDevice = Annotated[sqlite3.Row, Depends(relay_device)]


def check_budget(conn: sqlite3.Connection, device_id: str, settings: Settings) -> None:
    """402 once the device or the whole relay has reached its cap. The global
    cap is all-time: Meta's credits are granted once."""
    row = conn.execute("SELECT spent_usd FROM devices WHERE device_id = ?", (device_id,)).fetchone()
    total = conn.execute("SELECT COALESCE(SUM(cost_usd), 0) FROM ledger").fetchone()[0]
    if row is None or row["spent_usd"] >= settings.device_cap_usd or total >= settings.global_cap_usd:
        raise RelayError(402, "budget", BUDGET_MESSAGE)


def record_charge(
    db_path: Path, device_id: str, endpoint: str, model: str, usage: Mapping[str, Any], cost: float
) -> None:
    """Write the ledger row and add the cost to the device in one transaction,
    so the ledger (what the admin sees) and spent_usd (what the cap reads)
    never drift. Uses its own connection: a streamed response outlives the
    request's."""
    row = (
        device_id,
        datetime.now(UTC).isoformat(timespec="seconds"),
        endpoint,
        model,
        json.dumps(usage, sort_keys=True),
        cost,
    )
    for attempt in range(3):
        try:
            conn = db.connect(db_path)
            try:
                with db.transaction(conn):
                    conn.execute(
                        "INSERT INTO ledger (device_id, at, endpoint, model, usage_json, cost_usd)"
                        " VALUES (?, ?, ?, ?, ?, ?)",
                        row,
                    )
                    conn.execute(
                        "UPDATE devices SET spent_usd = spent_usd + ? WHERE device_id = ?", (cost, device_id)
                    )
            finally:
                conn.close()
            return
        except sqlite3.OperationalError:
            if attempt == 2:
                raise
            time.sleep(0.5 * (attempt + 1))


@dataclass
class Meter:
    """What a billable call needs to be priced, from its usage or, when the
    usage is never seen, from an estimate of the request."""

    endpoint: str
    model: str
    request_bytes: int
    audio_bytes: int = 0
    images: int = 0
    has_video: bool = False
    # Video, files or earlier context by reference, or SAM: the request size
    # says little about the cost, so an estimate gets a floor.
    media: bool = False

    def price(self, usage: Any, *, finished: bool = True) -> tuple[dict[str, Any], float]:
        """The usage to record and its cost. `finished` is False when Meta's
        answer was cut off, so `usage` (if any) may not be the final count:
        the call then costs the higher of that usage and the estimate."""
        seen = None
        if isinstance(usage, Mapping):
            usage = dict(usage)
            if self.model == SAM_MODEL:
                usage[SAM_IMAGES_KEY] = self.images
            try:
                seen = _numbers_only(usage), cost_usd(self.endpoint, self.model, usage)
            except ValueError:
                logger.warning("Meta usage for %s could not be priced, charging an estimate", self.endpoint)
        if seen is not None and finished:
            return seen
        estimate = estimate_usage(
            self.endpoint,
            self.model,
            request_bytes=self.request_bytes,
            audio_bytes=self.audio_bytes,
            images=self.images,
            has_video=self.has_video,
            media=self.media,
        )
        estimated = estimate, cost_usd(self.endpoint, self.model, estimate)
        return seen if seen is not None and seen[1] > estimated[1] else estimated

    def usage_from_body(self, body: bytes) -> Any:
        """The usage in a complete JSON response, or None."""
        if self.endpoint == "asr/transcribe":
            return _find_value(body, "audioDurationMs", _DURATION_KEY, lambda v: {"audioDurationMs": v})
        return _find_value(body, "usage", _USAGE_KEY, lambda v: v)


async def charge(
    settings: Settings, device_id: str, meter: Meter, usage_seen: Any, *, finished: bool = True
) -> None:
    usage, cost = meter.price(usage_seen, finished=finished)
    try:
        await anyio.to_thread.run_sync(
            record_charge, settings.db_path, device_id, meter.endpoint, meter.model, usage, cost
        )
    except Exception:
        logger.exception("Could not record a charge of $%.6f for device %s", cost, device_id[:8])
        return
    logger.info(
        "Charged device %s $%.6f for %s %s%s",
        device_id[:8],
        cost,
        meter.endpoint,
        meter.model,
        " (estimated)" if usage.get("estimated") else "",
    )


@dataclass
class Call:
    """One upstream request, prepared and checked."""

    method: str
    path: str
    meter: Meter | None = None
    capped: bool = True
    content: bytes | AsyncIterator[bytes] | None = None
    files: dict[str, tuple[str | None, bytes, str]] | None = None
    headers: dict[str, str] = field(default_factory=dict)


class _UploadTooLarge(Exception):
    pass


# Endpoints.


@router.post("/meta/v1/responses")
async def create_response(request: Request, device: RelayDevice, conn: Conn, settings: AppSettings) -> Response:
    async def prepare() -> Call:
        payload = _json_object(await _read_body(request, MAX_JSON_BYTES, "The request is too large."))
        model = payload.get("model")
        _check_model("responses", model, settings)
        _check_response_options(payload)
        body = _encode(payload)
        inputs = payload.get("input")
        has_video = bool(_count_parts(inputs, "input_video") or _count_parts(inputs, "input_file"))
        if model == SAM_MODEL and has_video and payload.get("stream") is not True:
            # A long SAM video call can outlast the read timeout; streamed, the
            # relay reads it to its final usage.
            raise RelayError(400, "unsupported", "SAM video calls must stream through the relay.")
        meter = Meter(
            "responses",
            model,
            request_bytes=len(body),
            images=_count_parts(inputs, "input_image"),
            has_video=has_video,
            media=(
                model == SAM_MODEL
                or has_video
                or _has_key(inputs, "file_id")
                or payload.get("previous_response_id") is not None
                or payload.get("conversation") is not None
            ),
        )
        return Call("POST", "responses", meter, content=body, headers={"content-type": "application/json"})

    return await _proxy(request, device, conn, settings, prepare)


@router.post("/meta/v1/files")
async def upload_file(request: Request, device: RelayDevice, conn: Conn, settings: AppSettings) -> Response:
    async def prepare() -> Call:
        content_type = request.headers.get("content-type", "")
        if parse_options_header(content_type)[0] != b"multipart/form-data":
            raise RelayError(400, "invalid_request", "Upload the file as multipart/form-data.")
        limit = MAX_UPLOAD_BYTES + UPLOAD_OVERHEAD_BYTES
        declared = _declared_length(request, limit, "The file is larger than 1 GiB.")
        headers = {"content-type": content_type}
        if declared is not None:
            headers["content-length"] = str(declared)
        # Streamed: an upload is never held whole in memory.
        return Call("POST", "files", content=_limited_stream(request, limit), headers=headers)

    return await _proxy(request, device, conn, settings, prepare)


@router.get("/meta/v1/files/{file_id}")
async def read_file(
    file_id: str, request: Request, device: RelayDevice, conn: Conn, settings: AppSettings
) -> Response:
    return await _proxy(request, device, conn, settings, _file_call("GET", file_id))


@router.delete("/meta/v1/files/{file_id}")
async def delete_file(
    file_id: str, request: Request, device: RelayDevice, conn: Conn, settings: AppSettings
) -> Response:
    return await _proxy(request, device, conn, settings, _file_call("DELETE", file_id))


@router.post("/meta/v1/asr/transcribe")
async def transcribe(request: Request, device: RelayDevice, conn: Conn, settings: AppSettings) -> Response:
    async def prepare() -> Call:
        too_large = "The audio is larger than 32 MB."
        limit = MAX_AUDIO_BYTES + MAX_TRANSCRIBE_SPEC_BYTES + UPLOAD_OVERHEAD_BYTES
        parts = _form_parts(request.headers.get("content-type", ""), await _read_body(request, limit, too_large))
        spec_part, audio = parts["request"], parts["audio"]
        if len(spec_part.data) > MAX_TRANSCRIBE_SPEC_BYTES:
            raise RelayError(400, "invalid_request", "The request part is too large.")
        spec = _json_object(bytes(spec_part.data))
        model = spec.get("model")
        _check_model("asr/transcribe", model, settings)
        if len(audio.data) > MAX_AUDIO_BYTES:
            raise RelayError(413, "too_large", too_large)
        # The form is rebuilt from what was checked, so Meta reads exactly the
        # model the relay allowed.
        filename = audio.filename if audio.filename and _SAFE_FILENAME.fullmatch(audio.filename) else "audio.wav"
        media_type = audio.content_type if audio.content_type and _SAFE_MEDIA_TYPE.fullmatch(audio.content_type) else "audio/wav"
        files = {
            "request": (None, _encode(spec), "application/json"),
            "audio": (filename, bytes(audio.data), media_type),
        }
        meter = Meter(
            "asr/transcribe", model, request_bytes=len(spec_part.data) + len(audio.data), audio_bytes=len(audio.data)
        )
        return Call("POST", "asr/transcribe", meter, files=files)

    return await _proxy(request, device, conn, settings, prepare)


@router.api_route(
    "/meta/v1/{rest:path}",
    methods=["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
    include_in_schema=False,
)
async def not_proxied(rest: str) -> Response:
    raise RelayError(404, "not_found", "Not found.")


# Forwarding.


async def _proxy(
    request: Request,
    device: sqlite3.Row,
    conn: sqlite3.Connection,
    settings: Settings,
    prepare: Callable[[], Awaitable[Call]],
) -> Response:
    device_id = device["device_id"]
    slot = request.app.state.in_flight.acquire(device_id)
    handed_off = False
    try:
        call = await prepare()
        if call.capped:
            # As late as possible, right before the call goes out.
            await run_in_threadpool(check_budget, conn, device_id, settings)
        response = await _forward(request, settings, device_id, call, slot)
        # A streamed response frees its slot when the stream ends.
        handed_off = isinstance(response, MeteredStream)
        return response
    except ClientDisconnect:
        # The client went away while sending its body; nobody reads this.
        return error_response(400, "client_disconnected", "The request was cut off.")
    finally:
        if not handed_off:
            slot.release()


async def _forward(request: Request, settings: Settings, device_id: str, call: Call, slot: Slot) -> Response:
    client: httpx.AsyncClient = request.app.state.meta_client
    url = httpx.URL(f"{META_BASE_URL}/{call.path}")
    if request.url.query:
        url = url.copy_with(query=request.url.query.encode("ascii"))
    headers = {name: value for name in _REQUEST_HEADERS if (value := request.headers.get(name)) is not None}
    headers.update(call.headers)
    # Plain bytes from Meta, so usage can be read as they pass.
    headers["accept-encoding"] = "identity"
    headers["authorization"] = f"Bearer {settings.meta_api_key}"
    upstream_request = client.build_request(call.method, url, headers=headers, content=call.content, files=call.files)

    try:
        upstream = await client.send(upstream_request, stream=True)
    except _UploadTooLarge:
        raise RelayError(413, "too_large", "The file is larger than 1 GiB.") from None
    except httpx.HTTPError as exc:
        raise await _transport_error(exc, settings, device_id, call) from None

    meter = call.meter
    if meter is not None and upstream.status_code < 300 and _is_event_stream(upstream):

        async def finish(usage_seen: Any, finished: bool) -> None:
            try:
                await charge(settings, device_id, meter, usage_seen, finished=finished)
            finally:
                slot.release()

        return MeteredStream(upstream, finish)

    try:
        if upstream.status_code >= 300:
            return await _meta_error(upstream, settings)
        try:
            body = await upstream.aread()
        except httpx.HTTPError as exc:
            raise await _transport_error(exc, settings, device_id, call) from None
    finally:
        await _close_quietly(upstream)

    if meter is not None:
        await charge(settings, device_id, meter, meter.usage_from_body(body))
    return Response(body, status_code=upstream.status_code, headers=_response_headers(upstream))


async def _close_quietly(upstream: httpx.Response) -> None:
    # A failed close must not stop the call from being charged.
    try:
        await upstream.aclose()
    except Exception:
        logger.warning("Could not close the Meta connection cleanly", exc_info=True)


def _is_event_stream(upstream: httpx.Response) -> bool:
    return upstream.headers.get("content-type", "").split(";")[0].strip().lower() == "text/event-stream"


class MeteredStream(Response):
    """Passes Meta's event stream to the client as it arrives and charges the
    call from the stream's final usage once Meta has finished.

    A client hanging up never makes a call cheaper than finishing it: Meta's
    stream is still read to its end, the bytes dropped, and the real usage
    charged. The device's in-flight slot stays taken until then, so a client
    can't leave more than four such calls running. Uvicorn does not cancel a
    request when its client goes away (sends are just dropped), so the reading
    goes on in this request's task, bounded by the upstream read timeout.

    Only when Meta's stream itself breaks off (an error, a timeout, a dropped
    connection) is the call charged the higher of the usage seen and the
    request estimate, which has a floor for media.
    """

    def __init__(self, upstream: httpx.Response, finish: Callable[[Any, bool], Awaitable[None]]) -> None:
        self.status_code = upstream.status_code
        self.media_type = None
        self.background = None
        self.init_headers(_response_headers(upstream))
        self._upstream = upstream
        self._finish = finish

    async def __call__(self, scope: Any, receive: Any, send: Any) -> None:
        events = SSEUsage()
        finished = False
        try:
            finished = await self._relay(receive, send, events)
        finally:
            # Shielded: this must run even if the request is cancelled.
            with anyio.CancelScope(shield=True):
                await _close_quietly(self._upstream)
                await self._finish(events.close(), finished)

    async def _relay(self, receive: Any, send: Any, events: "SSEUsage") -> bool:
        """Read Meta's stream to its end, passing it on while the client is
        there. True if the stream ended normally."""
        client = _Client(send)
        finished = True
        async with anyio.create_task_group() as group:
            group.start_soon(client.watch, receive)
            await client.send({"type": "http.response.start", "status": self.status_code, "headers": self.raw_headers})
            try:
                async for chunk in self._upstream.aiter_bytes():
                    events.feed(chunk)
                    await client.send({"type": "http.response.body", "body": chunk, "more_body": True})
            except Exception as exc:
                # httpx closes the response itself once the body is fully
                # read, so a failure after that leaves the stream complete.
                # Any other failure cut it off.
                finished = self._upstream.is_closed
                logger.warning(
                    "Meta's stream %s: %s", "closed badly" if finished else "broke off", type(exc).__name__
                )
            await client.send({"type": "http.response.body", "body": b"", "more_body": False})
            group.cancel_scope.cancel()
        if client.gone:
            logger.info("Client left mid-stream; Meta's stream was read to its end to charge it")
        return finished


class _Client:
    """The client end of a stream. Once it has hung up, sends are dropped."""

    def __init__(self, send: Any) -> None:
        self._send = send
        self.gone = False

    async def watch(self, receive: Any) -> None:
        while (await receive())["type"] != "http.disconnect":
            pass
        self.gone = True

    async def send(self, message: dict[str, Any]) -> None:
        if self.gone:
            return
        try:
            await self._send(message)
        except Exception:
            # Usually OSError: the connection is gone. Any failure to send
            # counts as a hang-up, so only Meta's side can end the reading.
            self.gone = True


class SSEUsage:
    """Finds the latest `usage` in a Responses event stream as it passes
    through, usually in the final `response.completed` event.

    An event up to FULL_PARSE_BYTES is parsed whole. A larger one (a final
    event that repeats a long output) keeps only its last USAGE_TAIL_BYTES,
    where the usage is, so memory stays bounded.
    """

    def __init__(self) -> None:
        self.usage: Any = None
        self._line = bytearray()
        # Set once a line grows too long to keep whole: is it a data line?
        self._long_line_is_data: bool | None = None
        self._data = bytearray()
        self._data_lines = 0
        self._data_cut = False

    def feed(self, chunk: bytes) -> None:
        try:
            start = 0
            while (end := chunk.find(b"\n", start)) >= 0:
                self._add(chunk[start:end])
                self._end_line()
                start = end + 1
            self._add(chunk[start:])
        except Exception:
            # Metering never breaks the stream; without usage the call is
            # charged the estimate.
            logger.warning("Could not read usage from Meta's stream", exc_info=True)

    def close(self) -> Any:
        """The latest usage seen, after reading an unterminated last event."""
        try:
            if self._line or self._long_line_is_data is not None:
                self._end_line()
            self._dispatch()
        except Exception:
            logger.warning("Could not read usage from Meta's stream", exc_info=True)
        return self.usage

    def _add(self, part: bytes) -> None:
        self._line += part
        if self._long_line_is_data is None and len(self._line) > FULL_PARSE_BYTES:
            self._long_line_is_data = self._line.startswith(b"data:")
        if self._long_line_is_data is not None and len(self._line) > 2 * USAGE_TAIL_BYTES:
            del self._line[:-USAGE_TAIL_BYTES]

    def _end_line(self) -> None:
        line = bytes(self._line)
        if line.endswith(b"\r"):
            line = line[:-1]
        long_line_is_data, self._long_line_is_data = self._long_line_is_data, None
        self._line.clear()
        if long_line_is_data is not None:
            if long_line_is_data:
                self._add_data(line, cut=True)
        elif not line:
            self._dispatch()
        elif line.startswith(b"data:"):
            value = line[5:]
            self._add_data(value[1:] if value.startswith(b" ") else value)
        # Other fields (event, id, retry) and comments carry no usage.

    def _add_data(self, value: bytes, cut: bool = False) -> None:
        if self._data_lines:
            self._data += b"\n"
        self._data += value
        self._data_lines += 1
        if cut or len(self._data) > FULL_PARSE_BYTES:
            self._data_cut = True
            del self._data[:-USAGE_TAIL_BYTES]

    def _dispatch(self) -> None:
        data, cut = bytes(self._data), self._data_cut
        self._data.clear()
        self._data_lines = 0
        self._data_cut = False
        if b'"usage"' not in data:
            return
        if cut:
            usage = _find_in_tail(data, _USAGE_KEY)
        else:
            try:
                usage = _event_usage(json.loads(data))
            except ValueError:
                return
        if isinstance(usage, dict):
            self.usage = usage


def _event_usage(event: Any) -> Any:
    if not isinstance(event, dict):
        return None
    response = event.get("response")
    if isinstance(response, dict) and isinstance(response.get("usage"), dict):
        return response["usage"]
    return event.get("usage")


async def _transport_error(exc: httpx.HTTPError, settings: Settings, device_id: str, call: Call) -> RelayError:
    logger.warning("Meta call to %s failed: %s", call.path, type(exc).__name__)
    if isinstance(exc, httpx.ConnectError | httpx.ConnectTimeout | httpx.PoolTimeout):
        # Nothing reached Meta.
        return RelayError(502, "meta_unreachable", "Could not reach Meta. Try again.")
    if call.meter is not None:
        # The request may have reached Meta and been billed.
        await charge(settings, device_id, call.meter, None)
    if isinstance(exc, httpx.TimeoutException):
        return RelayError(504, "meta_timeout", "Meta took too long to answer. Try again.")
    return RelayError(502, "meta_unreachable", "Could not reach Meta. Try again.")


async def _meta_error(upstream: httpx.Response, settings: Settings) -> Response:
    body = await _read_at_most(upstream, MAX_ERROR_BYTES)
    status = upstream.status_code
    logger.warning("Meta answered %d", status)
    if status == 401:
        # Passed through, a 401 would tell the app its own device token is bad.
        return error_response(502, "meta_auth", "The relay could not use its Meta key. Try again later.")
    if status < 400:
        return error_response(502, "meta_error", "Meta sent an unexpected answer. Try again.")
    retry_after = upstream.headers.get("retry-after")
    headers = {"Retry-After": retry_after} if retry_after else None
    return error_response(status, "meta_error", _meta_reason(body, settings.meta_api_key), headers)


def _meta_reason(body: bytes, key: str) -> str:
    try:
        data = json.loads(body)
    except ValueError:
        data = None
    message = None
    if isinstance(data, dict):
        error = data.get("error")
        message = error.get("message") if isinstance(error, dict) else error
        if not isinstance(message, str):
            message = data.get("message")
    if not isinstance(message, str) or not message.strip():
        return "Meta returned an error."
    message = " ".join(message.split())
    if key:
        message = message.replace(key, "[redacted]")
    return message if len(message) <= 300 else message[:297] + "..."


def _response_headers(upstream: httpx.Response) -> dict[str, str]:
    return {name: value for name in _RESPONSE_HEADERS if (value := upstream.headers.get(name)) is not None}


async def _read_at_most(response: httpx.Response, limit: int) -> bytes:
    data = bytearray()
    try:
        async for chunk in response.aiter_bytes():
            data += chunk
            if len(data) >= limit:
                break
    except httpx.HTTPError:
        pass
    return bytes(data[:limit])


# Request bodies.


def _declared_length(request: Request, limit: int, too_large: str) -> int | None:
    declared = request.headers.get("content-length")
    if declared is None:
        return None
    if not declared.isdigit():
        raise RelayError(400, "invalid_request", "The Content-Length header is not valid.")
    if int(declared) > limit:
        raise RelayError(413, "too_large", too_large)
    return int(declared)


async def _read_body(request: Request, limit: int, too_large: str) -> bytes:
    _declared_length(request, limit, too_large)
    body = bytearray()
    async for chunk in request.stream():
        body += chunk
        if len(body) > limit:
            raise RelayError(413, "too_large", too_large)
    return bytes(body)


async def _limited_stream(request: Request, limit: int) -> AsyncIterator[bytes]:
    size = 0
    async for chunk in request.stream():
        size += len(chunk)
        if size > limit:
            raise _UploadTooLarge
        yield chunk


def _json_object(raw: bytes) -> dict[str, Any]:
    try:
        value = json.loads(raw, object_pairs_hook=_unique_keys)
    except (ValueError, RecursionError):
        value = None
    if not isinstance(value, dict):
        raise RelayError(400, "invalid_request", "The request body must be a JSON object.")
    return value


def _unique_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    # Duplicate keys could make the relay and Meta read different models.
    value = dict(pairs)
    if len(value) != len(pairs):
        raise ValueError("duplicate key")
    return value


def _encode(payload: dict[str, Any]) -> bytes:
    # Forward what was checked, re-encoded, rather than the client's bytes.
    try:
        return json.dumps(payload, separators=(",", ":"), allow_nan=False).encode()
    except ValueError:
        raise RelayError(400, "invalid_request", "The request body must be a JSON object.") from None


def _check_model(endpoint: str, model: Any, settings: Settings) -> None:
    # Both lists: the configured one, and the models the relay can price for
    # this endpoint, which never include Muse Wonder or a contributor tier.
    if not isinstance(model, str) or model not in settings.allowed_models or model not in MODELS_BY_ENDPOINT[endpoint]:
        raise RelayError(
            403,
            "model_not_allowed",
            "This model is not available through the Klipolo relay. Add your own Meta key in Settings to use it.",
        )


def _check_response_options(payload: dict[str, Any]) -> None:
    """Refuse options whose cost the relay could not see."""
    if payload.get("background") not in (None, False):
        # Billed later, with the usage only on a fetch the relay doesn't proxy.
        raise RelayError(400, "unsupported", "Background responses are not supported through the relay.")
    tools = payload.get("tools")
    if tools is not None and (
        not isinstance(tools, list) or any(not isinstance(t, dict) or t.get("type") != "function" for t in tools)
    ):
        # Hosted tools are billed outside the response's token usage.
        raise RelayError(400, "unsupported", "Only function tools are supported through the relay.")
    if payload.get("service_tier") not in (None, "auto", "default"):
        raise RelayError(400, "unsupported", "Only the default service tier is supported through the relay.")


def _objects(value: Any) -> Iterator[dict[str, Any]]:
    """Every JSON object inside `value`, at any depth."""
    stack = [value]
    while stack:
        item = stack.pop()
        if isinstance(item, dict):
            yield item
            stack.extend(item.values())
        elif isinstance(item, list):
            stack.extend(item)


def _count_parts(value: Any, part_type: str) -> int:
    return sum(1 for item in _objects(value) if item.get("type") == part_type)


def _has_key(value: Any, key: str) -> bool:
    return any(key in item for item in _objects(value))


def _file_call(method: str, file_id: str) -> Callable[[], Awaitable[Call]]:
    async def prepare() -> Call:
        if not _FILE_ID.fullmatch(file_id):
            raise RelayError(404, "not_found", "Not found.")
        # Reading or deleting a file costs nothing, so it works over the cap.
        return Call(method, f"files/{file_id}", capped=False)

    return prepare


@dataclass
class _Part:
    name: str | None = None
    filename: str | None = None
    content_type: str | None = None
    data: bytearray = field(default_factory=bytearray)


def _form_parts(content_type: str, body: bytes) -> dict[str, _Part]:
    """Exactly one `request` part and one `audio` part, from multipart/form-data."""
    invalid = RelayError(400, "invalid_request", "Send multipart/form-data with one request part and one audio part.")
    kind, options = parse_options_header(content_type)
    boundary = options.get(b"boundary")
    if kind != b"multipart/form-data" or not boundary:
        raise invalid

    parts: list[_Part] = []
    headers: dict[bytes, bytes] = {}
    name, value = bytearray(), bytearray()

    def on_part_begin() -> None:
        parts.append(_Part())
        headers.clear()

    def on_header_end() -> None:
        headers[bytes(name).lower()] = bytes(value)
        name.clear()
        value.clear()

    def on_headers_finished() -> None:
        _, disposition = parse_options_header(headers.get(b"content-disposition"))
        part = parts[-1]
        part.name = disposition.get(b"name", b"").decode("utf-8", "replace") or None
        filename = disposition.get(b"filename")
        part.filename = filename.decode("utf-8", "replace") if filename is not None else None
        media_type = headers.get(b"content-type")
        part.content_type = media_type.decode("latin-1").strip() if media_type else None

    callbacks = {
        "on_part_begin": on_part_begin,
        "on_header_field": lambda data, start, end: name.extend(data[start:end]),
        "on_header_value": lambda data, start, end: value.extend(data[start:end]),
        "on_header_end": on_header_end,
        "on_headers_finished": on_headers_finished,
        "on_part_data": lambda data, start, end: parts[-1].data.extend(data[start:end]),
    }
    try:
        parser = MultipartParser(boundary, callbacks)
        parser.write(body)
        parser.finalize()
    except (MultipartParseError, ValueError):
        raise invalid from None
    if parser.state != MultipartState.END:
        raise invalid

    by_name = {part.name: part for part in parts}
    if len(parts) != 2 or set(by_name) != {"request", "audio"}:
        raise invalid
    return by_name


# Usage.


def _find_value(body: bytes, key: str, pattern: re.Pattern[str], wrap: Callable[[Any], Any]) -> Any:
    """`key` at the top of a JSON response, or None. A body too large to parse
    whole is searched from its end, where Meta puts the usage."""
    if len(body) <= FULL_PARSE_BYTES:
        try:
            data = json.loads(body)
        except ValueError:
            return None
        return wrap(data[key]) if isinstance(data, dict) and key in data else None
    value = _find_in_tail(body[-USAGE_TAIL_BYTES:], pattern)
    return None if value is None else wrap(value)


def _find_in_tail(tail: bytes, pattern: re.Pattern[str]) -> Any:
    # Inside a JSON string a quote is escaped, so `"usage":` with bare quotes
    # can only be a key. The last one is the response's own.
    text = tail.decode("utf-8", "replace")
    decoder = json.JSONDecoder()
    for match in reversed(list(pattern.finditer(text))):
        try:
            value, _ = decoder.raw_decode(text, match.end())
        except ValueError:
            continue
        return value
    return None


def _numbers_only(value: Mapping[str, Any], depth: int = 0) -> dict[str, Any]:
    """The numbers in a usage object, for the ledger. Nothing else is kept."""
    out: dict[str, Any] = {}
    for key, item in value.items():
        if isinstance(item, bool) or (isinstance(item, int | float) and math.isfinite(item)):
            out[str(key)] = item
        elif isinstance(item, Mapping) and depth < 3:
            nested = _numbers_only(item, depth + 1)
            if nested:
                out[str(key)] = nested
    return out
