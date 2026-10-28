"""Drives the shim with the gateway's NDJSON protocol over a real socket.

The exchange is full-duplex, as rolter runs it: every line goes out as its own
HTTP/1.1 chunk and its reply is read before the next goes out, so a shim that
buffered its answers would hang here rather than pass. The mapping mirrors the
stub in crates/rolter-gateway/src/pii_stream_tests.rs, and so does the check:
on every segmentation the streamed text equals the buffered restore.
"""

import asyncio
import json
import logging
import socket
import threading
import time

import pytest
import uvicorn

from shim import InMemoryStore, create_app

MAP = {
    "<PERSON_1>": 'Jan "JK" Kowalski',
    "<EMAIL_ADDRESS_12>": "jan.k@example.test",
    "<CITY_3>": "Łódź",
}
TOKEN = "tok-stream"
# placeholders, a `<` that starts none, a placeholder never issued that shares a
# prefix with one that was, multi-byte text and a trailing `<` held to the close
TEXT = "Hi <PERSON_1>, a < b; mail <EMAIL_ADDRESS_12> from <CITY_3>! <PERSON_10> zażółć <"
TEXT2 = "<CITY_3> then <PERSON_1><EMAIL_ADDRESS_12>."
ARGS = '{"name":"<PERSON_1>","city":"<CITY_3>","note":"a < b","mail":"<EMAIL_ADDRESS_12>"}'


def restored(text: str, as_json: bool = False) -> str:
    for placeholder, value in MAP.items():
        if as_json:
            value = json.dumps(value, ensure_ascii=False)[1:-1]
        text = text.replace(placeholder, value)
    return text


class Server:
    def __init__(self) -> None:
        self.store = InMemoryStore()
        self.store.put(TOKEN, MAP)
        sock = socket.socket()
        sock.bind(("127.0.0.1", 0))
        self.port = sock.getsockname()[1]
        sock.close()
        config = uvicorn.Config(
            create_app(self.store), host="127.0.0.1", port=self.port, log_level="warning"
        )
        self.server = uvicorn.Server(config)
        self.thread = threading.Thread(target=self.server.run, daemon=True)

    def start(self) -> None:
        self.thread.start()
        for _ in range(200):
            if self.server.started:
                return
            time.sleep(0.05)
        raise RuntimeError("server did not start")

    def stop(self) -> None:
        self.server.should_exit = True
        self.thread.join(timeout=5)


@pytest.fixture(scope="module")
def server():
    srv = Server()
    srv.start()
    yield srv
    srv.stop()


class Exchange:
    """One stream, spoken the way rolter speaks it."""

    def __init__(self, port: int, token: str = TOKEN) -> None:
        self.port, self.token, self.seq = port, token, 0

    async def __aenter__(self) -> "Exchange":
        self.reader, self.writer = await asyncio.open_connection("127.0.0.1", self.port)
        self.writer.write(
            b"POST /restore-stream HTTP/1.1\r\nHost: shim\r\n"
            b"Content-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\n\r\n"
        )
        await self._chunk(
            {
                "trace_id": "0af7651916cd43dd8448eb211c80319c",
                "tenant": {"org": "acme", "team": "platform", "project": "chat"},
                "restoration_token": self.token,
            }
        )
        return self

    async def __aexit__(self, *_: object) -> None:
        self.writer.close()

    async def _chunk(self, line: dict) -> None:
        data = (json.dumps(line, ensure_ascii=False) + "\n").encode()
        self.writer.write(b"%x\r\n" % len(data) + data + b"\r\n")
        await self.writer.drain()

    async def _reply(self) -> dict:
        if not hasattr(self, "_head_read"):
            # status line and headers; the body is chunked
            status = await asyncio.wait_for(self.reader.readline(), 5)
            assert b" 200 " in status, status
            while (await asyncio.wait_for(self.reader.readline(), 5)).strip():
                pass
            self._head_read = True
        # one chunk is one reply line
        size = int((await asyncio.wait_for(self.reader.readline(), 5)).strip(), 16)
        body = await asyncio.wait_for(self.reader.readexactly(size + 2), 5)
        return json.loads(body)

    async def send(self, line: dict) -> dict:
        self.seq += 1
        await self._chunk({"seq": self.seq, **line})
        reply = await self._reply()
        assert reply["seq"] == self.seq
        return reply

    async def feed(self, channel: str, text: str, fmt: str | None = None) -> dict:
        line = {"channel": channel, "text": text}
        if fmt:
            line["format"] = fmt
        return await self.send(line)

    async def close_channel(self, channel: str) -> dict:
        reply = await self.send({"close": channel})
        assert reply["held"] == 0
        return reply

    async def end(self) -> dict:
        return await self.send({"end": True})


def segmentations(text: str):
    """Every cut size from one character to the whole text."""
    chars = list(text)
    for size in (1, 2, 3, 5, 8, len(chars)):
        yield [("".join(chars[i : i + size])) for i in range(0, len(chars), size)]


async def stream(port: int, channels: dict[str, tuple[list[str], str | None]]) -> dict[str, str]:
    """Interleave the channels' segments in one exchange; collect each channel's output."""
    out = {name: "" for name in channels}
    async with Exchange(port) as ex:
        longest = max(len(parts) for parts, _ in channels.values())
        for i in range(longest):
            for name, (parts, fmt) in channels.items():
                if i < len(parts):
                    reply = await ex.feed(name, parts[i], fmt)
                    out[name] += reply["text"]
                    assert 0 <= reply["held"] <= 1024
        for name in channels:
            reply = await ex.close_channel(name)
            out[name] += reply["text"]
        await ex.end()
    return out


def run(coro):
    return asyncio.run(coro)


def test_every_segmentation_restores_what_the_buffered_restore_does(server):
    for parts in segmentations(TEXT):
        out = run(stream(server.port, {"0/content": (parts, None)}))
        assert out["0/content"] == restored(TEXT), parts


def test_channels_hold_back_independently(server):
    # one byte per segment, two channels interleaved: a half-placeholder in one
    # must not hold or restore text of the other
    one = list(TEXT)
    two = list(TEXT2)
    out = run(stream(server.port, {"0/content": (one, None), "1/content": (two, None)}))
    assert out["0/content"] == restored(TEXT)
    assert out["1/content"] == restored(TEXT2)


def test_a_json_channel_gets_values_escaped(server):
    for parts in segmentations(ARGS):
        out = run(stream(server.port, {"0/args": (parts, "json")}))
        assert out["0/args"] == restored(ARGS, as_json=True), parts
        # still valid JSON, with the quote in the name intact
        assert json.loads(out["0/args"])["name"] == MAP["<PERSON_1>"]


def test_only_a_possible_placeholder_prefix_is_held(server):
    async def go() -> list[tuple[str, int]]:
        replies = []
        async with Exchange(server.port) as ex:
            for part in ["plain ", "a < b ", "<PERS", "ON_1>", " end <", "x"]:
                reply = await ex.feed("0/content", part)
                replies.append((reply["text"], reply["held"]))
            replies.append(((await ex.close_channel("0/content"))["text"], 0))
            await ex.end()
        return replies

    assert run(go()) == [
        ("plain ", 0),
        ("a < b ", 0),  # a `<` followed by a space starts no placeholder
        ("", 5),  # `<PERS` could still be one
        (MAP["<PERSON_1>"], 0),
        (" end ", 1),  # a trailing `<` could be the start of one
        ("<x", 0),  # it was not
        ("", 0),
    ]


def test_close_releases_what_could_not_complete(server):
    async def go() -> tuple[dict, dict]:
        async with Exchange(server.port) as ex:
            first = await ex.feed("0/content", "tail <CITY")
            closing = await ex.close_channel("0/content")
            await ex.end()
        return first, closing

    first, closing = run(go())
    assert (first["text"], first["held"]) == ("tail ", 5)
    assert closing["text"] == "<CITY"


def test_an_unknown_token_passes_text_through_unchanged(server):
    async def go() -> str:
        async with Exchange(server.port, token="not-issued") as ex:
            reply = await ex.feed("0/content", "<PERSON_1> stays")
            await ex.close_channel("0/content")
            await ex.end()
        return reply["text"]

    assert run(go()) == "<PERSON_1> stays"


def test_a_reset_mid_stream_ends_quietly_and_the_service_keeps_serving(server, caplog):
    async def reset() -> None:
        async with Exchange(server.port) as ex:
            reply = await ex.feed("0/content", "half <PERSON")
            assert reply["held"] == 7
            # a hard reset, as a caller that disconnects mid-stream sends
            ex.writer.transport.abort()
        await asyncio.sleep(0.2)

    async def after() -> str:
        async with Exchange(server.port) as ex:
            out = (await ex.feed("0/content", "<PERSON"))["text"]
            out += (await ex.feed("0/content", "_1> again"))["text"]
            out += (await ex.close_channel("0/content"))["text"]
            await ex.end()
        return out

    with caplog.at_level(logging.DEBUG):
        run(reset())
        assert run(after()) == restored("<PERSON_1> again")
    assert not [r for r in caplog.records if r.levelno >= logging.ERROR]


def test_close_restores_a_placeholder_that_is_also_a_prefix_of_a_longer_one(server):
    server.store.put("tok-prefix", {"PERSON_1": "Jan", "PERSON_10": "Ola"})

    async def go() -> tuple[dict, dict]:
        async with Exchange(server.port, token="tok-prefix") as ex:
            fed = await ex.feed("0/content", "hi PERSON_1")
            closed = await ex.close_channel("0/content")
            await ex.end()
        return fed, closed

    fed, closed = run(go())
    # PERSON_1 could still grow into PERSON_10, so it is held until the close
    assert (fed["text"], fed["held"]) == ("hi ", 8)
    assert closed["text"] == "Jan"


def test_a_malformed_line_ends_the_exchange_without_echoing_it(server, caplog):
    secret = "needle-" + "that-must-not-be-logged"

    async def go() -> bytes:
        async with Exchange(server.port) as ex:
            line = secret.encode() + b"\n"
            ex.writer.write(b"%x\r\n" % len(line) + line + b"\r\n")
            await ex.writer.drain()
            # the shim ends its reply: a chunked body closes with a zero chunk
            return await asyncio.wait_for(ex.reader.readuntil(b"0\r\n\r\n"), 5)

    with caplog.at_level(logging.DEBUG):
        raw = run(go())
    assert secret.encode() not in raw
    assert secret not in caplog.text


def test_no_text_value_or_mapping_reaches_the_logs(server, caplog):
    with caplog.at_level(logging.DEBUG):
        run(stream(server.port, {"0/content": (list(TEXT), None)}))
    for sensitive in [*MAP.values(), *MAP.keys(), "zażółć"]:
        assert sensitive not in caplog.text
