"""A streamed-restoration service for rolter's PII sanitizer, over Presidio.

rolter's `streaming = "restore"` pipes the text deltas of a streamed response to
`restore_stream_url` over one full-duplex NDJSON exchange and expects each line
answered with `{"seq", "text", "held"}`. Presidio only offers a whole-text
`deanonymize`, so this service adds what sits between the two: a per-channel
holdback that keeps back any suffix that could still become one of the
exchange's placeholders, and hands every whole placeholder it finds to
Presidio's `DeanonymizeEngine` to be replaced.

The protocol is documented in docs/user-docs/security/pii-sanitizer.mdx
("Restoring a stream"); the algorithm mirrors `holdback` in
crates/rolter-gateway/src/pii_stream_tests.rs.

This service never logs a text delta, a restored value or a mapping. It logs
counts and fixed phrases only, and nothing that rides in a request line
reaches an exception message.
"""

from __future__ import annotations

import json
import logging
import threading
from dataclasses import dataclass, field
from typing import Protocol

from presidio_anonymizer import DeanonymizeEngine
from presidio_anonymizer.entities import OperatorConfig, OperatorResult
from presidio_anonymizer.operators import Operator, OperatorType
from starlette.applications import Starlette
from starlette.responses import JSONResponse
from starlette.routing import Route
from starlette.types import Receive, Scope, Send

log = logging.getLogger("presidio_restore_shim")

# rolter treats a reply that holds more than this many characters as a failure
MAX_HELD = 1024
# one request line, text included, is far below this; a longer one is refused
MAX_LINE_BYTES = 1 << 20


class MappingStore(Protocol):
    """Where the placeholder-to-value mapping of a restoration token lives.

    The mapping is written by the service that sanitizes the request leg and
    read here, so a real deployment backs this with the store both share
    (Redis with a TTL, say). `InMemoryStore` is for one process and for tests.
    """

    def get(self, token: str) -> dict[str, str] | None: ...


class InMemoryStore:
    def __init__(self) -> None:
        self._mappings: dict[str, dict[str, str]] = {}
        self._lock = threading.Lock()

    def put(self, token: str, mapping: dict[str, str]) -> None:
        with self._lock:
            self._mappings[token] = dict(mapping)

    def get(self, token: str) -> dict[str, str] | None:
        with self._lock:
            return self._mappings.get(token)


class _Restore(Operator):
    """Presidio deanonymizer: swap a placeholder for the value it stood for."""

    def operate(self, text: str, params: dict | None = None) -> str:
        params = params or {}
        value = params["mapping"].get(text, text)
        if params.get("json"):
            # a value restored into JSON source sits between quotes
            value = json.dumps(value, ensure_ascii=False)[1:-1]
        return value

    def validate(self, params: dict | None = None) -> None:
        if not params or "mapping" not in params:
            raise ValueError("expected a mapping")

    def operator_name(self) -> str:
        return "restore_placeholder"

    def operator_type(self) -> OperatorType:
        return OperatorType.Deanonymize


_engine = DeanonymizeEngine()
_engine.add_deanonymizer(_Restore)


class _Trie:
    """A trie over one exchange's placeholders."""

    def __init__(self, placeholders: list[str]) -> None:
        self.root: dict = {}
        for placeholder in placeholders:
            node = self.root
            for ch in placeholder:
                node = node.setdefault(ch, {})
            node[""] = True  # a placeholder ends here

    def walk(self, text: str, start: int, final: bool = False) -> tuple[int | None, bool]:
        """Match at `start`: the end of the longest whole placeholder there, and
        whether `text` ran out while still inside one that could grow.

        With `final` the text is all there will be (the channel closed), so
        nothing is held and the longest whole placeholder wins even when a
        longer one shares its prefix."""
        node = self.root
        matched: int | None = None
        i = start
        while i < len(text):
            node = node.get(text[i])
            if node is None:
                return matched, False
            i += 1
            if "" in node:
                matched = i
        # out of text: hold it if a longer placeholder is still possible
        growing = any(key != "" for key in node)
        return (None, True) if growing and not final else (matched, False)


@dataclass
class Channel:
    json: bool
    pending: str = ""


@dataclass
class Exchange:
    """The state of one stream: the mapping, its trie and a holdback per channel."""

    mapping: dict[str, str]
    channels: dict[str, Channel] = field(default_factory=dict)

    def __post_init__(self) -> None:
        # a placeholder longer than the cap could never be held; leave it as
        # it is rather than fail the stream
        usable = [p for p in self.mapping if 0 < len(p) <= MAX_HELD]
        if len(usable) != len(self.mapping):
            log.warning(
                "ignoring %d placeholder(s) over the holdback cap",
                len(self.mapping) - len(usable),
            )
        self.trie = _Trie(usable)

    def feed(self, name: str, text: str, is_json: bool) -> tuple[str, int]:
        channel = self.channels.setdefault(name, Channel(json=is_json))
        channel.json = is_json or channel.json
        channel.pending += text
        restored, channel.pending = self._scan(channel, final=False)
        return restored, len(channel.pending)

    def close(self, name: str) -> str:
        """The channel ended: nothing more can arrive, so what is still held is
        restored if it is a whole placeholder and released as it is if not."""
        channel = self.channels.pop(name, None)
        if channel is None:
            return ""
        return self._scan(channel, final=True)[0]

    def _scan(self, channel: Channel, final: bool) -> tuple[str, str]:
        """Restore the whole placeholders in `channel.pending`; return the text to
        emit and the suffix to keep holding."""
        pending = channel.pending
        out: list[str] = []
        spans: list[OperatorResult] = []
        emitted = 0  # length of the output so far, in characters
        held = ""
        i = 0
        while i < len(pending):
            end, hold = self.trie.walk(pending, i, final)
            if hold:
                held = pending[i:]
                break
            if end is not None:
                spans.append(OperatorResult(emitted, emitted + (end - i), "PLACEHOLDER"))
                out.append(pending[i:end])
                emitted += end - i
                i = end
            else:
                out.append(pending[i])
                emitted += 1
                i += 1
        return self._deanonymize("".join(out), spans, channel.json), held

    def _deanonymize(self, text: str, spans: list[OperatorResult], is_json: bool) -> str:
        if not spans:
            return text
        operators = {
            "DEFAULT": OperatorConfig(
                "restore_placeholder", {"mapping": self.mapping, "json": is_json}
            )
        }
        return _engine.deanonymize(text, spans, operators).text


class _Asgi:
    def __init__(self, app) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        await self.app(scope, receive, send)


class BadLine(Exception):
    """A request line the protocol does not allow. Carries no content."""


def create_app(store: MappingStore) -> Starlette:
    """The service. `/restore-stream` is a bare ASGI endpoint rather than a
    framework response: the exchange is full duplex, so the request body has to
    be read while the reply is written, and a response class that listens for
    the client's disconnect would take body messages from the same receive
    channel."""

    async def restore_stream(scope: Scope, receive: Receive, send: Send) -> None:
        if scope["method"] != "POST":
            response = JSONResponse({"error": "method not allowed"}, status_code=405)
            await response(scope, receive, send)
            return
        await send(
            {
                "type": "http.response.start",
                "status": 200,
                "headers": [(b"content-type", b"application/x-ndjson")],
            }
        )
        exchange: Exchange | None = None
        buf = b""
        try:
            more = True
            while more:
                message = await receive()
                if message["type"] == "http.disconnect":
                    # the caller is gone: drop the exchange and its holdback
                    return
                buf += message.get("body", b"")
                more = message.get("more_body", False)
                if len(buf) > MAX_LINE_BYTES and b"\n" not in buf:
                    raise BadLine("line too long")
                while (nl := buf.find(b"\n")) != -1:
                    raw, buf = buf[:nl], buf[nl + 1 :]
                    if not raw.strip():
                        continue
                    exchange, reply = _handle(store, exchange, raw)
                    if reply is not None:
                        await send({"type": "http.response.body", "body": reply, "more_body": True})
            if buf.strip():
                exchange, reply = _handle(store, exchange, buf)
                if reply is not None:
                    await send({"type": "http.response.body", "body": reply, "more_body": True})
        except BadLine as err:
            # rolter treats an ended exchange as a failure and carries on
            # unrestored; the message names the rule, never the content
            log.warning("exchange refused: %s", err)
        # the exchange, its holdback included, is dropped here whether the
        # stream ended, was reset or failed
        await send({"type": "http.response.body", "body": b"", "more_body": False})

    async def healthz(_: object) -> JSONResponse:
        return JSONResponse({"status": "ok"})

    return Starlette(
        routes=[
            Route("/healthz", healthz, methods=["GET"]),
            # an instance, not a function: Starlette wraps a function as a
            # request/response endpoint, and this one speaks ASGI itself
            Route("/restore-stream", _Asgi(restore_stream), methods=["POST"]),
        ]
    )


def _handle(
    store: MappingStore, exchange: Exchange | None, raw: bytes
) -> tuple[Exchange, bytes | None]:
    """Apply one request line; return the exchange and the reply line, if the
    line is answered (the first one is not)."""
    try:
        line = json.loads(raw)
    except ValueError:
        raise BadLine("not json") from None
    if not isinstance(line, dict):
        raise BadLine("not an object")
    if exchange is None:
        # the first line names the exchange and is not answered
        token = line.get("restoration_token")
        mapping = store.get(token) if isinstance(token, str) else None
        if mapping is None:
            log.warning("exchange opened with an unknown restoration token")
        return Exchange(mapping or {}), None
    seq = line.get("seq")
    if not isinstance(seq, int) or isinstance(seq, bool):
        raise BadLine("no seq")
    if isinstance(line.get("close"), str):
        text, held = exchange.close(line["close"]), 0
    elif line.get("end") is True:
        text, held = "", 0
    elif isinstance(line.get("channel"), str) and isinstance(line.get("text"), str):
        text, held = exchange.feed(line["channel"], line["text"], line.get("format") == "json")
    else:
        raise BadLine("unknown line")
    reply = json.dumps({"seq": seq, "text": text, "held": held}, ensure_ascii=False) + "\n"
    return exchange, reply.encode()


def app_from_env() -> Starlette:
    """The app `uvicorn shim:app_from_env --factory` serves.

    It starts with an empty `InMemoryStore`; wire your sanitize service's store
    in here (see the README).
    """
    return create_app(InMemoryStore())
