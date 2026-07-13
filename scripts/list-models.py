#!/usr/bin/env python3
"""Fetch /v1/models and print a compact, human-friendly model list."""

from __future__ import annotations

import argparse
import json
import os
import sys
import textwrap
import urllib.error
import urllib.request
from collections.abc import Iterable
from typing import Any

DEFAULT_BASE_URL = "http://codex2api:3402"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="List models from a codex2api /v1/models endpoint in a readable table.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument(
        "--base-url",
        default=os.environ.get("CODEX2API_BASE_URL", DEFAULT_BASE_URL),
        help="codex2api base URL. Can also be set with CODEX2API_BASE_URL.",
    )
    parser.add_argument(
        "--api-key",
        default=os.environ.get("CODEX2API_API_KEY"),
        help="API key. Can also be set with CODEX2API_API_KEY.",
    )
    parser.add_argument(
        "--raw",
        action="store_true",
        help="Print the raw JSON response after the friendly table.",
    )
    return parser.parse_args()


def fetch_models(base_url: str, api_key: str) -> Any:
    url = f"{base_url.rstrip('/')}/v1/models"
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            charset = response.headers.get_content_charset() or "utf-8"
            return json.loads(response.read().decode(charset))
    except urllib.error.HTTPError as err:
        body = err.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"GET {url} failed with HTTP {err.code}: {body}") from err
    except urllib.error.URLError as err:
        raise RuntimeError(f"GET {url} failed: {err.reason}") from err
    except json.JSONDecodeError as err:
        raise RuntimeError(f"GET {url} did not return valid JSON: {err}") from err


def models_from_response(payload: Any) -> list[dict[str, Any]]:
    if isinstance(payload, dict):
        for key in ("data", "models", "items"):
            value = payload.get(key)
            if isinstance(value, list):
                return [item for item in value if isinstance(item, dict)]
        if all(isinstance(value, dict) for value in payload.values()):
            return [dict(value, id=key) for key, value in payload.items()]
    if isinstance(payload, list):
        return [item for item in payload if isinstance(item, dict)]
    return []


def first_value(model: dict[str, Any], keys: Iterable[str], default: str = "-") -> str:
    for key in keys:
        value = model.get(key)
        if value is None or value == "":
            continue
        if isinstance(value, (list, tuple)):
            if value:
                return ", ".join(str(item) for item in value)
            continue
        if isinstance(value, dict):
            continue
        return str(value)
    return default


def summarize_context(model: dict[str, Any]) -> str:
    keys = (
        "context_window",
        "context_length",
        "max_context_length",
        "max_input_tokens",
        "input_token_limit",
    )
    value = first_value(model, keys)
    return format_number(value)


def summarize_output(model: dict[str, Any]) -> str:
    keys = ("max_output_tokens", "output_token_limit", "max_completion_tokens")
    value = first_value(model, keys)
    return format_number(value)


def format_number(value: str) -> str:
    if value == "-":
        return value
    try:
        return f"{int(value):,}"
    except ValueError:
        return value


def summarize_capabilities(model: dict[str, Any]) -> str:
    capability_keys = (
        "capabilities",
        "supported_features",
        "features",
        "supported_tools",
        "tools",
    )
    parts: list[str] = []
    for key in capability_keys:
        value = model.get(key)
        if isinstance(value, list):
            parts.extend(str(item) for item in value)
        elif isinstance(value, dict):
            parts.extend(str(name) for name, enabled in value.items() if enabled)
    for key, label in (
        ("supports_reasoning", "reasoning"),
        ("supports_vision", "vision"),
        ("supports_tools", "tools"),
        ("supports_parallel_tool_calls", "parallel-tools"),
    ):
        if model.get(key) is True:
            parts.append(label)
    deduped = list(dict.fromkeys(parts))
    return ", ".join(deduped) if deduped else "-"


def table_rows(models: list[dict[str, Any]]) -> list[list[str]]:
    rows = [["Model", "Name", "Context", "Output", "Capabilities"]]
    for model in sorted(models, key=lambda item: first_value(item, ("id", "model", "slug", "name"))):
        rows.append(
            [
                first_value(model, ("id", "model", "slug", "name")),
                first_value(model, ("display_name", "name", "title")),
                summarize_context(model),
                summarize_output(model),
                summarize_capabilities(model),
            ]
        )
    return rows


def print_table(rows: list[list[str]]) -> None:
    widths = [max(len(row[index]) for row in rows) for index in range(len(rows[0]))]
    for index, row in enumerate(rows):
        print("  ".join(value.ljust(widths[col]) for col, value in enumerate(row)).rstrip())
        if index == 0:
            print("  ".join("-" * width for width in widths))


def main() -> int:
    args = parse_args()
    if not args.api_key:
        print("Missing API key. Pass --api-key or set CODEX2API_API_KEY.", file=sys.stderr)
        return 2

    try:
        payload = fetch_models(args.base_url, args.api_key)
    except RuntimeError as err:
        print(err, file=sys.stderr)
        return 1

    models = models_from_response(payload)
    if not models:
        print("No model list found in the response. Use --raw to inspect the payload.")
    else:
        print(f"Endpoint: {args.base_url.rstrip('/')}/v1/models")
        print(f"Models: {len(models)}\n")
        print_table(table_rows(models))

    if args.raw:
        if models:
            print()
        print("Raw response:")
        print(textwrap.indent(json.dumps(payload, ensure_ascii=False, indent=2), "  "))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
