"""The agent against Klipolo's own stdio bridge, with Klipolo closed: skipped unless KLIPOLO_BRIDGE names a program that
runs the bridge for `--mcp` (klipolo-app itself). Meta is mocked here too, so it costs nothing.

The bridge gets an empty support folder (KLIPOLO_SUPPORT_DIR), so it finds no Klipolo listening: it still answers
`initialize` and `tools/list` itself, and refuses each call with its own sentence. This checks that the SDK's client
and the bridge agree on the protocol, and that `fixtures/tools_list.json` is still what Klipolo lists.
"""

import asyncio
import io
import json
import os
from pathlib import Path

import httpx
import pytest

import agent
from test_agent import KEY, Meta, call, response

BRIDGE = os.environ.get("KLIPOLO_BRIDGE")
FIXTURES = Path(__file__).resolve().parent / "fixtures"
# `words::OFF`.
OFF = (
    'Klipolo isn\'t running, or "Let other apps edit with Klipolo" is off. Ask the person to open Klipolo and turn it '
    "on in Settings."
)

pytestmark = pytest.mark.skipif(not BRIDGE, reason="KLIPOLO_BRIDGE names no bridge to run")


def test_the_real_bridge_lists_klipolos_tools_and_says_when_klipolo_is_closed(tmp_path, monkeypatch):
    monkeypatch.setenv("KLIPOLO_SUPPORT_DIR", str(tmp_path))
    tools = json.loads((FIXTURES / "tools_list.json").read_text())["tools"]
    instructions = json.loads((FIXTURES / "initialize.json").read_text())["instructions"]
    meta = Meta(response(call("getContext", {}, "call_1"), usage=(6000, 0, 40)))
    out, err = io.StringIO(), io.StringIO()
    code = asyncio.run(
        agent.run(
            "Make a teaser.",
            command=[BRIDGE, "--mcp"],
            model=agent.DEFAULT_MODEL,
            key=KEY,
            out=out,
            err=err,
            transport=httpx.MockTransport(meta),
        )
    )
    assert (code, err.getvalue()) == (1, "")
    assert out.getvalue() == f"→ getContext {{}}\n← {OFF}\n\nCost: $0.008\n"
    body = meta.bodies()[0]
    assert body["tools"] == [agent.function_tool(t["name"], t["description"], t["inputSchema"]) for t in tools]
    assert all(tool["strict"] for tool in body["tools"])
    assert body["instructions"] == agent.instructions_for(instructions)
