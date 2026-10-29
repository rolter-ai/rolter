# Observability

Status-page polling redacts URL credentials from both request and body-read error logs. A transport failure still skips that poll without changing provider health.

The [environment variable reference](../../user-docs/configuration/environment-variables.mdx#shared-variables) lists the general OTLP endpoint and each signal override. Metrics export independently; the log bridge is installed alongside an active trace exporter, so a logs-only endpoint does not activate it. Stdout logs and Prometheus scraping remain available.

## Metrics

- The gateway exposes Prometheus metrics at `GET /metrics`: counters (`rolter_requests_total`, `rolter_upstream_errors_total`, `rolter_auth_failures_total`, reload/log/budget/rate-limit/retry/cooldown/health/breaker/scrape counters), gauges (`rolter_config_version`, `rolter_breaker_entries`), and per-model **latency histograms** — `rolter_request_latency_ms` (total) and `rolter_request_ttft_ms` (time-to-first-token), each labelled `{model=...}` with the standard `_bucket`/`_sum`/`_count` series. Histograms are observed once per completed request from the log sink, off the response hot path.
- The exporter is hand-rolled (atomic counters + non-cumulative histogram buckets cumulated at render) rather than the `metrics` facade + global recorder, which does not fit the lock-free `arc-swap` design where an explicit `Arc<Metrics>` is threaded through the request path.
- Passive per-target SLA signal: `rolter_target_requests_total{provider,target,outcome}` (a counter, `outcome` = `ok` for 2xx else `error`) is tallied once per completed request from the log sink — free, derived from real traffic, no extra upstream calls. A per-target error rate / uptime is `sum(rate(rolter_target_requests_total{outcome="error"}[5m])) / sum(rate(rolter_target_requests_total[5m]))`. This is the first slice of provider stability tracking (ROL-123); the ClickHouse `provider_health_events` table and the dashboard land in later slices. The active prober is guarded: bounded probe concurrency with per-provider jitter, consecutive-failure/-recovery thresholds gating the unhealthy flip (no single-probe flapping), and exponential probe backoff when a probe itself gets a 429.
- Client disconnects (#1083): `rolter_client_disconnects_total` counts requests whose caller left before the response completed, and `rolter_inflight_requests` is the live in-flight gauge those requests must return to zero. Abandoned requests are logged with status `499` and are never retried — see [Client disconnects](client-disconnects.md).
- Provider queues (#1855): `rolter_provider_queue_depth{provider}` and `rolter_provider_inflight{provider}` gauges and the `rolter_provider_queue_wait_ms{provider}` histogram, kept by RAII guards in `metrics.rs`. A job carries a `QueuedGuard` from the moment it is built until a worker takes it (`picked()` observes the wait and hands over an `InflightGuard`) or it is dropped unrun, so a shed or stranded job can never leave the depth raised; the in-flight guard lives until the upstream's response headers arrive, which is also when a worker is released, and covers the direct path when queueing is off. The steady-state lookup borrows the provider name, so the path allocates nothing. Over OTLP (#1862) the two gauges are a _labelled gauge family_ (`LABELLED_GAUGE_FAMILIES`, registered as observable gauges beside the labelled counter families by `register_labelled` in `rolter_core::telemetry`), and each wait is also recorded into an OTLP `rolter_provider_queue_wait_ms` histogram on the Prometheus boundaries, through a per-provider `QueueWaitRecorder` whose `provider` attribute is built once when the provider's counters are. The Redis consumers' connection state (#1772) rides the same two families; see [Redis connections](redis-connections.md). The `rolter · gateway capacity` board in `integration/dogfood/signoz/dashboards/` charts all of it.
- Guardrail outcomes (#2158): `RequestLog::guardrail_events` (`clickhouse/018_guardrail_events.sql`, `Array(String)`, serialized only when non-empty) names what each guardrail did to the request as `<action>:<rule>` with the action `blocked`, `redacted` or `annotated` (`GuardrailReport::events` in `rolter-core`; names only, never matched text, and once per rule per request however often it matched), plus `provider_blocked:<name>` / `provider_transformed:<name>` for the guardrail webhook, named after `GuardrailWebhookConfig::name` (the registry row's name, set by `load_guardrail_webhook`; empty for a config-file webhook that sets none, which records the bare action as every row did before #3038). The rollup splits at the first `:`, so the provider's name lands in `rule`; the Providers card reads its own name and states the bare rows apart. A pre-call block records `report.events()` of the `InputBlocked` that `apply_input` returns, so redactions applied before the blocking rule are kept. The pre-call stage in `handlers.rs` collects them and every row the request can leave starts from them: the refusal row (`Refusals::log_with_events`, so a pre-call block names its rule beside the existing `error`), the cancel guard's template, the cache-hit rows through `RequestLogCtx`, and the success and failure rows; an output rule that withholds a response adds `blocked:<rule>` in `UsageLoggingStream::withhold`, keyed off `GUARDRAIL_WITHHELD_PREFIX`. Since #459 the provider events come from `crate::semantic_guards` for every provider a policy runs, plus `provider_error:<name>` for a call with no verdict and `policy_blocked:<policy>` / `policy_flagged:<policy>` for a threshold violation; a `post_call` refusal is recorded through the withheld reason as `blocked:<provider or policy>`. The response stage records the same way since #3330: `DeliveryPolicy::apply` collects its outcomes into a `PolicyEvents`, which keeps them apart by column — `OutputGuard::apply`'s `redacted:<rule>` / `annotated:<rule>` and every response-stage provider event from `SemanticLeg::apply` (`provider_transformed:`, `provider_error:`, `policy_flagged:`, and on a refusal `provider_blocked:` / `policy_blocked:` beside the withheld reason's `blocked:`) in `guardrail_events`, the `post_response` plugins' entries in `plugin_events` — and each caller (the live and buffered-stream body in `stream_response`, the cacheable miss, and `cached_response` for a hit) appends both to the row it logs with `PolicyEvents::record`. A cache hit the policy withholds adds `blocked:<rule>` itself, as `withhold` does for a live one. An output rule that blocks keeps the earlier output rules' entries too (#3403): `apply_output` returns an `OutputBlocked` carrying the blocking rule and the report gathered up to it, as `apply_input` does, and `OutputGuard::apply` appends that report's `redacted:` / `annotated:` entries before the withheld reason adds `blocked:<rule>`. The control plane reads it with `GET /api/v1/analytics/guardrails` (an `array join` over the events, one row per action and rule, weighted by `sample_weight`, behind `ROW_VISIBLE`; `recorded: false` while the table lacks the column, see `request_log_columns.rs`) and filters the invocation list with `guardrail=`. The dashboard's Guardrail Rules and Providers cards count the last seven days and link to LLM Logs with `guardrail=`. Tests: `crates/rolter-gateway/tests/guardrail_outcomes.rs`, the rollup tests in `analytics.rs`.
- Plugin outcomes (#2243): `RequestLog::plugin_events` (`clickhouse/019_plugin_events.sql`, `Array(String)`, serialized only when non-empty) holds one `<outcome>:<slug>` entry per plugin call, the outcome being `allowed`, `blocked`, `transformed` or `annotated`, or `error:<slug>:<kind>` when `call_once` returned a `CallFailure` (`refused`, `timeout`, `connect`, `status_<code>`, `body`; a fixed vocabulary, never the endpoint's text). `plugin_dispatch::dispatch` and `apply_post_response` append them to a caller-owned `events` vector, so a chain that stops at a block still reports the plugins that ran. An `error` entry stands in for the `blocked`/`allowed` the failure policy then produced, because the plugin gave no verdict; `plugin_blocks_total` still counts the fail-closed refusal. The pre-route and pre-upstream stages note their events on `Refusals` (a `parking_lot::Mutex`, as the handler future must stay `Sync`), so a request a later check refuses still shows them, and `plugin_events()` hands them to every row the request can leave (`RequestLogCtx`, the cache-hit rows, the success and failure rows). The post-response chain runs inside `DeliveryPolicy::apply`, whose caller merges the entries into the row it is about to log, including the withheld row of a block. A realtime session is not one row, so `ContentPolicy::client_event` hands each dispatch's events to a `PluginCalls` tally shared with the session's meter, which writes them onto its rows (#3071, see [Realtime metering](realtime-metering.md#plugin-calls-on-a-session)). Counters per plugin were considered and rejected: the control plane would need a store of its own for them, they would not survive a gateway restart or sum across replicas, and they could not be scoped to the caller's visible rows or sampled the way every other count here is. The control plane reads the column with `GET /api/v1/analytics/plugins` (the same `array join` rollup as the guardrails one, split with `splitByChar`, grouped by `org_id` and slug, `last_error_*` from `argMaxIf`; `recorded: false` while the table lacks the column), and the Plugins cards count the last seven days. Tests: `crates/rolter-gateway/tests/plugin_outcomes.rs`, the `plugin_dispatch` unit tests, the rollup tests in `analytics.rs`.
- Request outcome (#3307): `RequestLog::outcome` (`clickhouse/030_request_outcome.sql`, `LowCardinality(String) default 'unknown'`, always serialized) names what happened to the request with a published `rolter_core::request_outcome::RequestOutcome` name: `served`, `upstream_error`, `withheld`, `cancelled`, and the post-auth refusals `refused_budget`, `refused_rate_limit`, `refused_guardrail`, `refused_plugin`, `refused_access`, `refused_invalid` and `refused`. This is the maintainer's decision on #1395: a refusal after the key resolves gets a distinct outcome rather than being recognised by inference (no provider or target, `attempts = 0`, a 4xx). A refusal path names its reason: `Refusals::log_with_events` and `LifecycleLog`'s refused arm call `refusal_outcome`, which reads the `ErrorCode` response extension `ApiError` stamps beside `ErrorMessage` (`RequestOutcome::for_refusal`: the code first, the status where there is none), and `log_builtin` does the same for a `fake-llm` error. Every other row is named in `LogSink::enqueue`, the one point every row passes, by `logging::outcome_of` from what the row records (`withheld`, a client disconnect, `status < 400`, an attempt or target, else a refusal by status), so no row reaches ClickHouse without one. A gateway against an un-migrated ClickHouse keeps writing (`input_format_skip_unknown_fields=1` drops the field), and every row older than the column reads `unknown`, the column default. The control plane reads it as an `OPTIONAL_COLUMNS` entry (default `'unknown'`), returns it on `GET /api/v1/analytics/invocations` and filters on `outcome=` (one of `outcome_filter_values()`, else a 400); LLM Logs has an **Outcome** filter and shows it in the drawer.
- Parameter adjustments (#1586): `RequestLog::param_adjustments` (`clickhouse/022_param_adjustments.sql`, `Array(String)`, serialized only when non-empty) holds one `<disposition>:<param>` entry per request parameter dialect translation dropped, clamped or translated, the same record the `x-rolter-param-adjustments` response header is rendered from. The translators produce it (`rolter_proxy::ParamAdjustments`), the forwarder hands it over in the upstream response's extensions, and only the success arm of `proxy` sets it, since a request that never reached an upstream was never translated. Not yet read by the control plane (#3323). See [Parameters translation did not forward as sent](../api/openai-and-anthropic.md#parameters-translation-did-not-forward-as-sent). Read back by the control plane since #3323: `param_adjustments` is an `OPTIONAL_COLUMNS` entry (default `emptyArrayString()`) returned on every invocation row, and `GET /api/v1/analytics/param-adjustments` rolls it up per `(route, provider_org_id, provider)` in one scan (the org since #3441, read as an optional column defaulting to `''`, so two orgs' same-named providers are two rows) (`param_adjustments_sql` in `analytics.rs`: each group sums its `sample_weight` and, through `sumMap` over the entries with the row weight as value, the weight of each `<disposition>:<param>`, which an outer `array join` unfolds into one row per entry with `requests`, `route_requests` and `share`), behind `ROW_VISIBLE` and answering `recorded: false` without the column, like the guardrail rollup. The dashboard reads it on the Compatibility screen (`ParamAdjustmentsPanel`) and lists a row's own entries in the LLM Logs drawer.
- Billed but withheld (#1478): a response an output guardrail or post-response plugin refuses to deliver is still billed. Its row carries the caller's `403`, `withheld = 1`, the policy in `error`, and the provider's tokens and cost, and `rolter_withheld_responses_total` counts it. `usage_unknown = 1` marks any successful row whose upstream reported no usage. See [Billed but withheld](billed-but-withheld.md).
- Multi-key providers: `rolter_key_cooldowns_tripped_total` counts api keys parked after a key-level failure (429/401 on a provider with several keys); the request retries in-flight on a sibling key.
- A/B attribution: `rolter_variant_requests_total{model,variant}` (a counter) tallies requests per chosen variant, so traffic splits are visible in Prometheus/Grafana without querying ClickHouse. Classic single-pool routes (no variant) emit nothing. Observed from the same log-sink funnel (ROL-195, part of ROL-188). Its `model` label, the per-model latency histograms' and `rolter_complexity_route_requests_total`'s `model` and `route` are the route's runtime namespace (`RouteEntry::namespace`), not the requested name, so two orgs' same-named routes are two series (#3462, ADR-0040): the proxy carries the requested route's namespace on `RequestLog::route_namespace` (`#[serde(skip)]`, never written to ClickHouse; an `Arc<str>` shared with `RouteEntry::namespace`, so a row takes a reference rather than a copy) on every row a resolved route leaves: an upstream answer, a cache hit (through `RequestLogCtx`), a client disconnect (the `CancelGuard` row) and a realtime turn (`SessionMeter`). `LogSink::observe` labels by it, falling back to `model` where no route was resolved. The OTLP histograms keep the requested model.
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
- **Inbound**: accept W3C `traceparent`/`tracestate` (and `b3`) from clients and continue the trace; honor the caller's request id in `x-request-id`, or in the header `[client] request_id_header` names (`x-correlation-id`, say).
- **Outbound to engines**: inject the active trace context into upstream requests so vLLM/SGLang/TGI spans join the **same** distributed trace. vLLM and SGLang support OpenTelemetry tracing (e.g. vLLM `--otlp-traces-endpoint`); point them at the same OTLP collector so engine prefill/decode spans line up with rolter's request span.
- A per-request `request_id` is echoed in a response header and stamped on the request log row and on the `gateway.request` span as `rolter.request.id` for correlation (no metric exemplars yet).
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

  Every dialled attempt opens that span (`genai_attempt_span` in
  `crates/rolter-gateway/src/handlers.rs`) and injects from inside it
  (`attempt_trace_context`): an attempt on the route's classic pool and one on
  its weighted-variant chain, for a JSON body and for an audio upload. The span
  names the target actually dialled (`gen_ai.request.model`, and the variant's
  target on a variant route) and the operation of the endpoint, so a
  transcription or translation is `generate_content` with
  `gen_ai.output.type=text`. The answering attempt's span is handed on as
  `ForwardOutcome::genai_span`, which is where `UsageLoggingStream` records the
  usage and the response attributes. A superseded attempt keeps its own span,
  which records how it ended: `http.response.status_code` and `error.type` (the
  status) for an answer, `error.type` alone (`timeout` or `transport`,
  `genai::error_type_for_transport`) for a call that got no response. With no
  pipeline installed the span is disabled, returned before it is named, and not
  handed on, so `UsageLoggingStream` parses nothing for it. Until #3535 only a
  JSON attempt on the
  classic pool did this: a route with variants and every upload exported no
  `upstream.request` span and forwarded the caller's trace headers as they came.
  Tests: `crates/rolter-gateway/tests/attempt_spans.rs` (a failover case
  included), against an in-memory
  pipeline (`rolter_core::telemetry::test_pipeline`, behind the dev-only
  `test-pipeline` feature).

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
| `guardrails.local`  | `provider`, `truncated` (one per [local classifier](semantic-guards.md#local-providers-adr-0039) call, at either stage; `truncated` when the text was longer than `max_scored_bytes`, #3513)            |

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

The MCP dashboard tools need SigNoz v0.135.0 or newer, which the pinned `signoz`
image is.

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
| `rolter_snapshot_build_ms`      | ms   | `outcome` (`ok` / `not_modified` / `error` / `refused`)           |
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

And the config problems, as an observable gauge (#3542):

| Metric                   | Meaning                                                              | Attributes                                                                    |
| ------------------------ | -------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `rolter_config_problems` | what `GET /api/v1/config/problems` lists for the deployment, by kind | `kind` (`omitted` / `sanitized` / `invalid` / `misread` / `bootstrap_shadow`) |

`build_snapshot` samples it through `sample_config_problems`, once per
`config_version`: the first build of a version claims it
(`ConfigProblemGauge::claim`), so a fleet of gateways polling one version costs
one sample, and a `304` costs none. The sample runs beside the snapshot, never
in it, since it reads the store's records a second time. Each kind is counted
from where its lines come from, never from their wording: `omitted` is the
sanitizer's `OmittedEntry` records, `sanitized` its other lines, `invalid` the
lines of `validate_snapshot`, all for the config as the store loaded it before
any format's down-step; `misread` and `bootstrap_shadow` are the store's
`ConfigProblem` records by their `kind`, `bootstrap_shadow` being the overlap
`MergedConfigStore` reports (#3516). A store that refuses to load counts its
records alone, as the endpoint lists them alone. A sample that cannot read the
records gives its claim back (`release`), and a slower sample of an older version
never overwrites a newer one. Nothing is reported before the first sample, so
"not yet sampled" never reads as "no problems". The only label is the kind: a
row's name or org would make a series per row and put tenant names into the
metrics pipeline.

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
row. The update handler keeps a stored `managed_auth_secret` when the body omits it,
removes it on an explicit `null` or a blank string (#2402, the same tri-state as a provider's
`api_key_env`; `crud::explicit_null` tells `null` from absent), and refuses a blank one on create. It also, as `update_channel` does for an alert channel, clears
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

**Rules and channels are owned (#1829, migration `0122`).** `alert_channels.org_id` and `alert_rules.org_id` name the owning org, `NULL` for a deployment row. A rule also carries `scope_type`/`scope_id` (the budget vocabulary: org, team, project, virtual key, business unit, customer), fixed at creation, and `org_id` is derived from the scope's chain then, so a rule is checked against its channel's org without walking the tree. The capability rows `alert_rule` and `alert_history` are `project`-scoped `MULTI_SCOPE` rows (a rule is guarded at its own scope's chain) and `alert_channel` is `org`-scoped, all `ADMIN_OR_OPERATOR` (`Authority::RoleOrDeployment`, #3362): an admin at the row's scope, or a deployment operator at any, which `authorize`, `ScopeFilter` and `rbac/effective` all honour, so the handlers carry no operator special case. A row with no `org_id` is refused by `deployment_only` to anyone but a superadmin or an operator, because no membership could satisfy it. Lists for a scoped caller pre-select in SQL (`visible_rules`, #3361): the rules of the orgs, teams and projects the caller holds the role at, reading a project or a key's project through its row, so the cost follows their own rules and not every org's; `ScopeFilter::allows` then decides each remaining rule's chain, falling back to the org when the scope row is gone so the org's admins can still remove it. A deployment operator lists every rule. A rule may notify a channel of its own org or a deployment one (`channel_reaches`): checked at create and update, and again in `evaluate_rule`, which treats a mismatched channel as none. The evaluation narrows the ClickHouse query with `and <column> = {scope_id:String}` and binds the id as `param_scope_id`; `scope_column` is the one list of columns, and `provider_health_flaps` can only be org-scoped because `provider_health_events` carries just the provider's org. A deleted team, project or key leaves the rule behind (the polymorphic id has no foreign key, like budgets), and the evaluation records `scope_gone` instead of reading an empty window.

**One evaluation is a short transaction around the rule's row lock, and the
send happens after it commits (#1967).** The ClickHouse read runs first,
unlocked and bounded to 30 seconds, because it is the slow part. Then the
evaluation takes `select … for key share` on the rule's channel, `select … for
update` on the rule, decides whether the reading is a transition, writes the
rule and a history row, and commits. For a channel that can be sent to the row
is written `pending` and the send is an outbox entry (`Outbox`) the caller
settles once the transaction is gone: `settle()` POSTs and then updates the row
to `delivered` or `failed`. A rule with no channel, or a disabled one, gets its
final `skipped` row in the same transaction. The scheduled pass takes the rule
lock with `skip locked` and `and enabled`, so a rule another replica is
evaluating is left to that replica and a rule disabled mid-pass is skipped;
**Evaluate now** waits for the lock instead. That is what keeps
`control.replicaCount > 1` from reporting each transition once per replica. An
edit that lands between the read and the lock and changes the signal, window or
channel discards the reading, since it measures a query the rule no longer asks
for or would go to a channel that was not locked.

Sending inside the transaction had two costs. Deliveries ran one after another
while the pass held each rule's lock, so thirty rules firing at a receiver that
black-holes connections spent up to 300 seconds of 10-second timeouts before the
pass reached the rest. And a Postgres session whose
`idle_in_transaction_session_timeout` was shorter than a slow `200` was killed
mid-delivery: the commit failed, no history row was written and the next pass
sent the same alert again. Now the scheduled pass hands each `Outbox` to a
background task and goes on to the next rule. A process-wide semaphore
(`MAX_DELIVERIES`, 16) bounds the deliveries in flight, and the slot is awaited
inside the task so a burst never holds up the pass. **Evaluate now** awaits its
own `settle()`, since the operator is waiting for the outcome. The transaction
now holds a connection for a few queries and no network call.

`pending` is also the cross-replica guard. The row is committed before the send,
so a replica that evaluates the rule next finds a newest row that is `pending`
and `transition()` returns nothing for it until the row is older than
`PENDING_GRACE_SECS` (5 minutes, aged by the database clock so a skewed replica
cannot misjudge it). Past that the process that owed the send is presumed gone
(it died between the commit and the POST, or the write that settles the row
failed), the row counts as an unreported attempt and the state is sent again, so
a crash costs a late alert, not a lost one. A recovery is a new transition and is
never held back by a pending `firing`.

The channel is locked before the rule because deleting a channel takes the
locks in that order: the `delete` locks the channel row, then its
`on delete set null` locks every rule naming it. With the evaluator taking the
rule first and meeting the channel only at the history insert's foreign-key
check, a delete issued during an evaluation could deadlock with it (`40P01`).
In the same order, the delete simply waits for the evaluation's transaction,
which is now milliseconds. A delete that lands while a delivery is in flight no
longer waits for it: the row's `channel_id` is cleared by the cascade and the
delivery settles the same row by `id`. `key share` rather than `share` keeps the
channel editable meanwhile, and `update_channel` locks with `for no key update`
so an edit does not queue behind an evaluation either.

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
enabled channel. Each attempt is a new history row with a new `id`. A `pending` row is not yet an attempt that failed: it counts toward neither the delay nor the retries until it is old enough to be presumed lost. The
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
response becomes `analytics query failed` with code `clickhouse_error`. SQL and
exception details stay in the server log as `alert signal query failed`, with
URL credentials redacted, rather than reaching the stored rule or API response.

## Request & cost logs

- Every proxied request is logged to **ClickHouse** (`request_logs`): identifiers, model, provider/target, status, token counts, `cost_usd`, latency, TTFT, cache flag, error.
- **Where a row's tokens are read from** (#2819). `logging::parse_usage` walks the buffered upstream body (every `data:` object for SSE) and `merge_usage` looks for a `usage` object in three places: the top level (OpenAI Chat Completions, Anthropic `message_delta`, and a buffered Responses API body), under `message` (Anthropic `message_start`) and under `response` (every event of a streamed Responses API answer). The last was missing, so a streamed `/v1/responses` request was logged with zero tokens, zero cost and `usage_unknown = 1`, and its spend never reached a budget. Only `response.created` and `response.in_progress` carry `usage: null`, which is not an object and does not count as a report; `response.completed` and `response.incomplete` carry the counts, and `response.failed` carries them when the provider settled any. The Responses API spells the counts `input_tokens`/`output_tokens`/`total_tokens` like Anthropic, so the same key mapping serves both. Cost and budget spend follow from the tokens with no further change: `UsageLoggingStream::finalize` prices `prompt`/`completion`/`cache_read` through `ModelPriceConfig::cost` and hands `cost_usd` to the budget recorder. A Responses client served by a Chat Completions or Messages provider is covered too, since the translated stream's own `response.completed` is what the buffer holds. The prompt-cache share of the input is read the same way on both paths (#2847): `merge_usage` takes `cache_read` from Anthropic's `cache_read_input_tokens`, Chat Completions' `prompt_tokens_details.cached_tokens` or the Responses API's `input_tokens_details.cached_tokens`, and the last two count it inside the prompt total, which is how `ModelPriceConfig::cost` reads it, so a model whose price row sets `cached_input_per_mtok` bills those tokens at the cached rate. The reasoning share (`completion_tokens_details.reasoning_tokens`, `output_tokens_details.reasoning_tokens`) has no column and is inside the completion count, so the log does not read it as a figure of its own (see the bullet on cache writes and reasoning below for the one provider that states it beside the completion, and for what the translators do with it). A request translated between dialects is logged from the translated body, so the translators carry the cache split across it (#2863), each side in its own convention. Chat Completions, the Responses API and Gemini count the cached tokens inside the prompt total and name them (`prompt_tokens_details.cached_tokens`, `input_tokens_details.cached_tokens`, `cachedContentTokenCount`, and `total_cached_tokens` on Interactions); Anthropic's `input_tokens` leaves `cache_read_input_tokens` and `cache_creation_input_tokens` out. `TokenUsage` in `crates/rolter-proxy/src/translation.rs` holds one usage object in the first form, the whole prompt, and every translator converts through it, buffered and streamed: an Anthropic answer reaches a chat or Responses client with `prompt_tokens` = `input_tokens` + reads + writes and `cached_tokens` = the reads, and a chat, Responses, Gemini or Interactions answer reaches a Messages client with `input_tokens` = the prompt less the cache and `cache_read_input_tokens` = the cached share. A write count has no OpenAI field, so the translators name it `cache_write_tokens` beside `cached_tokens` (OpenRouter's spelling) and `merge_usage` reads it from there, along with the other spellings in `proxy::cache_written_prompt_tokens` (see the bullet on cache writes and reasoning below). A body whose source reported no caching is translated without cache fields, so no zero is stated that the provider never reported. On an Anthropic stream `message_start` carries the input side and `message_delta` the output side (or the cumulative figures, depending on the API version), so the converter keeps the stream's usage so far and the closing chunk a client reads has both; before, it said `prompt_tokens: 0`. `merge_usage` changed in one place to match: an object whose prompt total is `input_tokens` and which carries `cache_read_input_tokens` or `cache_creation_input_tokens` has them added into `prompt`. `ModelPriceConfig::cost` takes the cached share to be part of the prompt it is handed, and a Messages body, native or translated from Chat Completions, used to hand it `input_tokens` alone, which capped the cached share at the uncached remainder (10 uncached and 80 cached tokens were priced as 10 cached ones). So the `prompt_tokens` column of a native Messages row now holds the whole prompt, as every other dialect's always did; the cache writes sit inside it, and since #2876 they are priced at `ModelPriceConfig::cache_write_per_mtok` (the input rate when a row sets none). A usage object that has `prompt_tokens` is never added to, which keeps a Chat Completions body from a gateway that also spells Anthropic's fields from counting them twice. Tests: the `responses_api` and cached-input cases in `logging.rs` and `crates/rolter-gateway/tests/responses_stream_usage.rs`; for the translated shapes, the cache cases in `translation.rs` (each direction, buffered and streamed, Gemini and Interactions included) and `crates/rolter-gateway/tests/translated_cache_usage.rs`, which asserts the logged `cost_usd` and `cache_read_tokens` and the usage the client is shown, per direction.
- **Cache writes are priced at their own rate** (#2876). `ModelPriceConfig::cost(prompt, completion, cache_read, cache_write, cache_write_1h)` splits the prompt, which holds every class once, into three shares and prices each at its own rate: the reads at `cached_input_per_mtok`, the writes at `cache_write_per_mtok`, the remainder at `input_per_mtok`. Both optional rates fall back to the input rate, so a row that sets neither charges as it always did. The classes are taken in that order and each is capped by what is left of the prompt (reads, then writes out of the rest), so a provider reporting more cache tokens than the prompt holds cannot make a token pay twice. Its two callers hand it the counts they read: `UsageLoggingStream::finalize` the ones `merge_usage` found, and the realtime turn rows in `realtime_metering.rs` theirs (always 0 writes: the Realtime API reports none). `state::to_base_currency` scales the write rate with the others, so a row priced in another currency is converted whole. Tests: the `cost` cases in `config.rs`; `crates/rolter-gateway/tests/cache_write_pricing.rs`, which asserts the logged `cost_usd` of a request with a cache write, native, streamed and translated, and of a row with no write rate.
- **A 1 hour cache write is priced at its own rate** (#2891). Anthropic bills a write to its 5 minute cache at 1.25 times the input rate and a write to its 1 hour cache at 2 times, and reports the two in `usage.cache_creation` (`ephemeral_5m_input_tokens`, `ephemeral_1h_input_tokens`) beside the combined `cache_creation_input_tokens`, which its prompt-caching guide documents as the sum of the two ([Messages API reference](https://platform.claude.com/docs/en/api/messages), [prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)). `cache_write` stays the total everywhere; the 1 hour share is carried beside it and is never an addition to it. `proxy::cache_written_one_hour_tokens` is the one list of spellings (`cache_creation.ephemeral_1h_input_tokens`, and `prompt_tokens_details.cache_write_1h_tokens` / `input_tokens_details.cache_write_1h_tokens`, the names the translators give it on the two OpenAI shapes, which no provider's own usage is known to spell) and `proxy::anthropic_cache_written_tokens` reads Anthropic's total, falling back to the breakdown added up for a body that states only that. `logging::merge_usage` keeps the largest figure seen, like the others: Anthropic states the split on `message_start` only, and the `message_delta` that closes the stream repeats the totals without it (the SDK's `MessageDeltaUsage` has no `cache_creation`), so keeping the maximum is what stops the closing event from erasing the split. `Usage::cache_write_1h` becomes `RequestLog::cache_write_1h_tokens`, clamped to `cache_write_tokens`, and `ModelPriceConfig::cost` takes it as its fifth count: the writes are taken out of the prompt as before, the 1 hour share is taken out of the writes at `cache_write_1h_per_mtok`, the remaining writes are charged at `cache_write_per_mtok`, and a count beyond the writes is capped at them. The 1 hour rate falls back to `cache_write_per_mtok` and then the input rate, so a price that sets only the one write rate charges both caches at it. The translators (`TokenUsage::cache_write_1h` in `translation.rs`) carry the split because the gateway prices the body the client is shown: an Anthropic answer reaches a chat or Responses client with `cache_write_1h_tokens` in the details block beside `cache_write_tokens`, and a chat or Responses answer that states one reaches a Messages client as Anthropic's own `cache_creation` object, whose two shares add up to `cache_creation_input_tokens`. A source that did not split its writes is translated without a split, and a share beyond the write count is stated within it. Only a provider of kind `anthropic` reports it (a custom `api_base` that passes Anthropic's `usage` through included); Bedrock and Vertex AI are plain Chat Completions to rolter, and the providers in the survey below document no per-lifetime split, so their writes are priced at `cache_write_per_mtok`. The request-log column is `cache_write_1h_tokens` (`clickhouse/017_cache_write_1h.sql`, `UInt32 default 0`), serialized only when it is not zero, so a gateway upgraded before the file is applied keeps logging and only loses the share on requests that used the 1 hour cache, and the control plane's `invocations` select returns it as `cache_write_1h_tokens` when the table has the column and a typed `0` when it does not (#2903, see [Optional `request_logs` columns](#optional-request_logs-columns-2903)), so the file is not a condition of upgrading the control plane. The price column is `model_prices.cache_write_1h_per_mtok` (`0085`, non-negative `check`, no new trigger for the reason `0083` gives); `ModelPriceRepo::upsert` takes it with the same `Option<Option<&str>>` rule as `cache_write_per_mtok` (omitted keeps, `null` clears), the control plane validates it with `require_rate`, `state::to_base_currency` scales it with the other rates, and `rolter-seed --import` and `rolter config export` carry it like the 5 minute rate. Tests: the `cost` cases in `config.rs`; the split cases in `translation.rs` (each direction, buffered and streamed) and `logging.rs`; `crates/rolter-gateway/tests/cache_write_pricing.rs`, which asserts the logged `cost_usd` and `cache_write_1h_tokens` of a split request, native, streamed and translated, and of one priced without a 1 hour rate or reported without a split.
- **Audio tokens are priced apart from text** (#1882). `ModelPriceConfig::cost_with_audio` takes an `AudioTokens` (the audio share of the prompt, of the completion and of the cache hits) read by `logging::audio_usage` from `prompt_tokens_details`/`completion_tokens_details` (Chat Completions) or `input_token_details`/`output_token_details` (realtime, a token-billed transcription), and charges each share at `audio_input_per_mtok`, `audio_output_per_mtok` or `cached_audio_input_per_mtok`, falling back to the text rate. `Usage::cost` is the call both the HTTP finaliser and the realtime meter make. See [Realtime metering](realtime-metering.md#audio-and-text-tokens-1882).
- **Non-token units are priced beside the tokens** (#2937). `ModelPriceConfig::unit_cost(&BilledUnits)` prices a count of images, seconds of audio or characters of speech at `image_per_unit`, `audio_input_per_minute` (seconds / 60) or `speech_per_mchar` (characters / 1e6), and `None` when the row sets no rate for that unit. Which unit a request has is `gateway::unit_billing::UnitBasis`, decided from the endpoint where the request log is built (`for_json` for `/v1/images/generations` and `/v1/audio/speech`, `for_multipart` for the two transcription endpoints) and carried on `RequestLog.unit_basis` (`#[serde(skip)]`, not a column). `UsageLoggingStream::finalize` resolves it against the buffered response: the images are the `data` array, falling back to the request's `n`; the audio seconds are `duration` or a `usage` of type `duration`, read as `Decimal` from the number's text; speech characters were counted from `input` at request time. The unit cost is added to `ModelPriceConfig::cost`, so `cost_usd`, the budget counters and the rate-limit recorders see one figure. `usage_unknown` is cleared only when the units were counted and the row has a rate for them; `unpriced` is set for a unit-billed endpoint whose response reports no tokens and whose row has no rate. Tests: `crates/rolter-gateway/tests/unit_pricing.rs` (one fake upstream per unit kind, plus a redis-gated budget check).
- **Gemini thinking and tool-use tokens are counted** (#2875). `usageMetadata.thoughtsTokenCount` (Interactions: `total_thought_tokens`) is billed at the output rate and sits _beside_ `candidatesTokenCount` (`total_output_tokens`), not inside it; Google's reference gives `totalTokenCount` as "prompt + thoughts + response candidates". The translators used to read the candidates alone, so for a thinking model the client's body, the request log and the budget spend were short by the thinking tokens at the output rate, and a buffered body's `total_tokens` (the provider's own) did not equal the `prompt_tokens + completion_tokens` beside it, while the streamed one (summed) did. `TokenUsage::from_gemini` and `interaction_usage` now add the thinking tokens into `completion`. `toolUsePromptTokenCount` (`total_tool_use_tokens`) is what a built-in tool (URL context, say) fed back to the model and is "charged as input tokens" per the pricing page, so it joins `prompt`; the Interactions guide's own usage example sums to its total with the tool-use tokens in it (27 + 10309 + 31 + 45 = 10412), though the one function-calling example in the Interactions reference does not, which reads as illustrative. The provider's `totalTokenCount` is no longer carried: `TokenUsage.total` is `None` for both, so the shown `total_tokens` is always `prompt + completion` and cannot disagree with the counts beside it (the streamed converters used to strip it by hand for the same reason). Cost needs no further change, since `ModelPriceConfig::cost` prices the completion count at the output rate. **What the client sees against what is logged**: the gateway has no Gemini-dialect client route (the client dialects are Chat Completions, Messages and Responses, resolved in `TranslationPlan::resolve`), so a native Gemini body is always translated before it is delivered, and `UsageLoggingStream` builds the row from that same translated body; the two cannot differ. Were a native `generateContent` surface added, its body would be passed through and `merge_usage` would have to learn `usageMetadata`, or it would log zero tokens and `usage_unknown = 1`. The thinking share is also named for chat and Responses clients, as a part of the completion rather than on top of it (#2881, see the bullet on cache writes and reasoning below); Messages has no field for it. Sources: the Gemini API reference for `UsageMetadata` (<https://ai.google.dev/api/generate-content#UsageMetadata>), the Interactions `Usage` reference (<https://ai.google.dev/api/interactions-api>), the thinking guide (<https://ai.google.dev/gemini-api/docs/thinking>), the URL context guide (<https://ai.google.dev/gemini-api/docs/url-context>) and the pricing page (<https://ai.google.dev/gemini-api/docs/pricing>). Tests: the thinking cases in `translation.rs` and `crates/rolter-gateway/tests/translated_cache_usage.rs`, which asserts the logged `cost_usd` of a thinking model, buffered and streamed, for Gemini and Interactions.
- **Prompt-cache hits spelled outside the OpenAI details blocks** (#2877). `proxy::cached_prompt_tokens` is the one list of spellings, read by `logging::merge_usage` (the row of a request that passes through) and `TokenUsage::from_openai` (the translators, so a Messages or Responses client served by such a provider is shown the cache and priced for it). It takes the first present of `prompt_tokens_details.cached_tokens`, `input_tokens_details.cached_tokens`, `prompt_cache_hit_tokens`, top-level `cached_tokens` and `precached_prompt_tokens`; all count the hit inside the prompt total. Anthropic's `cache_read_input_tokens` stays outside the list because it is beside `input_tokens` and is added back in. Survey of the `ProviderKind`s, from the providers' own references:

  | Kind(s)                                         | Field                                                                                                                                                                | Inside the prompt total                                                                                      | Source                                                                                                                                                                                         |
  | ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `YandexGpt`                                     | `prompt_tokens_details.cached_tokens`                                                                                                                                | yes: 3,008 cached of 3,595 prompt tokens; `total_tokens = prompt_tokens + completion_tokens`                 | Direct capture, 2026-10-28, `ai.api.cloud.yandex.net/v1/chat/completions`, `yandexgpt/latest` (#3314); `crates/rolter-proxy/tests/fixtures/yandexgpt-cache-hit.json`                           |
  | `Openai`, `AzureOpenai`                         | `prompt_tokens_details.cached_tokens`; Responses: `input_tokens_details.cached_tokens`                                                                               | yes                                                                                                          | OpenAI prompt caching guide                                                                                                                                                                    |
  | `Openrouter`                                    | `prompt_tokens_details.cached_tokens`, `cache_write_tokens`                                                                                                          | yes (by its example; the page does not say)                                                                  | OpenRouter prompt caching guide                                                                                                                                                                |
  | `Groq`                                          | `prompt_tokens_details.cached_tokens`                                                                                                                                | yes                                                                                                          | Groq prompt caching page                                                                                                                                                                       |
  | `Xai`                                           | `prompt_tokens_details.cached_tokens`                                                                                                                                | yes                                                                                                          | xAI chat completions guide                                                                                                                                                                     |
  | `Qwen`                                          | `prompt_tokens_details.cached_tokens`, writes in `prompt_tokens_details.cache_creation_input_tokens`                                                                 | yes, the docs say so                                                                                         | Alibaba Model Studio context cache                                                                                                                                                             |
  | `Zhipu`                                         | `prompt_tokens_details.cached_tokens`                                                                                                                                | not stated by the page                                                                                       | Z.ai chat completion reference                                                                                                                                                                 |
  | `Anthropic`                                     | `cache_read_input_tokens`, `cache_creation_input_tokens`                                                                                                             | **no**, beside `input_tokens`                                                                                | already added back (#2863)                                                                                                                                                                     |
  | `Deepseek`                                      | `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`; the details block repeats the hit                                                                          | yes, `prompt_tokens` = hit + miss                                                                            | DeepSeek chat completion reference                                                                                                                                                             |
  | `Kimi`                                          | top-level `cached_tokens`; also `prompt_tokens_details.cached_tokens`                                                                                                | yes, the docs say it sums with the rest                                                                      | Kimi chat API reference                                                                                                                                                                        |
  | `Gigachat`                                      | `precached_prompt_tokens`                                                                                                                                            | taken as yes (see below)                                                                                     | GigaChat OpenAPI spec                                                                                                                                                                          |
  | `GeminiNative`, `GeminiInteractions`            | `cachedContentTokenCount`, `total_cached_tokens`                                                                                                                     | yes                                                                                                          | Gemini API reference; read by the translators since #2863                                                                                                                                      |
  | `Mistral`                                       | `prompt_tokens_details.cached_tokens`                                                                                                                                | yes, the guide says `prompt_tokens` contains all prompt tokens                                               | Mistral prompt caching guide (#2882)                                                                                                                                                           |
  | `Together`                                      | `prompt_tokens_details.cached_tokens`; some models put `cached_tokens` at the top of `usage` instead                                                                 | yes, its example is 3,327 of 3,417                                                                           | Together prompt caching guide and OpenAI compatibility page (#2882)                                                                                                                            |
  | `Fireworks`                                     | `prompt_tokens_details.cached_tokens` (in the `UsageInfo` schema with no description); the `fireworks-cached-prompt-tokens` response header on dedicated deployments | header: yes, "out of" the prompt tokens; body: not found                                                     | Fireworks chat completions reference and prompt caching guide (#2882); the header is read by `cached_prompt_tokens_header`                                                                     |
  | `Minimax`                                       | `prompt_tokens_details.cached_tokens`                                                                                                                                | yes, its example is 800 of 1,200                                                                             | MiniMax prompt caching guide (#2882)                                                                                                                                                           |
  | `Doubao`                                        | nothing found: the Ark chat reference was not reachable; read under `prompt_tokens_details.cached_tokens` on the assumption it copies the shape                      | not found                                                                                                    | none (#2882: capture a real response, see the follow-up issue)                                                                                                                                 |
  | `Perplexity`                                    | `prompt_tokens_details.cached_tokens`, `cache_write_tokens`                                                                                                          | yes, the spec says `prompt_tokens` includes cache reads and writes                                           | Perplexity Router OpenAPI spec (#2882)                                                                                                                                                         |
  | `Bedrock`                                       | `input_tokens_details.cached_tokens`, `cache_write_tokens` (OpenAI models, Responses API)                                                                            | yes, its example is 1,920 of 2,048                                                                           | Bedrock prompt caching guide (#2882)                                                                                                                                                           |
  | `Databricks`                                    | `cache_read_input_tokens`, `cache_creation_input_tokens` at the top of `usage`, Claude endpoints only                                                                | not stated; read as inside, like any body that carries `prompt_tokens` (`cost` caps the share at the prompt) | Databricks Foundation Model APIs reference (#2882); read on the OpenAI path as the last spelling of `cached_prompt_tokens`                                                                     |
  | `Ollama`, `LlamaCpp`, `OpenaiCompatible` (vLLM) | `prompt_tokens_details.cached_tokens`; vLLM also `created_cache_tokens` for writes                                                                                   | not stated                                                                                                   | the servers' own README and source (#2882)                                                                                                                                                     |
  | `Gemini` (OpenAI-compatible)                    | none documented on the official OpenAI-compatibility page or Vertex's overview; read under `prompt_tokens_details.cached_tokens`                                     | not found                                                                                                    | the only evidence is a response posted to the Google AI Developers Forum, 2025-05-13 (114,667 of 116,482, the first row of the #2880 table below), which is a forum post, not a primary source |
  | `MetaLlamaApi`                                  | `prompt_tokens_details.cached_tokens` (Chat Completions), `input_tokens_details.cached_tokens` (Responses)                                                           | yes, "a subset of your input tokens", counted once in `prompt_tokens`                                        | Meta Llama API prompt caching guide, <https://ai.developer.meta.com/docs/features/prompt-caching/> (base `https://api.meta.ai/v1`) (#2882)                                                     |
  | `Nebius`                                        | `prompt_tokens_details.cached_tokens`                                                                                                                                | not stated; `total_tokens` is "prompt + completion"                                                          | Token Factory create-chat-completion reference, `Usage` schema, <https://docs.tokenfactory.nebius.com/api-reference/inference/create-chat-completion> (#2882)                                  |
  | `Sarvam`                                        | `prompt_tokens_details.cached_tokens` (`null` on a miss); Responses: `input_tokens_details.cached_tokens`                                                            | taken as yes: "prompt tokens served from cache"                                                              | Sarvam chat completions v2 and Responses references, <https://docs.sarvam.ai/api-reference/open-source/chat-completions.md> (#2882)                                                            |
  | `Hunyuan`                                       | `prompt_tokens_details.cached_tokens` appears in the examples, always 0                                                                                              | not stated; read as inside                                                                                   | Tencent Cloud Hy API guide (TokenHub, `hy4-preview`), <https://www.tencentcloud.com/document/product/1300/80695> (#2882): unconfirmed, no hit printed                                          |

  **YandexGpt capture (#3314, 2026-10-28).** Three identical non-streaming requests to the direct OpenAI-compatible endpoint used a fixed 3,595-token prompt and `max_tokens = 8`. The first two reported zero cached tokens, five seconds apart; after another minute the third reported 3,008 cached tokens, 3,595 prompt tokens, 2 completion tokens and 3,597 total tokens. The cached share is inside the prompt total, not added to it. This settles YandexGpt's row above, which the documentation survey could not; no other unconfirmed kind was probed. The fixture retains only `usage`; response ids, model URI (folder id), timestamps and identifying headers are not checked in. No new reader spelling or production behaviour was needed.

  Re-checked on 2026-10-13 for #2882, with no change to the mapping: Cohere's compatibility page (<https://docs.cohere.com/docs/compatibility-api>) prints no `usage` block (its examples print the message only), and its v2 chat reference (<https://docs.cohere.com/reference/chat>) lists `usage.cached_tokens` ("the number of prompt tokens that hit the inference cache") without saying whether `usage.tokens.input_tokens` includes it; Upstage's chat page (<https://console.upstage.ai/docs/capabilities/generate/chat-completions>) mentions no cached count; Vertex AI's OpenAI overview page (<https://docs.cloud.google.com/vertex-ai/generative-ai/docs/migrate/openai/overview>) could not be read past its navigation; the Scaleway, Baidu Qianfan, Naver and Yandex AI Studio pages timed out, returned 404 or were blocked. Where the docs are silent nothing is guessed, so those kinds keep logging `cache_read_tokens = 0`.

  The kinds that are still unconfirmed log `cache_read_tokens = 0` until a spelling is confirmed from a real response, which is what #984 surfaces. The 2026-10-27 survey found two gaps in the code, now closed: Databricks' top-level `cache_read_input_tokens` was read by `merge_usage` but not on the translators' path (`TokenUsage::from_openai`), and Fireworks' header was read nowhere. Every other settled row names a field that was already read; what #2882 added for them is the evidence and a test per block. Doubao, Hunyuan, Gemini's compatible endpoint and Cohere's compatibility endpoint have no primary source and wait for a captured real response. `Vertex`: the thinking half of its usage is settled (above), but no capture of its OpenAI-compatible endpoint with a cache hit was found; LiteLLM's #42093 says Vertex has started returning a prompt count that leaves the cached content out, but its author states the raw payload was never captured and the shape is the native one. `Cohere`: the native v2 chat names `usage.cached_tokens` ("the number of prompt tokens that hit the inference cache", at the top of `usage`, which the top-level spelling already reads; re-read on the v2 reference), and the compatibility page prints no `usage` block at all, so the compatible endpoint's spelling is unconfirmed. `Hunyuan`: the one captured shape is the new TokenHub gateway's, and every example in it is a miss, so the hit's place in the prompt total is unverified, as is whether the older `api.hunyuan.cloud.tencent.com` base answers the same way. `Nebius`: the schema names the field with no description and prints no example that has it. `Upstage`: nothing found names a cached count in `usage` (the `langchain-upstage` integration lists a `prompt_cache_key` property, which suggests it caches). `Ovhcloud`: no caching page or usage reference naming a cached count was found. `Scaleway`: a downstream issue of June 2026 says it "doesn't implement caching" and another comment of August 2026 that it added cache pricing for DeepSeek V4 Flash, but no page naming the `usage` field was found. `Ernie`: the reference is rendered in the browser and could not be read. `Naver`: the OpenAI-compatibility page was read and names no cached count. Not looked up individually: `Yi`, `Baichuan`, `CloudRu`, `MtsAi`, `Rinna`, `Rakuten`, `Krutrim`, `Falcon` and `AlephAlpha`, none of which has a public reference that was found to print a `usage` block with a cache field. Fireworks' `fireworks-cached-prompt-tokens` response _header_ (dedicated deployments) is read by `proxy::cached_prompt_tokens_header` in the handler, since `merge_usage` only sees the body: `Usage::fill_cache_read` takes it only when the body states no hit, so a body figure wins. The header sits in the buffered and the streamed path alike. GigaChat's schema says the cached tokens are deducted from `total_tokens`, but its own examples show `total_tokens` equal to `prompt_tokens + completion_tokens` with a non-zero `precached_prompt_tokens`, so the hit is read as part of `prompt_tokens`; if it were not, `ModelPriceConfig::cost` caps the cached share at the prompt, so the error is bounded. Tests: one per spelling in `logging.rs` and `translation.rs` (the `DOCUMENTED_HITS` table in `translation.rs` and the documented-shapes case in `logging.rs` pin the usage block each #2882 provider's page prints or describes, and for Gemini's compatible endpoint a response a forum poster captured), and the DeepSeek, Kimi and GigaChat cases of `translated_cache_usage.rs`, which assert the logged `cost_usd` for a request that passes through and for one that is translated.

- **Cache writes, the reasoning share and a provider that states it beside the completion** (#2879, #2881, #2888). `proxy::cache_written_prompt_tokens` is the write-side twin of `cached_prompt_tokens`, read by `logging::merge_usage` and `TokenUsage::from_openai`, so a body keeps its figure across a dialect hop. It takes the first present of `prompt_tokens_details.cache_write_tokens` (OpenRouter, Perplexity, and the name the translators emit), `input_tokens_details.cache_write_tokens` (Bedrock's OpenAI models, Responses form), `prompt_tokens_details.cache_creation_input_tokens` (Qwen's explicit cache, which the context-cache guide counts inside `prompt_tokens` like the hits) and `prompt_tokens_details.created_cache_tokens` (vLLM's local prefix-cache writes); a write read under any of them is priced at `cache_write_per_mtok` like Anthropic's (#2876). Anthropic's own top-level `cache_creation_input_tokens` stays out for the reason `cache_read_input_tokens` does: it sits beside `input_tokens` and the callers that read it add it back in. A Qwen request that created a cache entry used to be logged with `cache_write_tokens = 0` and reached a Messages client without `cache_creation_input_tokens`. The reasoning share is the other half. `TokenUsage` keeps an optional `reasoning` count, read from `thoughtsTokenCount` and the Interactions `total_thought_tokens` (also spelled `thought_tokens`, `thoughts_tokens`) and from `completion_tokens_details.reasoning_tokens` (Chat Completions) and `output_tokens_details.reasoning_tokens` (Responses API). It is a part of `completion`, never added to it: for Gemini the thinking tokens are added to the completion once, by `from_gemini` and `interaction_usage` (#2875), and `reasoning` only records how much of it they are. The Chat Completions and Responses emitters then name it in `completion_tokens_details` and `output_tokens_details`, for buffered and streamed answers; Messages has no field for it, so a Messages client is shown the completion alone. A body that reported no reasoning states none, and one that reported a zero states the zero. The request log has no column for it, so `merge_usage` does not read the share as a figure of its own. One provider breaks the "inside" rule: xAI states `reasoning_tokens` beside `completion_tokens`, and only its `total_tokens` adds the three up (its OpenAPI example prints 32 + 9 + 94 = 135; LiteLLM carries the same correction), while billing the reasoning as output. `proxy::reasoning_tokens_beside_completion` lets the provider's own total decide: the reasoning is beside the completion exactly when `total_tokens` is prompt + completion + reasoning, and then both readers add it to the completion. A provider whose total is prompt + completion, which is OpenAI's convention and most providers', is never added to twice. Vertex AI's OpenAI-compatible endpoint is a second provider of this kind (its `completion_tokens` leaves the thinking out and `reasoning_tokens` names it, with a total that adds the three up) and the same rule counts it; Gemini's own endpoint names it nowhere, which is the bullet below (#2880). The providers whose references name the share in the details block, for reading elsewhere: Together (reasoning models), Fireworks, Doubao, Perplexity, Upstage and vLLM; Databricks puts `reasoning_tokens` at the top of `usage` and does not say whether it is inside `completion_tokens`, so it is not read. Tests: the write and reasoning cases in `translation.rs` and `logging.rs`, and `translated_cache_usage.rs`, which asserts the logged `cost_usd`, the cache write and the usage each client is shown, per direction, for Qwen, the Gemini thinking answers, a Chat Completions provider that breaks out its reasoning and xAI.
- **Gemini's OpenAI-compatible `completion_tokens` leaves the thinking out of the count and Vertex AI's names it** (#2880). Google documents the endpoints' request side (`reasoning_effort`, `extra_body.google.thinking_config`, `extra_body.google.cached_content`) and never the response's `usage`: the compatibility page prints `chunk.usage.to_dict()` without its output and the one response either site prints (Vertex's page, an image model that does not think) has the three counts and nothing else. So the answer comes from responses people captured and posted, and a provider's behaviour can change, so each is dated. `ProviderKind::Gemini` (`https://generativelanguage.googleapis.com/v1beta/openai`) maps the native `usageMetadata` one to one (`promptTokenCount` to `prompt_tokens`, `candidatesTokenCount` to `completion_tokens`, `totalTokenCount` to `total_tokens`) and has no field for `thoughtsTokenCount`, so the thinking tokens, which Google bills as output, are in `total_tokens` and nowhere else. `ProviderKind::Vertex` (`.../endpoints/openapi/chat/completions`) keeps `completion_tokens` as short but names the thinking in `completion_tokens_details.reasoning_tokens`, and its total adds the three up. The captures (the first four of a thinking model; `prompt + completion` is what `completion_tokens` alone gives, and the last column is the part of the total beyond it):

  | Endpoint, date                 | Model                                 | `usage` as posted (prompt, completion, total)                                      | Beyond prompt + completion | Source                                                                                                                                                                                                                                                   |
  | ------------------------------ | ------------------------------------- | ---------------------------------------------------------------------------------- | -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | Gemini, 2025-05-13             | `gemini-2.5-flash-preview-04-17`      | 116,482, 44, 117,488 (`prompt_tokens_details.cached_tokens` 114,667)               | 962                        | Google AI Developers Forum, "Does OpenAI compatible format support implicit caching" (<https://discuss.ai.google.dev/t/does-openai-compatible-format-support-implicit-caching/82872>)                                                                    |
  | Gemini, 2025-08-21             | `gemini-2.5-pro`                      | 758, 102, 1,725                                                                    | 865                        | the same forum, "Question with Usage in OpenAI compatibility" (<https://discuss.ai.google.dev/t/question-with-usage-in-openai-compatibility/98760>)                                                                                                      |
  | Gemini, 2026-01-15             | `gemini-2.5-flash`                    | 15, 18, 175, no `completion_tokens_details`                                        | 142                        | the same forum, "No access to reasoning_tokens via Gemini API's OpenAI compatible layer" (<https://discuss.ai.google.dev/t/no-access-to-reasoning-tokens-via-gemini-apis-openai-compatible-layer/115433>)                                                |
  | Gemini, 2026-02-27 (streamed)  | `gemini-3-flash-preview`              | 9, 1, 103; then 9, 3, 105; then 9, 3, 105: `usage` on every chunk                  | 93, 93, 93                 | the same forum, "Endpoint .../v1beta/openai/chat/completions is not compliant with API specs" (<https://discuss.ai.google.dev/t/endpoint-https-generativelanguage-googleapis-com-v1beta-openai-chat-completions-is-not-compliant-with-api-specs/127400>) |
  | Gemini, 2026-09-19 (tool call) | `gemini-3.1-pro-preview`              | 27, 8, 135, "no explicit reasoning-token field"                                    | 100                        | C360Studio/semstreams#1346 (<https://github.com/C360Studio/semstreams/issues/1346>)                                                                                                                                                                      |
  | Vertex AI, 2026-01-15          | `gemini-2.5-flash` ("the same model") | 14, 21, 113, `reasoning_tokens` 78 (beside `extra_properties.google.traffic_type`) | 78                         | the same topic 115433, the poster's comparison                                                                                                                                                                                                           |
  | Vertex AI, 2026-10-07          | `google/gemini-2.5-flash`             | 14, 59, 633, `reasoning_tokens` 560                                                | 560                        | TAIPANBOX/tokenfuse#367 (<https://github.com/TAIPANBOX/tokenfuse/pull/367>; one non-streamed answer; the PR says the figures are its fixture)                                                                                                            |

  Why the part beyond is the thinking and nothing else: Google's native reference gives `totalTokenCount` as the prompt plus the thoughts plus the candidates (see the #2875 bullet above), and the compatible endpoint copies it across; the streamed capture shows the beyond-part constant at 93 while the completion grew from 1 to 3, which is thinking spent before the first visible token and not the answer; and on Vertex's endpoint, for the same model, the beyond-part equals `reasoning_tokens` to the token (78, 560). The only other count in a native total is `toolUsePromptTokenCount`, which a built-in tool feeds back and which is billed as input. The compatibility page documents one built-in tool on the endpoint, Google Search grounding (`extra_body` `tools`, Gemini 3 and newer), and none of the captures used it; if a grounded answer's total carries those tokens too they would be priced at the output rate instead of the input rate, which bounds the error and does not leave a billed token out. Strength: five independent captures of the Gemini endpoint over sixteen months and two of Vertex's, all consistent, none from Google; nothing found shows a thinking model whose compatible `completion_tokens` included its thinking. What was not done is the comparison the issue asks for, a native `:generateContent` for the same prompt, which needs a key; the Vertex pair stands in for it.

  What it does, in `rolter-proxy` (`translation.rs`) and where the gateway reads usage (`logging.rs`, `handlers.rs`). `ThinkingCount::of(kind)` is `TotalOnly` for `ProviderKind::Gemini`, `NamedBeside` for xAI and Vertex AI (#3093) and `Stated` for every other kind, so no other provider is given a gap to fill: no other provider's reference says that what its total has beyond the parts is thinking, and the one that states thinking beside its completion (xAI) names it in a field. `TranslationPlan::resolve` carries it, because the request log reads a passthrough answer's usage from the provider's own bytes, where only the kind says what the counts mean. `ThinkingCount::beside_completion` returns the explicit-field reading of #2888 (which is how Vertex's `reasoning_tokens` is added, for every kind) or, for `TotalOnly`, the `total_tokens` beyond `prompt_tokens + completion_tokens`, and `merge_usage` adds that to the completion of every usage object in the buffered body, so the streamed form, which repeats `usage` on every chunk, ends on the final total's completion. A translated answer is folded the same way before the pair reads it (`ThinkingCount::fold_into_completion`, buffered and per chunk), so a Messages client is shown `output_tokens` with the thinking in it and a chat or Responses client has the thinking named in `completion_tokens_details.reasoning_tokens` / `output_tokens_details.reasoning_tokens`, with `total_tokens` the sum of what it is shown. A chat client served by `Gemini` has its `usage` completed the same way (#2926): `TranslationPlan::completes_chat_usage` is true for a Chat Completions client on a `TotalOnly` upstream, `rewrites_response` (translation or completion) is what the gateway asks before it buffers a non-streamed body or wraps an SSE stream, and `translate_response` / `SseConverter` apply `ThinkingCount::completes`. The cost is a byte search for `"total_tokens"` in the body or event before anything is parsed, so an event with no usage is forwarded as it came, and the `Gemini` kind alone pays it (the endpoint repeats `usage` on every chunk, so for it most events are parsed). An event or body that changes is re-serialized, and the rest of the stream, comments and `[DONE]` included, is byte for byte the provider's. The chat client is then shown `completion_tokens` with the thinking in it, `completion_tokens_details.reasoning_tokens` naming it and `total_tokens` unchanged, which is the figure the row, the cost and the budget spend already used, so the row reads the completed body and `beside_completion` finds nothing left to add (no double count). A chat client on xAI or Vertex AI, which state thinking beside the completion in a field, is completed the same way since #3093 (see the bullet below); every other `Stated` kind is passed the provider's bytes untouched. The rule is inert when there is nothing beyond the parts: a model that did not think, a total below the parts, an embeddings answer, an endpoint fixed to count the thinking inside `completion_tokens`, and one that names it beside a total that adds up (the explicit reading finds it, counted once). The cache hit is unaffected: the 2025-05-13 capture has `prompt_tokens_details.cached_tokens` inside `prompt_tokens` and a thinking gap beside the completion. Tests: the captures above, verbatim, in `translation.rs` (`GEMINI_COMPAT_ANSWERS`, `VERTEX_COMPAT_ANSWERS`; the chat client cases there include a stream split at every byte and the bodies that must come back byte for byte), `logging.rs` and `crates/rolter-gateway/tests/translated_cache_usage.rs`, which asserts the logged `cost_usd` and what each client is shown, for a passthrough and a translated answer, buffered and streamed.

- **One rule for the reasoning tokens a chat client is shown** (#3093). After #2926 a Chat Completions client on Gemini was shown the thinking inside `completion_tokens`, while xAI's and Vertex AI's clients read the short count (xAI) or the short count with `reasoning_tokens` beside it (Vertex). The rule is OpenAI's own: `completion_tokens` includes the reasoning, `completion_tokens_details.reasoning_tokens` is the breakdown and `total_tokens` is the prompt plus the completion. OpenAI's OpenAPI description says exactly that: `completion_tokens` is the "number of tokens in the generated completion", `total_tokens` is "prompt + completion", `completion_tokens_details` is a "breakdown of tokens used in a completion" and `reasoning_tokens` are "tokens generated by the model for reasoning" (<https://github.com/openai/openai-openapi>, `CompletionUsage`). The three providers that count thinking beside the completion are completed to it: `ThinkingCount::NamedBeside` (xAI, Vertex AI) joins `TotalOnly` (Gemini), `TranslationPlan::completes_chat_usage` is true for all non-`Stated` kinds, and `ThinkingCount::completes` takes the amount from `beside_completion`, so one function decides what is beside the completion for the log, the translators and the chat client. For `NamedBeside` the byte search also wants `"reasoning_tokens"` in the event, so a stream event with usage and no thinking is never parsed. The log reads the completed body: once the thinking is inside, the total is prompt plus completion, `reasoning_tokens_beside_completion` finds the three no longer add up and nothing is added a second time, and a second pass over a completed body changes nothing. An answer that already follows the rule (total equals prompt plus completion, or a named zero) is returned byte for byte. Sources, official documentation only, no key and no provider call: xAI's API reference prints a usage whose parts add up to the total with the reasoning beside the completion (<https://docs.x.ai/docs/api-reference>, `prompt_tokens` 32, `completion_tokens` 9, `reasoning_tokens` 110, `total_tokens` 151; the OpenAPI example the earlier fixtures use is 32 + 9 + 94 = 135), and xAI's reasoning guide says reasoning tokens are billed as part of consumption without stating the field layout (<https://docs.x.ai/docs/guides/reasoning>). Google documents neither compatible endpoint's response: the Gemini compatibility page only prints `chunk.usage.to_dict()` (<https://ai.google.dev/gemini-api/docs/openai>) and Vertex AI's OpenAI-library page prints a three-count usage with no thinking breakdown (`completion_tokens` 1299, `prompt_tokens` 7, `total_tokens` 1306, <https://docs.cloud.google.com/vertex-ai/generative-ai/docs/start/openai>); the Vertex thinking guide documents only the native `thoughtsTokenCount` (886 against a total of 1525 in its example) and says thinking is billed. The Gemini and Vertex fixtures therefore stay the dated captures of the bullet above, and the xAI ones are the documented examples. Tests: `xai_and_vertex_answers_reach_a_chat_client_with_the_thinking_in_the_completion`, `a_stream_of_xai_or_vertex_reaches_a_chat_client_with_the_final_usage_completed` and `a_beside_provider_answer_that_already_includes_its_thinking_is_returned_as_it_came` in `translation.rs`, and the xAI and Vertex chat cases in `crates/rolter-gateway/tests/translated_cache_usage.rs`, which assert what the client is shown and that the row and the cost are the same figures as before.
- **The built-in `fake-llm` model is logged too** (#2802). It answers before the pipeline that builds a row for every provider request, so for a long time its traffic was missing from LLM Logs and from every usage roll-up, and the first smoke test an operator ran looked like logging was broken. `handlers::log_builtin` now writes its row for every endpoint it serves: chat completions and messages (streamed and not), responses, embeddings, rerank, images, speech, transcription and the 404 for a path it does not serve. The row is built from `RequestLogCtx::into_log`, the same base the response-cache hit path uses, so attribution (key, org, team, project, business unit, customer, `request_id`, `trace_id`, `sample_rate`) cannot drift between the two. It carries `provider = builtin` (`fake_llm::PROVIDER_LABEL`) and `target = fake-llm`, so every per-provider view groups it without a special case. Token counts come from `fake_llm::respond`, which reports the numbers the body is built from rather than parsing them back out: an OpenAI chat stream carries no usage object, so parsing would log zero for it. `cost_usd` is `0`, and the row is neither `unpriced` nor `usage_unknown`, because for a model that costs nothing the zero is exact. `LogSink::log_builtin` records the latency histograms and the row but not the per-target outcome counter or the passive health event, since there is no target to be unhealthy. Not done, deliberately: the fake tokens are not charged against token-per-minute windows or budgets (as before this change), and payload capture does not apply. A configured route named `fake-llm` shadows the built-in and is logged under its own provider like any route. Tests: `crates/rolter-gateway/tests/builtin_request_log.rs`.
- **Every request from an identified caller leaves exactly one row, and a failure keeps its upstream reason** (#2807). The rule has two halves.
  - _After an upstream_: a request that failed after reaching, or trying to reach, an upstream is logged with the status the caller received (`status`), the HTTP status of the last attempt that answered (`upstream_status`, `0` when none did) and the attempts made (`attempts`, `0` when it never reached one), plus the provider and target of that last attempt. Both columns are `clickhouse/015_upstream_attempts.sql`, default `0`, and the control plane's `invocations` select reads them when the table has them (#2903, see [Optional `request_logs` columns](#optional-request_logs-columns-2903)). The three ends of a forward loop each have their own row: a transport failure (`502`, or the queue's own `429`/`503`, taken from the response actually sent; `upstream_status = 0`), failover exhaustion (below), and the balancer having nothing to pick (`503 no target selected`, no provider or target, because nothing was tried).
  - _Failover exhaustion_ used to answer `503 no target selected` with no row: the loop re-picks on a retryable status, drops the failed response, finds no untried target and reports a routing problem. The loop now keeps the last status as a `StatusFailure` (`crates/rolter-gateway/src/upstream_failure.rs`), reading the response's body for its reason before dropping it, bounded to 8 KiB and 500 ms so a stalled body costs a row its reason rather than the request its latency. `upstream_failure::exhausted` answers the caller: an upstream `429` stays a `429` (`rate_limit_error`, code `upstream_rate_limited`, the upstream's `Retry-After`), any other status is a `503` (`overloaded_error`, code `upstream_unavailable`, naming the last upstream status and saying the targets are cooling down only when cooldowns are on). The same envelope serves `/v1/chat/completions` and `/v1/messages`: rolter has one error shape for every dialect. The loop's three copies (classic, variants, multipart) share this module so they cannot drift. When the retry budget ends on a retryable status first, the upstream's response is handed to the caller as it was, as before.
  - _The reason_ is `reason_from_body`: `error.message` (OpenAI, Anthropic, most compatibles), a bare `message` or `detail`, and `error.metadata.raw` appended when a router nests the provider's own words there (OpenRouter's `temporarily rate-limited upstream`), reduced to one line of at most 300 characters with URLs masked by `rolter_core::redact::redact_urls_in_text`, never the raw body. A response handed back as it was gets it in `UsageLoggingStream::finalize` from the buffer it already holds, so it costs nothing extra. The caller-facing exhaustion message names the status only; the reason is for the operator. Transport errors are masked by the same function before they reach a row, matching what the redacted `502` bodies show the caller.
  - `passive_health_event` ignores `error` when the caller was handed the upstream's own status, since a `429` whose message says "timed out" is a rate limit, not a timeout.
  - _Refusals_: a refusal before the first upstream attempt is logged through `Refusals::log` in `handlers.rs`, which reads the message off the response (`ApiError` stamps it as an `ErrorMessage` response extension, never sent to the client) and writes `LogSink::log_refusal`: the ClickHouse row only. It skips `observe_request` because the per-model latency histogram is keyed by the model the caller named, which on a refusal no route has vouched for, so a key could mint series without bound; the refusal has its own counter (`budget_blocks_total`, `rate_limit_blocks_total`, `guardrail_blocks_total`, ...). The row's model is cut to 256 bytes and its error to 512. The refusals logged: the model gate and the route gate (`403`), a spent budget or refused unpriced model (`402`), a rate limit (`429`), a guardrail, plugin, prompt template or output-token cap (`400`), a streamed request a post-call policy cannot cover (`400`), the PII sanitizer being unavailable (`502`), an unknown model (`404`) and a route with no targets (`503`), on the JSON and the multipart pipelines both.
  - _Deliberately unlogged_, each for a reason: anything before the caller is identified (a body that is not JSON, a missing `model`, a missing or unknown key, the required-header and body-size middleware) because no tenant owns the row and anonymous traffic could otherwise write to ClickHouse without limit (`rolter_auth_failures_total` counts the keys and `rolter_rejected_requests_total{reason}` splits them by `missing_key`, `invalid_key` and `required_header_missing`, #1395; the decision is to log what a valid key bounds and count the rest); and `GET /v1/models`, health and metrics, which are not requests to a model. The response-lifecycle calls used to be a third entry and are not any more (below).
  - _Lifecycle calls on a stored response_ (#2836). `GET`, `DELETE`, `cancel` and `input_items` on `/v1/responses/{id}`, and the two routes the gateway answers `501` for (`compact`, `input_tokens`), write one row each through `LifecycleLog` in `handlers.rs`, so the rule above has no exception. The decision was yes: they are requests from an identified caller, and a lifecycle call that failed upstream was visible only as its `502`. The row takes its `model` from the stored `ResponseRoute` and, once a call went upstream, its `provider` and `target` from it too; it carries no tokens and no cost, and is not `unpriced` or `usage_unknown`, because a call that generates nothing has no usage to be unknown and no price to be missing. `lifecycle_operation` says which call it was (`retrieve`, `delete`, `cancel`, `input_items`, `compact`, `input_tokens`); it is empty on every other row and omitted from their JSON. It is `clickhouse/016_lifecycle_operation.sql` — no existing column tells endpoints apart, and overloading `variant`, `target` or `error` would corrupt the roll-ups that read them. A gateway upgraded before the migration is applied keeps logging and drops the field (`input_format_skip_unknown_fields=1`); the control plane's `invocations` select reads it since #2865 (`analytics.rs`, next to `upstream_status` and `attempts`) and, since #2903, only when the table has the column, so `016` is not a condition of upgrading the control plane. The rows go to the dashboard as ClickHouse returned them (`format JSON`), so there is no Rust row type: `lifecycle_operation` is a string on every row, `""` for a model request and for a row older than the column. A filter on the operation was left out on purpose: no screen needs one, and a `request_id` lookup already finds the one call.
  - _On screen_ (#2865): LLM Logs reads `lifecycle_operation` as optional (`InvocationRow.lifecycle_operation?` in `ui/src/lib/api/logs.ts`), so an empty string and a missing field both mean a request that ran a model and print nothing. `lifecycleLabel` in `ui/src/pages/Logs.tsx` maps the six values to `pages.logs.detail.lifecycle.operations.*` and prints a value it has no copy for as it came, so an operation a newer gateway adds is still readable. The table puts the label in an outline `Badge` beside the model name (not a status hue: it says what kind of row this is, not whether it went well) and the drawer opens its Routing group with it under "Stored response call". A call on an id that does not exist is logged with no `model`, so `rowName` names the row by the call in the chevron's accessible name and the drawer's title. Stories: `Screens/Logs`, from `ALifecycleCallNamesItsOperationBesideTheModel` to `TheOperationSurvivesALongModelAtMobileInRussian`.
    - _What the row says about the upstream_ follows the model requests: a provider that answered has `upstream_status` and `attempts = 1`, with `error = upstream returned <status>` when the status is `4xx` or `5xx` (the body is streamed to the caller and not read for a reason); a provider that could not be reached is the `502` with `upstream_status = 0`, `attempts = 1` and the transport error with URLs masked; a call the gateway refused before the provider (unknown or foreign id `404`, access since revoked `403`, an operation the provider has no API for `501`) names the model but no provider or target and has `attempts = 0`, which reads as "never reached an upstream". The unknown-id row has no model, since no route was stored, and never quotes the id the caller sent. Callers the gateway cannot identify still leave no row.
    - _Not billable_. The path builds no `SpendRecorder` and no `TokenRecorder` and never asks the budget or rate-limit gates, so no budget counter, token window or request admission moves; `LogSink::log_lifecycle` writes the ClickHouse row only, so a `GET` that answers in a few milliseconds does not enter the per-model latency histograms, and the call does not feed the passive health series because it is pinned to the provider holding the response rather than chosen by the balancer. The latency is the time to the provider's answer, not to the end of the body.
    - Tests: `crates/rolter-gateway/tests/response_lifecycle_log.rs`, including the Redis-gated checks that a rate-limit window of two and an org budget are untouched by lifecycle calls.
  - _The backoff is skipped when it would wait for nothing_ (#2835). A forward loop sleeps `retry_delay_ms` (the upstream's `Retry-After` on a `429`, up to 30 s, else the jittered exponential backoff) before its next attempt. On a route with one target there is no next attempt, and the caller used to wait the whole delay to be told what the upstream had already said. The three loops (classic, multipart, variants) now ask `has_untried_target` (`has_untried_candidate` for variants) first and `back_off` only sleeps when the answer is yes. `has_untried_target` is the tail of `TargetPool::pick` taken on its own, without the balancer, so it cannot advance a round-robin cursor and cannot disagree with the pick that follows (a unit test sweeps every combination of tried, parked and key-restricted targets). The multi-key path keeps its wait: a `429` or `401` on a provider with several keys parks the key and calls `tried.pop()`, which makes the same target untried again, so the check says yes. A failover to a sibling target waits as it always did. Tests: `crates/rolter-gateway/tests/retry_backoff.rs`.
  - _`rolter_retries_total` counts the retries that happen_ (#2866). The counter used to move in the block that supersedes a failed attempt, before the loop knew whether a target was left, so a single-target route that answered `429`, or the last target of several, counted a retry that never took place. `count_retry` in `handlers.rs` now takes the answer of `has_untried_target` (`has_untried_candidate` for variants) and increments only on yes, then hands the same answer to `back_off`, so the metric and the wait agree on whether another attempt follows. A failover to a sibling is counted once per hop, and a `429` on one key of a multi-key provider, which makes the same target untried again, is counted. The help text and `docs/user-docs/api/health-and-metrics.mdx` say what is counted. Tests: the `rolter_retries_total` section of `crates/rolter-gateway/tests/retry_backoff.rs` (a single target answering `429` and `500`, a refused connection, an upload, a variant route, a failover, a second key).
  - _On screen_ (#2837): LLM Logs reads both columns as optional (`InvocationRow.upstream_status?` and `attempts?` in `ui/src/lib/api/logs.ts`), a missing field being `0`. `upstreamNote` in `ui/src/pages/Logs.tsx` turns a row into the sentence under the drawer's status badge: the upstream's status and the attempts it took when it answered and the request failed, was handed on under another status, or was retried; "did not respond" when attempts were made and no status line came back; and "refused before reaching an upstream" only for a failed row with `attempts = 0` and an empty provider and target, because `attempts = 0` alone is also a cache hit, a `fake-llm` call and every row older than the columns, and a failed old row that names its provider must not be called a refusal. The status cell shows the upstream's number beside the caller's only where the two differ. Stories: `Screens/Logs`, from `TheStatusCellHintsAtTheUpstreamStatusWhenItDiffers` to `TheUpstreamHintWrapsUnderTheBadgeInANarrowTable`.
  - Tests: `crates/rolter-gateway/tests/upstream_failure_log.rs` (a single-target route whose upstream answers `429`, both dialects, variants, uploads, failover exhaustion, a retry budget of zero, a refused connection, every logged refusal, and the unlogged pre-identification ones).
- **Sampling** (`logging_settings.sample_rate`, default `1`): `LogSink::enqueue` drops a row whose request-id hash bucket is at or above the rate (`should_sample_request` in `crates/rolter-gateway/src/logging.rs`) before it reaches the ClickHouse channel. `LogSink::observe`, the budget spend recorder and the rate-limit token recorder all run before that decision, so budgets, rate limits and `/metrics` count every request. `LogSink::enqueue` stamps each kept row with `sample_weight = 1 / sample_rate` (`clickhouse/014_sample_weight.sql`, default `1`), the rate in force when it was written. The analytics endpoints (`summary`, `timeseries`, `by-model`, `by-variant`, `by-attribution` in `crates/rolter-control/src/analytics.rs`; `by-variant` also reports `first_attempt_requests`, `sumIf(sample_weight, request_logs.fallback_reason = '')` (#3409), selected as `null` when the table lacks the `023` column so a split it cannot make is not claimed, and `assigned_requests` (#3505), grouped by the first-dialled variant `first_variant` (`clickhouse/048`) and `null` without it, and `sampled_requests` (#3547), grouped by the sampled variant `sampled_variant` (`clickhouse/050`) and `null` without it; see [Variant analytics](load-balancing.md#variant-analytics)) scale counts and sums by it (`sum(sample_weight)`, `sum(cost_usd * sample_weight)`, `sumIf(sample_weight, …)`) weight the mean latency (`AVG_LATENCY`, #3420: `sum(latency_ms * sample_weight) / sum(sample_weight)`, `0` for no rows, so a plain `avg` no longer skews it when weights differ) and the latency percentiles (`P50_LATENCY` / `P95_LATENCY`, #3355): `quantileExactWeighted(level)(latency_ms, toUInt64(round(sample_weight * 1000)))`, because ClickHouse's weighted quantiles take an unsigned integer weight and `sample_weight` is a `Float64` (scaled to thousandths, a constant factor that moves no quantile), and not the timing variant, which clamps values over 30 000 ms. The `p95_latency_ms` alert signal reads the same expression. `/api/v1/me/usage` (`usage_sql` in `me.rs`) reuses the same `REQUESTS` / `TOKENS` / `COST` / `ERRORS` expressions, and the `request_volume` / `spend_velocity` / `error_rate` alert signals (`metric_sql` in `alerting.rs`) weight the same way, `error_rate` as `sumIf(sample_weight, status >= 500) / sum(sample_weight)`; the `samples` column the no-data check reads stays a plain row count (#2278). Tests: `crates/rolter-control/tests/sampled_readers.rs`, against a real ClickHouse. The **Logs Settings** screen warns with the resulting share whenever the form holds a rate below 100 % (#2088). The Dashboard, Business Units, Customers and LLM Logs carry the same note (`SampledLogNotice`, #2231) whenever the saved rate is below 100 %. `useLogSampling` (`ui/src/lib/use-log-sampling.ts`) reads the rate from `GET /api/v1/analytics/sampling` (`get_log_sampling` in `crates/rolter-control/src/logging_settings.rs`), which answers `{"sample_rate": …}` and nothing else from the policy. It takes the `AnalyticsAccess` extractor the analytics reads take, so every role that can open those screens learns the rate, while `GET /api/v1/logging-settings` stays `logging_settings:read`, superadmin only (#3057). It answers the rate the snapshot serves: the `logging_settings` row merged with the bootstrap file's `[logging]` through `LoggingSettings::served`, which applies the same `merge_logging` the snapshot does, so a file-config `sample_rate` reaches the note while the stored row still holds its seeded defaults (#3171). It reads the one row and the file kept on `ControlState::file_logging`, never a whole `ConfigStore::load()`, which would decrypt every provider key per dashboard read. `GET /api/v1/logging-settings` still answers the stored row, which is what the settings form edits.
- **Request log CSV export bounds** (`crates/rolter-control/src/analytics_export.rs`, #3360). `GET /api/v1/analytics/invocations/export` walks the log a page at a time, so memory is bounded, but a window over the whole retention period holds a connection and a run of ClickHouse queries. `ExportLimits` (the `ROLTER_EXPORT_*` flags of `rolter-control`, `0` for off) caps the rows and the wall-clock time of one export and, through `ExportGate`, the exports running at once in the process and per caller (`caller_key`: an account, an API token, or the shared admin credential). `LogWalk` asks for one row past the row cap, so a log of exactly the cap is a complete file; a page query runs under the time that is left. Past either cap the next chunk is an `Err`, which ends the body with the error a failed page produces (the status line is already `200`); past a concurrency bound the handler answers `429` with `Retry-After` before any ClickHouse work. The `ExportPermit` rides in the `LogWalk` inside a `TimedPermit`, so it is given back when the body ends, fails or is dropped by a client that left. A client that stops reading without closing parks the body on a full socket buffer, where nothing polls it, so the row, time and cancel checks never run: `TimedPermit` therefore spawns a timer for `ROLTER_EXPORT_MAX_SECS` that empties the permit's slot whether or not the body is polled, and `LogWalk` ends in the time-limit error if it is polled after that (#3433). The slot is also emptied in `Drop`, because an aborted timer task only releases its clone of the slot when the runtime next polls it. The timer is the only deadline; a per-chunk idle deadline was left out. Test: `a_client_that_stops_reading_loses_its_place_at_the_deadline`. Tests: `analytics_export::tests`, against a fake ClickHouse. The timer also closes the stalled connection on HTTP/1.x (#3477): a body can only end a connection when hyper polls it, and hyper stops polling while the socket buffer is full, so `rolter-control` serves its public listener through `conn_close::CloseableListener`, whose sockets fail their next read or write once a per-connection `CloseSwitch` is tripped, and close with a reset so data queued for the peer that stopped reading is discarded. `MakeCloseable` hands each request its connection's `ConnectionHandle` as an extension. `TimedPermit` trips it at the deadline, so hyper drops the connection, its task and the buffered chunk without the body being polled; the trip is decided under the permit's lock and is skipped once the export produced its last page, so a keep-alive connection that moved on is not cut. On HTTP/2 (the control plane also serves h2c, and a proxy may multiplex many users onto one connection) nothing is tripped: the timer only gives the place back, and h2's per-stream flow control bounds what is buffered for the stalled stream (`an_h2_export_past_the_deadline_leaves_the_shared_connection_open`, `only_an_http1_connection_is_cut_by_the_timer`). With `ROLTER_EXPORT_MAX_SECS=0` there is no timer, so neither the place nor the connection is ever cut. The HTTP/1.x close is covered by `a_client_that_stops_reading_has_its_connection_closed_at_the_deadline`. There is no server-wide write timeout, which would also cut slow but healthy streams.
- **Export download tickets** (`crates/rolter-control/src/export_tickets.rs`, #3504, [ADR-0041](../adr/2026-10-29-export-download-tickets.md)). A browser without `showSaveFilePicker` cannot stream an export to disk, and its download manager cannot send the bearer header, so `POST /api/v1/analytics/exports/tickets` mints a 60-second, single-use ticket for one export path and query and `GET /api/v1/analytics/exports/download/{ticket}` redeems it. The mint reads the body first, since the export it names decides the checks, and then runs that export's own (`may_export`): `AnalyticsAccess` before `Caller` for the analytics exports, since without a database the access check is what refuses a wrong bearer and `Principal` would reach for a pool that is not there, and for the audit log's two (#3545, a control plane with a database only) the principal and `audit_export::authorize_export`, `audit_log:read` on the org or `deployment_audit_log:read`; a body that names no export is refused with `400` only after the bearer has been checked. It parses the query through `analytics_export::check_query` and, for the request log, asks `ExportGate::would_admit` without taking a place. A `TicketStore` trait has two implementations: `PgTickets` over `export_tickets` when the control plane has a pool, and `MemoryTickets` (`parking_lot`) otherwise; redemption is one `update … where used_at is null and expires_at > now() returning …`. The minter (`Minter`: a session or API token id, an admin token digest, open mode) is re-resolved by `principal_of`, and for an analytics export `analytics_access::analytics_reach` turns it into the `AnalyticsAccess` a bearer request would get; an audit export skips that reach, and `run_redeemed` hands it to `audit_export::run_redeemed`, which checks the audit capability again for the re-resolved principal. `analytics_export::run_redeemed` otherwise calls the bearer route's own handler with that principal and access, so the handlers' auth is unchanged: the only addition to them is an optional `Extension<Arc<ExportProgress>>`, through which `LogWalk` reports rows and the `Cut` (row or time limit) that ended it. The download route refuses any method but `GET` with `405` before it looks the ticket up (axum routes `HEAD` to the `GET` handler, and a link checker must not spend a ticket). It wraps the body in a `Tracker` that writes the ticket's `status` at the start, at most every two seconds while chunks flow (spawned, so a slow pool never stalls the body), and at the end after the last chunk (`complete`; `truncated` with its reason, spawned; `client_closed` from `Drop`, which also resends an end whose write did not complete), and records a non-`200` answer as `refused` with its `Retry-After`. `TicketStore::report` only moves a ticket forward (`TicketState::may_become`, and the same rule in the Postgres `update`'s `where`), so a late progress write cannot overwrite the end. The `#3477` deadline timer records `Cut::TimeLimit` on the `ExportProgress`, so a stalled connection it closes reads `time_limit`, and the status read reports a `streaming` ticket unwritten for longer than the export time limit plus two progress intervals as `truncated` / `interrupted`. The admin token's digest is keyed with the ticket's secret, and an API token rotated after the mint (`rotated_at` against the ticket's `created_at`) is refused. The secret is read off the URI rather than a `Path` extractor; the request span and metrics carry the route template; `api_not_found` echoes the path through `export_tickets::redact_path`; and the response carries `Cache-Control: no-store` and `Referrer-Policy: no-referrer`. Tests: `export_tickets::tests` (the memory store, and the Postgres store under `--features postgres`). On the dashboard, `ExportCsvButton` keeps the `showSaveFilePicker` stream-to-file path and otherwise calls `downloadWithTicket` (`ui/src/lib/ticket-download.ts`): mint, follow `download_url` with a hidden `<a download rel="noreferrer">`, then read the status every second until `done`, reporting rows and mapping `truncated` to `DownloadTruncatedError` (or `DownloadStoppedError` for `client_closed`, shown as a neutral line), a refused `429` to the same `ApiError` the mint answers and any other refusal to `ExportRefusedError`. It tolerates three failed reads in a row and gives up with `ExportStalledError` after five minutes without a change; the button's Cancel on this path only stops the polling. The `Response.blob()` fallback is gone. Story: `ExportCsvButton.stories.tsx`; tests: `ticket-download.test.ts`.
- **Writer timeouts** (#2373): the request-log, health-event and MCP tool-call writers share one HTTP client (`crates/rolter-gateway/src/clickhouse_client.rs`) with a 3 s connect timeout and a 10 s whole-request timeout. They are constants, not `[logging]` keys: the rows are best-effort telemetry and no deployment has a reason to wait longer. A ClickHouse that accepts the connection and never answers therefore fails the flush like any HTTP error: the batch is dropped, counted in `rolter_logs_dropped_total` / `rolter_health_events_dropped_total` / `rolter_mcp_events_dropped_total`, and logged at `warn` (`timed_out=true`) with the URL stripped, since `CLICKHOUSE_URL` can carry a password. Without the bound one stalled flush froze the writer until its queue filled. `drain_sinks` keeps its own 5 s grace on top, so shutdown is bounded even when a flush is still in flight.
- **`ts` is the instant the request began** — the same instant `latency_ms` is measured from, and the one the passive health event derived from that request carries. It is reconstructed from the request's monotonic start (`Instant`), so a clock step during a long request cannot reorder rows, and it is written by the gateway as an RFC 3339 literal at the column's millisecond precision (the insert asks ClickHouse for `date_time_input_format=best_effort` so that literal parses into `DateTime64(3)`). Add `latency_ms` to `ts` for the completion time. The same rule applies to the matching `request_payloads` row, which copies its request's `ts`, `org_id`, `project_id` and the gateway-minted `log_id` that joins the two (#1937).
- Gateways older than #1210 did **not** write `ts` at all and let the column's `default now64(3)` stamp it, which recorded the _batch flush_ time: every row in one flush shared a single millisecond, bursts collapsed onto one point, timeseries buckets were skewed by the flush interval, and keyset paging by `(ts, request_id)` had no order within a batch. The column default is kept as a fallback for those writers, so historical rows and any pre-#1210 gateway still land — but on such rows `ts` means flush time, not request time.
- **Retention** defaults to 90 days for metadata and seven days for captured payloads, set as the TTL in the ClickHouse schema. Both are admin-managed: `PUT /api/v1/logging-settings` accepts `retention_days` (1–3650) and `payload_retention_hours` (1–8760) and issues the matching `alter table … modify ttl` against ClickHouse, which then expires parts on its own schedule. Payload retention may not exceed metadata retention, so raw prompt bodies never outlive the row they belong to. A ClickHouse failure leaves the stored policy in place and is logged rather than failing the admin write — re-saving reapplies it.
- **Payload capture** is disabled by default. Set `[logging.payload_capture] enabled = true` to write redacted request and response payloads to the separate `request_payloads` table, which has a seven-day TTL (versus 90 days for request metadata). `max_bytes` bounds each body; `redact_fields` adds recursively redacted JSON keys before storage, and `redact_detectors` / `redact_patterns` (#1835) replace credential shapes inside string values with typed markers such as `[redacted:aws_key]`. The policy is compiled once per snapshot into `rolter_core::payload_redaction::PayloadRedactor` (linear-time `regex` engine, size-limited, with a `RegexSet` prefilter) and applied in `capture_payload` before truncation, so nothing unredacted reaches the log writer. `POST /api/v1/logging-settings/redaction-preview` runs the same code over a pasted sample and stores nothing. Optional `models` and `virtual_key_ids` allow-lists make the deployment-level switch route- or key-specific.
- **Request id / trace continuation**: every request carries a request id — the caller's when supplied, otherwise a generated UUID — which is echoed on the response and stored on the log row for end-to-end correlation. It travels in `x-request-id` unless `[client] request_id_header` names another header (#2925). `trace::ensure_request_id` reads that name from the live snapshot once per request, so a reload applies to the next request and never splits one request's id across two headers, and it hands the id to handlers as a `trace::RequestId` request extension. Handlers, the request-log row, the MCP tool-call log and the trace layer's response log line all take the id from that extension rather than from a header, so none of them needs the name. `ensure_request_id` also records it onto the `gateway.request` span as `rolter.request.id` (#2964): the tower layer builds that span before the middleware runs, so the field is declared empty at creation and filled in afterwards, like the tenant attributes, which keeps the layer order unchanged and is a no-op when no OTLP pipeline is installed. The id is also written back onto the request under the configured name, which is how a minted id reaches an upstream that forwards that header. An inbound W3C `traceparent` or B3 (`b3` / `x-b3-traceid`) header is parsed and its trace id stored in `request_logs.trace_id`, so gateway logs join the caller's distributed trace instead of starting a disconnected one.
- **Outbound propagation**: when the caller sent trace context, it is forwarded verbatim to the chosen upstream (`traceparent`, `tracestate`, and the `b3` / `x-b3-*` family) so vLLM/SGLang/TGI continue the same trace. An untraced request adds nothing to the upstream wire — this is the caller's own context, not a rolter fingerprint, so it preserves wire transparency.
- Writes are **async and batched off the hot path** so logging never adds request latency.
- The dashboard queries ClickHouse for usage, spend, latency percentiles and error rates, sliced by org/team/project/key/model.
- `by-model` groups by `(org_id, model)` and returns `org_id` (#3486): since #1857 two orgs may each have a route of one name, and grouped by the bare name a multi-org caller read them as one row summed across tenants. The Dashboard keys its per-model maps (`p95Of`, `costOf`, the table's `rowKey`) on both and shows `OwnerOrgPill` on a shared name; Pricing's unpriced list and the capture-filter picker fold the rows back by name, since a price and a capture filter are per name. `tests/analytics_scoping.rs` (`by_model_keeps_two_orgs_same_named_routes_as_two_rows`) holds it against a real ClickHouse, a member of one org included.
- The dashboard's summary, timeseries and by-model reads take the invocation list's `model`, `key`, `business_unit` and `customer` filters (#2453): one `DashboardQuery` type and one `analytics::ROW_FILTERS` predicate, a unit test holding it word for word inside `invocations_sql`. Every value is a bound parameter, and the predicate is `and`-ed beside `ROW_VISIBLE`, so a filter narrows the caller's rows and cannot reach past them; `tests/analytics_scoping.rs` checks that against a real ClickHouse. The `dashboard` saved-view surface allow-lists the same four keys.
- **Who reads it** is decided per row: every analytics and health query binds the caller's tenancy, and captured bodies are masked below the `request_payload` floor (member, or viewer on a project that allows it). See [Who reads the request log](security.md#who-reads-the-request-log-1820).

### Optional `request_logs` columns (#2903)

ClickHouse only runs the files under `clickhouse/` when its data directory is first created, so on an existing deployment each one is applied by hand, possibly after the control plane that reads its column has been upgraded. A select that names a column the table lacks fails the whole statement, and for the invocation list that was the LLM Logs screen: `015` and `016` each made a control plane newer than the file unable to list a single request, and `017` was left unread for a release to avoid a third (#2901).

`crates/rolter-control/src/request_log_columns.rs` replaces that precondition with a list. `OPTIONAL_COLUMNS` names each column a hand-applied file adds (`upstream_status`, `attempts`, `lifecycle_operation`, `cache_write_1h_tokens`) with the typed SQL default a row older than the column reads as (`toUInt16(0)`, `toUInt8(0)`, `''`, `toUInt32(0)`). `invocations_sql` takes a `Columns` and writes each of them through `Columns::select(name)`: the column itself when the table has it, `<default> as <name>` when it does not, so the row keeps its shape and the JSON types do not depend on whether the file ran. A test builds the select with every column present and every column absent and checks each list entry in both forms, so a new entry that the select does not name fails there.

- **The probe.** `ClickHouseClient::request_log_columns` runs `select name from system.columns where database = currentDatabase() and table = 'request_logs'`. `currentDatabase()` is where an unqualified `from request_logs` resolves, so the answer is about the table the list reads. The names are fixed text, nothing caller-supplied reaches it.
- **The cache.** The answer lives in an `Arc<ColumnCache>` on the client, so every clone shares it. An answer in which every optional column was found is kept ten minutes (only a recreated table could change it), one in which a column is missing is kept a minute, so a file applied by hand shows within a minute and needs no restart. The cache is a `parking_lot` mutex around a few names, never held across an await; two requests that miss together probe twice, which is cheaper than coordinating them. `ColumnCache::fresh` and `store` take the instant as an argument, so the lifetimes are tested without sleeping.
- **What it does not guess.** A probe that cannot say reads as "every column is there", the behaviour before the list existed: a user with no grant on `system.columns`, or a table the probe lists no column for, must not be read as a table without the columns, which would blank data that is there. That answer is kept for the short lifetime so a refusing server costs one failed read a minute, not one per page. A probe that gets no answer at all (refused or timed-out connection) is the request's error, not a default, because the select would fail on the same connection, and it is not cached.
- **When it is asked.** After the query string is validated, so a `400` costs ClickHouse nothing, and once per window rather than per request. A list that fails forgets the answer (`forget_request_log_columns`), so a column dropped or a table recreated under a cached answer is looked at again on the next request.
- **On screen.** LLM Logs reads the share as `InvocationRow.cache_write_1h_tokens?: number | string` (`ui/src/lib/api/logs.ts`; ClickHouse's `FORMAT JSON` quotes only 64-bit integers, so the `UInt32` arrives as a number and the field is a number on a table without the column too). `ui/src/pages/Logs.tsx` shows **Cache write by lifetime** in the row detail when it is above zero, the 5 minute share being `cache_write_tokens - cache_write_1h_tokens` (#2902).
- **Adding a column.** Add it to `OPTIONAL_COLUMNS` with the default a row older than the file reads as, name it in `invocations_sql` through `columns.select`, and the upgrade note for its file is already written: `docs/user-docs/deployment/upgrading.mdx` says no file is a condition of upgrading the control plane. A column that every deployment has had for a long time (`log_id`, `unpriced`, the attribution ids) stays out of the list and stays required.
- **Tests.** `request_log_columns.rs` (the list, the defaults, the cache lifetimes, a table the probe cannot see); `optional_column_tests` in `analytics.rs` (a stand-in ClickHouse that fails a select naming a missing column, as ClickHouse does: a table with every column, one with none, one with only `017` missing, one asked once for many pages, a file applied between two pages, a refused probe, a failed list, a refused request, an unreachable server); and `crates/rolter-control/tests/invocations_optional_columns.rs`, which builds a database per test on a real ClickHouse migrated through `014`, `015`, `016` or `017` and reads the list through the control plane (`ROLTER_TEST_CLICKHOUSE_URL`, self-skipping without it).

### Subject-scoped erasure (#1085)

`rolter-control/src/log_erasure.rs` deletes one subject's `request_logs`, `request_payloads` and `mcp_tool_call_logs` rows on request. ClickHouse mutations are asynchronous and `alter table … delete` accepts no query parameters, so the module (a) builds the predicate from validated uuids and an escaped request id only, (b) appends `and '<erasure id>' != ''` to every statement so the mutation can be found again in `system.mutations`, whose `command` keeps the text, and (c) reports `completed` only when every submitted mutation is done. A mutation's subquery runs when the mutation does, so payloads are matched by their own `virtual_key_id` / `org_id` columns (`clickhouse/020`) rather than by a subquery over rows the same erasure deletes; rows from before the column are resolved to explicit `log_id`s at submission, payload mutation first. The record is `log_erasures` in Postgres and the action is audited there; nothing in ClickHouse holds either, so the erasure cannot erase its own trace. The MCP table (#3278) carries `org_id`, `virtual_key_id`, `user_id` and `request_id` itself, so it has its own predicate (`Predicates::mcp`) and needs no lookup; its `user_id` is the key owner, so the user scope is `user_id = … or virtual_key_id in (…)`. A `request` scope's `ts` matches the MCP row inside a five minute window (the row is stamped when the call was made), and a `user` scope with no keys leaves the request statements as `1 = 0` and reaches only the MCP rows by `user_id`. `include_logs: false` becomes an `alter table … update arguments = '', result = ''` on that table, since its bodies share a row with its metadata. The audit log and the response cache are out of scope. `tests/log_erasure_clickhouse.rs` (#3280) runs the flow against a real ClickHouse (`ROLTER_TEST_CLICKHOUSE_URL`): two organizations' logs and payloads, including payload rows from before `clickhouse/020`, an erased key and an erased request, polled to `completed`, with the marker found in `system.mutations` and only the intended rows gone.

The dashboard half (#3279) is Observability › Log erasure (`ui/src/pages/LogErasure.tsx`: a form for the key, user and organization scopes with a reason, and the recent erasures) and an **Erase captured payload** action in the LLM Logs detail (`ErasePayloadAction` in `ui/src/components/LogErasure.tsx`: a `request` scope with `include_logs: false`, the row's own `ts` normalized to UTC by `erasureTimestamp`). Both are `log_erasure:*`, superadmin only. The form's subject is picked by name (#3385): `GET /api/v1/log-erasures/subjects` (`find_subjects`, gated as the erasure is) returns up to 500 named keys (with `org / team / project`), users (email, display name) or organizations of a `kind`, narrowed by `q` or to one `id`, and `useSubjects` in `LogErasure.tsx` lists them in a `Combobox` with `allowCustom`, so a pasted id stays usable, is named by an `id` lookup when the list does not show it, and is flagged when nothing has it. The confirmation prints name, detail and id. A submission is shown as `pending`, never as erased: `ErasureStatus` re-reads `GET /api/v1/log-erasures/{id}` through a react-query `refetchInterval` (`erasureRefetchInterval`, the convention `use-settings-pickup.ts` and `mcp-discovery.ts` follow) until the record leaves `pending`, since only that read asks ClickHouse; the list endpoint does not.

### ClickHouse call timeouts (#1951)

Every ClickHouse call the control plane makes goes through one `reqwest` client built in `crates/rolter-control/src/analytics.rs`, with a 3s connect timeout and a 15s whole-request timeout. That covers the analytics, health, MCP-log and `/api/v1/me/usage` reads, the alert signal reads, and the MCP and UX-event ingest inserts. Without a bound, a ClickHouse that accepted the connection and stopped answering held the dashboard request (or the alert pass) open indefinitely. The gateway's client has the same connect bound (#2373).

- **Why 15s.** Above what an interactive read over the indexed window needs, short enough that the operator sees an error rather than a spinner, and under the 30s `QUERY_TIMEOUT` the alert evaluator keeps as an outer backstop. The log-retention `alter table` statements run under their own 60s bound, since an admin triggers them and they are not an interactive read.
- **How it surfaces.** A read that times out answers `504` with the `analytics_query_timeout` code (#2475), so the dashboard can say the query was slow rather than that ClickHouse is down; any other failure answers `502` with the curated `analytics query failed` message and `analytics_query_failed` code, never driver text. The timeout is detected by walking the `anyhow` chain for a `reqwest::Error` with `is_timeout()`, and the OpenAPI document lists both rows on every ClickHouse-backed read. An ingest insert goes through `ingest_failure::insert_failed`, the same path as any other insert failure. The alert evaluator records the rule as `error` with `last_error = "analytics query timed out"` and keeps the last value it read.
- **No credentials in logs.** Transport errors drop their URL (`reqwest::Error::without_url`) before they are logged or stored, because `CLICKHOUSE_URL` may carry userinfo.
- **Idle connections are retired after 2s (#1940).** ClickHouse closes an idle keep-alive connection after its `keep_alive_timeout` (10s by default, 3s on older releases), and reqwest's default pool kept handing such a connection out for 90s. A request written onto a socket at the instant the server closes it gets no response at all (hyper's `IncompleteMessage`), so an insert after a pause of about ten seconds intermittently became a `500` for a batch ClickHouse would have accepted. The pool now retires a connection after 2s idle, below both server defaults, so a reused connection is always one the server still holds open; a request after a longer pause opens a new connection. The insert is not retried instead, since a request that may have reached the server cannot be told apart from one that did not, and `ui_events` has no deduplication. A deployment that sets ClickHouse's `keep_alive_timeout` below 2s reopens the race.

- **The gateway's writers retire idle connections after 2s too (#2696).** The request-log, health-event and MCP tool-call writers share one client from `crates/rolter-gateway/src/clickhouse_client.rs`, and its pool had the same 90s default, so a flush after about ten idle seconds could land on a connection ClickHouse was closing, get no response, and be dropped and counted in the `*_dropped_total` metrics. It now carries the same 2s `POOL_IDLE_TIMEOUT` with the same compile-time check that it stays under 3s, proven by a fake server that drops any second request on a connection. A deployment that sets ClickHouse's `keep_alive_timeout` below 2s reopens the race.

### Timestamps on the read API (#2255)

`ClickHouse::query` sends `default_format=JSON` and `date_time_output_format=iso` on every read. Under ClickHouse's default (`simple`) a `DateTime64(3)` such as `ts`, `max(ts) as last_event` or `toStartOfHour(ts) as bucket` leaves as `2026-08-06 13:00:00.123`: the wall clock of the server's own zone, with no zone. The gateway writes `ts` as a UTC instant (`clickhouse_ts` in `logging.rs`), and the dashboard parses the string with `new Date(value)`, which reads a zone-less date-time as browser-local time, so every time on LLM Logs, MCP Logs, the Dashboard charts and Health was shifted by the difference between the viewer's zone and the server's.

With `iso`, every timestamp leaves as the UTC instant with a `Z`, whatever zone the server runs in. Measured against `clickhouse/clickhouse-server:24-alpine` with `TZ=Europe/Moscow`, a row written as `2026-08-06T10:00:00.123Z`:

| setting  | `ts`                       | `toStartOfHour(ts)`    |
| -------- | -------------------------- | ---------------------- |
| `simple` | `2026-08-06 13:00:00.123`  | `2026-08-06 13:00:00`  |
| `iso`    | `2026-08-06T10:00:00.123Z` | `2026-08-06T10:00:00Z` |

Milliseconds are kept (a `DateTime64(0)` or `DateTime` drops them, as before), and `session_timezone` does not change the output. Nothing on the server parses these strings: rows are passed through as JSON values, and the keyset cursor (`ts|id`) is fed back to `parseDateTime64BestEffortOrZero`, which reads the `Z` as UTC. Bucket _boundaries_ are a separate matter, handled below. The dashboard's Health screen reads a zone-less bucket as UTC for the "SLA at risk" comparison and accepts either form. `reads_ask_clickhouse_for_iso_timestamps_with_their_zone` in `analytics.rs` pins the setting.

#### Bucket boundaries and zone-less bounds in UTC (#2984)

`iso` fixes how an instant is printed, not where a bucket starts: `toStartOfDay(ts)` computes the boundary in the ClickHouse server's own zone, so a day started at `21:00Z` on a `Europe/Moscow` server and a month rolled over three hours early. The compose file and chart run ClickHouse on UTC, so this only shows on a ClickHouse the operator brought. `bucket_expr` in `analytics.rs` (shared by the dashboard timeseries and the health timeline) therefore passes the zone explicitly: `toStartOfHour(ts, 'UTC')`, `toStartOfDay(ts, 'UTC')`, `toStartOfWeek(ts, 0, 'UTC')` (mode 0, Sunday, as before) and `toStartOfMonth(ts, 'UTC')`.

The same default hit `WHERE_WINDOW`: `parseDateTime64BestEffortOrZero('2026-08-06')` read a zone-less `since`/`until` as midnight in the server's zone (`2026-08-05T21:00Z` on Moscow). Both bounds now parse with `, 3, 'UTC'`, so a bare date or zone-less date-time means UTC; a bound with `Z` or an offset keeps its own instant, and the dashboard always sends `toISOString()`. The keyset cursor parse is left alone: its timestamp is the `iso` string the API handed out, and a cursor minted before #2255 carries the server-zone wall clock, which the server's own zone still reads correctly.

The alternative, `session_timezone=UTC` on every read, was rejected: it needs ClickHouse 23.6 or newer (an older server rejects the setting, failing every read rather than bucketing wrongly), and it would also change how the cursor and every other zone-less literal parses. Measured against `clickhouse/clickhouse-server:24-alpine` with `TZ=Europe/Moscow`, for a row written at `2026-08-31T21:30:00.123Z`:

| expression              | before                 | after (`'UTC'`)        |
| ----------------------- | ---------------------- | ---------------------- |
| `toStartOfDay(ts)`      | `2026-08-31T21:00:00Z` | `2026-08-31T00:00:00Z` |
| `toStartOfMonth(ts)`    | `2026-09-01`           | `2026-08-01`           |
| parse of `'2026-08-06'` | `2026-08-05T21:00:00Z` | `2026-08-06T00:00:00Z` |

On a UTC server both columns are identical. `bucket_boundaries_and_window_bounds_are_computed_in_utc` pins the zone in the generated SQL.

### Time bounds on the read API

Every control-plane read over ClickHouse (the five `/api/v1/analytics/*` endpoints, the three `/api/v1/health/*` rollups, `/api/v1/mcp/logs` and its summary, and `/api/v1/me/usage`) takes a caller-supplied `since`/`until`, and the two keyset-paged lists also take a cursor whose first half is a timestamp. All of them are bound as ClickHouse parameters and parsed in SQL with `parseDateTime64BestEffortOrZero`. The `OrZero` variant is required: ClickHouse constant-folds both branches of the `if` that picks the default window, so a strict parse of an absent (empty) bound aborts the query (#1177). Its side effect is that anything the parser cannot read becomes `1970-01-01`, so before #1192 a typo in `since` silently scanned the whole table.

`crates/rolter-control/src/time_bounds.rs` closes that gap before any SQL is built:

- `is_time_bound` accepts one grammar: `YYYY-MM-DD`, optionally followed by `T` or a space and `hh:mm[:ss[.f{1,9}]]`, optionally followed by `Z` or `±hh:mm`. That is RFC 3339 as the dashboard writes it (`toISOString()`), the `YYYY-MM-DD hh:mm:ss.sss` form ClickHouse returns for a `DateTime64(3)` under its default `date_time_output_format=simple` (a cursor minted before #2255 carries it), and the `YYYY-MM-DDThh:mm:ss.sssZ` form the read API now returns (so every cursor it hands out), and a bare date.
- It is narrower than RFC 3339 exactly where ClickHouse misreads it. Measured against ClickHouse 24.10, each of these parses to the epoch: a lowercase `t` or `z`, year `0000`, a day the calendar lacks (`2026-02-30`), and an offset whose `+` a client left unencoded, which the query string decodes to a space. Years outside `DateTime64`'s `1900`-`2299` range are accepted because ClickHouse clamps them to that range instead.
- The analytics, health, MCP-log and self-service modules import `time_bounds::Query` in place of axum's `Query`. It deserializes the same way, then checks `since` and `until` through the `TimeBounds` trait every windowed query type implements, so a new query type that never names its window does not compile. An empty bound still means "the default".
- `analytics::parse_keyset_cursor` applies the same grammar to the cursor's timestamp half, for both the invocation list and the MCP call log.
- A refusal is a `400` in the gateway's OpenAI-style envelope, `{"error": {"message", "type": "invalid_request_error", "param", "code"}}`, with `code` `invalid_time_bound`, `invalid_cursor`, or `invalid_query` for a query string that does not deserialize at all (axum's own rejection is plain text). When the value is a valid bound with its `+` turned into a space, the message says to send `%2B`.
- A bound inside the grammar must also lie between `2000-01-01T00:00:00Z` and one day past now (`EARLIEST_BOUND`, `LATEST_BOUND_AHEAD` in `time_bounds.rs`), or it is `time_bound_out_of_range` (#3506). The day of slack is for a client whose clock runs ahead; the dashboard sends no bound past its own clock, since `windowBounds` clamps a custom range's start to 2000 and to now and omits `until` for a range that has not ended. A bound without a zone is read as UTC for the check, as `WHERE_WINDOW` reads it.
- A query that takes a `bucket` (`TimeBounds::bucket`) is refused with `too_many_buckets` when the bucket would cut the window into more than `MAX_BUCKETS` (20,000) points, counted with the shortest each bucket can be (a month as 28 days). `Query` checks a bucket the caller sent, on the three reads that cut their window into buckets only (`BUCKETED_READS`: `/analytics/timeseries`, its export and `/health/timeline`, the rows `openapi.rs` marks `bucketed()`, held to each other by a test), so a summary sent `bucket=hour` over years still answers; `/analytics/timeseries` (default `day`) and `/health/timeline` (default `hour`) check their own default too. The dashboard stays inside the limit: `chartBucket()` is hourly over at most a month and daily over a custom range, which starts in 2000 at the earliest, so a daily chart reaches the limit only in 2054.

A valid bound is forwarded byte for byte, so it means what it always meant. The router-level tests in `time_bounds.rs` run every windowed route against a stand-in ClickHouse to prove both halves: a malformed bound never reaches the database, and a valid one arrives unchanged. `every_documented_time_bound_is_on_a_checked_route` fails when the served OpenAPI document gains a route with a `since`, `until` or `cursor` that the table there does not list. `/api/v1/me/usage` authenticates a session first, so `self_service_key_lifecycle` in `tests/control/` covers it.

### Savings advisor (#1581)

`GET /api/v1/analytics/savings` (`crates/rolter-control/src/savings.rs`) turns the request log into a model-substitution shortlist. It is read-only and re-prices; it never evaluates quality, and every answer carries that in `limits`.

- **Two queries.** One groups `request_logs` by `(model, target, reason)`, where `model` is the route's public name, `target` the upstream model that served the row, and `reason` a `multiIf` that files each row as `eligible` or the first reason it is not (`unpriced`, `failed`, `cache_hit`, `usage_unknown`, `not_a_model_call`, `uses_tools`, `image_input`, `audio_input`, `structured_output`, `multi_turn`, `no_token_usage`, `prompt_too_large`, `output_too_large`). The rows carry weighted token totals, so the cardinality is bounded by the configured routes and targets rather than by traffic. The other ranks callers (project, virtual key) by cost. Both bind `ROW_VISIBLE` (the #1820 scoping) with `ROW_FILTERS` and an org filter beside it, so the scope parameters only narrow. `attempts`, `lifecycle_operation` and `cache_write_1h_tokens` are read through the optional-column probe.
- **The re-pricing is pure Rust** (`analyse`), over the grouped rows and the live `GatewayConfig` from `ConfigStore::load`: the route table supplies each route's pool (`every_target`, variants included), and `model_prices` converted to the base currency through `StaticRates` supplies the rates, the same table and conversion the gateway prices with. A candidate is any other model in the pool whose price is known and whose optimistic projection is below the recorded cost. A price that cannot be converted reads as unknown. It does not depend on the price catalogue import (#966).
- **The range.** `low` re-prices the same token counts with the same cache reuse; `high` lengthens the output by `output_margin` and takes the larger of a warm and a cold prompt cache, since a provider's cache does not carry over to another model. The cost function mirrors `ModelPriceConfig::cost` on summed (fractional, sampling-weighted) tokens, which can exceed `u32`; the audio split is not in the log, so every token is text.
- **Callers are named by the report.** `name_callers` gives each `spend.by_caller` row `key_name` from one `VirtualKeyRepo::given_names` read over every key the rows hold (#3499), the name only and never the key's prefix, which a role that reads the report but not the keys should not see, so a report across several projects names each caller's key rather than only those the dashboard's one-project key list holds. The rows are already the caller's own traffic, so no name outside it is read; a key that is gone, a non-uuid id, a build without `postgres` or a store that fails reads `null`, and the report is still served. The Savings screen falls back to its key list for a control plane that sends no `key_name`.
- **Every reason is a catalog entry.** The `reason` values `reason_expr` emits (`uses_tools`, `image_input`, `audio_input`, `structured_output`, `multi_turn` among them) are shown on the dashboard's "Left out" table by `EXCLUDED_REASONS` in `ui/src/lib/savings-reasons.ts` and `pages.savings.reasons.<reason>` in every catalog; a reason without an entry is drawn as the raw snake_case name, so a new server reason needs both in the same change. Nothing is left to memory (#3423): `EXCLUSION_REASONS` in `savings.rs` lists every reason, a rust test holds it to what `reason_expr` and `analyse` can emit, it is published as `crates/rolter-control/savings-reasons.json` (`just gen-savings-reasons` rewrites it), and `ui/scripts/savings-reasons.test.ts` fails while the list in `ui/src/lib/savings-reasons.ts` differs from it or a catalog lacks `pages.savings.reasons.<reason>` for one.
- **Unknown prices** are never zero: an unpriced row is excluded and counted, and a pool model without a price is named under `unknown_candidate_prices`.
- **Access** is `SavingsAccess`, the `AnalyticsAccess` resolution floored on the `savings` capability rather than `analytics`, so a custom role can grant the report alone.
- **The request's shape** (#3317). The gateway records five flags on the served row, read off the parsed body in the caller's dialect by `RequestShape::of` (`crates/rolter-gateway/src/request_shape.rs`, one walk of the message list, no allocation): `has_tools` (a non-empty `tools` or `functions`), `has_image_input` (an `image_url`, `input_image` or Anthropic `image` part), `has_audio_input` (`input_audio` or `audio`), `has_response_schema` (`response_format`, Responses' `text.format` or Anthropic's `output_format` of type `json_schema`; a bare `json_object` mode is not a schema) and `turns` (user messages, or user items of a Responses input; a string input is one). They are columns of their own (`clickhouse/031_request_shape.sql`), `#[serde(flatten)]`ed off `RequestLog::request_shape` and each skipped while zero, so an un-migrated ClickHouse keeps accepting rows. The control plane reads them as `OPTIONAL_COLUMNS` (default `0`), so `reason_expr` files a row under `uses_tools`, `image_input`, `audio_input`, `structured_output` or `multi_turn` after the outcome reasons and before the sizes, and a table without the columns reads `0` for each, excluding nothing for them. While the table lacks any of them the report appends `NO_SHAPE_LIMIT` to `limits`, the sentence that used to be permanent; rows written before the columns read `0` and can still be listed. The invocation list returns the five fields too.

## Provider health events

- Every health signal is written to **ClickHouse** (`provider_health_events`): `target_id`, `provider`, `org_id` (the provider's org, empty for a config-file provider), `source`, `outcome`, `status_code`, `latency_ms`, `error_kind`, and `ts`.
- **`ts` is the instant the observation was made**, stamped by the emitter, not by the batch writer: a `passive` event carries the `ts` of the request it was derived from, and a `probe` or `status_page` event carries the moment that poll completed. As with `request_logs`, gateways older than #1210 left this to `default now64(3)` and so recorded the flush time, which collapsed a whole sweep onto one millisecond and skewed the uptime/MTTR buckets below.
- `source` distinguishes where the observation came from: `passive` (real traffic completing through the request funnel), `probe` (active liveness sweeps), and the opt-in `llm_call` / `status_page` sources.
- `outcome` is `ok` / `error` / `timeout`; `error_kind` gives a coarse label (`rate_limited`, `upstream_error`, `connect_error`, `timeout`). An `llm_call` event adds two for a status the free probe would have called healthy: `wrong_endpoint` (`404`/`405`) and `key_refused` (`401`/`403`); see [How the llm-call check is aimed and judged](../deployment/configuration.md#how-the-llm-call-check-is-aimed-and-judged).
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

#### One series per org's provider

Two orgs may each have a provider of one name (#1857, ADR-0040), so the
rollups group by `(org_id, provider, target_id)` and return `org_id`: a caller
who sees both orgs (the admin token, a superadmin) gets two series, not one
merged one (#3441). The key is the provider's stable identity, never its
runtime id, which changes from `openai` to `openai@<org>` once a second org
takes the name. The gateway writes the display name in `provider` and, for a
probe or status-page row, in `target_id` too (`HealthEventSink::stamp_org`);
a probe row written between #1857 and #3441 holds the runtime id in
`target_id`, which `health::events` reads back as `provider` whenever it is
exactly `provider@org_id`. So a provider's series neither splits nor loses its
`provider` grain at the moment its name becomes shared.

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

Two organizations' same-named providers are two cards (#3460): the screen
groups rows by `(org_id, provider)` (`groupByProvider` in
`ui/src/lib/provider-health.ts`), and a card whose name another organization's
card also shows carries an `OwnerOrgPill` naming its owner. The screens that
look a provider's health or a route's breaker up from a route (Models, Routing
Rules) resolve the provider the way the gateway does, the route's own
organization first and a config-file provider after it, never another
organization's (`providerRows` in `ui/src/lib/route-targets.ts`), and match a
breaker on the route's organization as well as its name (`breakerFor`). The
breaker panel groups by the route's organization; a config-file provider's card
takes the breakers of every route with no provider of that name in its own
organization. Rows from a control plane that predates `org_id` match on the
name alone, as before.

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
