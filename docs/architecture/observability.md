# Observability

## Metrics

- The gateway exposes Prometheus metrics at `GET /metrics`: counters (`rolter_requests_total`, `rolter_upstream_errors_total`, `rolter_auth_failures_total`, reload/log/budget/rate-limit/retry/cooldown/health/breaker/scrape counters), a `rolter_config_version` gauge, and per-model **latency histograms** — `rolter_request_latency_ms` (total) and `rolter_request_ttft_ms` (time-to-first-token), each labelled `{model=...}` with the standard `_bucket`/`_sum`/`_count` series. Histograms are observed once per completed request from the log sink, off the response hot path.
- The exporter is hand-rolled (atomic counters + non-cumulative histogram buckets cumulated at render) rather than the `metrics` facade + global recorder, which does not fit the lock-free `arc-swap` design where an explicit `Arc<Metrics>` is threaded through the request path.
- Passive per-target SLA signal: `rolter_target_requests_total{provider,target,outcome}` (a counter, `outcome` = `ok` for 2xx else `error`) is tallied once per completed request from the log sink — free, derived from real traffic, no extra upstream calls. A per-target error rate / uptime is `sum(rate(rolter_target_requests_total{outcome="error"}[5m])) / sum(rate(rolter_target_requests_total[5m]))`. This is the first slice of provider stability tracking (ROL-123); the ClickHouse `provider_health_events` table and the dashboard land in later slices. The active prober is guarded: bounded probe concurrency with per-provider jitter, consecutive-failure/-recovery thresholds gating the unhealthy flip (no single-probe flapping), and exponential probe backoff when a probe itself gets a 429.
- Multi-key providers: `rolter_key_cooldowns_tripped_total` counts api keys parked after a key-level failure (429/401 on a provider with several keys); the request retries in-flight on a sibling key.
- A/B attribution: `rolter_variant_requests_total{model,variant}` (a counter) tallies requests per chosen variant, so traffic splits are visible in Prometheus/Grafana without querying ClickHouse. Classic single-pool routes (no variant) emit nothing. Observed from the same log-sink funnel (ROL-195, part of ROL-188).
- Roadmap: add per-provider/route labels on the histograms, in-flight gauges, cache-hit ratio, and circuit-breaker state gauges.
- Roadmap: **scrape/federate upstream engine metrics** from vLLM/SGLang/TGI `/metrics` and correlate them per target (queue depth, KV-cache usage, running/waiting requests) to feed load- and cache-aware routing and the dashboard.

## Tracing & context propagation

- `tracing` + `tracing-subscriber` with `RUST_LOG` filtering; `TraceLayer` logs each HTTP request.
- **Inbound**: accept W3C `traceparent`/`tracestate` (and `b3`) from clients and continue the trace; honor `x-request-id` / `x-correlation-id`.
- **Outbound to engines**: inject the active trace context into upstream requests so vLLM/SGLang/TGI spans join the **same** distributed trace. vLLM and SGLang support OpenTelemetry tracing (e.g. vLLM `--otlp-traces-endpoint`); point them at the same OTLP collector so engine prefill/decode spans line up with rolter's request span.
- A per-request `request_id` is echoed in a response header and stamped on logs, metric exemplars and spans for correlation.

## Exporters (OTel-compatible)

rolter emits traces and metrics via **OpenTelemetry OTLP** (gRPC/HTTP), so any OTel-compatible backend works without code changes — just set an endpoint and headers:

- **SigNoz**, **Grafana Tempo/Mimir**, **Honeycomb**, **Datadog** (OTLP intake or the OTel Collector `datadog` exporter).
- **Langfuse** for LLM-specific observability (prompt/response, token usage and cost as traces), ingested via its OTLP endpoint or SDK.

Recommended topology: rolter → **OpenTelemetry Collector** → fan-out to the chosen backends. The collector also scrapes the upstream engines' `/metrics` and rolter's `/metrics`, keeping vendor specifics out of rolter. Configure via env, e.g. `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME=rolter-gateway`.

## Request & cost logs

- Every proxied request is logged to **ClickHouse** (`request_logs`): identifiers, model, provider/target, status, token counts, `cost_usd`, latency, TTFT, cache flag, error.
- Writes are **async and batched off the hot path** so logging never adds request latency.
- The dashboard queries ClickHouse for usage, spend, latency percentiles and error rates, sliced by org/team/project/key/model.

## Provider health events

- Every health signal is written to **ClickHouse** (`provider_health_events`): `target_id`, `provider`, `source`, `outcome`, `status_code`, `latency_ms`, `error_kind`, timestamped by ClickHouse on insert.
- `source` distinguishes where the observation came from: `passive` (real traffic completing through the request funnel), `probe` (active liveness sweeps), and the opt-in `llm_call` / `status_page` sources.
- `outcome` is `ok` / `error` / `timeout`; `error_kind` gives a coarse label (`rate_limited`, `upstream_error`, `connect_error`, `timeout`).
- Writes reuse the same **async, batched, off-hot-path** writer and ClickHouse endpoint as `request_logs`; when no `clickhouse_url` is configured the sink is a no-op.
- Counters `rolter_health_events_written_total` and `rolter_health_events_dropped_total` track the writer, mirroring the request-log counters.
- This event stream feeds uptime %/MTTR rollups and the dashboard health panel.

## Health

- `GET /healthz` on both binaries for liveness/readiness probes.
