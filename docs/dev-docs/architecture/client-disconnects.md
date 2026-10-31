# Client disconnects

A caller can leave at any point in a request: a browser tab closes mid-stream, a
client-side timeout fires while the upstream is still thinking, a Kubernetes
rollout kills the pod holding the connection. The gateway sees this as the
response future — or the response body stream — being dropped, and it happens
often enough at scale that "undefined" is not an acceptable answer. This page
states what rolter does (#1083).

## The three guarantees

1. **The request is logged exactly once, with status `499`.** The status is
   nginx's convention for _client closed request_: it is not a real HTTP status,
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

The row names the target the caller left (#1816). The guard is armed before a
target is picked, so each attempt of the forward loop calls
`CancelGuard::attribute(provider, target, variant)` as it picks one, and a retry
that fails over moves the row with it. The classic loop, `forward_variants` and
the multipart loop all do. A request abandoned before any pick keeps an empty
`provider`, which is how that rarer case stays distinguishable. Without the
attribution, the slow target that made callers give up was exactly the one
missing from the logs: a dogfood run logged 17 timed-out requests as
`provider = ''`.

**A disconnect is never charged to the target it names** (#1933, a maintainer
decision). `LogSink::log` takes the per-request signals from every row, but
skips the two per-target ones, `rolter_target_requests_total` (neither `error`
nor `ok`) and the passive health event, for a row where
`RequestLog::is_client_disconnect()` holds: the gateway's status is
`CLIENT_DISCONNECT_STATUS` and the upstream's is not. The second half matters:
an upstream that answers `499` itself is passed through with both statuses
agreeing, and that is still the target's failure. The rule covers a caller who
left while queued in rolter's own provider queue (before any upstream I/O, so a
healthy target is not billed for rolter's queueing), while the upstream was
working, and part-way through a stream. The disconnect stays countable in
`rolter_client_disconnects_total`, and the row still names the target so an
operator can see where callers give up. Because the health event is skipped, the
uptime rollup and the `provider_health_flaps` alert never see it.

What was already settled is that one upstream failure is charged once. The forward loops funnel a failed
attempt that a retry will supersede through `CancelGuard::record_failed_attempt`
(#1646), which also marks the guard, and then back off. A caller who leaves
during that backoff still gets a row naming the target, but the guard's `Drop`
logs it through `LogSink::log_recorded_attempt`, which skips the per-target
counter and the health event, the same way the loop's own error row does for an
attempt it already funnelled. `attribute` clears the mark, since the next
attempt has not been counted anywhere yet.

## How it is enforced

| Where                      | Mechanism                                                                                                                                                                                                                                                                                                                              |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Before the response starts | `CancelGuard` (`crates/rolter-gateway/src/cancel.rs`) is armed before the forward loop in `proxy()` and `proxy_multipart()`. Its `Drop` stamps `499`, the error text and the elapsed latency, counts the disconnect and logs the row. It is _disarmed_ the moment an arm takes ownership of the row, so a normal answer never trips it |
| While the body streams     | `UsageLoggingStream` (`logging.rs`) tracks whether it reached `Poll::Ready(None)`. A stream dropped before that finalizes its row as a disconnect, keeping whatever token counts the frames already carried                                                                                                                            |
| In-flight accounting       | `LoadGuard`'s `Drop` returns the slot; it is held by the same future, so cancellation releases it on the same unwind                                                                                                                                                                                                                   |

Both paths are RAII rather than explicit cleanup on purpose: a cancelled future
is never resumed, so anything written as "log after the await" would simply not
run.

## Metrics

| Series                            | Meaning                                                                                                                                                                                                                                   |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `rolter_client_disconnects_total` | counter of requests whose caller left before the response completed. A rising rate usually means client timeouts are tighter than upstream latency, not that anything is broken here                                                      |
| `rolter_inflight_requests`        | gauge of requests currently in flight, summed across targets and sampled at scrape time. On an idle gateway it must read `0` — a floor that never returns to zero is a leaked slot, and the fastest signal that guarantee 3 has regressed |

`crates/rolter-gateway/tests/client_disconnect.rs` pins all of it: a
non-streaming caller that times out, a caller that leaves during a retry backoff
(one `error` on the target and one health event, not two), a stream abandoned
mid-body, and a normal request that must not be marked or counted. Three tests
pin the #1933 rule for the queued, in-flight and mid-stream cases: no
`rolter_target_requests_total` series moves and no passive health event
arrives, while `rolter_client_disconnects_total` reads `1`. A fourth pins that
an upstream's own `499` is still charged.
