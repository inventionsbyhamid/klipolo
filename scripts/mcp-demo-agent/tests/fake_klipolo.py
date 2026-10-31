#!/usr/bin/env python3
"""A stand-in for `klipolo-app --mcp` in the tests: an MCP server over stdio, made with the `mcp` SDK, that lists a few
of Klipolo's real tools (`fixtures/tools_list.json`) with Klipolo's real instructions and answers calls as Klipolo
words them.

    fake_klipolo.py --mcp [--mode on|off|silent|crash|slow] [--log <file>]

`--mode off` answers every call as the bridge does while Klipolo isn't running. `silent` never answers at all (a
program that isn't the bridge), `crash` dies on the first call, and `slow` takes 30 s over each call. `--log` appends
one JSON line when it starts (its argv, and whether MODEL_API_KEY reached it) and one per call (the tool and its
arguments).
"""

import argparse
import json
import os
import sys
from pathlib import Path

import anyio
import mcp.types as types
from mcp.server.lowlevel import Server
from mcp.server.stdio import stdio_server

FIXTURES = Path(__file__).resolve().parent / "fixtures"
TOOLS = json.loads((FIXTURES / "tools_list.json").read_text())["tools"]
INSTRUCTIONS = json.loads((FIXTURES / "initialize.json").read_text())["instructions"]
LISTED = ("cutRange", "keepOnly", "setFormat", "setCaptions", "addText", "getSummary", "getContext", "getProposal")

# What the bridge answers while Klipolo isn't listening (`words::OFF`).
OFF = (
    'Klipolo isn\'t running, or "Let other apps edit with Klipolo" is off. Ask the person to open Klipolo and turn it '
    "on in Settings."
)
CONTEXT = """Revision 3

Project "Launch talk", landscape 16:9, 182.4 s, captions off.
c1 0.0-182.4 talk.mov
[61.2-75.8] Maya: The moment we saw the first frame, we knew.
[75.8-91.0] Maya: Everything after that was just catching up.
Now: playhead 0.0, no selection."""
SUMMARY = 'Revision 3\n\nProject "Launch talk", landscape 16:9, 182.4 s.'
REFUSED = (
    "cutRange: Klipolo can't cut at 12.3 s: no word, pause, scene or clip edge is there, the person's selection or "
    "playhead in Klipolo, or the video's start or end. Use a time from getContext or getTranscript.\n"
    "Nothing changed."
)


def text(said: str) -> list[types.TextContent]:
    return [types.TextContent(type="text", text=said)]


def answer(name: str, arguments: dict, mode: str) -> types.CallToolResult:
    if mode == "off":
        return types.CallToolResult(content=text(OFF), isError=True)
    if name == "getContext":
        return types.CallToolResult(content=text(CONTEXT), isError=False)
    if name == "getSummary":
        return types.CallToolResult(content=text(SUMMARY), isError=False)
    if name == "getProposal":
        outcome = {"status": "applied", "id": "p1"}
        return types.CallToolResult(content=text(json.dumps(outcome)), structuredContent=outcome, isError=False)
    if name == "keepOnly":
        return types.CallToolResult(
            content=text(
                "Proposed in Klipolo (keepOnly): 152.6 s of footage would go (0.0-61.2, 91.0-182.4). Nothing is cut "
                "until the person clicks Apply in Klipolo's chat panel. Call getProposal to see what they decided."
            ),
            structuredContent={"outcome": "proposed", "revision": 5, "proposalId": "p1", "removedSec": 152.6},
            isError=False,
        )
    if name == "cutRange":
        return types.CallToolResult(content=text(REFUSED), structuredContent={"outcome": "refused"}, isError=True)
    return types.CallToolResult(
        content=text(f"Done in Klipolo ({name}). The person can undo it there."),
        structuredContent={"outcome": "done", "revision": 4},
        isError=False,
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mcp", action="store_true")
    parser.add_argument("--mode", choices=("on", "off", "silent", "crash", "slow"), default="on")
    parser.add_argument("--log")
    args = parser.parse_args()

    def log(entry: dict) -> None:
        if args.log:
            with open(args.log, "a", encoding="utf-8") as f:
                f.write(json.dumps(entry) + "\n")

    log({"started": sys.argv[1:], "has_key": "MODEL_API_KEY" in os.environ})
    if args.mode == "silent":
        for _ in sys.stdin:
            pass
        return
    server = Server("klipolo", instructions=INSTRUCTIONS)

    @server.list_tools()
    async def list_tools() -> list[types.Tool]:
        return [types.Tool(**tool) for tool in TOOLS if tool["name"] in LISTED]

    @server.call_tool()
    async def call_tool(name: str, arguments: dict) -> types.CallToolResult:
        log({"call": name, "arguments": arguments})
        if args.mode == "crash":
            os._exit(3)
        if args.mode == "slow":
            await anyio.sleep(30)
        return answer(name, arguments, args.mode)

    async def serve() -> None:
        async with stdio_server() as (read, write):
            await server.run(read, write, server.create_initialization_options())

    anyio.run(serve)


if __name__ == "__main__":
    main()
