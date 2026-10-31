"""MCP input schemas as strict function tools, and the nulls strict mode adds taken back out.

`fixtures/tools_list.json` is what Klipolo's `tools/list` answers (`klipolo_mcp::listed("tools/list")`), and
`fixtures/initialize.json` what its `initialize` answers. Klipolo's schemas are strict already, so they must pass
through unchanged.
"""

import copy
import json
from pathlib import Path

import agent

FIXTURES = Path(__file__).parent / "fixtures"
KLIPOLO_TOOLS = json.loads((FIXTURES / "tools_list.json").read_text())["tools"]


def klipolo_tool(name):
    return next(t for t in KLIPOLO_TOOLS if t["name"] == name)


def test_klipolo_lists_23_tools():
    names = [t["name"] for t in KLIPOLO_TOOLS]
    assert len(names) == 23
    for name in ("getContext", "setFormat", "setCaptions", "keepOnly", "addText", "getProposal"):
        assert name in names


def test_klipolo_schemas_are_strict_already_and_pass_unchanged():
    for tool in KLIPOLO_TOOLS:
        schema = tool["inputSchema"]
        before = copy.deepcopy(schema)
        strict, is_strict = agent.strict_schema(schema)
        assert is_strict, tool["name"]
        assert strict == schema, tool["name"]
        # Byte for byte, key order and the order of `required` too.
        assert json.dumps(strict) == json.dumps(schema), tool["name"]
        assert schema == before, "the input is not changed"


def test_properties_named_like_keywords_stay_properties():
    # followSubject and blurObjects take a property named "description"; setFormat one named "format".
    for name, prop in (("followSubject", "description"), ("blurObjects", "description"), ("setFormat", "format")):
        schema = klipolo_tool(name)["inputSchema"]
        strict, _ = agent.strict_schema(schema)
        assert prop in strict["properties"], name
        assert prop in strict["required"], name
    loose = {
        "type": "object",
        "properties": {
            "description": {"type": "string", "description": "What to follow."},
            "format": {"type": "string", "enum": ["a", "b"]},
            "type": {"type": "string"},
        },
    }
    strict, is_strict = agent.strict_schema(loose)
    assert is_strict
    assert strict["properties"]["description"] == {"type": ["string", "null"], "description": "What to follow."}
    assert strict["properties"]["format"] == {"type": ["string", "null"], "enum": ["a", "b", None]}
    assert strict["properties"]["type"] == {"type": ["string", "null"]}
    assert strict["required"] == ["description", "format", "type"]


def test_optional_properties_become_required_and_nullable():
    loose = {
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "count": {"type": "integer", "minimum": 1},
            "size": {"type": "string", "enum": ["small", "large"]},
            "when": {"type": ["number", "string"]},
            "either": {"anyOf": [{"type": "string"}, {"type": "number"}]},
            "already": {"type": ["boolean", "null"]},
            "ref": {"$ref": "#/$defs/thing"},
        },
        "required": ["text"],
        "$defs": {"thing": {"type": "object", "properties": {"x": {"type": "number"}}}},
    }
    strict, is_strict = agent.strict_schema(loose)
    assert is_strict
    assert "$schema" not in strict
    assert strict["additionalProperties"] is False
    assert strict["required"] == ["text", "count", "size", "when", "either", "already", "ref"]
    props = strict["properties"]
    assert props["text"] == {"type": "string"}, "a required property is left as it is"
    assert props["count"] == {"type": ["integer", "null"], "minimum": 1}
    assert props["size"] == {"type": ["string", "null"], "enum": ["small", "large", None]}
    assert props["when"] == {"type": ["number", "string", "null"]}
    assert props["either"] == {"anyOf": [{"type": "string"}, {"type": "number"}, {"type": "null"}]}
    assert props["already"] == {"type": ["boolean", "null"]}
    assert props["ref"] == {"anyOf": [{"$ref": "#/$defs/thing"}, {"type": "null"}]}
    thing = strict["$defs"]["thing"]
    assert thing == {
        "type": "object",
        "properties": {"x": {"type": ["number", "null"]}},
        "required": ["x"],
        "additionalProperties": False,
    }


def test_nested_objects_and_array_items_are_closed():
    loose = {
        "type": "object",
        "properties": {
            "ranges": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"startSec": {"type": "number"}, "label": {"type": "string"}},
                    "required": ["startSec"],
                },
            },
            "style": {"type": "object", "properties": {"color": {"type": "string"}}, "required": ["color"]},
        },
        "required": ["ranges", "style"],
    }
    strict, is_strict = agent.strict_schema(loose)
    assert is_strict
    item = strict["properties"]["ranges"]["items"]
    assert item["additionalProperties"] is False
    assert item["required"] == ["startSec", "label"]
    assert item["properties"]["label"] == {"type": ["string", "null"]}
    style = strict["properties"]["style"]
    assert style["additionalProperties"] is False
    assert style["required"] == ["color"]


def test_a_schema_with_no_properties_is_an_empty_closed_object():
    for loose in ({}, {"type": "object"}, {"type": "object", "properties": {}}):
        strict, is_strict = agent.strict_schema(loose)
        assert is_strict
        assert strict == {"type": "object", "properties": {}, "required": [], "additionalProperties": False}


def test_free_form_objects_are_sent_as_they_are_without_strict():
    # Strict mode can't say "any keys": the tool goes as listed, not strict.
    for loose in (
        {"type": "object", "properties": {"tags": {"type": "object", "additionalProperties": {"type": "string"}}}},
        {"type": "object", "additionalProperties": True},
        {"type": "object", "properties": {"a": {"type": "string"}}, "patternProperties": {"^x": {}}},
    ):
        before = copy.deepcopy(loose)
        schema, is_strict = agent.strict_schema(loose)
        assert not is_strict
        assert schema == before


def test_function_tool_wraps_an_mcp_tool_as_meta_takes_it():
    tool = klipolo_tool("setCaptions")
    assert agent.function_tool(tool["name"], tool["description"], tool["inputSchema"]) == {
        "type": "function",
        "name": "setCaptions",
        "description": tool["description"],
        "parameters": tool["inputSchema"],
        "strict": True,
    }
    loose = {"type": "object", "additionalProperties": True}
    assert agent.function_tool("free", None, loose) == {
        "type": "function",
        "name": "free",
        "description": "",
        "parameters": loose,
        "strict": False,
    }


def test_nulls_strict_mode_added_are_dropped_and_klipolos_own_nulls_kept():
    # setCaptions requires style and allows null: Klipolo reads null as "keep the current style", so it stays.
    captions = klipolo_tool("setCaptions")["inputSchema"]
    assert agent.without_added_nulls({"on": True, "style": None}, captions) == {"on": True, "style": None}
    music = klipolo_tool("addMusic")["inputSchema"]
    args = {"file": "song.mp3", "volume": None, "duck": None, "atSec": None}
    assert agent.without_added_nulls(args, music) == args

    loose = {
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "count": {"type": "integer"},
            "ranges": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"startSec": {"type": "number"}, "label": {"type": "string"}},
                    "required": ["startSec"],
                },
            },
        },
        "required": ["text"],
    }
    args = {"text": "Hi", "count": None, "ranges": [{"startSec": 1.0, "label": None}, {"startSec": 2.0, "label": "x"}]}
    assert agent.without_added_nulls(args, loose) == {
        "text": "Hi",
        "ranges": [{"startSec": 1.0}, {"startSec": 2.0, "label": "x"}],
    }
    # A null for a required property is the model's own: it goes to the tool, which refuses or reads it.
    assert agent.without_added_nulls({"text": None}, loose) == {"text": None}
