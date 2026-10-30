"""Metered proxy to Meta's Model API.

`/meta/v1/{path}` forwards to `https://api.meta.ai/v1/{path}` with the relay's
own key, for an allowlist of endpoints and of models (ALLOWED_MODELS: Spark
1.3 and 1.2 and Voice Transcribe by default; SAM 3.1 only when listed, and
then its video calls must stream):

    POST   responses        JSON, at most 8 MiB
    POST   files            multipart upload, streamed, at most 1 GiB
    GET    files/{id}
    DELETE files/{id}
    POST   asr/transcribe   multipart `request` (JSON) and `audio` parts;
                            `sessionId` is the only query parameter allowed

Anything else is 404. Every call is gated per request: a known device token
(401), relay AI for the device's code right now (403), a configured key (503).

Caps are kept with reservations. Before a billable call goes out it reserves
the most it could cost: for Spark, request bytes / 4 at the input price plus
its whole max_output_tokens (set to 8192 when missing, at most 16384) at the
output price, plus $0.75 when the input refers to media or earlier context;
for transcription, the audio's length from its WAV header. The call is
refused with 402 unless the device and the relay have room for it on top of
what is spent and what calls in flight have reserved. A device with no room
left is refused before its body is read, and bodies have a deadline (408).

After Meta answers, the call is charged from Meta's own `usage`: the ledger
row and the device's `spent_usd` are written in one transaction, and only then
is the reservation freed. Failed calls (non-2xx) are not charged. A client
hanging up never makes a call cheaper: a non-streamed call still waits for
Meta's answer, and a streamed one is read to its final event. When the real
usage can't be read (no usage, missing counters, a parse failure, a timeout
after the request reached Meta, a stream without a final event), the call is
charged its reservation, or the usage seen if that is higher. If a charge
can't be written, capped calls are refused (503) until it can be.

Nothing from a proxied request is stored except the ledger row, and request
or response headers and bodies are never logged.
"""

import json
import logging
import math
import re
import sqlite3
import time
import urllib.parse
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
from relay.pricing import (
    DEFAULT_MAX_OUTPUT_TOKENS,
    MAX_OUTPUT_TOKENS,
    MODELS_BY_ENDPOINT,
    SAM_IMAGES_KEY,
    SAM_MODEL,
    SPARK_MODELS,
    UNREADABLE_AUDIO_SECONDS,
    cost_usd,
    reservation_usage,
)

logger = logging.getLogger(__name__)

router = APIRouter()

META_BASE_URL = "https://api.meta.ai/v1"
# Video calls take tens of seconds before the first byte.
UPSTREAM_TIMEOUT = httpx.Timeout(connect=10.0, read=300.0, write=300.0, pool=10.0)

# Media goes up through the Files API and is referred to by file_id, so a
# Responses body is text: prompts, tools and earlier turns.
MAX_JSON_BYTES = 2 * 1024 * 1024
MAX_UPLOAD_BYTES = 1024**3
# Multipart framing and the small form fields around an upload.
UPLOAD_OVERHEAD_BYTES = 64 * 1024
# How long a client may take to send its body: a slow client can't hold a
# slot and its reservation open for long.
JSON_BODY_DEADLINE_S = 60.0
UPLOAD_DEADLINE_S = 15 * 60.0
MAX_AUDIO_BYTES = 32 * 1024 * 1024
MAX_TRANSCRIBE_SPEC_BYTES = 64 * 1024
MAX_ERROR_BYTES = 64 * 1024
# Deeper request JSON is refused, so nothing the relay does with it recurses far.
MAX_JSON_DEPTH = 64
# Larger bodies are not parsed whole: `usage` follows the output, so it is
# looked for in the last USAGE_TAIL_BYTES instead.
FULL_PARSE_BYTES = 8 * 1024 * 1024
USAGE_TAIL_BYTES = 64 * 1024

# Pause between attempts to write a charge, times the attempt number.
CHARGE_RETRY_DELAY_S = 0.5

MAX_IN_FLIGHT_PER_DEVICE = 4
MAX_IN_FLIGHT_TOTAL = 32

# Request headers worth passing on. The client's Authorization, cookies, any
# other credential and all hop-by-hop or forwarding headers stay here.
_REQUEST_HEADERS = ("accept", "idempotency-key")
_RESPONSE_HEADERS = ("content-type", "retry-after", "x-request-id")

_FILE_ID = re.compile(r"[A-Za-z0-9_-]{1,128}")
_QUERY_VALUE = re.compile(r"[A-Za-z0-9_.:-]{1,128}")
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
    """The proxied calls in progress: how many, per device and in total, and
    how much each has reserved against the caps.

    A call reserves its worst-case cost at admission and keeps it until its
    charge is written, so calls running at once can't together pass a cap.
    This state lives in this process and is only touched from the event loop:
    run the server with a single worker.
    """

    def __init__(self, per_device: int = MAX_IN_FLIGHT_PER_DEVICE, total: int = MAX_IN_FLIGHT_TOTAL) -> None:
        self.per_device = per_device
        self.total = total
        self._by_device: dict[str, int] = {}
        self._count = 0
        self._reserved_by_device: dict[str, float] = {}
        self._reserved_total = 0.0
        # Admissions run one at a time, so two can't both claim the same room.
        self.admission = anyio.Lock()
        # Charges that could not be written. While any is held, capped calls
        # are refused: the caps can't be checked against spend that isn't
        # recorded.
        self.unwritten: list[Charge] = []

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

    def reserved(self, device_id: str) -> tuple[float, float]:
        """What calls in flight have reserved: for this device, and in total."""
        return self._reserved_by_device.get(device_id, 0.0), self._reserved_total

    def _reserve(self, device_id: str, amount: float) -> None:
        self._reserved_by_device[device_id] = self._reserved_by_device.get(device_id, 0.0) + amount
        self._reserved_total += amount

    def _release(self, device_id: str, reserved: float) -> None:
        left = self._by_device[device_id] - 1
        if left:
            self._by_device[device_id] = left
            self._reserved_by_device[device_id] -= reserved
        else:
            del self._by_device[device_id]
            self._reserved_by_device.pop(device_id, None)
        self._count -= 1
        # Back to exactly zero when idle, so float sums never drift.
        self._reserved_total = self._reserved_total - reserved if self._count else 0.0


class Slot:
    """One call's place in flight and its reservation, freed together once
    the call is charged."""

    def __init__(self, owner: InFlight, device_id: str) -> None:
        self.owner = owner
        self.device_id = device_id
        self.reserved = 0.0
        self._released = False

    def reserve(self, amount: float) -> None:
        self.owner._reserve(self.device_id, amount)
        self.reserved += amount

    def release(self) -> None:
        if not self._released:
            self._released = True
            self.owner._release(self.device_id, self.reserved)


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


def _spend(conn: sqlite3.Connection, device_id: str) -> tuple[float, float]:
    """The device's spend and the relay's all-time total. The global cap is
    all-time: Meta's credits are granted once."""
    row = conn.execute("SELECT spent_usd FROM devices WHERE device_id = ?", (device_id,)).fetchone()
    total = conn.execute("SELECT COALESCE(SUM(cost_usd), 0) FROM ledger").fetchone()[0]
    return (math.inf if row is None else row["spent_usd"]), total


async def admit(conn: sqlite3.Connection, slot: Slot, settings: Settings, amount: float) -> None:
    """Reserve `amount` for the call, or refuse with 402 when the device or the
    relay has no room for it on top of what is spent and what calls in flight
    have reserved. With `amount` 0 this only checks that any room is left."""
    in_flight = slot.owner
    async with in_flight.admission:
        if in_flight.unwritten:
            try:
                await anyio.to_thread.run_sync(_write_unwritten, settings.db_path, in_flight.unwritten)
            except Exception:
                logger.warning("Charges still can't be written; refusing capped calls")
                raise RelayError(
                    503, "charging_unavailable", "The relay can't record charges right now. Try again in a minute."
                ) from None
        # Reservations are read before the spend, and a call frees its
        # reservation only after its charge is written. Whatever has left the
        # reservations by the time the spend is read is in that spend, so no
        # call is ever missed from both.
        device_reserved, total_reserved = in_flight.reserved(slot.device_id)
        spent, total = await run_in_threadpool(_spend, conn, slot.device_id)
        claims = (
            (spent + device_reserved, settings.device_cap_usd),
            (total + total_reserved, settings.global_cap_usd),
        )
        for used, cap in claims:
            if used >= cap or used + amount > cap:
                raise RelayError(402, "budget", BUDGET_MESSAGE)
        slot.reserve(amount)


@dataclass(frozen=True)
class Charge:
    device_id: str
    endpoint: str
    model: str
    usage: dict[str, Any]
    cost: float
    at: str = field(default_factory=lambda: datetime.now(UTC).isoformat(timespec="seconds"))


def record_charge(db_path: Path, charge: Charge) -> None:
    """Write the ledger row and add the cost to the device in one transaction,
    so the ledger (what the admin sees) and spent_usd (what the cap reads)
    never drift. Uses its own connection: a streamed response outlives the
    request's."""
    device_id, cost = charge.device_id, charge.cost
    row = (device_id, charge.at, charge.endpoint, charge.model, json.dumps(charge.usage, sort_keys=True), cost)
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
            time.sleep(CHARGE_RETRY_DELAY_S * (attempt + 1))


def _write_unwritten(db_path: Path, unwritten: list[Charge]) -> None:
    # Oldest first; each leaves the list only once it is written.
    while unwritten:
        record_charge(db_path, unwritten[0])
        unwritten.pop(0)


@dataclass
class Meter:
    """What a billable call needs to be priced: its usage once Meta answers,
    and its reservation, the most it could cost, worked out from the request.
    The reservation is held against the caps while the call runs, and charged
    when the real usage can't be read."""

    endpoint: str
    model: str
    request_bytes: int = 0
    max_output_tokens: int = 0
    audio_seconds: float = 0.0
    images: int = 0
    has_video: bool = False
    # Video, files or earlier context by reference, or SAM.
    media: bool = False
    _reservation: tuple[dict[str, Any], float] | None = field(default=None, init=False, repr=False)

    def reservation(self) -> tuple[dict[str, Any], float]:
        """Worked out once, at admission, so charging it can't fail later."""
        if self._reservation is None:
            self._reservation = self._reserve()
        return self._reservation

    def _reserve(self) -> tuple[dict[str, Any], float]:
        usage = reservation_usage(
            self.endpoint,
            self.model,
            request_bytes=self.request_bytes,
            max_output_tokens=self.max_output_tokens,
            media=self.media,
            audio_seconds=self.audio_seconds,
            images=self.images,
            has_video=self.has_video,
        )
        return usage, cost_usd(self.endpoint, self.model, usage)

    def price(self, candidates: list[Any], *, final: bool) -> tuple[dict[str, Any], float]:
        """The usage to record and its cost. `candidates` are the usage objects
        found in Meta's answer; when several turn up (a long answer searched
        from its end) the highest priced wins. `final` says they come from
        Meta's complete answer. If none can be priced, or the answer was cut
        off before its final usage, the call costs at least its reservation."""
        priced = []
        for candidate in candidates:
            if not isinstance(candidate, Mapping):
                continue
            usage = dict(candidate)
            if self.model == SAM_MODEL:
                usage[SAM_IMAGES_KEY] = self.images
            try:
                if self.model == SAM_MODEL and self.has_video and usage.get("video_frames_processed") is None:
                    raise ValueError("SAM video usage has no video_frames_processed")
                priced.append((_numbers_only(usage), cost_usd(self.endpoint, self.model, usage)))
            except ValueError:
                logger.warning("Meta usage for %s could not be priced", self.endpoint)
        if final and priced:
            return max(priced, key=lambda item: item[1])
        return max([*priced, self.reservation()], key=lambda item: item[1])

    def usage_from_body(self, body: bytes) -> list[Any]:
        """The usage candidates in a complete JSON response, whatever the body."""
        try:
            if self.endpoint == "asr/transcribe":
                return _find_value(body, "audioDurationMs", _DURATION_KEY, lambda v: {"audioDurationMs": v})
            return _find_value(body, "usage", _USAGE_KEY, lambda v: v)
        except Exception:
            logger.warning("Could not read the usage in Meta's answer for %s", self.endpoint, exc_info=True)
            return []


async def charge(settings: Settings, slot: Slot, meter: Meter, candidates: list[Any], *, final: bool) -> None:
    try:
        usage, cost = meter.price(candidates, final=final)
    except Exception:
        # Whatever goes wrong reading the usage, the call is still charged.
        logger.warning("Could not price Meta's usage for %s, charging the reservation", meter.endpoint, exc_info=True)
        usage, cost = meter.reservation()
    entry = Charge(slot.device_id, meter.endpoint, meter.model, usage, cost)
    try:
        await anyio.to_thread.run_sync(record_charge, settings.db_path, entry)
    except Exception:
        # Fail closed: the charge is held, and capped calls are refused until
        # it is written.
        slot.owner.unwritten.append(entry)
        logger.exception("Could not record a charge of $%.6f for device %s", cost, slot.device_id[:8])
        return
    logger.info(
        "Charged device %s $%.6f for %s %s%s",
        slot.device_id[:8],
        cost,
        meter.endpoint,
        meter.model,
        " (reservation)" if usage.get("reserved") else "",
    )


@dataclass
class Call:
    """One upstream request, prepared and checked."""

    method: str
    path: str
    meter: Meter | None = None
    content: bytes | AsyncIterator[bytes] | None = None
    files: dict[str, tuple[str | None, bytes, str]] | None = None
    headers: dict[str, str] = field(default_factory=dict)


# Endpoints.


@router.post("/meta/v1/responses")
async def create_response(request: Request, device: RelayDevice, conn: Conn, settings: AppSettings) -> Response:
    async def prepare() -> Call:
        payload = _json_object(await _read_body(request, MAX_JSON_BYTES, "The request is too large."))
        model = payload.get("model")
        _check_model("responses", model, settings)
        _check_fields(payload)
        _check_response_options(payload)
        if model in SPARK_MODELS:
            payload["max_output_tokens"] = _max_output_tokens(payload.get("max_output_tokens"))
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
            max_output_tokens=payload.get("max_output_tokens", 0) if model in SPARK_MODELS else 0,
            images=_count_parts(inputs, "input_image"),
            has_video=has_video,
            media=model == SAM_MODEL or _refers_to_media(payload),
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


# Reading or deleting a file costs nothing, so it works over the cap.


@router.get("/meta/v1/files/{file_id}")
async def read_file(
    file_id: str, request: Request, device: RelayDevice, conn: Conn, settings: AppSettings
) -> Response:
    return await _proxy(request, device, conn, settings, _file_call("GET", file_id), capped=False)


@router.delete("/meta/v1/files/{file_id}")
async def delete_file(
    file_id: str, request: Request, device: RelayDevice, conn: Conn, settings: AppSettings
) -> Response:
    return await _proxy(request, device, conn, settings, _file_call("DELETE", file_id), capped=False)


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
        seconds = _wav_seconds(bytes(audio.data))
        meter = Meter(
            "asr/transcribe",
            model,
            request_bytes=len(spec_part.data) + len(audio.data),
            audio_seconds=UNREADABLE_AUDIO_SECONDS if seconds is None else seconds,
        )
        return Call("POST", "asr/transcribe", meter, files=files)

    return await _proxy(request, device, conn, settings, prepare, query_keys=frozenset({"sessionId"}))


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
    query_keys: frozenset[str] = frozenset(),
    capped: bool = True,
) -> Response:
    device_id = device["device_id"]
    params = _query_params(request, query_keys)
    slot = request.app.state.in_flight.acquire(device_id)
    handed_off = False
    try:
        if capped:
            # Before the body is read: a device with no room left is refused
            # without the relay reading its upload.
            await admit(conn, slot, settings, 0.0)
        call = await prepare()
        if call.meter is not None:
            # The call's worst case, held until its charge is written.
            await admit(conn, slot, settings, call.meter.reservation()[1])
        response = await _forward(request, settings, call, slot, params)
        # A streamed response frees its slot when the stream ends.
        handed_off = isinstance(response, MeteredStream)
        return response
    except ClientDisconnect:
        # The client went away while sending its body; nobody reads this.
        return error_response(400, "client_disconnected", "The request was cut off.")
    finally:
        if not handed_off:
            slot.release()


async def _forward(
    request: Request, settings: Settings, call: Call, slot: Slot, params: dict[str, str]
) -> Response:
    client: httpx.AsyncClient = request.app.state.meta_client
    url = httpx.URL(f"{META_BASE_URL}/{call.path}", params=params)
    headers = {name: value for name in _REQUEST_HEADERS if (value := request.headers.get(name)) is not None}
    headers.update(call.headers)
    # Plain bytes from Meta, so usage can be read as they pass.
    headers["accept-encoding"] = "identity"
    headers["authorization"] = f"Bearer {settings.meta_api_key}"
    upstream_request = client.build_request(call.method, url, headers=headers, content=call.content, files=call.files)

    try:
        # An upload that is too large or too slow raises RelayError from its
        # body stream, which passes through here unchanged.
        upstream = await client.send(upstream_request, stream=True)
    except httpx.HTTPError as exc:
        raise await _transport_error(exc, settings, slot, call) from None

    meter = call.meter
    if meter is not None and upstream.status_code < 300 and _is_event_stream(upstream):

        async def finish(candidates: list[Any], final: bool) -> None:
            try:
                await charge(settings, slot, meter, candidates, final=final)
            finally:
                slot.release()

        return MeteredStream(upstream, finish)

    try:
        if upstream.status_code >= 300:
            return await _meta_error(upstream, settings)
        try:
            body = await upstream.aread()
        except httpx.HTTPError as exc:
            raise await _transport_error(exc, settings, slot, call) from None
    finally:
        await _close_quietly(upstream)

    if meter is not None:
        await charge(settings, slot, meter, meter.usage_from_body(body), final=True)
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

    Only when the stream ends without a final event (Meta's side broke off,
    timed out or dropped the connection) is the call charged the higher of
    the usage seen and its reservation.
    """

    def __init__(self, upstream: httpx.Response, finish: Callable[[list[Any], bool], Awaitable[None]]) -> None:
        self.status_code = upstream.status_code
        self.media_type = None
        self.background = None
        self.init_headers(_response_headers(upstream))
        self._upstream = upstream
        self._finish = finish

    async def __call__(self, scope: Any, receive: Any, send: Any) -> None:
        events = SSEUsage()
        try:
            await self._relay(receive, send, events)
        finally:
            # Shielded: this must run even if the request is cancelled.
            with anyio.CancelScope(shield=True):
                await _close_quietly(self._upstream)
                await self._finish(*events.close())

    async def _relay(self, receive: Any, send: Any, events: "SSEUsage") -> None:
        """Read Meta's stream to its end, passing it on while the client is
        there."""
        client = _Client(send)
        async with anyio.create_task_group() as group:
            group.start_soon(client.watch, receive)
            await client.send({"type": "http.response.start", "status": self.status_code, "headers": self.raw_headers})
            try:
                async for chunk in self._upstream.aiter_bytes():
                    events.feed(chunk)
                    await client.send({"type": "http.response.body", "body": chunk, "more_body": True})
            except Exception as exc:
                # Includes a failure while httpx closes a fully read stream.
                # Whether the call is settled depends only on the final event.
                logger.warning("Meta's stream ended with an error: %s", type(exc).__name__)
            await client.send({"type": "http.response.body", "body": b"", "more_body": False})
            group.cancel_scope.cancel()
        if client.gone:
            logger.info("Client left mid-stream; Meta's stream was read to its end to charge it")


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


_FINAL_EVENTS = frozenset({"response.completed", "response.incomplete", "response.failed"})
_EVENT_TYPE = re.compile(rb'"type"\s*:\s*"([^"]{1,64})"')
_HEAD_BYTES = 512


class SSEUsage:
    """Reads the usage in a Responses event stream as it passes through.

    The call is settled by its final event (`response.completed`,
    `.incomplete` or `.failed`); until one arrives, the latest usage seen is
    kept. An event up to FULL_PARSE_BYTES is parsed whole. A larger one (a
    final event that repeats a long output) keeps only its first
    _HEAD_BYTES, for its type, and its last USAGE_TAIL_BYTES, where the usage
    is, so memory stays bounded.
    """

    def __init__(self) -> None:
        self.final: list[Any] | None = None
        self.seen: list[Any] = []
        self._line = bytearray()
        # Set once a line grows too long to keep whole: is it a data line?
        self._long_line_is_data: bool | None = None
        self._long_line_head = b""
        self._event = ""
        self._data = bytearray()
        self._head = b""
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
            # Metering never breaks the stream; without a final usage the call
            # costs at least its reservation.
            logger.warning("Could not read usage from Meta's stream", exc_info=True)

    def close(self) -> tuple[list[Any], bool]:
        """The usage candidates and whether they come from a final event,
        after reading an unterminated last event."""
        try:
            if self._line or self._long_line_is_data is not None:
                self._end_line()
            self._dispatch()
        except Exception:
            logger.warning("Could not read usage from Meta's stream", exc_info=True)
        return (self.final, True) if self.final is not None else (self.seen, False)

    def _add(self, part: bytes) -> None:
        self._line += part
        if self._long_line_is_data is None and len(self._line) > FULL_PARSE_BYTES:
            self._long_line_is_data = self._line.startswith(b"data:")
            self._long_line_head = _data_value(bytes(self._line[: _HEAD_BYTES + 6]))
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
                self._add_data(line, head=self._long_line_head, cut=True)
        elif not line:
            self._dispatch()
        elif line.startswith(b"data:"):
            value = _data_value(line)
            self._add_data(value, head=value)
        elif line.startswith(b"event:"):
            self._event = line[6:].strip().decode("latin-1")
        # Other fields (id, retry) and comments carry no usage.

    def _add_data(self, value: bytes, *, head: bytes, cut: bool = False) -> None:
        if len(self._head) < _HEAD_BYTES:
            self._head = (self._head + (b"\n" if self._data_lines else b"") + head)[:_HEAD_BYTES]
        if self._data_lines:
            self._data += b"\n"
        self._data += value
        self._data_lines += 1
        if cut or len(self._data) > FULL_PARSE_BYTES:
            self._data_cut = True
            del self._data[:-USAGE_TAIL_BYTES]

    def _dispatch(self) -> None:
        data, head, cut, event = bytes(self._data), self._head, self._data_cut, self._event
        self._data.clear()
        self._head = b""
        self._data_lines = 0
        self._data_cut = False
        self._event = ""
        if not data:
            return
        match = _EVENT_TYPE.search(head)
        event_type = match.group(1).decode("latin-1") if match else ""
        candidates: list[Any] = []
        if b'"usage"' in data:
            if cut:
                candidates = [value for value in _find_in_tail(data, _USAGE_KEY) if isinstance(value, dict)]
            else:
                try:
                    parsed = json.loads(data)
                except Exception:
                    parsed = None
                if isinstance(parsed, dict) and isinstance(parsed.get("type"), str):
                    event_type = parsed["type"]
                usage = _event_usage(parsed)
                candidates = [usage] if isinstance(usage, dict) else []
        if event in _FINAL_EVENTS or event_type in _FINAL_EVENTS:
            self.final = candidates
        elif candidates:
            self.seen = candidates


def _data_value(line: bytes) -> bytes:
    value = line[5:] if line.startswith(b"data:") else line
    return value[1:] if value.startswith(b" ") else value


def _event_usage(event: Any) -> Any:
    if not isinstance(event, dict):
        return None
    response = event.get("response")
    if isinstance(response, dict) and isinstance(response.get("usage"), dict):
        return response["usage"]
    return event.get("usage")


async def _transport_error(exc: httpx.HTTPError, settings: Settings, slot: Slot, call: Call) -> RelayError:
    logger.warning("Meta call to %s failed: %s", call.path, type(exc).__name__)
    if isinstance(exc, httpx.ConnectError | httpx.ConnectTimeout | httpx.PoolTimeout | httpx.LocalProtocolError):
        # Nothing reached Meta.
        return RelayError(502, "meta_unreachable", "Could not reach Meta. Try again.")
    if call.meter is not None:
        # The request may have reached Meta and been billed.
        await charge(settings, slot, call.meter, [], final=False)
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


def _query_params(request: Request, allowed: frozenset[str]) -> dict[str, str]:
    """The query, if it only has the endpoint's allowed keys, each once, with
    plain values. Rebuilt from these values when forwarded."""
    raw = request.scope.get("query_string", b"")
    if not raw:
        return {}
    invalid = RelayError(400, "invalid_request", "This query parameter is not supported through the relay.")
    try:
        pairs = urllib.parse.parse_qsl(raw.decode("ascii"), keep_blank_values=True, strict_parsing=True)
    except (UnicodeDecodeError, ValueError):
        raise invalid from None
    params: dict[str, str] = {}
    for key, value in pairs:
        if key not in allowed or key in params or not _QUERY_VALUE.fullmatch(value):
            raise invalid
        params[key] = value
    return params


def _declared_length(request: Request, limit: int, too_large: str) -> int | None:
    declared = request.headers.get("content-length")
    if declared is None:
        return None
    if not declared.isdigit():
        raise RelayError(400, "invalid_request", "The Content-Length header is not valid.")
    if int(declared) > limit:
        raise RelayError(413, "too_large", too_large)
    return int(declared)


def _too_slow() -> RelayError:
    return RelayError(408, "request_timeout", "The request took too long to arrive. Try again.")


async def _read_body(request: Request, limit: int, too_large: str) -> bytes:
    _declared_length(request, limit, too_large)
    body = bytearray()
    try:
        with anyio.fail_after(JSON_BODY_DEADLINE_S):
            async for chunk in request.stream():
                body += chunk
                if len(body) > limit:
                    raise RelayError(413, "too_large", too_large)
    except TimeoutError:
        raise _too_slow() from None
    return bytes(body)


async def _limited_stream(request: Request, limit: int) -> AsyncIterator[bytes]:
    """The upload body as it arrives, cut off with 413 past `limit` or with
    408 past UPLOAD_DEADLINE_S."""
    deadline = anyio.current_time() + UPLOAD_DEADLINE_S
    chunks = request.stream().__aiter__()
    size = 0
    while True:
        try:
            # The time limit covers one wait only, never a yield.
            with anyio.fail_after(max(deadline - anyio.current_time(), 0)):
                chunk = await chunks.__anext__()
        except StopAsyncIteration:
            return
        except TimeoutError:
            raise _too_slow() from None
        size += len(chunk)
        if size > limit:
            raise RelayError(413, "too_large", "The file is larger than 1 GiB.")
        yield chunk


def _json_object(raw: bytes) -> dict[str, Any]:
    try:
        value = json.loads(raw, object_pairs_hook=_unique_keys)
    except (ValueError, RecursionError):
        value = None
    if not isinstance(value, dict):
        raise RelayError(400, "invalid_request", "The request body must be a JSON object.")
    if _depth(value) > MAX_JSON_DEPTH:
        raise RelayError(400, "invalid_request", f"The request JSON is nested more than {MAX_JSON_DEPTH} levels deep.")
    return value


def _depth(value: Any) -> int:
    deepest = 0
    stack = [(value, 1)]
    while stack:
        item, depth = stack.pop()
        if isinstance(item, dict | list):
            deepest = max(deepest, depth)
            stack.extend((child, depth + 1) for child in (item.values() if isinstance(item, dict) else item))
    return deepest


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


# The Responses fields the relay forwards. A field it doesn't know could carry
# media or cost (prompt templates, hosted features) its reservation can't see.
_RESPONSE_FIELDS = frozenset(
    {
        "model",
        "input",
        "instructions",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "text",
        "reasoning",
        "max_output_tokens",
        "temperature",
        "top_p",
        "stream",
        "store",
        "metadata",
        "user",
        "safety_identifier",
        "previous_response_id",
        "conversation",
        # Checked below, so they get their own reason.
        "background",
        "service_tier",
    }
)


def _check_fields(payload: dict[str, Any]) -> None:
    unknown = sorted(set(payload) - _RESPONSE_FIELDS)
    if unknown:
        raise RelayError(400, "unsupported", f"The relay does not accept these fields: {', '.join(unknown)}.")


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


# 16 kHz, 16-bit, mono: the lowest byte rate Meta's transcription takes, so
# the longest the audio could play for.
_MIN_WAV_BYTE_RATE = 32_000


def _wav_seconds(audio: bytes) -> float | None:
    """The longest the WAV audio could be, or None if no data chunk is found.
    The data actually sent is measured (a streamed WAV's declared size is often
    a placeholder) and divided by the lowest byte rate Meta accepts; the
    header's own byte rate is never trusted."""
    if len(audio) < 12 or audio[:4] != b"RIFF" or audio[8:12] != b"WAVE":
        return None
    position = 12
    while position + 8 <= len(audio):
        chunk = audio[position : position + 4]
        size = int.from_bytes(audio[position + 4 : position + 8], "little")
        start = position + 8
        if chunk == b"data":
            return (len(audio) - start) / _MIN_WAV_BYTE_RATE
        position = start + size + (size & 1)
    return None


def _max_output_tokens(value: Any) -> int:
    """A bound on a Spark call's output, so its worst-case cost is known."""
    if value is None:
        return DEFAULT_MAX_OUTPUT_TOKENS
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= MAX_OUTPUT_TOKENS:
        raise RelayError(
            400, "invalid_request", f"max_output_tokens must be a whole number from 1 to {MAX_OUTPUT_TOKENS}."
        )
    return value


def _count_parts(value: Any, part_type: str) -> int:
    return sum(1 for item in _objects(value) if item.get("type") == part_type)


# Item and part types whose cost is in the request's own bytes.
_TEXT_ITEMS = frozenset({"input_text", "output_text", "function_call", "reasoning"})
_TEXT_PARTS = frozenset({"input_text", "output_text"})
_MEDIA_TYPES = frozenset({"item_reference", "input_image", "input_file", "input_video", "input_audio"})


def _refers_to_media(payload: dict[str, Any]) -> bool:
    """False only for a text-only call, whose cost the request's size bounds.
    Anything else (media or files by link or id, references to stored items,
    earlier responses or conversations, or a type the relay doesn't know)
    gets the media amount in its reservation."""
    if payload.get("previous_response_id") is not None or payload.get("conversation") is not None:
        return True
    # Anywhere in the request: a stored file, a link, or a media part.
    for item in _objects(payload):
        if "file_id" in item or (item.get("type") in _MEDIA_TYPES and not _inline_image(item)):
            return True
        for key, value in item.items():
            if (key == "url" or key.endswith("_url")) and isinstance(value, str) and not value.startswith("data:"):
                return True
    # Items and their parts: only text-only types.
    return not all(_text_only(payload.get(field)) for field in ("input", "instructions"))


def _inline_image(item: dict[str, Any]) -> bool:
    # An image inside the request is paid for by its bytes in the estimate.
    url = item.get("image_url")
    return item.get("type") == "input_image" and isinstance(url, str) and url.startswith("data:")


def _text_only(items: Any) -> bool:
    if items is None or isinstance(items, str):
        return True
    for item in items if isinstance(items, list) else [items]:
        if not isinstance(item, dict):
            return False
        kind = item.get("type", "message" if "role" in item else None)
        if kind == "message":
            content = item.get("content")
            parts = [] if isinstance(content, str) else content
            if not isinstance(parts, list) or not all(
                isinstance(part, dict) and (part.get("type") in _TEXT_PARTS or _inline_image(part)) for part in parts
            ):
                return False
        elif kind == "function_call_output":
            if not isinstance(item.get("output"), str):
                return False
        elif kind not in _TEXT_ITEMS:
            return False
    return True


def _file_call(method: str, file_id: str) -> Callable[[], Awaitable[Call]]:
    async def prepare() -> Call:
        if not _FILE_ID.fullmatch(file_id):
            raise RelayError(404, "not_found", "Not found.")
        return Call(method, f"files/{file_id}")

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


def _find_value(body: bytes, key: str, pattern: re.Pattern[str], wrap: Callable[[Any], Any]) -> list[Any]:
    """`key` at the top of a JSON response, as a list of candidates. A body
    too large to parse whole is searched from its end, where Meta puts the
    usage, and every match found there is a candidate."""
    if len(body) <= FULL_PARSE_BYTES:
        try:
            data = json.loads(body)
        except ValueError:
            return []
        return [wrap(data[key])] if isinstance(data, dict) and key in data else []
    return [wrap(value) for value in _find_in_tail(body[-USAGE_TAIL_BYTES:], pattern)]


def _find_in_tail(tail: bytes, pattern: re.Pattern[str]) -> list[Any]:
    # Inside a JSON string a quote is escaped, so `"usage":` with bare quotes
    # can only be a key. Any object can carry one, so all are kept and the
    # caller charges the highest.
    text = tail.decode("utf-8", "replace")
    decoder = json.JSONDecoder()
    values = []
    for match in pattern.finditer(text):
        try:
            value, _ = decoder.raw_decode(text, match.end())
        except ValueError:
            continue
        values.append(value)
    return values


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
