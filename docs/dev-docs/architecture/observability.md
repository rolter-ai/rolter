# Observability

## Metrics

- The gateway exposes Prometheus metrics at `GET /metrics`: counters (`rolter_requests_total`, `rolter_upstream_errors_total`, `rolter_auth_failures_total`, reload/log/budget/rate-limit/retry/cooldown/health/breaker/scrape counters), gauges (`rolter_config_version`, `rolter_breaker_entries`), and per-model **latency histograms** — `rolter_request_latency_ms` (total) and `rolter_request_ttft_ms` (time-to-first-token), each labelled `{model=...}` with the standard `_bucket`/`_sum`/`_count` series. Histograms are observed once per completed request from the log sink, off the response hot path.
- The exporter is hand-rolled (atomic counters + non-cumulative histogram buckets cumulated at render) rather than the `metrics` facade + global recorder, which does not fit the lock-free `arc-swap` design where an explicit `Arc<Metrics>` is threaded through the request path.
- Passive per-target SLA signal: `rolter_target_requests_total{provider,target,outcome}` (a counter, `outcome` = `ok` for 2xx else `error`) is tallied once per completed request from the log sink — free, derived from real traffic, no extra upstream calls. A per-target error rate / uptime is `sum(rate(rolter_target_requests_total{outcome="error"}[5m])) / sum(rate(rolter_target_requests_total[5m]))`. This is the first slice of provider stability tracking (ROL-123); the ClickHouse `provider_health_events` table and the dashboard land in later slices. The active prober is guarded: bounded probe concurrency with per-provider jitter, consecutive-failure/-recovery thresholds gating the unhealthy flip (no single-probe flapping), and exponential probe backoff when a probe itself gets a 429.
- Client disconnects (#1083): `rolter_client_disconnects_total` counts requests whose caller left before the response completed, and `rolter_inflight_requests` is the live in-flight gauge those requests must return to zero. Abandoned requests are logged with status `499` and are never retried — see [Client disconnects](client-disconnects.md).
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
  its *parent*. A B3-only caller is normalized into an equivalent `traceparent`
  first, so one W3C propagator serves both wire formats. Without this the
  gateway's spans were disconnected roots — the trace id reached the request log,
  but nothing joined the caller's trace.

  It has to be the span-maker rather than a middleware layered inside the
  `TraceLayer`: `DefaultMakeSpan` builds the request span at **DEBUG**, so under
  the default `RUST_LOG=info` it is disabled, and setting a parent on a disabled
  span silently does nothing. With no pipeline installed it falls back to that
  stock DEBUG span, so the untraced path costs what it always did.
- **Inject.** The context handed to the provider is injected from the *current*
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

| Span | Attributes |
|---|---|
| `auth` | — |
| `guardrails.pre` | `redacted`, `webhook` |
| `route.select` | `route`, `strategy`, `candidates` |
| `cache.lookup` | `hit`, `kind` (`exact` / `semantic`) |
| `queue.wait` | `provider` |
| `upstream.request` | `attempt`, `gen_ai.system`, `gen_ai.request.model`, `gen_ai.response.id`, `http.response.status_code` (embeddings also carry `gen_ai.request.encoding_formats` and `gen_ai.embeddings.dimension.count`) |
| `translate.request` | — |
| `guardrails.post` | — |

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

| Span | Attributes |
|---|---|
| `control.request` | `http.route`, `http.request.method`, `http.response.status_code` |
| `snapshot.build` | `config_version`, `payload_bytes`, `outcome` |
| `snapshot.sanitize` | — |
| `snapshot.encode` | — |

`http.route` is the *matched* template (`/api/v1/providers/{id}`), never the
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
that rolter's job in that design is to *stamp the attribute the collector routes
on*. Until this landed there was nothing to stamp: the request logs in
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

| | `docker-compose.observability.yml` (default) | `docker-compose.signoz.yml` |
|---|---|---|
| backend | Jaeger v2 | SigNoz |
| signals | traces only | traces, metrics, logs |
| containers | 2 | 5, incl. its own ClickHouse + Zookeeper |
| storage | in memory, lost on restart | persistent |
| use it for | reading a waterfall, fast iteration | aggregate views, dashboards, dogfooding over time |

```
docker compose -f docker/docker-compose.yml \
               -f docker/docker-compose.signoz.yml up      # SigNoz on :8080
```

Jaeger is the default because it is two containers and starts in seconds, and
because reading a correctly-parented waterfall is what the tracing work needed.
Reach for SigNoz when "which stage is slow *across all requests*" matters, which
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
admin only) and supplied to the *server*, not the client:

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

| Metric | Unit | Attributes |
|---|---|---|
| `rolter_snapshot_build_ms` | ms | `outcome` (`ok` / `not_modified` / `error`) |
| `rolter_snapshot_payload_bytes` | By | `outcome` |
| `rolter_control_request_ms` | ms | `http.route`, `http.request.method`, `http.response.status_class` |
| `rolter_db_pool_acquire_ms` | ms | `outcome` (`ok` / `timeout`) |

And one counter, for the endpoint an unauthenticated attacker can reach (#1079):

| Metric | Meaning | Attributes |
|---|---|---|
| `rolter_control_login_attempts` | resolved control-plane sign-in attempts | `outcome` (`success` / `invalid` / `throttled` / `locked` / `error`) |

A counter rather than a histogram: the question it answers — "is somebody
running a credential-stuffing run against this deployment" — is a rate, not a
distribution. It carries no account or address label; either would be unbounded
cardinality *and* would put the identity an attacker is guessing into the
metrics pipeline. A rising `invalid` with a rising `locked` behind it is the
throttle working; a rising `invalid` with no `locked` means the run is spread
thin enough to stay inside the per-account budget, and the per-address budget is
the one to tighten.

And the connection pool, as observable gauges (#1052):

| Metric | Meaning |
|---|---|
| `rolter_db_pool_connections` | connections the pool holds open |
| `rolter_db_pool_idle` | how many of those are free right now |
| `rolter_db_pool_max` | the configured ceiling |

One pool serves `/internal/snapshot`, the whole CRUD surface and every RBAC
membership lookup, so a ceiling that is too low presents as "the control plane
got slow" with nothing to attribute it to. The three gauges together are what
separate the two cases: **pool-bound** is `connections == max` while `idle` is
zero *and* `rolter_db_pool_acquire_ms` shows waits; acquire waits without a
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
*class*, not a code: twelve statuses across ninety routes would be over a
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

Busy *time* is exported, not a busy ratio: a ratio computed in-process would
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

`service.instance.id` deliberately uses the same precedence as the cluster
watcher — `ROLTER_NODE_ID`, then `HOSTNAME`, then nothing — so a node in
`cluster_nodes` and a node in the trace backend are the same node by
construction. When neither is set it is omitted rather than invented per restart,
which would churn the identity on every deploy.

### Wrapping audit (#815)

OpenTelemetry's [*Don't wrap OpenTelemetry*](https://opentelemetry.io/blog/2026/dont-wrap-opentelemetry/)
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

| Item | Verdict | Why |
|---|---|---|
| `stage_span!` (`rolter-core/src/telemetry.rs`) | keep | code generation, not a runtime wrapper |
| The scalar-metrics list (`Metrics::scalars()`) | keep | the by-name lookup is on the export path, not the request path |
| `RequestHistograms::record` | keep | one unavoidable allocation; the alternative is the anti-pattern |
| `GatewayMakeSpan` (`rolter-gateway/src/trace.rs`) | keep | SDK/layer configuration, explicitly out of scope |

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
it is not on a hot path: the instruments are *observable*, so the SDK invokes
those callbacks on its own export interval (`OTEL_METRIC_EXPORT_INTERVAL`,
default 60s). The request path only ever does `fetch_add` on a named `AtomicU64`
field — there is no map, no lookup and no lock between a request and its counter.
The post's performance argument therefore does not bite here even though the
gateway hot path is where it would bite hardest.

What the export path does cost is one `Vec<ScalarMetric>` allocation per
instrument per cycle and a linear scan of it, so the work is quadratic in the
number of scalars. At the current eight scalars, once a minute, that is
immaterial. It is left as-is deliberately: the OTel Rust 0.32 API offers only
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
*not* wrapping. Recorded here only so the audit is complete.

### Turning telemetry off explicitly

`ROLTER_TELEMETRY_ENABLED=false` hard-disables every export — traces, metrics and
the dashboard's browser tracing — regardless of which `OTEL_*` endpoints are set
(#812). Unset means enabled, which changes nothing for an existing deployment:
with no endpoint configured nothing is exported anyway.

The switch can only *subtract*. It never turns export on by itself, and an
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

[`infra/otel/collector.yaml`](../../infra/otel/collector.yaml) is a ready-to-run
local collector example: it accepts OTLP/gRPC and OTLP/HTTP, scrapes the
compose gateway's Prometheus endpoint, exposes collected metrics on `:8889`,
and prints telemetry via the `debug` exporter. It intentionally has no external
backend configured, keeping the default example safe for air-gapped use. Replace
the `debug` exporter with an internal OTLP-compatible destination for production.

### Connector delivery: rendered collector config (#836)

The `observability_connectors` table (#511, `docs/architecture/data-model.md`)
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

Point a collector at it with the confmap HTTP provider
(`otelcol --config=http://control:4001/api/v1/connectors/collector-config`,
bearer-authenticated the same as any other control-plane endpoint), or fetch it
on a schedule and reload. `sampling_rate` becomes a `probabilistic_sampler`
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

## Request & cost logs

- Every proxied request is logged to **ClickHouse** (`request_logs`): identifiers, model, provider/target, status, token counts, `cost_usd`, latency, TTFT, cache flag, error.
- **`ts` is the instant the request began** — the same instant `latency_ms` is measured from, and the one the passive health event derived from that request carries. It is reconstructed from the request's monotonic start (`Instant`), so a clock step during a long request cannot reorder rows, and it is written by the gateway as an RFC 3339 literal at the column's millisecond precision (the insert asks ClickHouse for `date_time_input_format=best_effort` so that literal parses into `DateTime64(3)`). Add `latency_ms` to `ts` for the completion time. The same rule applies to the matching `request_payloads` row, which copies its request's `ts`.
- Gateways older than #1210 did **not** write `ts` at all and let the column's `default now64(3)` stamp it, which recorded the *batch flush* time: every row in one flush shared a single millisecond, bursts collapsed onto one point, timeseries buckets were skewed by the flush interval, and keyset paging by `(ts, request_id)` had no order within a batch. The column default is kept as a fallback for those writers, so historical rows and any pre-#1210 gateway still land — but on such rows `ts` means flush time, not request time.
- **Retention** defaults to 90 days for metadata and seven days for captured payloads, set as the TTL in the ClickHouse schema. Both are admin-managed: `PUT /api/v1/logging-settings` accepts `retention_days` (1–3650) and `payload_retention_hours` (1–8760) and issues the matching `alter table … modify ttl` against ClickHouse, which then expires parts on its own schedule. Payload retention may not exceed metadata retention, so raw prompt bodies never outlive the row they belong to. A ClickHouse failure leaves the stored policy in place and is logged rather than failing the admin write — re-saving reapplies it.
- **Payload capture** is disabled by default. Set `[logging.payload_capture] enabled = true` to write redacted request and response payloads to the separate `request_payloads` table, which has a seven-day TTL (versus 90 days for request metadata). `max_bytes` bounds each body; `redact_fields` adds recursively redacted JSON keys before storage. Optional `models` and `virtual_key_ids` allow-lists make the deployment-level switch route- or key-specific.
- **Request id / trace continuation**: every request carries an `x-request-id` — the caller's when supplied, otherwise a generated UUID — which is echoed on the response and stored on the log row for end-to-end correlation. An inbound W3C `traceparent` or B3 (`b3` / `x-b3-traceid`) header is parsed and its trace id stored in `request_logs.trace_id`, so gateway logs join the caller's distributed trace instead of starting a disconnected one.
- **Outbound propagation**: when the caller sent trace context, it is forwarded verbatim to the chosen upstream (`traceparent`, `tracestate`, and the `b3` / `x-b3-*` family) so vLLM/SGLang/TGI continue the same trace. An untraced request adds nothing to the upstream wire — this is the caller's own context, not a rolter fingerprint, so it preserves wire transparency.
- Writes are **async and batched off the hot path** so logging never adds request latency.
- The dashboard queries ClickHouse for usage, spend, latency percentiles and error rates, sliced by org/team/project/key/model.

## Provider health events

- Every health signal is written to **ClickHouse** (`provider_health_events`): `target_id`, `provider`, `source`, `outcome`, `status_code`, `latency_ms`, `error_kind`, and `ts`.
- **`ts` is the instant the observation was made**, stamped by the emitter, not by the batch writer: a `passive` event carries the `ts` of the request it was derived from, and a `probe` or `status_page` event carries the moment that poll completed. As with `request_logs`, gateways older than #1210 left this to `default now64(3)` and so recorded the flush time, which collapsed a whole sweep onto one millisecond and skewed the uptime/MTTR buckets below.
- `source` distinguishes where the observation came from: `passive` (real traffic completing through the request funnel), `probe` (active liveness sweeps), and the opt-in `llm_call` / `status_page` sources.
- `outcome` is `ok` / `error` / `timeout`; `error_kind` gives a coarse label (`rate_limited`, `upstream_error`, `connect_error`, `timeout`).
- Writes reuse the same **async, batched, off-hot-path** writer and ClickHouse endpoint as `request_logs`; when no `clickhouse_url` is configured the sink is a no-op.
- Counters `rolter_health_events_written_total` and `rolter_health_events_dropped_total` track the writer, mirroring the request-log counters.
- This event stream feeds uptime %/MTTR rollups and the dashboard health panel.

### Stability rollup API

Read-only, window-bounded rollups over `provider_health_events`, served by the control plane when `--clickhouse-url` is set (otherwise `503`). All accept `since`/`until` (RFC3339, default last 7 days); time bounds are passed as ClickHouse query parameters, never interpolated.

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

- A `provider` row answers *"is this provider reachable at all, right now?"*.
  Its instant is the last probe or status-page poll, and its denominator is the
  number of polls in the window.
- A `target` row answers *"what did real traffic through this route see?"*.
  Its instant is the last request that used it, and its denominator is the
  number of requests in the window.

The two are never added together. The dashboard renders one card per provider,
headlined by the `provider` row when there is one, with the `target` rows
nested inside it; a provider with no probes configured has no `provider` row,
so the card sums its `target` rows — which is sound, because every request went
through exactly one of them — and labels the headline as a roll-up. Before
#1257 the screen laid every row out as a peer card, so a single dead provider
appeared several times over with contradictory failure counts.

## Health

- `GET /healthz` on both binaries for liveness probes.
- `GET /readyz` on the gateway for readiness. It returns `503 draining` once the control plane marks the node as draining (`PUT /api/v1/cluster/nodes/{id}/drain`), so a load balancer stops sending new traffic while in-flight requests finish; `/healthz` stays `200` because the process is healthy. The drain reaches the node on the snapshot poll it already makes, and the control plane refuses to drain the last live gateway.
