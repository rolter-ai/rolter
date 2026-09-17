# Client disconnects

A caller can leave at any point in a request: a browser tab closes mid-stream, a
client-side timeout fires while the upstream is still thinking, a Kubernetes
rollout kills the pod holding the connection. The gateway sees this as the
response future — or the response body stream — being dropped, and it happens
often enough at scale that "undefined" is not an acceptable answer. This page
states what rolter does (#1083).

## The three guarantees

1. **The request is logged exactly once, with status `499`.** The status is
   nginx's convention for *client closed request*: it is not a real HTTP status,
   it never reaches a caller (there is nobody left to receive it), and it exists
   so an abandoned request is distinguishable in analytics from both a success
   and an upstream failure. The row carries `error = "client disconnected"`.
2. **No retry is attempted.** A disconnect is the caller's decision, not an
   upstream fault. Failing over to a second provider would double the cost of
   every abandoned request and add load precisely when callers are timing out.
3. **The in-flight slot is released.** The balancer routes on live in-flight
   counts, so a leaked slot permanently skews every later routing decision for
   that model — a silent, restart-only degradation.

Tokens the provider already produced stay on the row. An abandoned stream that
received a usage frame is real spend; dropping it would under-bill.

## How it is enforced

| Where | Mechanism |
|---|---|
| Before the response starts | `CancelGuard` (`crates/rolter-gateway/src/cancel.rs`) is armed before the forward loop in `proxy()` and `proxy_multipart()`. Its `Drop` stamps `499`, the error text and the elapsed latency, counts the disconnect and logs the row. It is *disarmed* the moment an arm takes ownership of the row, so a normal answer never trips it |
| While the body streams | `UsageLoggingStream` (`logging.rs`) tracks whether it reached `Poll::Ready(None)`. A stream dropped before that finalizes its row as a disconnect, keeping whatever token counts the frames already carried |
| In-flight accounting | `LoadGuard`'s `Drop` returns the slot; it is held by the same future, so cancellation releases it on the same unwind |

Both paths are RAII rather than explicit cleanup on purpose: a cancelled future
is never resumed, so anything written as "log after the await" would simply not
run.

## Metrics

| Series | Meaning |
|---|---|
| `rolter_client_disconnects_total` | counter of requests whose caller left before the response completed. A rising rate usually means client timeouts are tighter than upstream latency, not that anything is broken here |
| `rolter_inflight_requests` | gauge of requests currently in flight, summed across targets and sampled at scrape time. On an idle gateway it must read `0` — a floor that never returns to zero is a leaked slot, and the fastest signal that guarantee 3 has regressed |

`crates/rolter-gateway/tests/client_disconnect.rs` pins all of it: a
non-streaming caller that times out, a stream abandoned mid-body, and a normal
request that must not be marked or counted.
