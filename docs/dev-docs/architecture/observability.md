# Observability

## Metrics

- The gateway exposes Prometheus metrics at `GET /metrics`: counters (`rolter_requests_total`, `rolter_upstream_errors_total`, `rolter_auth_failures_total`, reload/log/budget/rate-limit/retry/cooldown/health/breaker/scrape counters), gauges (`rolter_config_version`, `rolter_breaker_entries`), and per-model **latency histograms** — `rolter_request_latency_ms` (total) and `rolter_request_ttft_ms` (time-to-first-token), each labelled `{model=...}` with the standard `_bucket`/`_sum`/`_count` series. Histograms are observed once per completed request from the log sink, off the response hot path.
- The exporter is hand-rolled (atomic counters + non-cumulative histogram buckets cumulated at render) rather than the `metrics` facade + global recorder, which does not fit the lock-free `arc-swap` design where an explicit `Arc<Metrics>` is threaded through the request path.
- Passive per-target SLA signal: `rolter_target_requests_total{provider,target,outcome}` (a counter, `outcome` = `ok` for 2xx else `error`) is tallied once per completed request from the log sink — free, derived from real traffic, no extra upstream calls. A per-target error rate / uptime is `sum(rate(rolter_target_requests_total{outcome="error"}[5m])) / sum(rate(rolter_target_requests_total[5m]))`. This is the first slice of provider stability tracking (ROL-123); the ClickHouse `provider_health_events` table and the dashboard land in later slices. The active prober is guarded: bounded probe concurrency with per-provider jitter, consecutive-failure/-recovery thresholds gating the unhealthy flip (no single-probe flapping), and exponential probe backoff when a probe itself gets a 429.
- Client disconnects (#1083): `rolter_client_disconnects_total` counts requests whose caller left before the response completed, and `rolter_inflight_requests` is the live in-flight gauge those requests must return to zero. Abandoned requests are logged with status `499` and are never retried — see [Client disconnects](client-disconnects.md).
- Provider queues (#1855): `rolter_provider_queue_depth{provider}` and `rolter_provider_inflight{provider}` gauges and the `rolter_provider_queue_wait_ms{provider}` histogram, kept by RAII guards in `metrics.rs`. A job carries a `QueuedGuard` from the moment it is built until a worker takes it (`picked()` observes the wait and hands over an `InflightGuard`) or it is dropped unrun, so a shed or stranded job can never leave the depth raised; the in-flight guard lives until the upstream's response headers arrive, which is also when a worker is released, and covers the direct path when queueing is off. The steady-state lookup borrows the provider name, so the path allocates nothing. Over OTLP (#1862) the two gauges are a _labelled gauge family_ (`LABELLED_GAUGE_FAMILIES`, registered as observable gauges beside the labelled counter families by `register_labelled` in `rolter_core::telemetry`), and each wait is also recorded into an OTLP `rolter_provider_queue_wait_ms` histogram on the Prometheus boundaries, through a per-provider `QueueWaitRecorder` whose `provider` attribute is built once when the provider's counters are. The Redis consumers' connection state (#1772) rides the same two families; see [Redis connections](redis-connections.md). The `rolter · gateway capacity` board in `integration/dogfood/signoz/dashboards/` charts all of it.
- Billed but withheld (#1478): a response an output guardrail or post-response plugin refuses to deliver is still billed. Its row carries the caller's `403`, `withheld = 1`, the policy in `error`, and the provider's tokens and cost, and `rolter_withheld_responses_total` counts it. `usage_unknown = 1` marks any successful row whose upstream reported no usage. See [Billed but withheld](billed-but-withheld.md).
- Multi-key providers: `rolter_key_cooldowns_tripped_total` counts api keys parked after a key-level failure (429/401 on a provider with several keys); the request retries in-flight on a sibling key.
- A/B attribution: `rolter_variant_requests_total{model,variant}` (a counter) tallies requests per chosen variant, so traffic splits are visible in Prometheus/Grafana without querying ClickHouse. Classic single-pool routes (no variant) emit nothing. Observed from the same log-sink funnel (ROL-195, part of ROL-188).
- Adaptive routing (#544): `rolter_adaptive_routing_decisions_total{model,mode}` splits a route's picks between `blend` (the latency/cost/load blend), `exploration` (the bounded random share that keeps starved targets sampled) and `fallback` (the deterministic `pipeline` stack served while the kill switch is off or the evidence is too thin), and `rolter_adaptive_routing_engaged{model}` is `1` while the blend is actually routing. Only routes on the `adaptive` strategy emit these. A config reload rebuilds the balancer and so resets the counters, which lines up with a `rolter_config_version` bump — alert on `rate()`, not the absolute value. A route sitting at `engaged 0` with all picks on `fallback` is the expected steady state before an operator enables the policy. `rolter_adaptive_routing_target_score{model,target}` (#751) adds the blended score each target currently carries — the same numbers the control plane serves at `GET /api/v1/adaptive-routing-telemetry`, computed at scrape time rather than on the request path.
- Roadmap: add per-provider/route labels on the histograms, in-flight gauges, cache-hit ratio, and circuit-breaker state gauges.
- Roadmap: **scrape/federate upstream engine metrics** from vLLM/SGLang/TGI `/metrics` and correlate them per target (queue depth, KV-cache usage, running/waiting requests) to feed load- and cache-aware routing and the dashboard.

## Where request logs go (#929)

The dashboard's Usage, Costs and Logs screens read ClickHouse through the
control plane. The rows they read are written by the **gateway**. For a long
time nothing connected the two: the control plane took `CLICKHOUSE_URL`, the
gateway took `[logging].clickhouse_url` from its own bootstrap TOML, and a
deployment that set only the first one logged nowhere while the screens queried
a table nobody filled. The snapshot even carried the field, as
`"clickhouse_url": null`, so the control plane was telling every gateway to log
nowhere.

There is now one variable and one order of precedence. The gateway resolves its
destination, at startup, as:

1. `[logging].clickhouse_url` in its own config file — an explicit local
   decision is never overridden.
2. `CLICKHOUSE_URL` in its environment. Both processes read this one variable,
   which is what a compose file, a Helm release or `rolter init` sets.
3. The value the control plane publishes in `/internal/snapshot`, when the
   gateway is started with `--snapshot-url`. The control plane puts its own
   `CLICKHOUSE_URL` there, so a fleet writes where the dashboard reads without
   anyone configuring the gateways individually.

Step 3 is read **before** `AppState` is built, not on the first poll, because
the ClickHouse writer is spawned once at startup — the hot-swappable snapshot
carries routing, not background tasks, so a destination arriving on a later
poll would arrive too late to open a sink. It is best-effort by construction: a
control plane that is not up yet costs one five-second timeout and the gateway
starts exactly as it did before.

When all three come up empty on a gateway that has a control plane, startup
logs a warning naming both fixes. An empty analytics screen and a quiet
deployment look identical, and `rolter check` says the same thing before
anything starts.

## Tracing & context propagation

- `tracing` + `tracing-subscriber` with `RUST_LOG` filtering; `TraceLayer` logs each HTTP request.
- **Inbound**: accept W3C `traceparent`/`tracestate` (and `b3`) from clients and continue the trace; honor `x-request-id` / `x-correlation-id`.
- **Outbound to engines**: inject the active trace context into upstream requests so vLLM/SGLang/TGI spans join the **same** distributed trace. vLLM and SGLang support OpenTelemetry tracing (e.g. vLLM `--otlp-traces-endpoint`); point them at the same OTLP collector so engine prefill/decode spans line up with rolter's request span.
- A per-request `request_id` is echoed in a response header and stamped on logs, metric exemplars and spans for correlation.
- **Events**: rolter calls no span-event API directly, but `tracing-opentelemetry` turns every `tracing` event fired inside an active span into an OTel span event. That API is deprecated upstream in favour of log-based events; the target model, and the sequencing it forces, are recorded in [ADR-0025](../adr/2026-08-06-events-as-logs.md).

### How the context actually moves

`rolter-core::telemetry` owns both directions, and both are inert unless an OTLP
endpoint is configured:

- **Extract.** `GatewayMakeSpan` (`rolter-gateway::trace`) is the `TraceLayer`'s
  span-maker: it builds the request span and makes the extracted inbound context
  its _parent_. A B3-only caller is normalized into an equivalent `traceparent`
  first, so one W3C propagator serves both wire formats. Without this the
  gateway's spans were disconnected roots — the trace id reached the request log,
  but nothing joined the caller's trace.

  It has to be the span-maker rather than a middleware layered inside the
  `TraceLayer`: `DefaultMakeSpan` builds the request span at **DEBUG**, so under
  the default `RUST_LOG=info` it is disabled, and setting a parent on a disabled
  span silently does nothing. With no pipeline installed it falls back to that
  stock DEBUG span, so the untraced path costs what it always did.

- **Inject.** The context handed to the provider is injected from the _current_
  span, inside the per-attempt `upstream.request` span, rather than copied from
  the caller. Copying it verbatim made the provider call a child of the caller's
  span and therefore a **sibling** of the gateway's own work, which silently
  invalidated every waterfall built from the data. The allowlisted client headers
  from `Forwarder::forwarded_header_names` are unaffected; only the trace headers
  changed hands.
- **Log correlation.** `RequestLog.trace_id` is read off the span context when a
  pipeline is installed and falls back to parsing the inbound header otherwise,
  so ClickHouse and the trace backend agree by construction.

### Pipeline spans

One span per stage, so a slow request is attributable rather than merely slow:

| Span                | Attributes                                                                                                                                                                                              |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `auth`              | —                                                                                                                                                                                                       |
| `guardrails.pre`    | `redacted`, `webhook`                                                                                                                                                                                   |
| `route.select`      | `route`, `strategy`, `candidates`                                                                                                                                                                       |
| `cache.lookup`      | `hit`, `kind` (`exact` / `semantic`)                                                                                                                                                                    |
| `queue.wait`        | `provider`                                                                                                                                                                                              |
| `upstream.request`  | `attempt`, `gen_ai.system`, `gen_ai.request.model`, `gen_ai.response.id`, `http.response.status_code` (embeddings also carry `gen_ai.request.encoding_formats` and `gen_ai.embeddings.dimension.count`) |
| `translate.request` | —                                                                                                                                                                                                       |
| `guardrails.post`   | —                                                                                                                                                                                                       |

`queue.wait` spans enqueue→dequeue only: the span travels with the queued job and
the worker closes it the moment it picks the job up, so it measures the wait and
not the wait plus the upstream call. The job carries the caller's span alongside
it, and the worker instruments the forward with it — the queue worker runs on its
own task where nothing is in scope, so without that the forwarder's own
`translate.request` span becomes an orphan root in a trace of its own. Names
follow the OTel
[GenAI semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/)
where they fit, so a backend's built-in GenAI views work.

Spans never carry prompt or completion content, API keys, virtual-key plaintext,
or injected header values — those are credential material, and redaction stays
owned by the existing `logging_settings` machinery rather than a second policy.

### Control-plane spans

The control plane runs the same pipelines as the gateway (`telemetry::init()`),
but until #845 emitted no spans of its own — everything it did was invisible
beyond what the HTTP layer produced by default.

| Span                | Attributes                                                       |
| ------------------- | ---------------------------------------------------------------- |
| `control.request`   | `http.route`, `http.request.method`, `http.response.status_code` |
| `snapshot.build`    | `config_version`, `payload_bytes`, `outcome`                     |
| `snapshot.sanitize` | —                                                                |
| `snapshot.encode`   | —                                                                |

`http.route` is the _matched_ template (`/api/v1/providers/{id}`), never the
concrete path. `control.request` comes from one middleware rather than an
attribute on each of ~90 handlers, so a route added tomorrow is instrumented the
moment it is mounted.

`snapshot.build` is the one to watch. Snapshot latency is fleet-wide
config-propagation delay: every gateway waits on it, so when an operator changes
a route and the fleet serves stale config, this span is what says whether the
delay is in generation or downstream of it. `config_version` on the span answers
"which config was this" without putting an unbounded value on a metric.

### Tenant attributes

The `gateway.request` span carries `rolter.org.id`, `rolter.team.id` and
`rolter.project.id`, recorded once the virtual key resolves. The span itself is
built by the tower layer, which runs before auth, so the fields start empty and
are filled in rather than passed at construction.

These exist so per-tenant telemetry destinations are routable. ADR-0026 decided
that fan-out to tenant-owned backends belongs in an OpenTelemetry Collector
rather than in-process exporters — one egress path in the gateway no matter how
many tenants, and no data-plane process POSTing to operator-supplied URLs — and
that rolter's job in that design is to _stamp the attribute the collector routes
on_. Until this landed there was nothing to stamp: the request logs in
ClickHouse always carried tenant identity, but no exported span ever did, which
made the routing half unimplementable.

An unattributed request — a config-defined key, which has no org — records
nothing rather than an empty string. An attribute that is present-but-blank on
some spans and absent on others is harder to write a routing rule against than
one that is consistently absent.

The names are deliberately rolter-local. The GenAI and HTTP conventions define
nothing for tenancy, and a convention-shaped guess like `tenant.id` would be
worse than an obviously-local name if the spec later defines it differently.

### Running it locally

The observability overlay starts a collector and a trace UI alongside the normal
stack:

```
docker compose -f docker/docker-compose.yml \
               -f docker/docker-compose.observability.yml up
```

Traces land at <http://localhost:16686>; the collector takes OTLP on 4317/4318
and re-exposes collected metrics on 8889. The overlay sets
`OTEL_EXPORTER_OTLP_ENDPOINT` on the `gateway` and `control` services, so
bringing it up is the only step — without it that variable is unset and tracing
stays off.

The collector binds `0.0.0.0`, not `localhost`: one bound to loopback inside its
container is unreachable from the gateway container.

#### Choosing an overlay

There are two, and they are mutually exclusive — both publish OTLP on 4317/4318.

|            | `docker-compose.observability.yml` (default) | `docker-compose.signoz.yml`                       |
| ---------- | -------------------------------------------- | ------------------------------------------------- |
| backend    | Jaeger v2                                    | SigNoz                                            |
| signals    | traces only                                  | traces, metrics, logs                             |
| containers | 2                                            | 5, incl. its own ClickHouse + Zookeeper           |
| storage    | in memory, lost on restart                   | persistent                                        |
| use it for | reading a waterfall, fast iteration          | aggregate views, dashboards, dogfooding over time |

```
docker compose -f docker/docker-compose.yml \
               -f docker/docker-compose.signoz.yml up      # SigNoz on :8080
```

Jaeger is the default because it is two containers and starts in seconds, and
because reading a correctly-parented waterfall is what the tracing work needed.
Reach for SigNoz when "which stage is slow _across all requests_" matters, which
Jaeger cannot answer.

Nothing in the Rust differs between them: the gateway speaks vendor-neutral OTLP
and only the destination changes. Any other OTLP backend works the same way —
repoint the exporter in `infra/otel/collector.compose.yaml`.

#### Querying SigNoz from an agent (MCP)

The SigNoz overlay also starts SigNoz's MCP server on `http://localhost:8000/mcp`,
so an agent can query traces, run ClickHouse queries, and manage dashboards
against the local instance. Point an MCP client at that URL; the
[SigNoz agent-skills plugin](https://github.com/SigNoz/agent-skills) ships a
`signoz` server entry to fill in.

It needs a SigNoz API key, which is created in the UI (**Settings → API Keys**,
admin only) and supplied to the _server_, not the client:

```
set -x SIGNOZ_API_KEY (pass show rolter/signoz-api-key)
docker compose -f docker/docker-compose.yml \
               -f docker/docker-compose.signoz.yml up -d signoz-mcp
```

The key is read from the environment and never written to a tracked file. With
no key set the container still starts, but every call returns
`Authorization or SIGNOZ-API-KEY header required`.

The MCP dashboard tools need SigNoz v0.135.0 or newer, which is why the `signoz`
image is pinned ahead of the collector/ClickHouse pair.

The SigNoz overlay is a pinned equivalent of what SigNoz's Foundry CLI generates,
since SigNoz deprecated its own compose manifests in v0.130.0. Two things to know
before touching it: its four images are a **tested set** and must be bumped
together (a newer migrator emits ClickHouse settings an older server rejects),
and it vendors 3.6 KB of ClickHouse config — the cluster topology and the
`{shard}`/`{replica}` macros that `ReplicatedMergeTree` needs — rather than
SigNoz's full 56 KB `config.xml`, which the stock image defaults cover.

### Metrics over OTLP

The counters and gauges `rolter-gateway::metrics` computes are exported over
OTLP as well as served on `/metrics`. The Prometheus endpoint is unchanged —
this is a second exporter over the same numbers.

Both read one list, `Metrics::scalars()`, so they cannot drift: a counter added
there reaches Prometheus and OTLP without a second edit. Counters export as
counters and gauges as gauges, since exporting a counter as a gauge would break
`rate()` on the backend.

The instruments are **observable**: nothing is pushed on the request path. The
SDK invokes the callback on its own schedule (`OTEL_METRIC_EXPORT_INTERVAL`,
default 60s) and reads the same atomics the Prometheus renderer does, so the hot
path still only does `fetch_add`. With no OTLP endpoint configured no meter
provider, exporter or callback is built at all.

Per-model histograms and label-bearing counters stay Prometheus-only for now;
the scalar set is what OTLP carries.

### Control-plane metrics

The control plane has no Prometheus registry to mirror, so these are the one
place it measures itself (#845). They are real histograms — measurements taken
as they happen — not observable instruments, because a duration cannot be
reconstructed from a counter after the fact.

| Metric                          | Unit | Attributes                                                        |
| ------------------------------- | ---- | ----------------------------------------------------------------- |
| `rolter_snapshot_build_ms`      | ms   | `outcome` (`ok` / `not_modified` / `error`)                       |
| `rolter_snapshot_payload_bytes` | By   | `outcome`                                                         |
| `rolter_control_request_ms`     | ms   | `http.route`, `http.request.method`, `http.response.status_class` |
| `rolter_db_pool_acquire_ms`     | ms   | `outcome` (`ok` / `timeout`)                                      |

And one counter, for the endpoint an unauthenticated attacker can reach (#1079):

| Metric                          | Meaning                                 | Attributes                                                                                                                                |
| ------------------------------- | --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `rolter_control_login_attempts` | resolved control-plane sign-in attempts | `outcome` (`success` / `invalid` / `throttled` / `locked` / `error` / `mfa_challenge` / `mfa_invalid` / `mfa_enrolment` / `mfa_required`) |

A counter rather than a histogram: the question it answers — "is somebody
running a credential-stuffing run against this deployment" — is a rate, not a
distribution. It carries no account or address label; either would be unbounded
cardinality _and_ would put the identity an attacker is guessing into the
metrics pipeline. A rising `invalid` with a rising `locked` behind it is the
throttle working; a rising `invalid` with no `locked` means the run is spread
thin enough to stay inside the per-account budget, and the per-address budget is
the one to tighten.

And one for the telemetry the control plane ingests but cannot store (#1747):

| Metric                           | Meaning                                         | Attributes                                                                |
| -------------------------------- | ----------------------------------------------- | ------------------------------------------------------------------------- |
| `rolter_control_ingest_failures` | UX-event or MCP-log writes that were not stored | `stream` (`ui_events` / `mcp_logs`), `reason` (`insert` / `unconfigured`) |

Both callers swallow the failure by design — the dashboard drops the batch and
keeps flushing — so without this a missing `ui_events` table loses the whole UX
stream with no signal at either end. `insert` is a store that refused or could
not be reached; `unconfigured` is a control plane with no `CLICKHOUSE_URL`. The
store's error text is deliberately not a label (it is unbounded); it is in the
matching `telemetry ingest failed` warning, which `crates/rolter-control/src/ingest_failure.rs`
rate-limits to one per minute per stream and which carries a `suppressed` count
of the failures it stands for. Any sustained non-zero rate is worth an alert.

And the connection pool, as observable gauges (#1052):

| Metric                       | Meaning                              |
| ---------------------------- | ------------------------------------ |
| `rolter_db_pool_connections` | connections the pool holds open      |
| `rolter_db_pool_idle`        | how many of those are free right now |
| `rolter_db_pool_max`         | the configured ceiling               |

One pool serves `/internal/snapshot`, the whole CRUD surface and every RBAC
membership lookup, so a ceiling that is too low presents as "the control plane
got slow" with nothing to attribute it to. The three gauges together are what
separate the two cases: **pool-bound** is `connections == max` while `idle` is
zero _and_ `rolter_db_pool_acquire_ms` shows waits; acquire waits without a
pinned pool mean the database itself is slow, and raising the ceiling there
makes it worse.

`rolter_db_pool_acquire_ms` is sampled on a 15-second timer rather than
instrumented per call. Wrapping every `acquire()` would mean touching every
repository method to answer a question that is a distribution over time, not a
per-request fact. The probe takes one connection and drops it: if that is
disruptive, the pool is already far too small, and that is the finding.

Boundaries are deliberately not the gateway's. A gateway request is dominated by
an upstream model call and is interesting out to tens of seconds; a snapshot
build and a CRUD write are database work where "is this 2 ms or 40 ms" is the
whole question, so reusing the gateway's boundaries would put nearly every
observation in the first bucket.

**Cardinality is bounded by construction.** `http.route` is the matched
template, so a thousand providers are one series. Status is recorded as a
_class_, not a code: twelve statuses across ninety routes would be over a
thousand series to answer a question five buckets answer. `config_version` is
unbounded — a new value on every config write — so it lives on the
`snapshot.build` span and never on a metric.

`rolter_snapshot_payload_bytes` skips the `304` case rather than recording a
zero: a not-modified poll transfers no body, and folding zeroes in would drag
the size distribution down and misreport what the fleet actually moves.

Payload size matters on its own. It is what every gateway transfers on every
poll, and it is the first thing to look at when propagation gets slower without
generation getting slower. The `snapshot` bench in `rolter-core` shows why the
encode is the stage to watch: at 1000 routes it costs ~2.8 ms against ~150 µs
for sanitize and ~120 µs for validate.

The control plane also registers the same process and runtime metrics as the
gateway — it degrades for the same reasons and previously answered none of those
questions either.

### Logs over OTLP

Logs export alongside traces and metrics, gated on the same environment
(`OTEL_EXPORTER_OTLP_ENDPOINT`, or the logs-specific
`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`). With neither set no exporter is built and
logging is stdout-only, exactly as before.

It is **additive, not a replacement**: the stdout `fmt` layer is untouched, so
`docker logs` shows what it always did and the collector additionally receives
the same records.

Records carry `trace_id` and `span_id`, which is most of the value — a log joins
the trace it came from instead of being a separate pile of text.
`tracing-opentelemetry` publishes the OpenTelemetry context when a span is
entered, and the logs SDK stamps it onto each record. That correlation is
asserted by a unit test rather than assumed from a dependency default, since it
would fail silently if the default ever changed.

Redaction is unaffected: this exports the same `tracing` events the stdout layer
renders, so anything already kept out of logs stays out of them.

### Process metrics

Alongside the domain counters, the process reports its own vitals: resident and
virtual memory, CPU time, open file descriptors, thread count and uptime. These
are the numbers an operator reaches for first when a node degrades, and the
gateway previously answered none of them.

They are observable instruments read from `/proc`, so nothing touches the request
path. On a platform where `/proc` is unavailable the instruments are **not
registered at all**, rather than registered and always zero — a metric that reads
zero forever is worse than an absent one, because a dashboard cannot tell it from
a healthy process. Names follow the OTel `process.*` conventions.

### Runtime metrics

The process metrics describe the machine; these describe the scheduler.
`tokio.runtime.queue.depth` is the one that matters: when latency rises, no
other signal separates "the provider is slow" from "the request sat in our own
run queue before we ever called the provider", and the two have opposite fixes.
It pairs directly with the `queue.wait` span. Alongside it are
`tokio.runtime.workers`, `tokio.runtime.tasks.alive` and
`tokio.runtime.worker.busy.time`.

Busy _time_ is exported, not a busy ratio: a ratio computed in-process would
average over whatever interval the SDK happens to use and would not re-aggregate
across instances. As a monotonic counter the backend derives utilisation with
`rate(tokio.runtime.worker.busy.time) / tokio.runtime.workers`, which does.

None of this requires `--cfg tokio_unstable`. #834 assumed it did, and that is
true only of part of tokio's surface: `num_workers`, `num_alive_tasks` and
`global_queue_depth` are stable, and `worker_total_busy_duration` sits behind
`cfg_64bit_metrics!`, which is `#[cfg(target_has_atomic = "64")]` — a property
of the target, not an instability gate. What remains genuinely gated is the
blocking pool and the per-worker steal/poll counters, which are therefore not
exported; the workspace-wide flag decision stays unmade rather than being
smuggled in with a telemetry change.

Outside an async runtime the instruments are not registered, for the same reason
the process metrics are absent without `/proc`.

### Connection-pool metrics

`rolter_upstream_connections_total` and `rolter_upstream_connect_errors_total`
count connections established to providers, and attempts that never got that
far.

Pool exhaustion presents as latency with healthy providers — the failure mode
the other metrics cannot explain. `reqwest` and `hyper` expose no pool
introspection at all, so this is instrumented rather than read: a tower layer
over the connector (`ClientBuilder::connector_layer`) sees every connection
hyper builds because it had none to reuse. The signal is the ratio
`rate(rolter_upstream_connections_total) / rate(rolter_requests_total)` — near
zero when the pool is working, approaching one when every request is paying a
fresh TCP and TLS handshake.

Idle-versus-active counts stay unavailable: that needs the connection object's
drop, and the connector layer's response type is opaque outside `reqwest`.

### Resource attributes

Every signal carries `service.name`, `service.version` (the crate version), and
where configured `service.instance.id` and `deployment.environment.name`.
`OTEL_RESOURCE_ATTRIBUTES` is honoured by the SDK for anything else.

`service.instance.id` is the very same value the cluster watcher sends, read
from `rolter_core::node_identity` rather than derived a second time, so a node
in `cluster_nodes` and a node in the trace backend are the same node by
construction. The precedence is `ROLTER_NODE_ID`, then `HOSTNAME`, then the
host's own name from `gethostname` (#1644) — the syscall is last so an operator
who names a replica keeps that name, and it exists because a gateway started
from a shell, a systemd unit or a launchd job has no `HOSTNAME` in its
environment. When even the syscall fails the attribute is omitted rather than
invented per restart, which would churn the identity on every deploy. A value
the control plane's ingest would reject — blank, longer than 128 bytes, or
carrying a control character — counts as not resolved, so nothing is ever
reported under an id that is discarded on arrival.

### Wrapping audit (#815)

OpenTelemetry's [_Don't wrap OpenTelemetry_](https://opentelemetry.io/blog/2026/dont-wrap-opentelemetry/)
argues that a house abstraction over the instrumentation API costs performance,
maintainability and developer education. Three anti-patterns are named: wrappers
that force callers to allocate an attribute collection, wrappers that look an
instrument up by name per measurement, and general API abstraction that teaches a
proprietary interface instead of the standard.

rolter has four things sitting between its code and the OTel API. Each was
audited against that post; the verdict is **keep** for all four, for the reasons
below. The post is guidance rather than a mandate, and it asks for a refactor
only where there is a measured cost or a real maintenance burden.

Note up front that rolter instruments through `tracing` + `tracing-opentelemetry`.
That is the ecosystem bridge, not a bespoke house wrapper, and it is not what the
post argues against. This audit is not a proposal to remove `tracing`.

| Item                                              | Verdict | Why                                                             |
| ------------------------------------------------- | ------- | --------------------------------------------------------------- |
| `stage_span!` (`rolter-core/src/telemetry.rs`)    | keep    | code generation, not a runtime wrapper                          |
| The scalar-metrics list (`Metrics::scalars()`)    | keep    | the by-name lookup is on the export path, not the request path  |
| `RequestHistograms::record`                       | keep    | one unavoidable allocation; the alternative is the anti-pattern |
| `GatewayMakeSpan` (`rolter-gateway/src/trace.rs`) | keep    | SDK/layer configuration, explicitly out of scope                |

**`stage_span!`** expands to a direct `tracing::info_span!` call guarded by an
`is_active()` check, so it is closer to the code generation the post recommends
than to a runtime wrapper. It takes `tracing`'s own compile-time field syntax and
never asks a caller to build a `Vec` or a slice of attributes, so the
force-allocation anti-pattern does not apply. The guard is the point of the
macro: with no pipeline installed it yields `Span::none()`, which allocates
nothing.

**The scalar-metrics list** is the shape most at risk, since `install_metrics`
registers one observable instrument per scalar and each instrument's callback
calls `collect()` and finds its own entry by name. That is a by-name lookup, but
it is not on a hot path: the instruments are _observable_, so the SDK invokes
those callbacks on its own export interval (`OTEL_METRIC_EXPORT_INTERVAL`,
default 60s). The request path only ever does `fetch_add` on a named `AtomicU64`
field — there is no map, no lookup and no lock between a request and its counter.
The post's performance argument therefore does not bite here even though the
gateway hot path is where it would bite hardest.

What the export path does cost is one `Vec<ScalarMetric>` allocation per
instrument per cycle and a linear scan of it, so the work is quadratic in the
number of scalars. At the current eight scalars, once a minute, that is
immaterial. It is left as-is deliberately: the OTel Rust 0.33 API offers only
per-instrument `with_callback`, so collecting once per cycle for all instruments
is not expressible, and matching by name rather than by index keeps the callbacks
independent of the order `scalars()` happens to return.

**`RequestHistograms::record`** is the one place a wrapper does force an
allocation on the request path — `model.to_string()`, to build the single
`KeyValue` both histograms take. It is unavoidable rather than incidental: OTel's
`Value::String` holds an owned or `'static` string and model names are neither.
The obvious way to avoid it is to cache a prebuilt attribute set per model, which
would put a sharded-lock map lookup on the request path — precisely the
lookup-based anti-pattern the post names, and something AGENTS.md forbids on the
data-plane hot path. One small allocation is the cheaper of the two, and it is
paid only when an OTLP endpoint is configured.

**`GatewayMakeSpan`** is a `tower_http::trace::MakeSpan` implementation: layer
configuration, which the post explicitly separates from instrumentation and calls
_not_ wrapping. Recorded here only so the audit is complete.

### Turning telemetry off explicitly

`ROLTER_TELEMETRY_ENABLED=false` hard-disables every export — traces, metrics and
the dashboard's browser tracing — regardless of which `OTEL_*` endpoints are set
(#812). Unset means enabled, which changes nothing for an existing deployment:
with no endpoint configured nothing is exported anyway.

The switch can only _subtract_. It never turns export on by itself, and an
unrecognized value leaves export on rather than silently blinding a deployment;
only `0`, `false`, `no` and `off` disable it.

It exists because "off" was previously implicit — achieved by leaving an endpoint
unset — which does not survive somebody setting the endpoint for one signal and
gives an operator nothing to point at in a security review. It is deliberately
environment-only and has no config-file equivalent; see
[ADR-0026](../adr/2026-08-06-tenant-telemetry-destinations.md), which also
records why per-tenant telemetry destinations belong in the collector rather than
in rolter.

### Cost when tracing is off

With no `OTEL_EXPORTER_OTLP_ENDPOINT` (the default) behaviour and hot-path cost
are unchanged: `telemetry::is_active()` is a single relaxed atomic load, stage
spans are `Span::none()` (no allocation, and instrumenting a future with one is a
no-op), no carrier is built, and outbound trace headers are copied verbatim
exactly as before.

## Exporters (OTel-compatible)

rolter emits traces and metrics via **OpenTelemetry OTLP** (gRPC/HTTP), so any OTel-compatible backend works without code changes — just set an endpoint and headers:

- **SigNoz**, **Grafana Tempo/Mimir**, **Honeycomb**, **Datadog** (OTLP intake or the OTel Collector `datadog` exporter).
- **Langfuse** for LLM-specific observability (prompt/response, token usage and cost as traces), ingested via its OTLP endpoint or SDK.

Recommended topology: rolter → **OpenTelemetry Collector** → fan-out to the chosen backends. The collector also scrapes the upstream engines' `/metrics` and rolter's `/metrics`, keeping vendor specifics out of rolter. Configure via env, e.g. `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME=rolter-gateway`.

[`infra/otel/collector.yaml`](../../../infra/otel/collector.yaml) is a ready-to-run
local collector example: it accepts OTLP/gRPC and OTLP/HTTP, scrapes the
compose gateway's Prometheus endpoint, exposes collected metrics on `:8889`,
and prints telemetry via the `debug` exporter. It intentionally has no external
backend configured, keeping the default example safe for air-gapped use. Replace
the `debug` exporter with an internal OTLP-compatible destination for production.

### Connector delivery: rendered collector config (#836)

The `observability_connectors` table (#511, `docs/dev-docs/architecture/data-model.md`)
lists the sinks a deployment wants its telemetry shipped to in addition to
ClickHouse, but the connector's `endpoint` is never called directly by rolter.
Consistent with [ADR-0026](../adr/2026-08-06-tenant-telemetry-destinations.md)
— fan-out belongs in the collector, not N in-process SDK exporters each
carrying its own connection/queue/retry buffer, and terminating an
operator-supplied URL in the collector keeps that SSRF surface out of the data
plane — `GET /api/v1/connectors/collector-config` (superadmin-only) renders an
OpenTelemetry Collector config document from the enabled rows instead: one
`otlphttp` exporter and one `traces`/`metrics`/`logs` pipeline set per
connector, receiving from the `otlp` receiver rolter's own
`OTEL_EXPORTER_OTLP_*` export targets.

The endpoint answers a superadmin principal only, and a collector has no session
to present one, so a collector is not pointed at it: that would put the admin
token in the collector's own deployment. The dashboard's **Collector config**
dialog (#1195, #2106) shows the document and the full endpoint URL, built from the
control plane's public base, and the operator saves the document as the collector's
config file, or an automation fetches it with its own credential and reloads the
collector. A connector that is switched off is left out of the document, and the
dashboard creates connectors switched off unless its add sheet's start switch is on
(#2349). The Collector config dialog therefore branches on the number of _enabled_
connectors, not on all rows (#2364): none at all and all switched off are two
different empty states, and neither fetches a document. A card's edit sheet (#2101)
sends one `PUT` to the connector's id and carries the fields it has no control for
(`enabled`, `auth_secret_ref`) back as found, since the update replaces the whole
row. The update handler keeps a stored `managed_auth_secret` when the body omits it
and refuses an empty one, but, as `update_channel` does for an alert channel, clears
the ciphertext and nonce together when the endpoint moves to another scheme, host or
port and the body brings no new secret (#2403); the audit entry carries
`secret_cleared`, never the endpoint. `sampling_rate` becomes a `probabilistic_sampler`
processor scoped to that connector's own pipeline, since sampling is now the
collector's decision, applied independently per destination rather than once
for the whole deployment. A managed secret (`managed_auth_secret`) is decrypted
server-side and rendered as a literal `Authorization: Bearer` header; an
external reference (`auth_secret_ref`) instead renders as a `${env:...}`
placeholder the collector's own environment must resolve, since rolter never
holds that secret's value.

[`ROLTER_TELEMETRY_ENABLED`](#turning-telemetry-off-explicitly) wins over every
connector row: when it is off, the rendered document has no exporters and no
pipelines regardless of what is enabled in the registry, the same way it
already silences every in-process exporter.

Backpressure to a slow or dead sink is the collector's problem now, not
rolter's: it already queues and retries per exporter, which is the reason
fan-out moved here in the first place rather than something this endpoint
needs to build.

The remaining sink kinds from #511 (Datadog, Prometheus remote-write,
Langfuse) become an exporter-block case in this renderer, not a new in-process
delivery adapter — widening `KINDS` in `connectors.rs` and the `kind` check
constraint in migration `0062` is the only rolter-side change; everything else
is collector configuration.

## Alert evaluation and delivery (#1871)

`crates/rolter-control/src/alerting.rs` owns alerting end to end: channels,
rules, a 60-second evaluator started by every control-plane process that has a
database, and the `alert_notification_history` table (migration `0025`). The
user-facing behaviour is in `docs/user-docs/observability/alerting.mdx`; this
section is the reasoning behind it.

**One evaluation is one transaction around the rule's row lock.** The
ClickHouse read runs first, unlocked and bounded to 30 seconds, because it is
the slow part. Then the evaluation takes `select … for key share` on the rule's
channel, `select … for update` on the rule, decides whether the reading is a
transition, POSTs it, writes the rule and the history row, and commits. The
scheduled pass takes the rule lock with `skip locked` and `and enabled`, so a
rule another replica is evaluating is left to that replica and a rule disabled
mid-pass is skipped; **Evaluate now** waits for the lock instead. That is what
keeps `control.replicaCount > 1` from reporting each transition once per
replica. An edit that lands between the read and the lock and changes the
signal, window or channel discards the reading, since it measures a query the
rule no longer asks for or would go to a channel that was not locked.

The channel is locked before the rule because deleting a channel takes the
locks in that order: the `delete` locks the channel row, then its
`on delete set null` locks every rule naming it. With the evaluator taking the
rule first and meeting the channel only at the history insert's foreign-key
check, a delete issued during a POST deadlocked with it (`40P01`), and either
the delete returned a `500` or the evaluation rolled back after the receiver
already had the alert. In the same order, the delete simply waits for the
delivery to commit. `key share` rather than `share` keeps the channel editable
meanwhile, and `update_channel` locks with `for no key update` so an edit does
not queue behind a delivery either.

**An older reading never overwrites a newer one.** The unlocked read also takes
`clock_timestamp()`, the database clock rather than the replica's, as the
reading's time, and that is what `last_evaluated_at` stores. Once the rule is
locked, a reading older than the rule's `last_evaluated_at` is dropped: another
evaluation started later and has already recorded its result, so applying the
older one would report a transition pair that never happened. This matters
most for a query that runs into the 30-second bound while a later pass
succeeds.

**A transition is decided against the history, not the `state` column.**
`state` also holds `unknown` and `error`, and neither says what an operator was
last told. The newest history row does: `firing` is reported when the reading
is high and the last row is not `firing`, `resolved` when the reading is low
and the last row is `firing`. That makes three cases fall out without special
handling: a rule enabled while its condition holds fires on its first pass; an
evaluation error between two high readings does not fire twice; and a
condition that cleared while evaluation was failing still resolves. It also
makes a crash between the rule update and the history insert self-healing,
since the next pass sees no row and reports again. The history insert uses
`clock_timestamp()` rather than the column's `now()` default, because `now()`
is the transaction's start and a transaction that waited on the lock started
before the one it waited for.

**Delivery never fails an evaluation.** Every way a POST can go wrong becomes a
`failed` row with a short detail (the HTTP status, or `timed out`, `could not
connect`, `endpoint denied by the egress policy`, `channel secret could not be
unsealed`, `channel secret is not a valid header value`), and the rule's state
still moves. The response body is never read, since a receiver's error body can
echo the bearer secret it just rejected.

**A state that was never delivered is retried.** One dropped POST at the moment
a rule fires would otherwise mean no page for the whole incident, since every
later pass reads `firing` against a newest row of `firing`. So `transition()`
treats a holding state as unreported while every row for it since the last
change is `failed` or `skipped`, and sends it again once the rule has an
enabled channel. Each attempt is a new history row with a new `id`. The
scheduled pass spaces attempts out by the number that failed: the next pass,
then 2, 4, 8, 16 and 32 passes on, then hourly (`retry_delay`), so a dead
endpoint writes a handful of rows an hour rather than one a minute, and the
retries stop at the first `delivered` row or at the next change of state. A
`skipped` row costs no delay, so a transition recorded while the channel was
off goes out on the first pass after it is switched on or attached, and a rule
with no enabled channel writes nothing more. **Evaluate now** retries without
waiting, as an explicit operator request.

The delivery client is a dedicated `reqwest::Client` with redirects off, a
5-second connect and a 10-second total timeout. Redirects are off for the same
reason as MCP OAuth discovery: a `3xx` is how a host that passed the egress
check hands the request to one that would not have. The endpoint is checked
with `EgressPolicy::check_url` at save time and again before each POST, which
matches connectors, and is stored as `reqwest::Url` serializes it, so the check
and the request read one spelling of the host. A denial is logged with the
channel id and the reason, never the endpoint, whose query string can carry a
token. A save that only switches a channel off, with the endpoint unchanged,
skips the check, so a channel a since-tightened policy denies can still be
disabled. Nothing classifies what a hostname resolves to at connect time; that
gap is shared by every control-plane outbound client and tracked in #1949.

A channel secret is bound to the endpoint's origin. `update_channel` clears
the sealed columns when the new endpoint's scheme, host or port differs from
the stored one and the request carries no new `managed_secret`; otherwise
anyone with `alert_channel:update` could repoint a channel and receive its
bearer secret. The audit entry records `endpoint_changed`, `secret_replaced`
and `secret_cleared`, never the endpoint. A secret must also pass
`HeaderValue::from_str` as `Bearer <secret>` at save time, since a trailing
newline from a file would otherwise fail every delivery with no hint.

A failed evaluation writes `state = 'error'` and a `last_error` that is safe to
show: a transport error is reduced to its class (`reqwest`'s message carries
the URL, and `CLICKHOUSE_URL` can hold a password), while a ClickHouse error
response keeps its status and exception text, cut at 512 bytes. The raw error
goes to the log as `alert signal query failed`.

## Request & cost logs

- Every proxied request is logged to **ClickHouse** (`request_logs`): identifiers, model, provider/target, status, token counts, `cost_usd`, latency, TTFT, cache flag, error.
- **Sampling** (`logging_settings.sample_rate`, default `1`): `LogSink::enqueue` drops a row whose request-id hash bucket is at or above the rate (`should_sample_request` in `crates/rolter-gateway/src/logging.rs`) before it reaches the ClickHouse channel. `LogSink::observe`, the budget spend recorder and the rate-limit token recorder all run before that decision, so budgets, rate limits and `/metrics` count every request. `LogSink::enqueue` stamps each kept row with `sample_weight = 1 / sample_rate` (`clickhouse/014_sample_weight.sql`, default `1`), the rate in force when it was written. The analytics endpoints (`summary`, `timeseries`, `by-model`, `by-attribution` in `crates/rolter-control/src/analytics.rs`) scale counts and sums by it (`sum(sample_weight)`, `sum(cost_usd * sample_weight)`, `sumIf(sample_weight, …)`) and leave `avg` and `quantile` unscaled (#2239). `/api/v1/me/usage` and the `request_volume` / `spend_velocity` alert signals still return the sampled share (#2278). The **Logs Settings** screen warns with the resulting share whenever the form holds a rate below 100 % (#2088).
- **Writer timeouts** (#2373): the request-log, health-event and MCP tool-call writers share one HTTP client (`crates/rolter-gateway/src/clickhouse_client.rs`) with a 3 s connect timeout and a 10 s whole-request timeout. They are constants, not `[logging]` keys: the rows are best-effort telemetry and no deployment has a reason to wait longer. A ClickHouse that accepts the connection and never answers therefore fails the flush like any HTTP error: the batch is dropped, counted in `rolter_logs_dropped_total` / `rolter_health_events_dropped_total` / `rolter_mcp_events_dropped_total`, and logged at `warn` (`timed_out=true`) with the URL stripped, since `CLICKHOUSE_URL` can carry a password. Without the bound one stalled flush froze the writer until its queue filled. `drain_sinks` keeps its own 5 s grace on top, so shutdown is bounded even when a flush is still in flight.
- **`ts` is the instant the request began** — the same instant `latency_ms` is measured from, and the one the passive health event derived from that request carries. It is reconstructed from the request's monotonic start (`Instant`), so a clock step during a long request cannot reorder rows, and it is written by the gateway as an RFC 3339 literal at the column's millisecond precision (the insert asks ClickHouse for `date_time_input_format=best_effort` so that literal parses into `DateTime64(3)`). Add `latency_ms` to `ts` for the completion time. The same rule applies to the matching `request_payloads` row, which copies its request's `ts`, `org_id`, `project_id` and the gateway-minted `log_id` that joins the two (#1937).
- Gateways older than #1210 did **not** write `ts` at all and let the column's `default now64(3)` stamp it, which recorded the _batch flush_ time: every row in one flush shared a single millisecond, bursts collapsed onto one point, timeseries buckets were skewed by the flush interval, and keyset paging by `(ts, request_id)` had no order within a batch. The column default is kept as a fallback for those writers, so historical rows and any pre-#1210 gateway still land — but on such rows `ts` means flush time, not request time.
- **Retention** defaults to 90 days for metadata and seven days for captured payloads, set as the TTL in the ClickHouse schema. Both are admin-managed: `PUT /api/v1/logging-settings` accepts `retention_days` (1–3650) and `payload_retention_hours` (1–8760) and issues the matching `alter table … modify ttl` against ClickHouse, which then expires parts on its own schedule. Payload retention may not exceed metadata retention, so raw prompt bodies never outlive the row they belong to. A ClickHouse failure leaves the stored policy in place and is logged rather than failing the admin write — re-saving reapplies it.
- **Payload capture** is disabled by default. Set `[logging.payload_capture] enabled = true` to write redacted request and response payloads to the separate `request_payloads` table, which has a seven-day TTL (versus 90 days for request metadata). `max_bytes` bounds each body; `redact_fields` adds recursively redacted JSON keys before storage. Optional `models` and `virtual_key_ids` allow-lists make the deployment-level switch route- or key-specific.
- **Request id / trace continuation**: every request carries an `x-request-id` — the caller's when supplied, otherwise a generated UUID — which is echoed on the response and stored on the log row for end-to-end correlation. An inbound W3C `traceparent` or B3 (`b3` / `x-b3-traceid`) header is parsed and its trace id stored in `request_logs.trace_id`, so gateway logs join the caller's distributed trace instead of starting a disconnected one.
- **Outbound propagation**: when the caller sent trace context, it is forwarded verbatim to the chosen upstream (`traceparent`, `tracestate`, and the `b3` / `x-b3-*` family) so vLLM/SGLang/TGI continue the same trace. An untraced request adds nothing to the upstream wire — this is the caller's own context, not a rolter fingerprint, so it preserves wire transparency.
- Writes are **async and batched off the hot path** so logging never adds request latency.
- The dashboard queries ClickHouse for usage, spend, latency percentiles and error rates, sliced by org/team/project/key/model.
- **Who reads it** is decided per row: every analytics and health query binds the caller's tenancy, and captured bodies are masked below the `request_payload` floor (member, or viewer on a project that allows it). See [Who reads the request log](security.md#who-reads-the-request-log-1820).

### ClickHouse call timeouts (#1951)

Every ClickHouse call the control plane makes goes through one `reqwest` client built in `crates/rolter-control/src/analytics.rs`, with a 3s connect timeout and a 15s whole-request timeout. That covers the analytics, health, MCP-log and `/api/v1/me/usage` reads, the alert signal reads, and the MCP and UX-event ingest inserts. Without a bound, a ClickHouse that accepted the connection and stopped answering held the dashboard request (or the alert pass) open indefinitely. The gateway's client has the same connect bound (#2373).

- **Why 15s.** Above what an interactive read over the indexed window needs, short enough that the operator sees an error rather than a spinner, and under the 30s `QUERY_TIMEOUT` the alert evaluator keeps as an outer backstop. The log-retention `alter table` statements run under their own 60s bound, since an admin triggers them and they are not an interactive read.
- **How it surfaces.** A read answers `502` with the curated `analytics query failed` message and `analytics_query_failed` code, never driver text. An ingest insert goes through `ingest_failure::insert_failed`, the same path as any other insert failure. The alert evaluator records the rule as `error` with `last_error = "analytics query timed out"` and keeps the last value it read.
- **No credentials in logs.** Transport errors drop their URL (`reqwest::Error::without_url`) before they are logged or stored, because `CLICKHOUSE_URL` may carry userinfo.

### Time bounds on the read API

Every control-plane read over ClickHouse (the five `/api/v1/analytics/*` endpoints, the three `/api/v1/health/*` rollups, `/api/v1/mcp/logs` and its summary, and `/api/v1/me/usage`) takes a caller-supplied `since`/`until`, and the two keyset-paged lists also take a cursor whose first half is a timestamp. All of them are bound as ClickHouse parameters and parsed in SQL with `parseDateTime64BestEffortOrZero`. The `OrZero` variant is required: ClickHouse constant-folds both branches of the `if` that picks the default window, so a strict parse of an absent (empty) bound aborts the query (#1177). Its side effect is that anything the parser cannot read becomes `1970-01-01`, so before #1192 a typo in `since` silently scanned the whole table.

`crates/rolter-control/src/time_bounds.rs` closes that gap before any SQL is built:

- `is_time_bound` accepts one grammar: `YYYY-MM-DD`, optionally followed by `T` or a space and `hh:mm[:ss[.f{1,9}]]`, optionally followed by `Z` or `±hh:mm`. That is RFC 3339 as the dashboard writes it (`toISOString()`), the `YYYY-MM-DD hh:mm:ss.sss` form ClickHouse returns for a `DateTime64(3)` under the default `date_time_output_format=simple` (so every cursor the API hands out), and a bare date.
- It is narrower than RFC 3339 exactly where ClickHouse misreads it. Measured against ClickHouse 24.10, each of these parses to the epoch: a lowercase `t` or `z`, year `0000`, a day the calendar lacks (`2026-02-30`), and an offset whose `+` a client left unencoded, which the query string decodes to a space. Years outside `DateTime64`'s `1900`-`2299` range are accepted because ClickHouse clamps them to that range instead.
- The analytics, health, MCP-log and self-service modules import `time_bounds::Query` in place of axum's `Query`. It deserializes the same way, then checks `since` and `until` through the `TimeBounds` trait every windowed query type implements, so a new query type that never names its window does not compile. An empty bound still means "the default".
- `analytics::parse_keyset_cursor` applies the same grammar to the cursor's timestamp half, for both the invocation list and the MCP call log.
- A refusal is a `400` in the gateway's OpenAI-style envelope, `{"error": {"message", "type": "invalid_request_error", "param", "code"}}`, with `code` `invalid_time_bound`, `invalid_cursor`, or `invalid_query` for a query string that does not deserialize at all (axum's own rejection is plain text). When the value is a valid bound with its `+` turned into a space, the message says to send `%2B`.

A valid bound is forwarded byte for byte, so it means what it always meant. The router-level tests in `time_bounds.rs` run every windowed route against a stand-in ClickHouse to prove both halves: a malformed bound never reaches the database, and a valid one arrives unchanged. `every_documented_time_bound_is_on_a_checked_route` fails when the served OpenAPI document gains a route with a `since`, `until` or `cursor` that the table there does not list. `/api/v1/me/usage` authenticates a session first, so `self_service_key_lifecycle` in `tests/control_integration.rs` covers it.

## Provider health events

- Every health signal is written to **ClickHouse** (`provider_health_events`): `target_id`, `provider`, `org_id` (the provider's org, empty for a config-file provider), `source`, `outcome`, `status_code`, `latency_ms`, `error_kind`, and `ts`.
- **`ts` is the instant the observation was made**, stamped by the emitter, not by the batch writer: a `passive` event carries the `ts` of the request it was derived from, and a `probe` or `status_page` event carries the moment that poll completed. As with `request_logs`, gateways older than #1210 left this to `default now64(3)` and so recorded the flush time, which collapsed a whole sweep onto one millisecond and skewed the uptime/MTTR buckets below.
- `source` distinguishes where the observation came from: `passive` (real traffic completing through the request funnel), `probe` (active liveness sweeps), and the opt-in `llm_call` / `status_page` sources.
- `outcome` is `ok` / `error` / `timeout`; `error_kind` gives a coarse label (`rate_limited`, `upstream_error`, `connect_error`, `timeout`).
- Writes reuse the same **async, batched, off-hot-path** writer and ClickHouse endpoint as `request_logs`; when no `clickhouse_url` is configured the sink is a no-op.
- Counters `rolter_health_events_written_total` and `rolter_health_events_dropped_total` track the writer, mirroring the request-log counters.
- `rolter_mcp_events_written_total` and `rolter_mcp_events_dropped_total` do the same for the MCP tool-call writer (see [MCP servers, OAuth grants and sessions](mcp-oauth.md#the-tool-call-log)).
- This event stream feeds uptime %/MTTR rollups and the dashboard health panel.

#### Passive events are per _attempt_, not per request

A `passive` event describes one **upstream attempt**, not one client request. A
request that fails over makes several, and each one is an independent
observation of the target it went to.

This used to be per request (#1646). The whole passive funnel — the health
event, `rolter_target_requests_total{provider,target,outcome}` and
`rolter_upstream_errors_total` — was derived from the request log row, whose
`provider`/`target` are those of the attempt that finally _answered the
caller_. So when a target 503'd a quarter of its requests and the gateway
failed over, the record read like this:

| surface                                           | what it said              | what was true                             |
| ------------------------------------------------- | ------------------------- | ----------------------------------------- |
| `GET /api/v1/health/uptime` (target grain)        | `ok=23 errors=0 uptime=1` | ~25% of attempts to that target failed    |
| `rolter_target_requests_total{provider="sick",…}` | _no series at all_        | 4 failed attempts                         |
| `rolter_upstream_errors_total`                    | `0`                       | 4                                         |
| `GET /api/v1/analytics/summary`                   | `requests=374 errors=0`   | correct — the clients really did get 200s |

Failover doing its job is exactly when an operator most needs to see that a
target is sick, so a sick target was indistinguishable from a healthy one on
every Health screen and in every metric. Now each superseded attempt is
recorded against the target that produced it as it happens, and the same fleet
reads `ok=23 errors=4` on the target grain with a matching `error` series in
Prometheus — "served 100%, but 25% of attempts to this target failed".

The request-level row still describes the attempt that answered the caller, and
the two never double-count: an attempt the failover funnel already recorded is
logged with the request-level target attribution suppressed. `request_logs` and
`/api/v1/analytics/summary` are unchanged — they are per request by
definition, and the client really did get a 200.

### Stability rollup API

Read-only, window-bounded rollups over `provider_health_events`, served by the control plane when `--clickhouse-url` is set (otherwise `503`). All accept `since`/`until` (default last 7 days), checked as described in [time bounds on the read API](#time-bounds-on-the-read-api) and then passed as ClickHouse query parameters, never interpolated.

- `GET /api/v1/health/uptime` — per provider/target: event counts, `uptime`, `failure_rate`, `error_budget_burn` and `sla_breached` against an `sla` target (query param, fraction in `(0,1]`, default `0.99`), and `last_event`.
- `GET /api/v1/health/mttr` — per provider/target mean time to recovery (`mttr_seconds`) and incident count, computed from downtime episodes (a run of non-`ok` events bounded by `ok`).
- `GET /api/v1/health/timeline?bucket=hour|day|week|month` — bucketed ok/error/timeout counts per provider/target for the failure timeline (default bucket `hour`).

#### Two grains, and why they are never summed

The three rollups group by `(provider, target_id)`, and `target_id` means two
different things depending on the `source` that wrote the row. A `probe` or
`status_page` event watches the provider as a whole and carries the provider's
own name as its `target_id`; a `passive` event is derived from one completed
request and carries that request's real target. Both grains therefore land in
the same rollup, describing the same provider, with numbers that are not
comparable — a probe fires every `probe_interval_secs` regardless of traffic,
while a passive observation only exists because a request happened.

Every row consequently reports a **`grain`**: `provider` when `target_id` is
the provider itself, `target` when it is one route through it. `uptime` also
returns `sources`, the sorted distinct `source` values behind the row.

Reading the numbers:

- A `provider` row answers _"is this provider reachable at all, right now?"_.
  Its instant is the last probe or status-page poll, and its denominator is the
  number of polls in the window.
- A `target` row answers _"what did real traffic through this route see?"_.
  Its instant is the last request that used it, and its denominator is the
  number of requests in the window.

The two are never added together. The dashboard renders one card per provider,
headlined by the `provider` row when there is one, with the `target` rows
nested inside it; a provider with no probes configured has no `provider` row,
so the card sums its `target` rows — which is sound, because every request went
through exactly one of them — and labels the headline as a roll-up. Before
#1257 the screen laid every row out as a peer card, so a single dead provider
appeared several times over with contradictory failure counts.

#### SLA states on the dashboard

The dashboard screen is labelled **Provider Health**. Its nav key is still
`circuit-breaker`, so old links keep working, but it never reads the gateway's
circuit breakers: those live in gateway memory and leave it only as `/metrics`
counters. Until #2113 the screen called `sla_breached` _tripped_, which read as
live breaker state when it was a 7-day SLA verdict. Each card and target row
carries one of three SLA states instead:

- `breaching` — `sla_breached` from the uptime rollup: the window's failure rate
  is over `1 - sla`.
- `atRisk` — the window meets the SLA, but the two newest hourly buckets of the
  timeline fail faster than it allows, i.e. `error_budget_burn` over those
  buckets alone is above 1. Over the whole window a burn above 1 _is_ a breach,
  so a middle state has to come from a shorter span.
- `within` — neither.

"Newest" is measured from the latest bucket in the timeline response, not from
the browser clock: ClickHouse writes buckets as zone-less `2026-08-06 10:00:00`
strings in its own zone, which the browser cannot know. A probed provider's card
takes the state of its `provider` row and buckets; a rolled-up card takes the
worst state of its targets, the same rule its breach flag already followed.

## Health

- `GET /healthz` on both binaries for liveness probes.
- `GET /readyz` on the gateway for readiness. It returns `503 draining` once the control plane marks the node as draining (`PUT /api/v1/cluster/nodes/{id}/drain`), so a load balancer stops sending new traffic while in-flight requests finish; `/healthz` stays `200` because the process is healthy. The drain reaches the node on the snapshot poll it already makes, and the control plane refuses to drain the last live gateway.
