#!/usr/bin/env python3
"""Minimal dependency-free OpenAI/SSE smoke suite for a real inference engine."""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from typing import Dict, Optional


def request(url: str, body: Optional[Dict] = None, timeout: float = 30.0):
    data = None if body is None else json.dumps(body).encode()
    headers = {"content-type": "application/json"} if data else {}
    return urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers), timeout=timeout)


def wait_for_models(base_url: str) -> None:
    deadline = time.monotonic() + 300
    while time.monotonic() < deadline:
        try:
            with request(f"{base_url}/v1/models", timeout=5) as response:
                if response.status == 200:
                    return
        except (urllib.error.URLError, OSError):
            pass
        time.sleep(2)
    raise RuntimeError(f"timed out waiting for {base_url}/v1/models")


STRATEGY_MODELS = (
    "dummy-round-robin", "dummy-random", "dummy-power-of-two",
    "dummy-consistent-hash", "dummy-cache-aware", "dummy-weighted",
    "dummy-pipeline", "dummy-cheapest", "dummy-fastest",
)


def chat(base_url: str, model: str, stream: bool) -> Optional[str]:
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "Reply with one token."}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": stream,
    }
    with request(f"{base_url}/v1/chat/completions", payload, timeout=90) as response:
        if response.status != 200:
            raise RuntimeError(f"chat returned HTTP {response.status}")
        content_type = response.headers.get("content-type", "")
        body = response.read().decode(errors="replace")
    if stream:
        if "text/event-stream" not in content_type or "data:" not in body or "[DONE]" not in body:
            raise RuntimeError("expected a complete SSE response")
    elif not json.loads(body).get("choices"):
        raise RuntimeError("expected a non-streaming OpenAI choices response")
    return response.headers.get("x-rolter-provider")


def responses(base_url: str, model: str, stream: bool) -> None:
    payload = {
        "model": model,
        "input": "Reply with one token.",
        "temperature": 0,
        "stream": stream,
    }
    try:
        with request(f"{base_url}/v1/responses", payload, timeout=90) as response:
            content_type = response.headers.get("content-type", "")
            body = response.read().decode(errors="replace")
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        provider = error.headers.get("x-rolter-provider", "unknown")
        target = error.headers.get("x-rolter-target", "unknown")
        raise RuntimeError(
            f"Responses returned HTTP {error.code} via {provider}/{target}: {detail}"
        ) from error
    if stream:
        if "text/event-stream" not in content_type or "response." not in body:
            raise RuntimeError("expected a Responses SSE event stream")
    elif json.loads(body).get("object") != "response":
        raise RuntimeError("expected a non-streaming Responses object")


def anthropic_messages(base_url: str, model: str, stream: bool) -> None:
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "Reply with one token."}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": stream,
    }
    try:
        with request(f"{base_url}/v1/messages", payload, timeout=90) as response:
            content_type = response.headers.get("content-type", "")
            body = response.read().decode(errors="replace")
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(f"Messages returned HTTP {error.code}: {detail}") from error
    if stream:
        if "text/event-stream" not in content_type or "event:" not in body:
            raise RuntimeError("expected an Anthropic Messages SSE event stream")
    elif json.loads(body).get("type") != "message":
        raise RuntimeError("expected a non-streaming Anthropic message")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine-url", action="append", required=True)
    parser.add_argument("--gateway-url", required=True)
    args = parser.parse_args()

    for engine_url in args.engine_url:
        wait_for_models(engine_url)
    wait_for_models(args.gateway_url)
    for engine_url in args.engine_url:
        chat(engine_url, "rolter-dummy", stream=False)
        chat(engine_url, "rolter-dummy", stream=True)

    # A successful request is sufficient for strategies whose choice is
    # intentionally stochastic or driven by runtime cache/latency signals.
    providers = {model: chat(args.gateway_url, model, stream=False) for model in STRATEGY_MODELS}
    # two consecutive round-robin calls must alternate across the pool;
    # only that route's own picks count so other strategies cannot mask it
    round_robin = {providers["dummy-round-robin"], chat(args.gateway_url, "dummy-round-robin", stream=False)}
    chat(args.gateway_url, "dummy-round-robin", stream=True)
    responses(args.gateway_url, "dummy-round-robin", stream=False)
    responses(args.gateway_url, "dummy-round-robin", stream=True)
    anthropic_messages(args.gateway_url, "dummy-round-robin", stream=False)
    anthropic_messages(args.gateway_url, "dummy-round-robin", stream=True)
    if round_robin != {"engine-1", "engine-2"}:
        raise RuntimeError(f"round-robin did not use both pool targets: {sorted(round_robin, key=str)}")
    print("real-engine smoke passed")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
