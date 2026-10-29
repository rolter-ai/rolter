# Presidio restore shim

A reference service for rolter's `streaming = "restore"` PII mode (#911). rolter
pipes the text deltas of a streamed response to `restore_stream_url` over one
full-duplex NDJSON exchange and needs each line answered with
`{"seq", "text", "held"}`. [Presidio](https://microsoft.github.io/presidio/) only
has a whole-text `deanonymize`, so this service adds the part between the two:

- a **holdback per channel**, a trie over the exchange's own placeholders, that
  keeps back only a suffix that could still become one (a `<` in code is not
  delayed);
- every whole placeholder it finds handed to Presidio's `DeanonymizeEngine`,
  through a small custom deanonymizer, to be replaced;
- values restored into a `"format": "json"` channel are JSON-escaped, so a `"`
  in a name does not break tool-call arguments;
- an exchange's state, holdback included, is dropped when the exchange ends,
  fails or is reset by the caller.

The protocol is specified in
[Restoring a stream](../../docs/user-docs/security/pii-sanitizer.mdx); this is
its algorithm in `holdback` of
`crates/rolter-gateway/src/pii_stream_tests.rs`, over Presidio.

**This is a reference, not a product.** Read it, then adapt it. It does not
sanitize anything and has no `/sanitize` or `/restore` endpoint: those are your
Presidio analyzer and anonymizer, and the buffered `restore_url` is yours too.

## The mapping store

`restoration_token` in the first line of an exchange names a placeholder-to-value
mapping that your sanitize service wrote when it replaced the request's
entities. The shim reads it through `MappingStore`:

```python
class MappingStore(Protocol):
    def get(self, token: str) -> dict[str, str] | None: ...
```

`InMemoryStore` is for one process and for the tests. In a deployment, back
`MappingStore` with whatever the sanitize service already writes to (Redis with a
TTL that outlives the longest stream, say) and build the app with
`create_app(your_store)`. A token the store does not know is served unrestored:
the text passes through and the caller sees placeholders, as with any shim
failure.

## Before you run it

- **Python 3.10 or newer.**
- **The service is unauthenticated.** A request that carries a valid
  `restoration_token` gets the real values back, which is personal data. Run it
  only on a private network that rolter and nothing else can reach, and put
  authentication or mutual TLS in front of it before it leaves a loopback or a
  pod network.
- **Only the three direct dependencies are pinned** (`requirements.txt`). Their
  transitive dependencies are not, so a rebuild can pick up newer ones. Generate
  a lock file (`pip-compile`, `uv pip compile`) for anything you deploy.

## Run it

```sh
python -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt
uvicorn shim:app_from_env --factory --host 127.0.0.1 --port 5002
```

`app_from_env` starts with an empty in-memory store; replace it with your own
(above) before you point rolter at it. The exchange needs HTTP/2 over `https`, or
an HTTP/1.1 server that answers while it is still reading the request body;
uvicorn does the latter. Terminate TLS in front of it for anything but a
loopback or private network, and set in `rolter.toml`:

```toml
[pii_sanitizer]
streaming = "restore"
restore_stream_url = "http://127.0.0.1:5002/restore-stream"
```

## Test it

```sh
pip install -r requirements-dev.txt
pytest
```

The tests start the service on a free port and drive it over a socket with the
gateway's exchange: every line goes out as its own HTTP/1.1 chunk and its reply
is read before the next goes out, so a shim that buffered its answers would hang
rather than pass. They check, on every segmentation of the text from one
character up, that the streamed result equals the buffered restore, that
channels hold back independently, that a JSON channel gets escaped values, that
`close` releases what could not complete (and restores a placeholder that only
a longer one could have outgrown), that an unknown token passes text
through and that a reset exchange leaves nothing behind for the next.

The gateway's own client is held to the shim too: `crates/rolter-gateway/tests/presidio_shim.rs`
starts this service under uvicorn and streams a chat completion through a real gateway route whose
`restore_stream_url` points at it, with a placeholder split across chunks and a JSON-format tool
call (#3444). It runs when `ROLTER_TEST_SHIM_PYTHON` names a Python with `requirements.txt`
installed; CI sets it, so there it never skips.

## What it never logs

No text delta, restored value or mapping reaches a log line or an error body. It
logs counts and fixed phrases only (an unknown token, a refused line), and a
rejected line is never echoed. A test holds this at `DEBUG`. Keep it that way
when you adapt it: a restore service sees the very values the sanitizer exists
to keep out of logs.

## Limits

- `held` may not exceed 1,024 characters in rolter; a placeholder longer than
  that is left as it is (and counted in one warning).
- A request line over 1 MiB ends the exchange; rolter treats that as a failure
  and carries on unrestored.
- One process, one event loop. The holdback state is small and per exchange;
  nothing is shared between exchanges but the store.
