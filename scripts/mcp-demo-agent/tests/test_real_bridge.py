"""The agent against Klipolo's own stdio bridge, with Klipolo closed: skipped unless KLIPOLO_BRIDGE names a program that
runs the bridge for `--mcp` (klipolo-app itself). Meta is mocked here too, so it costs nothing.

The bridge gets an empty support folder (KLIPOLO_SUPPORT_DIR), so it finds no Klipolo listening: it still answers
`initialize` and `tools/list` itself, and refuses each call with its own sentence, so the run stops at its first read,
before Muse Spark is asked. This checks that the SDK's client and the bridge agree on the protocol, and that
`fixtures/tools_list.json` is still what Klipolo lists.
"""

import asyncio
import io
import json
import os
from pathlib import Path

import httpx
import pytest
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

import agent
from test_agent import KEY, Meta

BRIDGE = os.environ.get("KLIPOLO_BRIDGE")
FIXTURES = Path(__file__).resolve().parent / "fixtures"
# `words::OFF`.
OFF = (
    'Klipolo isn\'t running, or "Let other apps edit with Klipolo" is off. Ask the person to open Klipolo and turn it '
    "on in Settings."
)

pytestmark = pytest.mark.skipif(not BRIDGE, reason="KLIPOLO_BRIDGE names no bridge to run")


async def listing():
    """The bridge's instructions and tools, as the agent's SDK reads them."""
    params = StdioServerParameters(command=BRIDGE, args=["--mcp"], env=agent.klipolo_env())
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            hello = await session.initialize()
            return hello.instructions, (await session.list_tools()).tools


def test_the_real_bridge_lists_klipolos_tools_and_says_when_klipolo_is_closed(tmp_path, monkeypatch):
    monkeypatch.setenv("KLIPOLO_SUPPORT_DIR", str(tmp_path))
    tools = json.loads((FIXTURES / "tools_list.json").read_text())["tools"]
    instructions = json.loads((FIXTURES / "initialize.json").read_text())["instructions"]
    meta = Meta()
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
    assert (code, out.getvalue(), err.getvalue()) == (1, "Cost: $0.000\n", OFF + "\n")
    assert meta.requests == [], "Muse Spark is never asked"
    said, listed = asyncio.run(listing())
    offered = [agent.function_tool(t.name, t.description, t.inputSchema) for t in listed]
    assert offered == [agent.function_tool(t["name"], t["description"], t["inputSchema"]) for t in tools]
    assert all(tool["strict"] for tool in offered)
    assert said == instructions
