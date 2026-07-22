"""a deliberately-misbehaving OpenAI-compatible upstream for chaos tests.

a single stdlib HTTP server (no third-party deps, so it runs in a bare
``python:3.13-alpine`` container) that speaks just enough of the OpenAI
``/v1/chat/completions`` surface for the rolter proxy to forward to it, and
injects a failure mode selected by the ``FAULT`` env var:

- ``ok``    always 200 with a valid, non-streaming chat completion
- ``500``   always 500 (a hard upstream error)
- ``429``   always 429 with ``Retry-After: 1`` (rate limited)
- ``slow``  sleeps ``SLOW_SECS`` (default 5) then 200 — trips a shorter gateway
            request timeout
- ``flap``  alternates 500, 200, 500, 200, … per request — a target whose health
            oscillates, to prove the balancer sheds and re-admits without a storm

every request is also logged to stderr so the container logs make the injected
behavior observable. the server is intentionally tiny and synchronous: the
chaos scenarios drive it with modest concurrency and assert the gateway's
contract, not the upstream's throughput.
"""

from __future__ import annotations

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from itertools import count

FAULT = os.environ.get("FAULT", "ok")
SLOW_SECS = float(os.environ.get("SLOW_SECS", "5"))
PORT = int(os.environ.get("PORT", "9000"))

# per-process request counter, used by the flap mode to alternate
_seq = count()


def _completion_body() -> bytes:
    return json.dumps(
        {
            "id": "chatcmpl-chaos",
            "object": "chat.completion",
            "created": 0,
            "model": "chaos-dummy",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "ok"},
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }
    ).encode()


def _error_body(message: str) -> bytes:
    return json.dumps({"error": {"message": message, "type": "upstream_error"}}).encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    # silence the default noisy per-request stderr line; we log our own
    def log_message(self, *_args) -> None:  # noqa: D401
        pass

    def _emit(self, status: int, body: bytes, extra_headers: dict[str, str] | None = None) -> None:
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        for k, v in (extra_headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        # a plain liveness/health probe target: always answers so the container
        # is reachable; the chat surface is where faults are injected
        self._emit(200, b'{"status":"ok"}')

    def do_POST(self) -> None:
        length = int(self.headers.get("content-length", "0") or "0")
        if length:
            # drain the request body so the client isn't left mid-write
            self.rfile.read(length)

        n = next(_seq)
        mode = FAULT
        if mode == "flap":
            # even → fail, odd → succeed (deterministic alternation)
            mode = "500" if n % 2 == 0 else "ok"

        sys.stderr.write(f"[faulty {FAULT}] POST {self.path} #{n} -> {mode}\n")
        sys.stderr.flush()

        if mode == "ok":
            self._emit(200, _completion_body())
        elif mode == "500":
            self._emit(500, _error_body("injected upstream 500"))
        elif mode == "429":
            self._emit(429, _error_body("injected upstream 429"), {"Retry-After": "1"})
        elif mode == "slow":
            time.sleep(SLOW_SECS)
            self._emit(200, _completion_body())
        else:
            self._emit(500, _error_body(f"unknown FAULT mode {FAULT!r}"))


def main() -> None:
    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    sys.stderr.write(f"faulty upstream: FAULT={FAULT} port={PORT} slow_secs={SLOW_SECS}\n")
    sys.stderr.flush()
    server.serve_forever()


if __name__ == "__main__":
    main()
