#!/usr/bin/env python3
"""Opt-in smoke checks against a real self-hosted Ollama daemon."""

import json
import sys
import urllib.request


base = sys.argv[1].rstrip("/")


def request(path: str, payload: dict | None = None) -> tuple[int, bytes, str]:
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(
        f"{base}{path}", data=data, headers={"content-type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=120) as response:
        return response.status, response.read(), response.headers.get_content_type()


status, body, _ = request("/v1/models")
assert status == 200
assert {"ollama-smoke", "ollama-embed-smoke"} <= {m["id"] for m in json.loads(body)["data"]}

status, body, _ = request(
    "/v1/chat/completions",
    {
        "model": "ollama-smoke",
        "messages": [{"role": "user", "content": "Reply with OK"}],
        "seed": 42,
        "response_format": {"type": "json_object"},
    },
)
assert status == 200 and json.loads(body)["choices"]

status, body, content_type = request(
    "/v1/chat/completions",
    {"model": "ollama-smoke", "messages": [{"role": "user", "content": "hi"}], "stream": True},
)
assert status == 200 and content_type == "text/event-stream" and b"data:" in body

status, body, _ = request("/v1/completions", {"model": "ollama-smoke", "prompt": "hello"})
assert status == 200 and json.loads(body)["choices"]

# a generative model answers 501 here on current ollama releases, so this goes
# to the dedicated embedding route
status, body, _ = request("/v1/embeddings", {"model": "ollama-embed-smoke", "input": "hello"})
vector = json.loads(body)["data"][0]["embedding"]
assert status == 200 and vector and all(isinstance(x, float) for x in vector)

print("ollama smoke passed: models, chat, completions, embeddings, sse")
