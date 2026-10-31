#!/usr/bin/env python3
"""Klipolo's demo agent: Muse Spark edits the video open in Klipolo through Klipolo's MCP server.

It starts Klipolo's stdio bridge (`klipolo-app --mcp`), lists Klipolo's tools and offers each to Muse Spark as a strict
function tool on Meta's Responses API, then makes the calls Muse Spark asks for and sends each result back, for at most
8 model turns. It prints each call on one line, then Muse Spark's reply, then what Muse Spark's tokens cost. Pictures
(makeSticker, makeTitleCard) are paid by Klipolo, about $0.01 each, and aren't in that total: it says how many it
started.

Run it on the Mac with Klipolo open on an analyzed project and "Let other apps edit with Klipolo" on in Settings:

    cd scripts/mcp-demo-agent
    python3.12 -m venv .venv
    .venv/bin/pip install -r requirements.txt
    .venv/bin/python agent.py "In Klipolo, make a 30 second vertical teaser of the best moment, with karaoke captions."

The Meta key comes from MODEL_API_KEY in the environment only. It goes to Meta and nowhere else: not to Klipolo, not to
the screen, not into an error. Options: --klipolo <path to klipolo-app> (the installed app's by default),
--model <a Muse Spark model> (muse-spark-1.3 by default) and --max-usd <dollars> (0.10 by default: once Muse Spark's
tokens have cost that much, it asks Muse Spark no more). KLIPOLO_META_BASE overrides Meta's base URL, as it does for
Klipolo (tests, staging).

Cuts are only proposed: they wait for the person's Apply in Klipolo. Run it again afterwards ("I clicked Apply. Is the
teaser ready?") and it reads what became of the proposal with getProposal.

Exit status: 0 with a reply, 1 when the run stopped (Klipolo not running or not answering, a Meta error, the turn or
spend limit), 2 for a missing key or a wrong option. Every run that started Klipolo prints its cost last, however it
ended.
"""

from __future__ import annotations

import argparse
import asyncio
import copy
import json
import math
import os
import re
import sys
from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any, NamedTuple, TextIO

import anyio
import httpx
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import get_default_environment, stdio_client
from mcp.shared.exceptions import McpError
from mcp.types import CONNECTION_CLOSED, Implementation

VERSION = "0.1.0"
# How Klipolo shows this app: "Connected: Muse Spark agent", and "(from Muse Spark agent)" on its changes.
CLIENT_NAME = "Muse Spark agent"
DEFAULT_KLIPOLO = "/Applications/Klipolo.app/Contents/MacOS/klipolo-app"
DEFAULT_MODEL = "muse-spark-1.3"
KEY_ENV = "MODEL_API_KEY"
# Klipolo's Meta adapter: the base URL, and the variable that overrides it (`klipolo-meta`, client.rs).
DEFAULT_META_BASE = "https://api.meta.ai/v1"
META_BASE_ENV = "KLIPOLO_META_BASE"
# Klipolo's support folder, when it isn't the usual one: the bridge reads it to find Klipolo.
SUPPORT_DIR_ENV = "KLIPOLO_SUPPORT_DIR"
MAX_TURNS = 8
# The most a run may cost before the agent stops asking Muse Spark (--max-usd), in US dollars.
MAX_USD = 0.10
# The bridge answers its hello and its tool list itself, at once; a program that doesn't is not the bridge.
HELLO_TIMEOUT_S = 15
# Longer than the bridge's own 30 s for a request plus its longest 60 s pace (`bridge::REQUEST_TIMEOUT`, `LONGEST_PACE`).
CALL_TIMEOUT_S = 120
# As Klipolo's chat asks (`meta_chat::CHAT_MAX_OUTPUT_TOKENS`, `CHAT_ANSWER_TIMEOUT`; `client::SHORT_TIMEOUT`).
MAX_OUTPUT_TOKENS = 8192
ANSWER_TIMEOUT_S = 90.0
CONNECT_TIMEOUT_S = 30.0
# Muse Spark's list prices per token (`klipolo-meta`, pricing.rs).
SPARK_INPUT_USD = 1.25 / 1_000_000
SPARK_CACHED_INPUT_USD = 0.15 / 1_000_000
SPARK_OUTPUT_USD = 4.25 / 1_000_000
# Pictures Klipolo draws with Muse Image for these tools: billed in Klipolo, not in this total (`pricing::IMAGE_USD`).
PICTURES = ("makeSticker", "makeTitleCard")
PICTURE_USD = 0.01
# The most of Meta's own message kept in an error (`error::MAX_MESSAGE_CHARS`).
MAX_MESSAGE_CHARS = 300
# The most of a call's arguments printed on its line.
MAX_SHOWN = 200

NO_KEY = "Set MODEL_API_KEY to your Meta key."
# How the bridge's answer starts while Klipolo isn't listening (`words::OFF`).
KLIPOLO_OFF = "Klipolo isn't running"
UNREACHABLE = "Could not reach Meta. Check your internet connection and try again."
TOO_SLOW = "Meta took too long to answer. Try again."
UNREADABLE = "Meta sent an answer the agent could not read. Try again."
NOT_AN_OBJECT = "The arguments were not a JSON object. Call the tool again with a JSON object."
NOT_A_BRIDGE = "{exe} did not answer as Klipolo's MCP bridge. Pass --klipolo with a Klipolo that has it."
STOPPED_ANSWERING = "Klipolo stopped answering. Check that it's open, then run the agent again."
PROPOSAL_TOOL = "getProposal"
# What a getProposal hears in the run that proposed (Klipolo's own texts say to ask it while the person decides).
WAITS_FOR_APPLY = (
    "The person hasn't decided yet, and this run can't wait for them. Tell them the cut waits for their Apply in "
    "Klipolo, and stop."
)
# What the SDK raises once the bridge's pipes are closed.
CLOSED = (anyio.ClosedResourceError, anyio.BrokenResourceError, anyio.EndOfStream)

SYSTEM_PROMPT = (
    "You edit the video open in Klipolo, a Mac video editor, with Klipolo's tools. "
    "Call getContext first, and take every time and clip from it. "
    "Format, caption and text changes apply at once. "
    "Cuts, such as keepOnly, are only proposed: nothing is cut until the person clicks Apply in Klipolo. "
    "After you propose a cut, say that it waits for their Apply, and stop. "
    "Call getProposal only when the person asks what became of it. "
    "You may put your own short title on screen with addText. "
    "Never say a cut was made unless getProposal says it was applied. "
    "When a tool refuses, read why, then fix the call or tell the person. "
    "Answer in two or three short sentences."
)

# Keywords that let an object take keys it doesn't list: strict mode can't say that.
_OPEN_KEYWORDS = ("patternProperties", "unevaluatedProperties", "propertyNames")


class _NotStrict(Exception):
    """A schema strict mode can't express."""


def strict_schema(schema: dict[str, Any]) -> tuple[dict[str, Any], bool]:
    """`schema` as the Responses API's strict mode takes it, and True; or `schema` unchanged and False when strict mode
    can't express it (an object that takes keys it doesn't list).

    Every object gets `additionalProperties: false` and lists every property in `required` (the ones it required
    first, in their order); a property it didn't require becomes nullable, so the model sends null for "unsaid".
    `without_added_nulls` takes those nulls back out. A schema that is strict already comes back unchanged, key order
    and all. The top level's `$schema` goes: it describes the schema, it is no constraint.
    """
    try:
        out = _strict(copy.deepcopy(schema) if isinstance(schema, dict) else {}, top=True)
    except _NotStrict:
        return schema, False
    out.pop("$schema", None)
    return out, True


def _is_object(schema: dict[str, Any]) -> bool:
    kind = schema.get("type")
    if isinstance(kind, list):
        return "object" in kind
    return kind == "object" or (kind is None and "properties" in schema)


def _strict(schema: Any, top: bool = False) -> Any:
    if not isinstance(schema, dict):
        return schema
    if top and "type" not in schema:
        schema["type"] = "object"
    if _is_object(schema):
        if any(key in schema for key in _OPEN_KEYWORDS):
            raise _NotStrict
        if schema.get("additionalProperties", False) is not False:
            raise _NotStrict
        properties = schema.get("properties") or {}
        required = [name for name in schema.get("required") or [] if name in properties]
        closed = {}
        for name, sub in properties.items():
            sub = _strict(sub)
            closed[name] = sub if name in required else _nullable(sub)
        schema["properties"] = closed
        schema["required"] = required + [name for name in properties if name not in required]
        schema["additionalProperties"] = False
    items = schema.get("items")
    if isinstance(items, dict):
        schema["items"] = _strict(items)
    elif isinstance(items, list):
        schema["items"] = [_strict(item) for item in items]
    for key in ("anyOf", "oneOf", "allOf", "prefixItems"):
        if isinstance(schema.get(key), list):
            schema[key] = [_strict(branch) for branch in schema[key]]
    for key in ("$defs", "definitions"):
        if isinstance(schema.get(key), dict):
            schema[key] = {name: _strict(sub) for name, sub in schema[key].items()}
    return schema


def _nullable(schema: Any) -> Any:
    """`schema` that also takes null."""
    if not isinstance(schema, dict):
        return schema
    kind = schema.get("type")
    if isinstance(kind, str) and "const" not in schema:
        if kind != "null":
            schema["type"] = [kind, "null"]
    elif isinstance(kind, list) and "const" not in schema:
        if "null" not in kind:
            schema["type"] = [*kind, "null"]
    elif isinstance(schema.get("anyOf"), list) and "type" not in schema:
        if not any(isinstance(b, dict) and b.get("type") == "null" for b in schema["anyOf"]):
            schema["anyOf"] = [*schema["anyOf"], {"type": "null"}]
        return schema
    else:
        return {"anyOf": [schema, {"type": "null"}]}
    if isinstance(schema.get("enum"), list) and None not in schema["enum"]:
        schema["enum"] = [*schema["enum"], None]
    return schema


def without_added_nulls(value: Any, schema: Any) -> Any:
    """`value` (the model's arguments) without the nulls `strict_schema` let it send for properties `schema` (the
    tool's own schema) doesn't require: those were left unsaid. A null for a property the tool requires is the
    model's own and stays (Klipolo reads it as "keep as it is")."""
    if not isinstance(schema, dict):
        return value
    properties = schema.get("properties")
    if isinstance(value, dict) and isinstance(properties, dict):
        required = set(schema.get("required") or [])
        out = {}
        for name, sub_value in value.items():
            if sub_value is None and name in properties and name not in required:
                continue
            out[name] = without_added_nulls(sub_value, properties.get(name))
        return out
    if isinstance(value, list) and isinstance(schema.get("items"), dict):
        return [without_added_nulls(item, schema["items"]) for item in value]
    for key in ("anyOf", "oneOf"):
        branches = schema.get(key)
        if isinstance(value, dict) and isinstance(branches, list):
            for branch in branches:
                if isinstance(branch, dict) and isinstance(branch.get("properties"), dict) and set(value) <= set(
                    branch["properties"]
                ):
                    return without_added_nulls(value, branch)
    return value


def function_tool(name: str, description: str | None, input_schema: dict[str, Any]) -> dict[str, Any]:
    """An MCP tool as a function tool for Meta's Responses API, strict when its schema can be."""
    parameters, strict = strict_schema(input_schema)
    return {
        "type": "function",
        "name": name,
        "description": description or "",
        "parameters": parameters,
        "strict": strict,
    }


# Meta's Responses API, called as Klipolo's Meta adapter calls it (`klipolo-meta`: client.rs, responses.rs, error.rs,
# pricing.rs; `klipolo-analysis`: providers/meta_chat.rs).


class MetaError(Exception):
    """A Meta call that gave no usable answer: one plain sentence, with the key already taken out."""

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


@dataclass
class Usage:
    """What one answer used, as Meta bills it. `cached_tokens` is the part of `input_tokens` Meta served from cache."""

    input_tokens: int = 0
    cached_tokens: int = 0
    output_tokens: int = 0


@dataclass
class FunctionCall:
    name: str
    # Echoed back in the `function_call_output` that answers the call.
    call_id: str
    # The arguments as the JSON text the model wrote.
    arguments: str


@dataclass
class Answer:
    # The `output_text` parts of each message item joined, with a newline between items; None when it only called tools.
    text: str | None
    calls: list[FunctionCall]
    # The answer's messages and calls as input items for the next turn: no item ids, no reasoning.
    replay: list[dict[str, Any]]
    usage: Usage
    # `completed`, or `incomplete` (cut off) or `failed`.
    status: str | None
    raw: dict[str, Any] = field(repr=False)


def _count(value: Any) -> int:
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else 0


def usage_of(usage: Any) -> Usage:
    """An answer's `usage`; zeros for what it doesn't report."""
    if not isinstance(usage, dict):
        return Usage()
    details = usage.get("input_tokens_details")
    cached = details.get("cached_tokens") if isinstance(details, dict) else None
    return Usage(_count(usage.get("input_tokens")), _count(cached), _count(usage.get("output_tokens")))


def spark_usd(usage: Usage) -> float:
    """What a Muse Spark answer costs: cached input at the cached price, the rest of the input and all output (reasoning
    included) at the list price."""
    cached = min(usage.cached_tokens, usage.input_tokens)
    return (
        (usage.input_tokens - cached) * SPARK_INPUT_USD
        + cached * SPARK_CACHED_INPUT_USD
        + usage.output_tokens * SPARK_OUTPUT_USD
    )


def parse_answer(raw: Any) -> Answer:
    """A Responses API answer."""
    if not isinstance(raw, dict):
        raise MetaError(UNREADABLE)
    items = raw.get("output") if isinstance(raw.get("output"), list) else []
    texts: list[str] = []
    calls: list[FunctionCall] = []
    replay: list[dict[str, Any]] = []
    for item in items:
        if not isinstance(item, dict):
            continue
        kind = item.get("type")
        if kind == "message":
            parts = [
                part["text"]
                for part in item.get("content") or []
                if isinstance(part, dict) and part.get("type") == "output_text" and isinstance(part.get("text"), str)
            ]
            if parts:
                text = "".join(parts)
                texts.append(text)
                replay.append({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]})
        elif kind == "function_call":
            name, arguments = item.get("name"), item.get("arguments")
            if not isinstance(name, str) or not isinstance(arguments, str):
                raise MetaError(UNREADABLE)
            call_id = next((item[k] for k in ("call_id", "id") if isinstance(item.get(k), str)), "")
            calls.append(FunctionCall(name=name, call_id=call_id, arguments=arguments))
            replay.append({"type": "function_call", "call_id": call_id, "name": name, "arguments": arguments})
    if texts:
        text: str | None = "\n".join(texts)
    else:
        text = raw["output_text"] if isinstance(raw.get("output_text"), str) else None
    status = raw["status"] if isinstance(raw.get("status"), str) else None
    return Answer(text=text, calls=calls, replay=replay, usage=usage_of(raw.get("usage")), status=status, raw=raw)


def _code(raw: dict[str, Any], *path: str) -> str:
    """A code from an answer, for the person: only lowercase letters, digits and underscores, never free text."""
    value: Any = raw
    for key in path:
        value = value.get(key) if isinstance(value, dict) else None
    if isinstance(value, str) and re.fullmatch(r"[a-z0-9_]{1,64}", value):
        return value
    return "no reason given"


def cut_short(answer: Answer) -> str | None:
    """Why an answer can't be used, or None. A `failed` or `incomplete` answer's calls may be partial: none is made. It
    is billed all the same."""
    if answer.status == "failed":
        return f"Meta's answer failed ({_code(answer.raw, 'error', 'code')})."
    if answer.status == "incomplete":
        return f"Meta's answer was cut off ({_code(answer.raw, 'incomplete_details', 'reason')})."
    return None


def redact(text: str, key: str) -> str:
    """`text` with the key taken out. Run before any shortening, so no part of the key survives."""
    return text.replace(key, "[redacted]") if key else text


def _plain(text: str) -> str:
    """Server text without em or en dashes."""
    text = re.sub("\\s+[\u2013\u2014]\\s+", ", ", text)
    return text.replace("\u2013", "-").replace("\u2014", "-")


def sentence(text: str | None, fallback: str) -> str:
    """`text` as plain copy ending in a full stop, or `fallback` when it is blank."""
    text = _plain(text or "").strip()
    if not text:
        return fallback
    return text if text.endswith((".", "!", "?")) else f"{text}."


def _json(body: bytes) -> Any:
    try:
        return json.loads(body)
    except ValueError:
        return None


def body_message(body: bytes, key: str) -> str | None:
    """The message in an error body (`{"error": {"message": ...}}`), redacted, at most `MAX_MESSAGE_CHARS`. Not JSON,
    blank text or a bare code gives none."""
    value = _json(body)
    if not isinstance(value, dict):
        return None
    error = value.get("error")
    if isinstance(error, dict) and "message" in error:
        found = error["message"]
    elif "message" in value:
        found = value["message"]
    else:
        found = error
    if not isinstance(found, str):
        return None
    text = redact(" ".join(found.split()), key)
    if re.fullmatch(r"[a-z0-9_]*", text):
        return None
    return text[:MAX_MESSAGE_CHARS] + "..." if len(text) > MAX_MESSAGE_CHARS else text


def body_code(body: bytes, key: str) -> str | None:
    """The error code in an error body (`{"error": {"code": ...}}`): only a code, never words."""
    value = _json(body)
    error = value.get("error") if isinstance(value, dict) else None
    code = error if isinstance(error, str) else error.get("code") if isinstance(error, dict) else None
    if not isinstance(code, str):
        return None
    code = redact(code, key)
    return code if re.fullmatch(r"[a-z0-9_]{1,64}", code) else None


def from_status(status: int, body: bytes, key: str) -> str:
    """What a refused call means, in one sentence, with Meta's own message when it gave one."""
    message = body_message(body, key)
    if status == 401:
        return "Meta did not accept the key in MODEL_API_KEY."
    if status == 402:
        return sentence(message, "Your Meta credit has run out.")
    if status == 403:
        return sentence(message, "Your Meta API key does not allow this.")
    if status == 429:
        return sentence(message, "Too many requests right now. Try again in a moment.")
    if status in (408, 504):
        return sentence(message, TOO_SLOW)
    if 500 <= status <= 599:
        return sentence(message, "Meta is busy right now. Try again in a moment.")
    if body_code(body, key) == "content_policy_violation":
        return "Meta turned this down under its content policy. Try wording it differently."
    if message:
        return f"Meta could not process this request: {sentence(message, '')}"
    return f"Meta could not process this request (status {status})."


def request_body(model: str, instructions: str, tools: list[dict[str, Any]], inputs: list[dict[str, Any]]) -> dict:
    """One turn as Klipolo's chat sends it: strict function tools, calls in parallel, Meta keeps no copy (`store`
    false), and the whole conversation each time (nothing stored at Meta to refer to)."""
    return {
        "model": model,
        "instructions": instructions,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": True,
        "max_output_tokens": MAX_OUTPUT_TOKENS,
        "store": False,
        "input": inputs,
    }


def user_message(text: str) -> dict[str, Any]:
    return {"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}


async def respond(http: httpx.AsyncClient, base: str, key: str, body: dict[str, Any]) -> Answer:
    """`POST {base}/responses` with the key as a bearer token; the answer, or a `MetaError`. Redirects are refused."""
    try:
        reply = await http.post(f"{base}/responses", json=body, headers={"Authorization": f"Bearer {key}"})
    except httpx.TimeoutException:
        raise MetaError(TOO_SLOW) from None
    except httpx.HTTPError:
        raise MetaError(UNREACHABLE) from None
    if not reply.is_success:
        raise MetaError(from_status(reply.status_code, reply.content, key))
    raw = _json(reply.content)
    if raw is None:
        raise MetaError(UNREADABLE)
    return parse_answer(raw)


def meta_base(environ: Any) -> str:
    """Meta's base URL: KLIPOLO_META_BASE when set, else Meta's own, without a trailing slash."""
    base = (environ.get(META_BASE_ENV) or "").strip() or DEFAULT_META_BASE
    return base.rstrip("/")


# Klipolo, through its MCP server.


class Console:
    """Where the run prints: lines for the person on `out`, why it stopped on `err`."""

    def __init__(self, out: TextIO, err: TextIO) -> None:
        self.out = out
        self.err = err
        self.printed = False

    def say(self, line: str = "") -> None:
        print(line, file=self.out, flush=True)
        self.printed = True

    def fail(self, line: str) -> None:
        print(line, file=self.err, flush=True)


def first_line(text: str) -> str:
    lines = text.strip().splitlines()
    return lines[0] if lines else ""


def shorten(text: str) -> str:
    return text if len(text) <= MAX_SHOWN else text[: MAX_SHOWN - 3] + "..."


def outcome(result: Any, text: str) -> str:
    """A call's result in a few words: applied, proposed, the proposal's status, what was read, or Klipolo's first
    line."""
    if result.isError:
        return first_line(text) or "refused"
    structured = result.structuredContent if isinstance(result.structuredContent, dict) else {}
    kind = structured.get("outcome")
    if kind == "done" and text.startswith("Done in Klipolo"):
        return "applied"
    if kind == "proposed":
        removed = structured.get("removedSec")
        if isinstance(removed, (int, float)) and not isinstance(removed, bool):
            return f"proposed, waits for Apply ({removed:.1f} s would go)"
        return "proposed, waits for Apply"
    if isinstance(structured.get("status"), str):
        return structured["status"]
    lines = text.splitlines()
    if len(lines) > 1:
        return f"read, {len(lines)} lines"
    return first_line(text)


def gone(error: McpError) -> bool:
    """Whether the SDK gave up on the bridge: its stream closed, or no answer came in `CALL_TIMEOUT_S`. Klipolo's own
    refusals carry other words."""
    data = error.error
    closed = data.code == CONNECTION_CLOSED and data.message == "Connection closed"
    return closed or data.code == httpx.codes.REQUEST_TIMEOUT


class Called(NamedTuple):
    """What came of one call."""

    # What goes back to the model.
    output: str
    # The run must end: Klipolo isn't running, or stopped answering.
    stop: bool = False
    # Klipolo's `structuredContent.outcome` (`done`, `proposed`, `started`...), when it gave one.
    outcome: str | None = None


async def run_call(
    session: Any, call: FunctionCall, schemas: dict[str, Any], console: Console, proposed: bool = False
) -> Called:
    """Makes one call in Klipolo and prints it. A refusal goes back as Klipolo wrote it (its refusals are written for
    agents). Once this run has `proposed`, a `getProposal` is answered here: the person can't have decided yet, and
    Klipolo's texts would have the model ask again and again."""
    try:
        arguments = json.loads(call.arguments) if call.arguments.strip() else {}
    except ValueError:
        arguments = None
    if not isinstance(arguments, dict):
        console.say(f"→ {call.name} {shorten(call.arguments)}")
        console.say(f"← {NOT_AN_OBJECT}")
        return Called(NOT_AN_OBJECT)
    if call.name in schemas:
        arguments = without_added_nulls(arguments, schemas[call.name])
    console.say(f"→ {call.name} {shorten(json.dumps(arguments, ensure_ascii=False))}")
    if proposed and call.name == PROPOSAL_TOOL:
        console.say("← waits for Apply")
        return Called(WAITS_FOR_APPLY)
    try:
        result = await session.call_tool(call.name, arguments)
    except McpError as e:
        if gone(e):
            console.say(f"← {STOPPED_ANSWERING}")
            return Called(STOPPED_ANSWERING, stop=True)
        said = e.error.message or "Klipolo refused the call."
        console.say(f"← {first_line(said)}")
        return Called(said)
    except CLOSED:
        console.say(f"← {STOPPED_ANSWERING}")
        return Called(STOPPED_ANSWERING, stop=True)
    text = "\n".join(part.text for part in result.content if getattr(part, "type", None) == "text")
    structured = result.structuredContent if isinstance(result.structuredContent, dict) else {}
    kind = structured.get("outcome") if isinstance(structured.get("outcome"), str) else None
    if kind == "started" and call.name in PICTURES:
        console.say(f"← started, a picture billed in Klipolo (about ${PICTURE_USD:.2f})")
    else:
        console.say(f"← {outcome(result, text)}")
    return Called(text, stop=bool(result.isError) and text.startswith(KLIPOLO_OFF), outcome=kind)


def klipolo_env() -> dict[str, str]:
    """The environment Klipolo's bridge starts with: the SDK's short safe list (HOME, PATH, USER...), and Klipolo's
    support folder when set. Never the Meta key."""
    env = get_default_environment()
    if os.environ.get(SUPPORT_DIR_ENV):
        env[SUPPORT_DIR_ENV] = os.environ[SUPPORT_DIR_ENV]
    return env


def instructions_for(server_instructions: str | None) -> str:
    if not server_instructions:
        return SYSTEM_PROMPT
    return f"{SYSTEM_PROMPT}\n\nKlipolo's instructions for apps:\n{server_instructions}"


async def run(
    request: str,
    *,
    command: list[str],
    model: str,
    key: str,
    out: TextIO,
    err: TextIO,
    transport: httpx.AsyncBaseTransport | None = None,
    meta_base: str = DEFAULT_META_BASE,
    max_turns: int = MAX_TURNS,
    max_usd: float = MAX_USD,
    hello_timeout: float = HELLO_TIMEOUT_S,
    call_timeout: float = CALL_TIMEOUT_S,
) -> int:
    """One request, start to end: Klipolo started with `command`, Muse Spark asked at most `max_turns` times, and not
    again once its tokens have cost `max_usd` (the calls it already asked for are still made). The bridge gets
    `hello_timeout` seconds for its hello and tool list, and `call_timeout` for each call. The exit status; the cost
    of Muse Spark's tokens is printed last, whatever happened, then the pictures Klipolo bills itself, if any."""
    console = Console(out, err)
    tally = Tally()
    params = StdioServerParameters(command=command[0], args=list(command[1:]), env=klipolo_env())
    client = Implementation(name=CLIENT_NAME, version=VERSION)
    code: int | None = None
    try:
        async with stdio_client(params) as (read, write):
            async with ClientSession(
                read, write, read_timeout_seconds=timedelta(seconds=call_timeout), client_info=client
            ) as session:
                code = await converse(
                    session,
                    request,
                    console,
                    tally,
                    exe=command[0],
                    model=model,
                    key=key,
                    transport=transport,
                    meta_base=meta_base,
                    max_turns=max_turns,
                    max_usd=max_usd,
                    hello_timeout=hello_timeout,
                )
    except Exception as e:
        # The SDK's own tasks met the bridge's closed pipes: it is gone, whatever the run was doing.
        if not only_closed(e):
            raise
        if code is None:
            console.fail(STOPPED_ANSWERING if tally.said_hello else NOT_A_BRIDGE.format(exe=command[0]))
    finally:
        if console.printed:
            console.say()
        console.say(f"Cost: ${tally.cost:.3f}")
        if tally.pictures:
            pictures = "1 picture" if tally.pictures == 1 else f"{tally.pictures} pictures"
            console.say(f"Not in this total: {pictures} billed in Klipolo, about ${PICTURE_USD:.2f} each.")
    return 1 if code is None else code


@dataclass
class Tally:
    """What a run has come to so far."""

    # Muse Spark's tokens, in US dollars.
    cost: float = 0.0
    # The bridge answered the hello and listed its tools.
    said_hello: bool = False
    # Pictures started in Klipolo, which bills them itself.
    pictures: int = 0


def _leaves(error: BaseException) -> list[BaseException]:
    inner = getattr(error, "exceptions", None)
    if isinstance(inner, (list, tuple)) and inner:
        return [leaf for e in inner for leaf in _leaves(e)]
    return [error]


def only_closed(error: BaseException) -> bool:
    """Whether `error` (or every error of its group) is a closed or broken pipe."""
    return all(isinstance(leaf, CLOSED) for leaf in _leaves(error))


async def converse(
    session: Any,
    request: str,
    console: Console,
    tally: Tally,
    *,
    exe: str,
    model: str,
    key: str,
    transport: httpx.AsyncBaseTransport | None,
    meta_base: str,
    max_turns: int,
    max_usd: float,
    hello_timeout: float,
) -> int:
    """The bridge's hello and tool list, then the model's turns and calls. The exit status."""
    try:
        with anyio.fail_after(hello_timeout):
            hello = await session.initialize()
            listed = await session.list_tools()
    except (TimeoutError, McpError, *CLOSED):
        console.fail(NOT_A_BRIDGE.format(exe=exe))
        return 1
    tally.said_hello = True
    tools = [function_tool(t.name, t.description, t.inputSchema) for t in listed.tools]
    schemas = {t.name: t.inputSchema for t in listed.tools}
    instructions = instructions_for(hello.instructions)
    inputs = [user_message(request)]
    # A call of this run left a proposal waiting for the person.
    proposed = False
    timeout = httpx.Timeout(ANSWER_TIMEOUT_S, connect=CONNECT_TIMEOUT_S)
    headers = {"User-Agent": f"klipolo-mcp-demo-agent/{VERSION}"}
    async with httpx.AsyncClient(transport=transport, timeout=timeout, headers=headers) as http:
        for turn in range(1, max_turns + 1):
            if tally.cost >= max_usd:
                console.fail(f"Stopped at the ${max_usd:.2f} limit. Run again with --max-usd to allow more.")
                return 1
            try:
                answer = await respond(http, meta_base, key, request_body(model, instructions, tools, inputs))
            except MetaError as e:
                console.fail(e.message)
                return 1
            tally.cost += spark_usd(answer.usage)
            problem = cut_short(answer)
            if problem:
                console.fail(problem)
                return 1
            if not answer.calls:
                if console.printed:
                    console.say()
                console.say(answer.text or "Muse Spark gave no reply.")
                return 0
            if turn == max_turns:
                console.fail(f"Stopped after {max_turns} model turns without a final reply.")
                return 1
            inputs.extend(answer.replay)
            for call in answer.calls:
                called = await run_call(session, call, schemas, console, proposed=proposed)
                proposed = proposed or called.outcome == "proposed"
                if called.outcome == "started" and call.name in PICTURES:
                    tally.pictures += 1
                inputs.append({"type": "function_call_output", "call_id": call.call_id, "output": called.output})
                if called.stop:
                    return 1
    return 1


def describe(error: BaseException) -> str:
    """An error in one line, the errors of a group one after another."""
    inner = getattr(error, "exceptions", None)
    if isinstance(inner, (list, tuple)) and inner:
        return "; ".join(describe(e) for e in inner)
    said = str(error)
    return f"{type(error).__name__}: {said}" if said else type(error).__name__


def main(
    argv: list[str] | None = None,
    *,
    environ: Any = None,
    out: TextIO | None = None,
    err: TextIO | None = None,
    transport: httpx.AsyncBaseTransport | None = None,
) -> int:
    out = out or sys.stdout
    err = err or sys.stderr
    environ = os.environ if environ is None else environ
    parser = argparse.ArgumentParser(prog="agent.py", description="Muse Spark edits the video open in Klipolo.")
    parser.add_argument("request", nargs="+", help="what to do, in plain words")
    parser.add_argument("--klipolo", default=DEFAULT_KLIPOLO, help="the path to klipolo-app (default: %(default)s)")
    parser.add_argument("--model", default=DEFAULT_MODEL, help="the Muse Spark model (default: %(default)s)")
    parser.add_argument(
        "--max-usd",
        type=float,
        default=MAX_USD,
        help=(
            "stop asking Muse Spark once its tokens have cost this many US dollars (default: %(default).2f); the turn "
            "that reaches it can go past it, and pictures Klipolo draws are billed in Klipolo and not counted"
        ),
    )
    args = parser.parse_args(argv)

    key = (environ.get(KEY_ENV) or "").strip()
    if not key:
        print(NO_KEY, file=err)
        return 2
    if not args.model.startswith("muse-spark-"):
        print(f"--model takes a Muse Spark model, such as {DEFAULT_MODEL}.", file=err)
        return 2
    if not (math.isfinite(args.max_usd) and args.max_usd > 0):
        print(f"--max-usd takes an amount above 0 in US dollars, such as {MAX_USD:.2f}.", file=err)
        return 2
    if not (os.path.isfile(args.klipolo) and os.access(args.klipolo, os.X_OK)):
        print(f"Klipolo isn't installed at {args.klipolo}. Pass --klipolo with the path to klipolo-app.", file=err)
        return 2
    try:
        return asyncio.run(
            run(
                " ".join(args.request),
                command=[args.klipolo, "--mcp"],
                model=args.model,
                key=key,
                out=out,
                err=err,
                transport=transport,
                meta_base=meta_base(environ),
                max_usd=args.max_usd,
            )
        )
    except KeyboardInterrupt:
        return 130
    except Exception as e:  # noqa: BLE001 - one line for the person, never a traceback that could carry the key
        print(f"The agent stopped: {redact(describe(e), key)}", file=err)
        return 1


if __name__ == "__main__":
    sys.exit(main())
