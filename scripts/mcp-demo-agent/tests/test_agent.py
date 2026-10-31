"""The agent end to end: a fake Klipolo over stdio (`fake_klipolo.py`, made with the `mcp` SDK's own server) and Meta's
Responses API answered from a script through `httpx.MockTransport`. No real Meta call and no real Klipolo."""

import asyncio
import io
import json
import os
import shutil
import sys
from pathlib import Path

import anyio
import httpx
import pytest
from mcp.shared.exceptions import McpError
from mcp.types import ErrorData

import agent
import fake_klipolo

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"
KLIPOLO_TOOLS = {t["name"]: t for t in json.loads((FIXTURES / "tools_list.json").read_text())["tools"]}
INSTRUCTIONS = json.loads((FIXTURES / "initialize.json").read_text())["instructions"]
KEY = "test-meta-key-not-real-0123456789"
DEMO = "In Klipolo, make a 30 second vertical teaser of the best moment, with karaoke captions."
LISTED = [
    "cutRange",
    "keepOnly",
    "setFormat",
    "setCaptions",
    "addText",
    "makeTitleCard",
    "getSummary",
    "getContext",
    "getProposal",
]
FINAL = (
    "Your video is vertical now, with karaoke captions. I proposed keeping 61.2 to 91.0 s, the best moment, as a 30 "
    "second teaser. Nothing is cut until you click Apply in Klipolo."
)


class Meta:
    """Meta's Responses API, answering from a script (then `repeat`, if given); it keeps every request."""

    def __init__(self, *answers, repeat=None):
        self.answers = list(answers)
        self.repeat = repeat
        self.requests = []

    def __call__(self, request):
        self.requests.append(request)
        reply = self.answers.pop(0) if self.answers else self.repeat
        if isinstance(reply, Exception):
            raise reply
        if isinstance(reply, httpx.Response):
            return reply
        assert reply is not None, "Meta was asked once more than the script answers"
        return httpx.Response(200, json=reply)

    def bodies(self):
        return [json.loads(r.content) for r in self.requests]


def response(*output, usage=(1000, 0, 100), status="completed", **extra):
    tokens_in, cached, tokens_out = usage
    return {
        "id": "resp_1",
        "object": "response",
        "model": "muse-spark-1.3",
        "status": status,
        "output": list(output),
        "usage": {
            "input_tokens": tokens_in,
            "input_tokens_details": {"cached_tokens": cached},
            "output_tokens": tokens_out,
            "output_tokens_details": {"reasoning_tokens": 0},
            "total_tokens": tokens_in + tokens_out,
        },
        **extra,
    }


def reasoning():
    return {"type": "reasoning", "id": "rs_1", "summary": [], "status": "completed"}


def call(name, arguments, call_id):
    return {
        "type": "function_call",
        "id": f"fc_{call_id}",
        "call_id": call_id,
        "name": name,
        "arguments": json.dumps(arguments),
        "status": "completed",
    }


def message(text):
    return {
        "type": "message",
        "id": "msg_1",
        "role": "assistant",
        "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": []}],
    }


def demo_meta():
    """The demo: read, then format, captions and the teaser's cut at once, then the reply."""
    return Meta(
        response(reasoning(), call("getContext", {}, "call_1"), usage=(6000, 0, 150)),
        response(
            reasoning(),
            call("setFormat", {"format": "portrait", "keepOthers": True}, "call_2"),
            call("setCaptions", {"on": True, "style": "karaoke"}, "call_3"),
            call("keepOnly", {"ranges": [{"startSec": 61.2, "endSec": 91.0}]}, "call_4"),
            usage=(21000, 5800, 420),
        ),
        response(message(FINAL), usage=(22500, 20800, 90)),
    )


def klipolo(tmp_path, mode="on"):
    """The fake Klipolo's command, and the file it logs to."""
    log = tmp_path / "klipolo.jsonl"
    return [sys.executable, str(HERE / "fake_klipolo.py"), "--mcp", "--mode", mode, "--log", str(log)], log


def installed(tmp_path):
    """An executable that stands where klipolo-app would, starting the fake Klipolo with its own arguments."""
    log = tmp_path / "klipolo.jsonl"
    exe = tmp_path / "klipolo-app"
    exe.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{HERE / "fake_klipolo.py"}" "$@" --log "{log}"\n')
    exe.chmod(0o755)
    return exe, log


def logged(log):
    return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []


def calls_made(log):
    return [entry for entry in logged(log) if "call" in entry]


def run(meta, command, request=DEMO, **options):
    out, err = io.StringIO(), io.StringIO()
    code = asyncio.run(
        agent.run(
            request,
            command=command,
            model=agent.DEFAULT_MODEL,
            key=KEY,
            out=out,
            err=err,
            transport=httpx.MockTransport(meta),
            **options,
        )
    )
    return code, out.getvalue(), err.getvalue()


def main(argv, meta, environ=None):
    out, err = io.StringIO(), io.StringIO()
    code = agent.main(argv, environ=environ, out=out, err=err, transport=httpx.MockTransport(meta))
    return code, out.getvalue(), err.getvalue()


def test_the_demo_prints_each_call_the_reply_and_the_cost(tmp_path):
    meta = demo_meta()
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert (code, err) == (0, "")
    assert out == (
        "→ getContext {}\n"
        "← read, 7 lines\n"
        '→ setFormat {"format": "portrait", "keepOthers": true}\n'
        "← applied\n"
        '→ setCaptions {"on": true, "style": "karaoke"}\n'
        "← applied\n"
        '→ keepOnly {"ranges": [{"startSec": 61.2, "endSec": 91.0}]}\n'
        "← proposed, waits for Apply (152.6 s would go)\n"
        "\n"
        f"{FINAL}\n"
        "\n"
        "Cost: $0.035\n"
    )
    assert calls_made(log) == [
        {"call": "getContext", "arguments": {}},
        {"call": "setFormat", "arguments": {"format": "portrait", "keepOthers": True}},
        {"call": "setCaptions", "arguments": {"on": True, "style": "karaoke"}},
        {"call": "keepOnly", "arguments": {"ranges": [{"startSec": 61.2, "endSec": 91.0}]}},
    ]


def test_requests_are_sent_as_klipolos_meta_adapter_sends_them(tmp_path):
    meta = demo_meta()
    command, _ = klipolo(tmp_path)
    run(meta, command)
    assert len(meta.requests) == 3
    for request in meta.requests:
        assert request.method == "POST"
        assert str(request.url) == "https://api.meta.ai/v1/responses"
        assert request.headers["authorization"] == f"Bearer {KEY}"
        assert request.headers["content-type"] == "application/json"
    first, second, third = meta.bodies()
    for body in (first, second, third):
        assert sorted(body) == sorted(
            [
                "model",
                "instructions",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "max_output_tokens",
                "store",
                "input",
            ]
        )
        assert body["model"] == "muse-spark-1.3"
        assert body["tool_choice"] == "auto"
        assert body["parallel_tool_calls"] is True
        assert body["max_output_tokens"] == 8192
        assert body["store"] is False, "Meta keeps no copy"
        assert body["instructions"] == f"{agent.SYSTEM_PROMPT}\n\nKlipolo's instructions for apps:\n{INSTRUCTIONS}"
        assert [tool["name"] for tool in body["tools"]] == LISTED
        for tool in body["tools"]:
            listed = KLIPOLO_TOOLS[tool["name"]]
            assert tool == {
                "type": "function",
                "name": listed["name"],
                "description": listed["description"],
                "parameters": listed["inputSchema"],
                "strict": True,
            }
    asked = {"type": "message", "role": "user", "content": [{"type": "input_text", "text": DEMO}]}
    assert first["input"] == [asked]
    # Each call goes back as the model made it, without its item id and without the reasoning, then its result.
    assert second["input"] == [
        asked,
        {"type": "function_call", "call_id": "call_1", "name": "getContext", "arguments": "{}"},
        {"type": "function_call_output", "call_id": "call_1", "output": fake_klipolo.CONTEXT},
    ]
    assert third["input"][: len(second["input"])] == second["input"]
    assert third["input"][len(second["input"]) :] == [
        {
            "type": "function_call",
            "call_id": "call_2",
            "name": "setFormat",
            "arguments": '{"format": "portrait", "keepOthers": true}',
        },
        {"type": "function_call", "call_id": "call_3", "name": "setCaptions", "arguments": '{"on": true, "style": "karaoke"}'},
        {
            "type": "function_call",
            "call_id": "call_4",
            "name": "keepOnly",
            "arguments": '{"ranges": [{"startSec": 61.2, "endSec": 91.0}]}',
        },
        {
            "type": "function_call_output",
            "call_id": "call_2",
            "output": "Done in Klipolo (setFormat). The person can undo it there.",
        },
        {
            "type": "function_call_output",
            "call_id": "call_3",
            "output": "Done in Klipolo (setCaptions). The person can undo it there.",
        },
        {
            "type": "function_call_output",
            "call_id": "call_4",
            "output": (
                "Proposed in Klipolo (keepOnly): 152.6 s of footage would go (0.0-61.2, 91.0-182.4). Nothing is cut "
                "until the person clicks Apply in Klipolo's chat panel. Call getProposal to see what they decided."
            ),
        },
    ]


def test_cost_is_priced_from_usage_with_cached_input_at_the_cached_price():
    usage = agent.usage_of({"input_tokens": 1_000_000, "input_tokens_details": {"cached_tokens": 400_000}, "output_tokens": 1_000_000})
    assert usage == agent.Usage(input_tokens=1_000_000, cached_tokens=400_000, output_tokens=1_000_000)
    assert agent.spark_usd(usage) == pytest.approx(0.6 * 1.25 + 0.4 * 0.15 + 4.25)
    # The demo's three answers: 6000 in and 150 out; 21000 in (5800 cached) and 420 out; 22500 in (20800 cached), 90 out.
    total = sum(
        agent.spark_usd(agent.Usage(i, c, o)) for i, c, o in ((6000, 0, 150), (21000, 5800, 420), (22500, 20800, 90))
    )
    assert total == pytest.approx(0.03542)
    assert agent.spark_usd(agent.usage_of(None)) == 0


def test_a_refusal_goes_back_to_the_model_and_prints_its_first_line(tmp_path):
    meta = Meta(
        response(call("cutRange", {"startSec": 12.3, "endSec": 14.0}, "call_1")),
        response(message("Klipolo needs a time from the transcript, so nothing was cut.")),
    )
    command, _ = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert (code, err) == (0, "")
    assert out.splitlines()[:2] == [
        '→ cutRange {"startSec": 12.3, "endSec": 14.0}',
        "← " + fake_klipolo.REFUSED.splitlines()[0],
    ]
    assert meta.bodies()[1]["input"][-1] == {
        "type": "function_call_output",
        "call_id": "call_1",
        "output": fake_klipolo.REFUSED,
    }


def test_after_apply_the_agent_reads_the_proposal(tmp_path):
    meta = Meta(
        response(call("getProposal", {}, "call_1")),
        response(message("You applied it. The 30 second vertical teaser is ready in Klipolo.")),
    )
    command, _ = klipolo(tmp_path)
    code, out, err = run(meta, command, request="I clicked Apply. Is the teaser ready?")
    assert (code, err) == (0, "")
    assert out.splitlines()[:2] == ["→ getProposal {}", "← applied"]


def test_after_proposing_the_run_answers_getproposal_itself(tmp_path):
    # Klipolo's texts say to call getProposal, but in the run that proposed, the person can't have decided yet.
    waits = (
        "The person hasn't decided yet, and this run can't wait for them. Tell them the cut waits for their Apply in "
        "Klipolo, and stop."
    )
    meta = Meta(
        response(call("keepOnly", {"ranges": [{"startSec": 61.2, "endSec": 91.0}]}, "call_1")),
        response(call("getProposal", {}, "call_2")),
        response(call("getProposal", {}, "call_3")),
        response(call("getProposal", {}, "call_4")),
        response(message("The teaser's cut waits for your Apply in Klipolo.")),
    )
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert (code, err) == (0, "")
    assert [entry["call"] for entry in calls_made(log)] == ["keepOnly"], "no getProposal reached Klipolo"
    assert out == (
        '→ keepOnly {"ranges": [{"startSec": 61.2, "endSec": 91.0}]}\n'
        "← proposed, waits for Apply (152.6 s would go)\n"
        + "→ getProposal {}\n← waits for Apply\n" * 3
        + "\nThe teaser's cut waits for your Apply in Klipolo.\n\nCost: $0.008\n"
    )
    bodies = meta.bodies()
    assert len(bodies) == 5
    for body, call_id in zip(bodies[2:], ("call_2", "call_3", "call_4")):
        assert body["input"][-1] == {"type": "function_call_output", "call_id": call_id, "output": waits}


def test_pictures_klipolo_draws_are_said_to_be_outside_the_cost(tmp_path):
    title = {"title": "Launch day", "prompt": "a rocket at dawn", "atSec": 2.5, "durationSec": None}
    meta = Meta(
        response(call("makeTitleCard", title, "call_1"), call("makeTitleCard", title, "call_2")),
        response(message("Your title cards are on their way.")),
    )
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert (code, err) == (0, "")
    line = '→ makeTitleCard {"title": "Launch day", "prompt": "a rocket at dawn", "atSec": 2.5, "durationSec": null}\n'
    assert out == (
        (line + "← started, a picture billed in Klipolo (about $0.01)\n") * 2
        + "\nYour title cards are on their way.\n\nCost: $0.003\n"
        + "Not in this total: 2 pictures billed in Klipolo, about $0.01 each.\n"
    )


def test_klipolo_not_running_ends_the_run_with_the_bridges_own_sentence(tmp_path):
    meta = Meta(response(call("getContext", {}, "call_1"), usage=(6000, 0, 40)))
    command, log = klipolo(tmp_path, mode="off")
    code, out, err = run(meta, command)
    assert code == 1
    assert out == f"→ getContext {{}}\n← {fake_klipolo.OFF}\n\nCost: $0.008\n"
    assert err == ""
    assert len(meta.requests) == 1, "Meta isn't asked again"


STOPPED_ANSWERING = "Klipolo stopped answering. Check that it's open, then run the agent again."


def test_a_program_that_never_answers_as_the_bridge_ends_the_run(tmp_path):
    assert (agent.HELLO_TIMEOUT_S, agent.CALL_TIMEOUT_S) == (15, 120)
    meta = Meta()
    command, _ = klipolo(tmp_path, mode="silent")
    code, out, err = run(meta, command, hello_timeout=1)
    assert code == 1
    assert err == f"{command[0]} did not answer as Klipolo's MCP bridge. Pass --klipolo with a Klipolo that has it.\n"
    assert out == "Cost: $0.000\n"
    assert meta.requests == []


def test_a_program_that_exits_at_once_ends_the_run_with_the_same_sentence():
    # The SDK's writer may meet the closed pipe before or after the hello fails: either way, one sentence.
    command = [shutil.which("true")]
    for _ in range(10):
        meta = Meta()
        code, out, err = run(meta, command)
        assert code == 1
        assert err == f"{command[0]} did not answer as Klipolo's MCP bridge. Pass --klipolo with a Klipolo that has it.\n"
        assert out == "Cost: $0.000\n"
        assert meta.requests == []


@pytest.mark.parametrize("said_hello, said", [(False, "not a bridge"), (True, STOPPED_ANSWERING)])
def test_a_pipe_that_breaks_under_the_sdk_ends_the_run_with_one_sentence(tmp_path, monkeypatch, said_hello, said):
    # When the SDK's own writer meets the closed pipe, its task group raises that error as the run leaves the session.
    async def breaks(session, request, console, tally, **options):
        tally.said_hello = said_hello
        raise ExceptionGroup("unhandled errors in a TaskGroup", [anyio.BrokenResourceError()])

    monkeypatch.setattr(agent, "converse", breaks)
    command, _ = klipolo(tmp_path)
    if said == "not a bridge":
        said = f"{command[0]} did not answer as Klipolo's MCP bridge. Pass --klipolo with a Klipolo that has it."
    code, out, err = run(Meta(), command)
    assert (code, out, err) == (1, "Cost: $0.000\n", said + "\n")


def test_klipolo_stopping_mid_run_ends_it_without_asking_meta_again(tmp_path):
    meta = Meta(response(call("getContext", {}, "call_1"), usage=(6000, 0, 40)), response(message("Never asked for.")))
    command, log = klipolo(tmp_path, mode="crash")
    code, out, err = run(meta, command)
    assert (code, err) == (1, "")
    assert out == f"→ getContext {{}}\n← {STOPPED_ANSWERING}\n\nCost: $0.008\n"
    assert len(meta.requests) == 1


def test_a_call_klipolo_never_answers_ends_the_run_at_the_time_limit(tmp_path):
    meta = Meta(response(call("getContext", {}, "call_1"), usage=(6000, 0, 40)), response(message("Never asked for.")))
    command, _ = klipolo(tmp_path, mode="slow")
    code, out, err = run(meta, command, call_timeout=1)
    assert (code, err) == (1, "")
    assert out == f"→ getContext {{}}\n← {STOPPED_ANSWERING}\n\nCost: $0.008\n"
    assert len(meta.requests) == 1


def test_the_run_stops_after_8_model_turns(tmp_path):
    meta = Meta(repeat=response(call("getSummary", {}, "call_n"), usage=(1000, 0, 10)))
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert code == 1
    assert len(meta.requests) == 8
    assert [entry["call"] for entry in calls_made(log)] == ["getSummary"] * 7, "the 8th turn's calls are not made"
    assert err == "Stopped after 8 model turns without a final reply.\n"
    assert out.endswith("\n\nCost: $0.010\n")


def test_the_run_stops_before_the_next_model_call_once_it_reaches_the_spend_limit(tmp_path):
    # Turn 1 costs $0.0081; turn 2 ($0.1004) takes the run to $0.1086, past the default $0.10.
    meta = Meta(
        response(call("getContext", {}, "call_1"), usage=(6000, 0, 150)),
        response(call("setFormat", {"format": "portrait", "keepOthers": True}, "call_2"), usage=(80000, 0, 100)),
        response(message("Never asked for.")),
    )
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert code == 1
    assert len(meta.requests) == 2, "no third request"
    assert err == "Stopped at the $0.10 limit. Run again with --max-usd to allow more.\n"
    assert out == (
        "→ getContext {}\n"
        "← read, 7 lines\n"
        '→ setFormat {"format": "portrait", "keepOthers": true}\n'
        "← applied\n"
        "\n"
        "Cost: $0.109\n"
    )
    made = [entry["call"] for entry in calls_made(log)]
    assert made == ["getContext", "setFormat"], "the calls already asked for are still made"


def test_an_answer_cut_off_is_an_error_and_still_costs(tmp_path):
    meta = Meta(
        response(
            call("getContext", {}, "call_1"),
            status="incomplete",
            usage=(4000, 0, 8192),
            incomplete_details={"reason": "max_output_tokens"},
        )
    )
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert code == 1
    assert err == "Meta's answer was cut off (max_output_tokens).\n"
    assert out == "Cost: $0.040\n"
    assert calls_made(log) == [], "a cut off answer's calls may be partial: none is made"


@pytest.mark.parametrize(
    "reply, said",
    [
        (
            httpx.Response(401, json={"error": {"message": "Invalid API key", "code": "invalid_api_key"}}),
            "Meta did not accept the key in MODEL_API_KEY.",
        ),
        (
            httpx.Response(400, json={"error": {"message": f"Bad key {KEY}: tools[0] is not valid", "code": "invalid"}}),
            "Meta could not process this request: Bad key [redacted]: tools[0] is not valid.",
        ),
        (httpx.Response(404, text="nothing here"), "Meta could not process this request (status 404)."),
        (
            httpx.Response(400, json={"error": {"code": "content_policy_violation", "message": "No."}}),
            "Meta turned this down under its content policy. Try wording it differently.",
        ),
        (httpx.Response(402, json={"error": {"message": "Out of credit"}}), "Out of credit."),
        (httpx.Response(429, json={"error": {"message": None}}), "Too many requests right now. Try again in a moment."),
        (httpx.Response(503, text="<html>down</html>"), "Meta is busy right now. Try again in a moment."),
        (httpx.Response(504, text=""), "Meta took too long to answer. Try again."),
        (httpx.Response(302, headers={"location": "https://elsewhere.example"}), "Meta could not process this request (status 302)."),
        (httpx.Response(200, text="not json"), "Meta sent an answer the agent could not read. Try again."),
        (httpx.Response(200, json={"output": [{"type": "function_call"}]}), "Meta sent an answer the agent could not read. Try again."),
        (httpx.ConnectError(f"could not connect with {KEY}"), "Could not reach Meta. Check your internet connection and try again."),
        (httpx.ReadTimeout(f"read timed out with {KEY}"), "Meta took too long to answer. Try again."),
    ],
)
def test_meta_errors_say_what_happened_and_never_show_the_key(tmp_path, reply, said):
    meta = Meta(reply)
    command, log = klipolo(tmp_path)
    code, out, err = run(meta, command)
    assert code == 1
    assert err == said + "\n"
    assert out == "Cost: $0.000\n"
    assert KEY not in out + err
    assert calls_made(log) == []


def test_a_meta_error_never_holds_the_key():
    body = json.dumps({"error": {"message": f"key {KEY} refused " + "x" * 400}}).encode()
    error = agent.MetaError(agent.from_status(400, body, KEY))
    assert KEY not in str(error)
    assert KEY not in repr(error)
    assert str(error).startswith("Meta could not process this request: key [redacted] refused xxx")
    assert str(error).endswith("x..."), "at most 300 characters of Meta's message"


def test_main_starts_klipolo_with_mcp_and_keeps_the_key_from_it(tmp_path, monkeypatch):
    monkeypatch.setenv("MODEL_API_KEY", KEY)
    monkeypatch.delenv("KLIPOLO_META_BASE", raising=False)
    exe, log = installed(tmp_path)
    meta = demo_meta()
    code, out, err = main(["--klipolo", str(exe), "make", "a", "teaser"], meta)
    assert (code, err) == (0, "")
    assert logged(log)[0] == {"started": ["--mcp", "--log", str(log)], "has_key": False}
    assert meta.bodies()[0]["input"][0]["content"][0]["text"] == "make a teaser"
    assert KEY not in out + err


def test_main_takes_meta_base_from_the_environment_as_klipolo_does(tmp_path):
    exe, _ = installed(tmp_path)
    meta = demo_meta()
    code, _, err = main(
        ["--klipolo", str(exe), DEMO], meta, environ={"MODEL_API_KEY": KEY, "KLIPOLO_META_BASE": " http://127.0.0.1:9/v1/ "}
    )
    assert (code, err) == (0, "")
    assert str(meta.requests[0].url) == "http://127.0.0.1:9/v1/responses"


def test_a_missing_key_says_how_to_set_it_and_starts_nothing(tmp_path):
    exe, log = installed(tmp_path)
    for environ in ({}, {"MODEL_API_KEY": ""}, {"MODEL_API_KEY": "   "}):
        meta = Meta()
        code, out, err = main(["--klipolo", str(exe), DEMO], meta, environ=environ)
        assert (code, out, err) == (2, "", "Set MODEL_API_KEY to your Meta key.\n")
        assert meta.requests == []
    assert not log.exists(), "Klipolo was not started"


def test_no_klipolo_at_the_path_says_so(tmp_path):
    assert agent.DEFAULT_KLIPOLO == "/Applications/Klipolo.app/Contents/MacOS/klipolo-app"
    missing = tmp_path / "nowhere" / "klipolo-app"
    for argv, path in ((["--klipolo", str(missing), DEMO], str(missing)), ([DEMO], agent.DEFAULT_KLIPOLO)):
        if os.path.exists(path):
            continue
        code, out, err = main(argv, Meta(), environ={"MODEL_API_KEY": KEY})
        assert (code, out) == (2, "")
        assert err == f"Klipolo isn't installed at {path}. Pass --klipolo with the path to klipolo-app.\n"


def test_only_muse_spark_runs_the_agent(tmp_path):
    exe, log = installed(tmp_path)
    code, out, err = main(["--klipolo", str(exe), "--model", "gpt-4o", DEMO], Meta(), environ={"MODEL_API_KEY": KEY})
    assert (code, out) == (2, "")
    assert err == "--model takes a Muse Spark model, such as muse-spark-1.3.\n"
    assert not log.exists()


def test_max_usd_sets_the_spend_limit(tmp_path):
    assert agent.MAX_USD == 0.10
    exe, _ = installed(tmp_path)
    # The demo's first two turns cost $0.0298, past $0.02: the reply's turn is never asked for.
    meta = demo_meta()
    code, out, err = main(["--klipolo", str(exe), "--max-usd", "0.02", DEMO], meta, environ={"MODEL_API_KEY": KEY})
    assert code == 1
    assert len(meta.requests) == 2
    assert err == "Stopped at the $0.02 limit. Run again with --max-usd to allow more.\n"
    assert out.endswith("← proposed, waits for Apply (152.6 s would go)\n\nCost: $0.030\n")
    for amount in ("0", "-1", "nan", "inf"):
        meta = Meta()
        code, out, err = main(["--klipolo", str(exe), "--max-usd", amount, DEMO], meta, environ={"MODEL_API_KEY": KEY})
        assert (code, out) == (2, ""), amount
        assert err == "--max-usd takes an amount above 0 in US dollars, such as 0.10.\n", amount
        assert meta.requests == []


def test_an_unexpected_failure_is_one_line_without_the_key(tmp_path):
    exe, _ = installed(tmp_path)
    code, out, err = main(["--klipolo", str(exe), DEMO], Meta(RuntimeError(f"boom with {KEY}")), environ={"MODEL_API_KEY": KEY})
    assert code == 1
    assert err == "The agent stopped: RuntimeError: boom with [redacted]\n"
    assert KEY not in out + err


class Session:
    """A stand-in for the MCP session: answers `call_tool` with `reply`, or raises it."""

    def __init__(self, reply):
        self.reply = reply
        self.calls = []

    async def call_tool(self, name, arguments):
        self.calls.append((name, arguments))
        if isinstance(self.reply, Exception):
            raise self.reply
        return self.reply


def run_call(session, name, arguments, schemas=None):
    console = agent.Console(io.StringIO(), io.StringIO())
    fc = agent.FunctionCall(name=name, call_id="call_1", arguments=arguments)
    called = asyncio.run(agent.run_call(session, fc, schemas or {}, console))
    return called.output, called.stop, console.out.getvalue()


def test_an_mcp_error_goes_back_to_the_model():
    session = Session(McpError(ErrorData(code=-32602, message="Unknown tool: nope.")))
    output, off, out = run_call(session, "nope", "{}")
    assert (output, off) == ("Unknown tool: nope.", False)
    assert out == "→ nope {}\n← Unknown tool: nope.\n"


def test_arguments_that_are_not_a_json_object_are_not_sent():
    session = Session(None)
    for arguments in ("{not json", "[1, 2]"):
        output, off, out = run_call(session, "setFormat", arguments)
        assert output == "The arguments were not a JSON object. Call the tool again with a JSON object."
        assert not off
        assert out == f"→ setFormat {arguments}\n← {output}\n"
    assert session.calls == []


def test_nulls_strict_mode_added_are_dropped_before_the_call():
    from mcp.types import CallToolResult, TextContent

    loose = {"type": "object", "properties": {"text": {"type": "string"}, "size": {"type": "string"}}, "required": ["text"]}
    session = Session(CallToolResult(content=[TextContent(type="text", text="Done.")], isError=False))
    output, off, out = run_call(session, "say", '{"text": "Hi", "size": null}', {"say": loose})
    assert session.calls == [("say", {"text": "Hi"})]
    assert out == '→ say {"text": "Hi"}\n← Done.\n'
