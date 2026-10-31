#!/usr/bin/env python3
"""Klipolo's demo agent: Muse Spark edits the video open in Klipolo through Klipolo's MCP server."""

from __future__ import annotations

import copy
from typing import Any

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
