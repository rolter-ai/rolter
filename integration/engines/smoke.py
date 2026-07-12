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
    providers = [chat(args.gateway_url, model, stream=False) for model in STRATEGY_MODELS]
    providers.append(chat(args.gateway_url, "dummy-round-robin", stream=False))
    chat(args.gateway_url, "dummy-round-robin", stream=True)
    if not {"engine-1", "engine-2"}.issubset(set(providers)):
        raise RuntimeError(f"round-robin did not use both pool targets: {providers}")
    print("real-engine smoke passed")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
