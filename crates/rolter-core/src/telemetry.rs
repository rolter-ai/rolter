use std::sync::atomic::{AtomicBool, Ordering};

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Set once an OTLP pipeline is installed by [`init`].
///
/// Every propagation entry point checks this first so a deployment with no
/// `OTEL_*` environment never builds a carrier, never touches the global
/// propagator and never allocates — the hot path stays exactly as it was before
/// context propagation existed.
static PIPELINE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether a real OTLP pipeline is installed.
///
/// Call this before doing any work to build a [`TraceCarrier`]: with no OTLP
/// endpoint configured there is no context to extract or inject, and the answer
/// is a single relaxed atomic load.
#[inline]
#[must_use]
pub fn is_active() -> bool {
    PIPELINE_ACTIVE.load(Ordering::Relaxed)
}

/// Inbound trace-context headers, normalized for the W3C propagator.
///
/// Built by the caller only when [`is_active`] is true. B3 headers are folded
/// into an equivalent `traceparent` on [`TraceCarrier::finish`], so a single
/// W3C propagator serves both wire formats and no extra propagator crate is
/// needed.
#[derive(Debug, Default, Clone)]
pub struct TraceCarrier {
    entries: Vec<(String, String)>,
}

impl TraceCarrier {
    /// Empty carrier.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one inbound header. Keys are lowercased; blank values are dropped
    /// so a header present but empty cannot shadow a usable one.
    pub fn insert(&mut self, key: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        self.entries
            .push((key.to_ascii_lowercase(), value.to_string()));
    }

    /// Normalize B3 into `traceparent` when the caller sent only B3, and return
    /// the finished carrier.
    #[must_use]
    pub fn finish(mut self) -> Self {
        if self.get("traceparent").is_none() {
            if let Some(tp) = self.b3_as_traceparent() {
                self.entries.push(("traceparent".to_string(), tp));
            }
        }
        self
    }

    /// True when the caller propagated no usable trace context.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Build a `traceparent` out of B3 single or multi headers, when both a
    /// well-formed trace id and span id are present.
    fn b3_as_traceparent(&self) -> Option<String> {
        // B3 single: `traceid-spanid[-sampled[-parentspanid]]`
        let (trace_id, span_id, sampled) = if let Some(b3) = self.get("b3") {
            let mut parts = b3.split('-');
            (
                parts.next().unwrap_or_default().to_string(),
                parts.next().unwrap_or_default().to_string(),
                parts.next().map(str::to_string),
            )
        } else {
            (
                self.get("x-b3-traceid")?.to_string(),
                self.get("x-b3-spanid")?.to_string(),
                self.get("x-b3-sampled").map(str::to_string),
            )
        };

        // a 64-bit B3 trace id is the low half of a 128-bit one
        let trace_id = match trace_id.len() {
            32 if is_hex(&trace_id) => trace_id.to_lowercase(),
            16 if is_hex(&trace_id) => format!("{:0>32}", trace_id.to_lowercase()),
            _ => return None,
        };
        if span_id.len() != 16 || !is_hex(&span_id) {
            return None;
        }
        // `d` (debug) implies sampled; absent defaults to sampled so a caller
        // that omitted the flag is not silently dropped from the trace
        let flags = match sampled.as_deref() {
            Some("0") | Some("false") => "00",
            _ => "01",
        };
        Some(format!("00-{trace_id}-{}-{flags}", span_id.to_lowercase()))
    }
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Environment variable that hard-disables every telemetry export.
pub const TELEMETRY_ENABLED_ENV: &str = "ROLTER_TELEMETRY_ENABLED";

/// Whether telemetry export is permitted at all (#812).
///
/// Returns `false` only when [`TELEMETRY_ENABLED_ENV`] is set to an explicit
/// falsy value. Unset means enabled, so an existing deployment is unaffected —
/// "off" remains the default in practice because nothing is exported until an
/// OTLP endpoint is configured.
///
/// This is a *kill switch*, not a second way to turn telemetry on: it can only
/// subtract. When false, no exporter is built regardless of which `OTEL_*`
/// endpoints are set, which is the point — "off" was previously implicit
/// (achieved by leaving an endpoint unset), so it did not survive somebody
/// setting the endpoint for one signal and there was nothing to point at in a
/// security review.
///
/// Deliberately environment-only rather than a config-file key. `init` installs
/// the subscriber in `main` before any config file is read, so a config-file
/// switch could not gate trace export at all, and honouring it only for the
/// signals initialized later would mean the same key meant different things
/// depending on which signal you asked about. The `OTEL_*` contract this
/// composes with is environment-based for the same reason.
#[must_use]
pub fn export_enabled() -> bool {
    match std::env::var(TELEMETRY_ENABLED_ENV) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Guard returned by [`init`]; holds the OTLP tracer provider (when configured)
/// so spans are flushed to the collector on process exit.
///
/// Bind it to a named local in `main` (`let _telemetry = telemetry::init();`) —
/// binding to `_` would drop it immediately and discard buffered spans.
#[must_use = "bind the guard to a named local so spans flush on exit"]
#[derive(Default)]
pub struct TelemetryGuard {
    #[cfg(feature = "otlp")]
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "otlp")]
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.provider.take() {
            // flush any batched spans before the runtime tears down
            let _ = provider.shutdown();
        }
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

/// Initialize the global tracing subscriber.
///
/// Reads the `RUST_LOG` environment variable for log filtering and falls back to
/// `info`. When the `otlp` feature is enabled (default) and an OTLP endpoint is
/// configured via the standard `OTEL_EXPORTER_OTLP_ENDPOINT` /
/// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` environment variable, spans are also
/// exported to that OpenTelemetry collector; otherwise only the stdout fmt layer
/// is installed and there is no OTLP overhead.
///
/// Safe to call more than once; subsequent calls are ignored.
pub fn init() -> TelemetryGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    #[cfg(feature = "otlp")]
    if let Some(provider) = otlp::try_build_provider() {
        use opentelemetry::trace::TracerProvider as _;
        let tracer = provider.tracer("rolter");
        // logs ride alongside traces rather than replacing stdout (#809). the
        // bridge is `Option`al because a deployment may point traces at a
        // collector and leave logs off; `Option<Layer>` is itself a layer, so
        // the registry below is one expression either way
        let logger_provider = otlp::try_build_logger_provider();
        let logs_layer = logger_provider.as_ref().map(|provider| {
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(provider)
        });
        let _ = tracing_subscriber::registry()
            .with(filter)
            // stdout stays the default and is never replaced: OTLP is additive,
            // so `docker logs` shows exactly what it did before
            .with(fmt::layer())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(logs_layer)
            .try_init();
        // the W3C propagator is what makes an inbound `traceparent` become a
        // real parent and an outbound one carry *this* gateway's span; without
        // it every span is a disconnected root
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        PIPELINE_ACTIVE.store(true, Ordering::Relaxed);
        return TelemetryGuard {
            provider: Some(provider),
            logger_provider,
        };
    }

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .try_init();
    TelemetryGuard::default()
}

/// Bucket boundaries for `gen_ai.server.time_per_output_token`, in seconds.
///
/// Per-token times are two to three orders of magnitude smaller than request
/// durations, so the request-latency boundaries would put every observation in
/// the first bucket and answer nothing.
#[cfg(feature = "otlp")]
const TOKEN_SECONDS_BUCKETS: [f64; 10] =
    [0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0];

/// Bucket boundaries for control-plane operation latency, in milliseconds.
///
/// Deliberately different from the gateway's request boundaries. A gateway
/// request is dominated by an upstream model call and is interesting out to
/// tens of seconds; a snapshot build and a CRUD write are database work and are
/// interesting from a millisecond up, where "is this 2ms or 40ms" is the whole
/// question. Reusing the gateway's boundaries would put nearly every
/// observation in the first bucket.
#[cfg(feature = "otlp")]
const CONTROL_LATENCY_BUCKETS_MS: [f64; 12] = [
    1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0,
];

/// Bucket boundaries for snapshot payload size, in bytes.
///
/// A snapshot grows with the tenant's config, and its size is what every
/// gateway in the fleet transfers on every poll. Spanning 1 KiB to 32 MiB
/// covers a single-route dev deployment through a large multi-tenant one.
#[cfg(feature = "otlp")]
const SNAPSHOT_BYTES_BUCKETS: [f64; 10] = [
    1024.0,
    4096.0,
    16_384.0,
    65_536.0,
    262_144.0,
    1_048_576.0,
    4_194_304.0,
    8_388_608.0,
    16_777_216.0,
    33_554_432.0,
];

/// One scalar metric to export: Prometheus type, name, help text, value.
///
/// Mirrors what the gateway's `Metrics::scalars()` produces, expressed in plain
/// types so `rolter-core` needs no dependency on the gateway.
pub type ScalarMetric = (&'static str, &'static str, &'static str, u64);

/// One series of a labelled counter: `(family name, attributes, value)`.
///
/// Separate from [`ScalarMetric`] because the *set of series* is discovered at
/// runtime — a model, target or variant only exists once traffic has used it —
/// while the family it belongs to is fixed. The attribute values are owned
/// because they come from `DashMap` keys that may change under the collection.
pub type LabelledMetric = (&'static str, Vec<(&'static str, String)>, u64);

/// A labelled counter family: `(name, help)`.
///
/// Declared up front rather than inferred from the first collection, because at
/// install time a gateway has served nothing and every family is legitimately
/// empty. Inferring would silently export nothing until a restart.
pub type LabelledFamily = (&'static str, &'static str);

/// What the gateway hands the exporter. A struct rather than four positional
/// arguments so the call site says which is which.
pub struct MetricsExport<S, L> {
    /// Unlabelled counters and gauges — the same list the Prometheus endpoint renders.
    pub scalars: S,
    /// Fixed set of labelled counter families.
    pub labelled_families: &'static [LabelledFamily],
    /// Every currently-known series across those families.
    pub labelled: L,
    /// Explicit histogram bucket boundaries, in milliseconds.
    ///
    /// Passed in rather than defaulted so the OTLP histogram and the Prometheus
    /// one share boundaries. Two exporters of the same measurement disagreeing
    /// about where the buckets fall is worse than either alone, because the
    /// numbers look comparable and are not.
    pub latency_buckets_ms: Vec<f64>,
}

/// Records per-request latency and time-to-first-token as OTLP histograms.
///
/// Unlike the counters, this cannot be an observable instrument: OTel builds a
/// histogram from individual measurements, and the gateway's pre-bucketed
/// counters cannot be handed over after the fact. So this is the one part of
/// metrics export that touches the request path — and it does so only when an
/// OTLP endpoint is configured.
///
/// A default-constructed value records nothing, which is what every deployment
/// without OTLP holds. Cloning is cheap; the instruments are shared.
#[derive(Clone, Default)]
pub struct RequestHistograms {
    #[cfg(feature = "otlp")]
    inner: Option<std::sync::Arc<HistogramSet>>,
}

#[cfg(feature = "otlp")]
struct HistogramSet {
    latency: opentelemetry::metrics::Histogram<u64>,
    ttft: opentelemetry::metrics::Histogram<u64>,
    /// the OTel GenAI *server* metric family (#808).
    ///
    /// The spec splits client and server metrics, and the server family is the
    /// one almost no SDK instrumentation emits, because almost nothing else
    /// sits where a gateway sits. These are the same measurements the
    /// `rolter_*` histograms carry, in the units and under the names the spec
    /// names, so a conformant backend's built-in GenAI views work.
    server_duration: opentelemetry::metrics::Histogram<f64>,
    server_ttft: opentelemetry::metrics::Histogram<f64>,
    server_tpot: opentelemetry::metrics::Histogram<f64>,
}

impl RequestHistograms {
    /// Record one completed request against the `model` attribute.
    ///
    /// Does nothing, and allocates nothing, when metrics export is off.
    pub fn record(
        &self,
        provider: &str,
        model: &str,
        latency_ms: u32,
        ttft_ms: u32,
        output_tokens: u32,
    ) {
        #[cfg(feature = "otlp")]
        if let Some(inner) = &self.inner {
            // one attribute set for both instruments — `model` is the same
            // bounded label the Prometheus histogram already uses, so this adds
            // no cardinality the deployment was not already carrying.
            // the `to_string` is the one hot-path allocation telemetry costs:
            // OTel's string value is owned or `'static` and a model name is
            // neither. caching a prebuilt attribute set per model would trade it
            // for a locked map lookup on the request path, which is worse (#815)
            let attrs = [opentelemetry::KeyValue::new("model", model.to_string())];
            inner.latency.record(u64::from(latency_ms), &attrs);
            inner.ttft.record(u64::from(ttft_ms), &attrs);

            // the spec names these attributes, and its unit is seconds
            let genai = [
                opentelemetry::KeyValue::new("gen_ai.provider.name", provider.to_string()),
                opentelemetry::KeyValue::new("gen_ai.request.model", model.to_string()),
            ];
            inner
                .server_duration
                .record(f64::from(latency_ms) / 1000.0, &genai);
            inner
                .server_ttft
                .record(f64::from(ttft_ms) / 1000.0, &genai);
            // time *per output token* is only meaningful once there is more
            // than one: the first token's cost is time-to-first-token, already
            // its own metric, and dividing by zero or one would report the
            // whole generation as per-token cost
            if output_tokens > 1 {
                let generation_ms = latency_ms.saturating_sub(ttft_ms);
                inner.server_tpot.record(
                    f64::from(generation_ms) / 1000.0 / f64::from(output_tokens - 1),
                    &genai,
                );
            }
            return;
        }
        let _ = (provider, model, latency_ms, ttft_ms, output_tokens);
    }

    /// Whether anything is actually being recorded.
    #[must_use]
    pub fn is_active(&self) -> bool {
        // a `let` per feature rather than a cfg'd `return`, which clippy reads
        // as a needless return on the branch that is actually compiled
        #[cfg(feature = "otlp")]
        let active = self.inner.is_some();
        #[cfg(not(feature = "otlp"))]
        let active = false;
        active
    }
}

/// Records control-plane operation latency as OTLP histograms (#845).
///
/// The control plane's two jobs are worth measuring for different reasons.
/// Snapshot generation is *fleet-wide config-propagation delay*: every gateway
/// waits on it, so when an operator changes a route and the fleet serves stale
/// config, this is what says whether the delay is in generation or downstream
/// of it. CRUD latency is what makes a slow dashboard write attributable.
///
/// A default-constructed value records nothing, which is what every deployment
/// without OTLP holds. Cloning is cheap; the instruments are shared.
#[derive(Clone, Default)]
pub struct ControlHistograms {
    #[cfg(feature = "otlp")]
    inner: Option<std::sync::Arc<ControlHistogramSet>>,
}

#[cfg(feature = "otlp")]
struct ControlHistogramSet {
    snapshot_duration: opentelemetry::metrics::Histogram<u64>,
    snapshot_bytes: opentelemetry::metrics::Histogram<u64>,
    crud_duration: opentelemetry::metrics::Histogram<u64>,
    pool_acquire: opentelemetry::metrics::Histogram<u64>,
    login_outcome: opentelemetry::metrics::Counter<u64>,
}

impl ControlHistograms {
    /// Record one completed snapshot build.
    ///
    /// `bytes` is the serialized payload every polling gateway transfers, and
    /// `outcome` is a bounded label (`ok`, `not_modified`, `error`) — never the
    /// `config_version`, which is unbounded and would make a new time series on
    /// every config write.
    pub fn record_snapshot(&self, outcome: &'static str, duration_ms: u64, bytes: u64) {
        #[cfg(feature = "otlp")]
        if let Some(inner) = &self.inner {
            let attrs = [opentelemetry::KeyValue::new("outcome", outcome)];
            inner.snapshot_duration.record(duration_ms, &attrs);
            // a 304 transfers no payload; recording a 0 would drag the size
            // distribution toward zero and misreport what the fleet moves
            if bytes > 0 {
                inner.snapshot_bytes.record(bytes, &attrs);
            }
            return;
        }
        let _ = (outcome, duration_ms, bytes);
    }

    /// Record one completed control-plane API call.
    ///
    /// `route` must be the *matched* path template (`/api/v1/providers/{id}`),
    /// never the concrete path: a per-entity-id label would give this metric
    /// unbounded cardinality, which is exactly what #845 rules out.
    pub fn record_crud(&self, route: &str, method: &str, status: u16, duration_ms: u64) {
        #[cfg(feature = "otlp")]
        if let Some(inner) = &self.inner {
            let attrs = [
                opentelemetry::KeyValue::new("http.route", route.to_string()),
                opentelemetry::KeyValue::new("http.request.method", method.to_string()),
                // the class, not the code: 12 statuses across 80 routes is 960
                // series for a question ("are writes failing") that 5 answers
                opentelemetry::KeyValue::new("http.response.status_class", status_class(status)),
            ];
            inner.crud_duration.record(duration_ms, &attrs);
            return;
        }
        let _ = (route, method, status, duration_ms);
    }

    /// Record how long one connection acquire waited on the pool.
    ///
    /// Sampled on a timer rather than instrumented per call: wrapping every
    /// `acquire()` in the control plane would mean touching every repository
    /// method, and the question this answers — "are callers queueing on the
    /// pool ceiling" — is a distribution over time, not a per-request fact.
    /// `outcome` is `ok` or `timeout`, so an exhausted pool is visible as a
    /// count and not only as a tail latency (#1052).
    pub fn record_pool_acquire(&self, outcome: &'static str, duration_ms: u64) {
        #[cfg(feature = "otlp")]
        if let Some(inner) = &self.inner {
            let attrs = [opentelemetry::KeyValue::new("outcome", outcome)];
            inner.pool_acquire.record(duration_ms, &attrs);
            return;
        }
        let _ = (outcome, duration_ms);
    }

    /// Record one resolved login attempt on the control plane.
    ///
    /// A counter rather than a histogram: the question is "is someone running a
    /// credential-stuffing run against this deployment", which is a rate, not a
    /// distribution. `outcome` is a closed set — `success`, `invalid`,
    /// `throttled`, `locked` — and carries no account or address label, since
    /// either would be unbounded cardinality *and* would put the identity an
    /// attacker is guessing into the metrics pipeline (#1079).
    pub fn record_login(&self, outcome: &'static str) {
        #[cfg(feature = "otlp")]
        if let Some(inner) = &self.inner {
            inner
                .login_outcome
                .add(1, &[opentelemetry::KeyValue::new("outcome", outcome)]);
            return;
        }
        let _ = outcome;
    }

    /// Whether anything is actually being recorded.
    #[must_use]
    pub fn is_active(&self) -> bool {
        #[cfg(feature = "otlp")]
        let active = self.inner.is_some();
        #[cfg(not(feature = "otlp"))]
        let active = false;
        active
    }
}

/// Collapse an HTTP status into its class, as a `'static` label.
// gated with its only caller in `record_crud`: without otlp there is no
// attribute set to build, so an ungated helper is dead code (#1399)
#[cfg(feature = "otlp")]
fn status_class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => "other",
    }
}

#[cfg(all(test, feature = "otlp"))]
mod status_class_tests {
    use super::status_class;

    #[test]
    fn every_status_collapses_to_a_bounded_label() {
        assert_eq!(status_class(100), "1xx");
        assert_eq!(status_class(204), "2xx");
        assert_eq!(status_class(304), "3xx");
        assert_eq!(status_class(404), "4xx");
        assert_eq!(status_class(503), "5xx");
        // anything outside the HTTP ranges must still land on one label rather
        // than opening a new time series
        assert_eq!(status_class(0), "other");
        assert_eq!(status_class(999), "other");
    }
}

/// Install the control plane's histograms (#845).
///
/// The control plane has no Prometheus registry to mirror, so unlike
/// [`install_metrics`] there is nothing to export observably — these are
/// measurements taken as they happen. Returns `None`, and costs nothing, when
/// no OTLP endpoint is configured.
#[must_use]
pub fn install_control_metrics() -> Option<MetricsGuard> {
    install_control_metrics_with_pool(None)
}

/// A reading of the database connection pool, sampled at collection time.
///
/// `connections` is every connection the pool holds open and `idle` how many
/// of those are free; `connections - idle` is what is checked out. A pool
/// sitting at `max` with `idle` at zero means callers are queueing on the
/// acquire timeout and the ceiling is the bottleneck (#1052).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolGauges {
    pub connections: u64,
    pub idle: u64,
    pub max: u64,
}

/// Reads the live pool occupancy. Called on the exporter's interval, never on
/// a request path, so it must be cheap and must not query the database.
pub type PoolSampler = std::sync::Arc<dyn Fn() -> PoolGauges + Send + Sync>;

/// [`install_control_metrics`] plus observable gauges for the Postgres pool.
///
/// The pool is one fixed budget shared by `/internal/snapshot`, the dashboard
/// CRUD surface and every RBAC membership lookup. Without these series, "the
/// control plane got slow under fan-out" and "the pool ceiling is too low" look
/// identical from outside, and the operator has nothing to size the ceiling
/// against (#1052).
#[must_use]
pub fn install_control_metrics_with_pool(pool: Option<PoolSampler>) -> Option<MetricsGuard> {
    #[cfg(not(feature = "otlp"))]
    let _ = pool;
    #[cfg(feature = "otlp")]
    {
        let provider = otlp::try_build_meter_provider()?;
        let meter = {
            use opentelemetry::metrics::MeterProvider as _;
            provider.meter("rolter")
        };

        // the control plane degrades for the same reasons the gateway does, and
        // answered none of those questions either
        process::install(&meter);
        runtime::install(&meter);

        // observable, so the SDK reads the pool on its export interval rather
        // than anything on the request path maintaining a counter
        if let Some(sample) = pool {
            for (name, help, pick) in [
                (
                    "rolter_db_pool_connections",
                    "postgres connections the control plane holds open",
                    (|g: PoolGauges| g.connections) as fn(PoolGauges) -> u64,
                ),
                (
                    "rolter_db_pool_idle",
                    "postgres connections currently free in the pool",
                    |g: PoolGauges| g.idle,
                ),
                (
                    "rolter_db_pool_max",
                    "configured ceiling on postgres connections",
                    |g: PoolGauges| g.max,
                ),
            ] {
                let sample = sample.clone();
                meter
                    .u64_observable_gauge(name)
                    .with_description(help)
                    .with_callback(move |obs| obs.observe(pick(sample()), &[]))
                    .build();
            }
        }

        let control = ControlHistograms {
            inner: Some(std::sync::Arc::new(ControlHistogramSet {
                snapshot_duration: meter
                    .u64_histogram("rolter_snapshot_build_ms")
                    .with_description("time to generate one config snapshot, in milliseconds")
                    .with_unit("ms")
                    .with_boundaries(CONTROL_LATENCY_BUCKETS_MS.to_vec())
                    .build(),
                snapshot_bytes: meter
                    .u64_histogram("rolter_snapshot_payload_bytes")
                    .with_description("serialized size of one config snapshot, in bytes")
                    .with_unit("By")
                    .with_boundaries(SNAPSHOT_BYTES_BUCKETS.to_vec())
                    .build(),
                crud_duration: meter
                    .u64_histogram("rolter_control_request_ms")
                    .with_description("control-plane API request latency, in milliseconds")
                    .with_unit("ms")
                    .with_boundaries(CONTROL_LATENCY_BUCKETS_MS.to_vec())
                    .build(),
                pool_acquire: meter
                    .u64_histogram("rolter_db_pool_acquire_ms")
                    .with_description(
                        "time to acquire a postgres connection from the pool, in milliseconds",
                    )
                    .with_unit("ms")
                    .with_boundaries(CONTROL_LATENCY_BUCKETS_MS.to_vec())
                    .build(),
                login_outcome: meter
                    .u64_counter("rolter_control_login_attempts")
                    .with_description(
                        "resolved control-plane login attempts, by outcome \
                         (success, invalid, throttled, locked)",
                    )
                    .build(),
            })),
        };

        Some(MetricsGuard {
            provider: Some(provider),
            histograms: RequestHistograms::default(),
            control,
        })
    }
    #[cfg(not(feature = "otlp"))]
    {
        None
    }
}

/// Guard holding the OTLP meter provider, flushing metrics on drop.
#[must_use = "bind the guard to a named local so metrics flush on exit"]
#[derive(Default)]
pub struct MetricsGuard {
    #[cfg(feature = "otlp")]
    provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    histograms: RequestHistograms,
    control: ControlHistograms,
}

impl MetricsGuard {
    /// The histogram recorder to hand the request path. Inert when export is off.
    pub fn histograms(&self) -> RequestHistograms {
        self.histograms.clone()
    }

    /// The control plane's histogram recorder. Inert when export is off, and
    /// on a guard built by [`install_metrics`] rather than
    /// [`install_control_metrics`].
    pub fn control_histograms(&self) -> ControlHistograms {
        self.control.clone()
    }
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otlp")]
        if let Some(provider) = self.provider.take() {
            let _ = provider.shutdown();
        }
    }
}

/// Export the gateway's existing counters and gauges over OTLP (#805).
///
/// A *second exporter over the same numbers*, not a rewrite: `collect` hands
/// back whatever the Prometheus endpoint would render, and both stay in step
/// because they read one list. The Prometheus endpoint is untouched.
///
/// Instruments are observable — nothing is pushed on the request path. The SDK
/// invokes `collect` on its own periodic interval (`OTEL_METRIC_EXPORT_INTERVAL`,
/// default 60s), so the hot path keeps doing nothing but `fetch_add` on an
/// atomic.
///
/// Returns `None` when no OTLP endpoint is configured, leaving the untraced
/// deployment exactly as it was.
pub fn install_metrics<S, L>(export: MetricsExport<S, L>) -> Option<MetricsGuard>
where
    S: Fn() -> Vec<ScalarMetric> + Send + Sync + Clone + 'static,
    L: Fn() -> Vec<LabelledMetric> + Send + Sync + Clone + 'static,
{
    #[cfg(feature = "otlp")]
    {
        let MetricsExport {
            scalars: collect,
            labelled_families,
            labelled,
            latency_buckets_ms,
        } = export;
        let provider = otlp::try_build_meter_provider()?;
        let meter = {
            use opentelemetry::metrics::MeterProvider as _;
            provider.meter("rolter")
        };

        // the instrument set is fixed at install time from one snapshot; each
        // instrument then re-reads its own value by name on every collection.
        // that by-name match is off the request path — these are observable
        // instruments, so the SDK drives them on its export interval while the
        // hot path only does `fetch_add` on a named atomic. audited against
        // OTel's don't-wrap guidance in #815; see docs/architecture/observability.md
        for (kind, name, help, _) in collect() {
            let pick = collect.clone();
            let observe = move |value: &dyn Fn(u64)| {
                if let Some((_, _, _, v)) = pick().into_iter().find(|(_, n, _, _)| *n == name) {
                    value(v);
                }
            };
            match kind {
                // a gauge can go down; a counter is monotonic. exporting a
                // counter as a gauge would break rate() on the backend
                "gauge" => {
                    let o = observe.clone();
                    meter
                        .u64_observable_gauge(name)
                        .with_description(help)
                        .with_callback(move |obs| o(&|v| obs.observe(v, &[])))
                        .build();
                }
                _ => {
                    let o = observe.clone();
                    meter
                        .u64_observable_counter(name)
                        .with_description(help)
                        .with_callback(move |obs| o(&|v| obs.observe(v, &[])))
                        .build();
                }
            }
        }

        // one observable counter per family; the callback emits every series
        // that family currently holds. registering per-series instead would
        // freeze the set at install time, when the gateway has served nothing
        for (family, help) in labelled_families {
            let pick = labelled.clone();
            meter
                .u64_observable_counter(*family)
                .with_description(*help)
                .with_callback(move |obs| {
                    for (name, attrs, value) in pick() {
                        if name != *family {
                            continue;
                        }
                        let kv: Vec<opentelemetry::KeyValue> = attrs
                            .into_iter()
                            .map(|(k, v)| opentelemetry::KeyValue::new(k, v))
                            .collect();
                        obs.observe(value, &kv);
                    }
                })
                .build();
        }

        // process-level metrics: when a node degrades the first questions are
        // memory, cpu and file descriptors, and until #809 the gateway answered
        // none of them. observable, so nothing touches the request path
        process::install(&meter);

        // scheduler-level metrics: they separate "the provider is slow" from
        // "we were saturated before we even called the provider", which the
        // domain counters cannot distinguish (#834)
        runtime::install(&meter);

        // the GenAI server family is specified in seconds, so the shared
        // millisecond boundaries are converted rather than redefined — two
        // views of one measurement must not disagree about where a bucket falls
        let seconds_buckets: Vec<f64> = latency_buckets_ms.iter().map(|ms| ms / 1000.0).collect();

        // histograms are the one instrument that cannot be observable: OTel
        // builds them from individual measurements, so the gateway's
        // pre-bucketed counters cannot be handed over after the fact
        let histograms = RequestHistograms {
            inner: Some(std::sync::Arc::new(HistogramSet {
                latency: meter
                    .u64_histogram("rolter_request_latency_ms")
                    .with_description("total request latency in milliseconds")
                    .with_unit("ms")
                    .with_boundaries(latency_buckets_ms.clone())
                    .build(),
                ttft: meter
                    .u64_histogram("rolter_request_ttft_ms")
                    .with_description("time to first token in milliseconds")
                    .with_unit("ms")
                    .with_boundaries(latency_buckets_ms)
                    .build(),
                server_duration: meter
                    .f64_histogram("gen_ai.server.request.duration")
                    .with_description("generative ai server request duration")
                    .with_unit("s")
                    .with_boundaries(seconds_buckets.clone())
                    .build(),
                server_ttft: meter
                    .f64_histogram("gen_ai.server.time_to_first_token")
                    .with_description("time to generate the first token")
                    .with_unit("s")
                    .with_boundaries(seconds_buckets.clone())
                    .build(),
                server_tpot: meter
                    .f64_histogram("gen_ai.server.time_per_output_token")
                    .with_description("time per output token after the first")
                    .with_unit("s")
                    .with_boundaries(TOKEN_SECONDS_BUCKETS.to_vec())
                    .build(),
            })),
        };

        Some(MetricsGuard {
            provider: Some(provider),
            histograms,
            control: ControlHistograms::default(),
        })
    }
    #[cfg(not(feature = "otlp"))]
    {
        let _ = export;
        None
    }
}

/// Build a span for one pipeline stage, or a disabled span when no OTLP
/// pipeline is installed (#805).
///
/// Stage spans are what make a slow request attributable to `auth`,
/// `route.select`, `queue.wait` or `upstream.request` rather than merely slow.
/// They are worth nothing to a deployment that exports no traces, so this
/// creates none there: [`tracing::Span::none`] allocates nothing, and
/// instrumenting a future with it is a no-op — the untraced hot path stays as it
/// was.
///
/// Takes the same arguments as [`tracing::info_span!`]. Hold the returned span
/// for the length of the stage and drop it at the end; do not call `.entered()`
/// on it inside an async fn, as the resulting guard is `!Send` and would make
/// the whole future `!Send`. Use `.instrument(span)` to cover an `await`.
#[macro_export]
macro_rules! stage_span {
    ($name:literal $(, $($field:tt)*)?) => {
        if $crate::telemetry::is_active() {
            tracing::info_span!($name $(, $($field)*)?)
        } else {
            tracing::Span::none()
        }
    };
}

/// Adopt the caller's inbound trace context as the parent of `span`.
///
/// Turns the gateway's request span from a disconnected root into a child of
/// whatever called it, so a dashboard click and the provider request it caused
/// land in one trace. A no-op when no pipeline is installed, when the carrier is
/// empty, or when the context it carries is malformed.
pub fn adopt_inbound(span: &tracing::Span, carrier: &TraceCarrier) {
    #[cfg(feature = "otlp")]
    {
        if !is_active() || carrier.is_empty() {
            return;
        }
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let parent = opentelemetry::global::get_text_map_propagator(|p| p.extract(carrier));
        // a span with no parent is better than a failed request: a malformed
        // inbound context is dropped, never propagated
        let _ = span.set_parent(parent);
    }
    #[cfg(not(feature = "otlp"))]
    {
        let _ = (span, carrier);
    }
}

/// Inject the *current* span's trace context as outbound headers.
///
/// This is what makes the upstream provider call a child of the gateway span
/// rather than a sibling: the emitted `traceparent` names this gateway's span as
/// the parent, not the caller's. Returns an empty vec when no pipeline is
/// installed or the current span has no valid context, so an untraced
/// deployment puts nothing extra on the wire.
#[must_use]
pub fn inject_current() -> Vec<(String, String)> {
    #[cfg(feature = "otlp")]
    {
        if !is_active() {
            return Vec::new();
        }
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let cx = tracing::Span::current().context();
        let mut sink = HeaderSink::default();
        opentelemetry::global::get_text_map_propagator(|p| p.inject_context(&cx, &mut sink));
        sink.0
    }
    #[cfg(not(feature = "otlp"))]
    {
        Vec::new()
    }
}

/// Trace id of the current span as lowercase hex, or `None` when there is no
/// valid context.
///
/// Preferred over re-parsing the inbound header: once a parent is adopted this
/// is the same id the exported spans carry, so the ClickHouse request log and
/// the trace backend agree by construction rather than by coincidence.
#[must_use]
pub fn current_trace_id() -> Option<String> {
    #[cfg(feature = "otlp")]
    {
        if !is_active() {
            return None;
        }
        use opentelemetry::trace::TraceContextExt as _;
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        let cx = tracing::Span::current().context();
        let span_cx = cx.span().span_context().clone();
        span_cx
            .is_valid()
            .then(|| span_cx.trace_id().to_string().to_lowercase())
    }
    #[cfg(not(feature = "otlp"))]
    {
        None
    }
}

#[cfg(feature = "otlp")]
impl opentelemetry::propagation::Extractor for TraceCarrier {
    fn get(&self, key: &str) -> Option<&str> {
        TraceCarrier::get(self, key)
    }

    fn keys(&self) -> Vec<&str> {
        self.entries.iter().map(|(k, _)| k.as_str()).collect()
    }
}

/// Collects the headers a propagator injects.
#[cfg(feature = "otlp")]
#[derive(Default)]
struct HeaderSink(Vec<(String, String)>);

#[cfg(feature = "otlp")]
impl opentelemetry::propagation::Injector for HeaderSink {
    fn set(&mut self, key: &str, value: String) {
        self.0.push((key.to_string(), value));
    }
}

#[cfg(test)]
mod kill_switch_tests {
    use super::*;

    /// `export_enabled` reads process-global env, so the cases that set it must
    /// not interleave with each other or with the pipeline tests.
    fn with_env(value: Option<&str>, f: impl FnOnce()) {
        let _guard = super::pipeline_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match value {
            Some(v) => std::env::set_var(TELEMETRY_ENABLED_ENV, v),
            None => std::env::remove_var(TELEMETRY_ENABLED_ENV),
        }
        f();
        std::env::remove_var(TELEMETRY_ENABLED_ENV);
    }

    #[test]
    fn unset_means_enabled_so_an_existing_deployment_is_unaffected() {
        with_env(None, || assert!(export_enabled()));
    }

    #[test]
    fn every_falsy_spelling_disables_export() {
        for value in ["0", "false", "FALSE", "no", "off", " Off "] {
            with_env(Some(value), || {
                assert!(!export_enabled(), "{value} should disable export");
            });
        }
    }

    #[test]
    fn truthy_and_unrecognized_values_leave_export_on() {
        // the switch can only subtract: it is not a second way to turn export
        // on, and an unparsable value must not silently blind a deployment
        for value in ["1", "true", "yes", "on", "maybe", ""] {
            with_env(Some(value), || {
                assert!(export_enabled(), "{value} should leave export on");
            });
        }
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn the_switch_beats_a_configured_endpoint() {
        // the whole point of #812: "off" must not depend on remembering to
        // leave every per-signal endpoint unset
        let _guard = super::pipeline_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4317");
        std::env::set_var(TELEMETRY_ENABLED_ENV, "false");

        assert!(super::otlp::try_build_provider().is_none());
        assert!(super::otlp::try_build_meter_provider().is_none());

        std::env::remove_var(TELEMETRY_ENABLED_ENV);
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }
}

#[cfg(test)]
mod carrier_tests {
    use super::*;

    fn carrier(pairs: &[(&str, &str)]) -> TraceCarrier {
        let mut c = TraceCarrier::new();
        for (k, v) in pairs {
            c.insert(k, v);
        }
        c.finish()
    }

    #[test]
    fn w3c_traceparent_is_kept_as_sent() {
        let c = carrier(&[(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )]);
        assert_eq!(
            c.get("traceparent"),
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        );
    }

    #[test]
    fn b3_single_becomes_a_traceparent() {
        let c = carrier(&[("b3", "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1")]);
        assert_eq!(
            c.get("traceparent"),
            Some("00-80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-01")
        );
    }

    #[test]
    fn b3_multi_becomes_a_traceparent() {
        let c = carrier(&[
            ("X-B3-TraceId", "80f198ee56343ba864fe8b2a57d3eff7"),
            ("X-B3-SpanId", "e457b5a2e4d86bd1"),
            ("X-B3-Sampled", "0"),
        ]);
        assert_eq!(
            c.get("traceparent"),
            Some("00-80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-00")
        );
    }

    #[test]
    fn a_64_bit_b3_trace_id_is_left_padded_to_128_bits() {
        let c = carrier(&[("b3", "a3ce929d0e0e4736-e457b5a2e4d86bd1-1")]);
        assert_eq!(
            c.get("traceparent"),
            Some("00-0000000000000000a3ce929d0e0e4736-e457b5a2e4d86bd1-01")
        );
    }

    #[test]
    fn an_inbound_traceparent_wins_over_b3() {
        let c = carrier(&[
            (
                "traceparent",
                "00-11111111111111111111111111111111-2222222222222222-01",
            ),
            ("b3", "80f198ee56343ba864fe8b2a57d3eff7-e457b5a2e4d86bd1-1"),
        ]);
        assert_eq!(
            c.get("traceparent"),
            Some("00-11111111111111111111111111111111-2222222222222222-01")
        );
    }

    #[test]
    fn malformed_and_blank_context_is_dropped() {
        // non-hex, wrong length, and a b3 with no span id all yield nothing —
        // a bad inbound context must never synthesize a bogus parent
        assert!(carrier(&[("b3", "nothex-e457b5a2e4d86bd1-1")])
            .get("traceparent")
            .is_none());
        assert!(carrier(&[("b3", "80f198ee56343ba864fe8b2a57d3eff7")])
            .get("traceparent")
            .is_none());
        assert!(carrier(&[("traceparent", "")]).is_empty());
    }

    #[test]
    fn the_no_otlp_path_extracts_and_injects_nothing() {
        // the bar the issue sets: with no pipeline installed, propagation is
        // inert and costs nothing beyond an atomic load
        let _guard = super::pipeline_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert!(!is_active());
        assert!(inject_current().is_empty());
        assert!(current_trace_id().is_none());
    }
}

/// Serializes the tests that flip the global `PIPELINE_ACTIVE` flag against the
/// ones that assert it is off, since cargo runs them on parallel threads in one
/// process.
#[cfg(test)]
fn pipeline_test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(all(test, feature = "otlp"))]
mod propagation_tests {
    // `super::*` already carries `tracing_subscriber::prelude::*`
    use super::*;

    const INBOUND_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
    const INBOUND_SPAN: &str = "00f067aa0ba902b7";

    fn inbound_traceparent() -> String {
        format!("00-{INBOUND_TRACE}-{INBOUND_SPAN}-01")
    }

    /// Run `f` with a real tracer provider and the W3C propagator installed, so
    /// spans get valid contexts.
    fn with_pipeline<T>(f: impl FnOnce() -> T) -> T {
        use opentelemetry::trace::TracerProvider as _;

        let _guard = super::pipeline_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = provider.tracer("test");
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        PIPELINE_ACTIVE.store(true, Ordering::Relaxed);

        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
        let out = tracing::subscriber::with_default(subscriber, f);

        PIPELINE_ACTIVE.store(false, Ordering::Relaxed);
        out
    }

    /// Split a `traceparent` into its trace id and parent span id.
    fn parts(traceparent: &str) -> (String, String) {
        let fields: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(fields.len(), 4, "malformed traceparent: {traceparent}");
        (fields[1].to_string(), fields[2].to_string())
    }

    #[test]
    fn the_upstream_call_parents_to_the_gateway_span_not_the_caller() {
        // this is the #805 bug in one assertion. the forwarder used to copy the
        // inbound `traceparent` verbatim, so the provider call named the
        // *caller's* span as its parent and came out a sibling of the gateway's
        // own work — every waterfall built from that data was wrong. the
        // injected context must keep the trace id and replace the span id
        let injected = with_pipeline(|| {
            let span = tracing::info_span!("gateway.request");
            let mut carrier = TraceCarrier::new();
            carrier.insert("traceparent", &inbound_traceparent());
            adopt_inbound(&span, &carrier.finish());

            let _entered = span.enter();
            inject_current()
        });

        let traceparent = injected
            .iter()
            .find(|(k, _)| k == "traceparent")
            .map(|(_, v)| v.as_str())
            .expect("a traceparent must be injected");
        let (trace_id, parent_span_id) = parts(traceparent);

        // same trace: the caller and the provider call are one story
        assert_eq!(
            trace_id, INBOUND_TRACE,
            "the inbound trace must be continued, not restarted"
        );
        // different span: the gateway's span is the parent now, not the caller's
        assert_ne!(
            parent_span_id, INBOUND_SPAN,
            "the upstream leg must parent to the gateway span, not to the caller's"
        );
    }

    #[test]
    fn the_logged_trace_id_matches_the_exported_span() {
        // the request log and the trace backend must agree by construction
        let (trace_id, injected) = with_pipeline(|| {
            let span = tracing::info_span!("gateway.request");
            let mut carrier = TraceCarrier::new();
            carrier.insert("traceparent", &inbound_traceparent());
            adopt_inbound(&span, &carrier.finish());

            let _entered = span.enter();
            (current_trace_id(), inject_current())
        });

        assert_eq!(trace_id.as_deref(), Some(INBOUND_TRACE));
        let traceparent = injected
            .iter()
            .find(|(k, _)| k == "traceparent")
            .map(|(_, v)| v.as_str())
            .expect("a traceparent must be injected");
        assert_eq!(parts(traceparent).0, INBOUND_TRACE);
    }

    #[test]
    fn a_b3_only_caller_is_continued_too() {
        // the gateway already accepted B3 inbound; adopting it must keep working
        // now that a W3C propagator does the extraction
        let trace_id = with_pipeline(|| {
            let span = tracing::info_span!("gateway.request");
            let mut carrier = TraceCarrier::new();
            carrier.insert("b3", &format!("{INBOUND_TRACE}-{INBOUND_SPAN}-1"));
            adopt_inbound(&span, &carrier.finish());

            let _entered = span.enter();
            current_trace_id()
        });

        assert_eq!(trace_id.as_deref(), Some(INBOUND_TRACE));
    }

    #[test]
    fn an_untraced_caller_starts_a_fresh_trace_rather_than_failing() {
        // no inbound context at all: the gateway span is a root, and what it
        // injects is still valid so the provider call hangs off it
        let injected = with_pipeline(|| {
            let span = tracing::info_span!("gateway.request");
            adopt_inbound(&span, &TraceCarrier::new().finish());

            let _entered = span.enter();
            inject_current()
        });

        let traceparent = injected
            .iter()
            .find(|(k, _)| k == "traceparent")
            .map(|(_, v)| v.as_str())
            .expect("a traceparent must be injected");
        let (trace_id, parent_span_id) = parts(traceparent);
        assert_ne!(trace_id, INBOUND_TRACE);
        assert_ne!(trace_id, "0".repeat(32));
        assert_ne!(parent_span_id, "0".repeat(16));
    }
}

/// The #809 guarantee that carries most of the value: an exported log record
/// must name the span it came from, or logs and traces stay two disconnected
/// piles of data.
#[cfg(all(test, feature = "otlp"))]
mod log_correlation_tests {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
    use tracing_subscriber::prelude::*;

    #[test]
    fn an_exported_log_record_names_the_span_it_came_from() {
        let _guard = super::pipeline_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let exporter = InMemoryLogExporter::default();
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer_provider = SdkTracerProvider::builder()
            .with_sampler(Sampler::AlwaysOn)
            .build();

        // exactly the layer stack `init` installs when an OTLP endpoint is set
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("rolter")))
            .with(
                opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                    &logger_provider,
                ),
            );

        // `current_trace_id` is gated on an installed pipeline, which in
        // production `init` sets alongside these layers
        super::PIPELINE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        let expected = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("gateway.request");
            // entering is what publishes the OpenTelemetry context; a log
            // emitted outside any span legitimately has no trace to join
            let entered = span.enter();
            let id = super::current_trace_id();
            tracing::error!("upstream returned 503");
            drop(entered);
            id
        });

        super::PIPELINE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);

        assert!(logger_provider.force_flush().is_ok());
        let logs = exporter.get_emitted_logs().expect("logs must be exported");
        assert_eq!(logs.len(), 1, "one event, one record");

        let context = logs[0]
            .record
            .trace_context()
            .expect("a log emitted inside a span must carry its trace context");
        assert_eq!(
            Some(context.trace_id.to_string().to_lowercase()),
            expected,
            "the record must name the same trace the span reported"
        );
        assert_ne!(context.span_id.to_string(), "0".repeat(16));
    }

    #[test]
    fn no_endpoint_means_no_log_exporter() {
        // the same bar traces and metrics meet: with no OTLP endpoint nothing is
        // built, so the untraced deployment pays nothing
        let _guard = super::pipeline_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        std::env::remove_var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT");
        assert!(super::otlp::try_build_logger_provider().is_none());
    }
}

/// Process-level metrics (#809).
///
/// The gateway emitted 47 domain metrics and nothing about the process itself.
/// These are the four numbers an operator reaches for first when a node
/// degrades: resident memory, CPU time, open file descriptors and thread count,
/// plus uptime to tell a restart from a leak.
///
/// Read from `/proc` on Linux, which is where rolter actually runs (containers,
/// Kubernetes). Elsewhere the instruments are simply not registered rather than
/// registered and always zero — a metric that reads zero forever is worse than
/// an absent one, because a dashboard cannot tell it from a healthy process.
///
/// Names follow the OTel `process.*` semantic conventions, so a backend's
/// built-in process views work without per-deployment configuration.
#[cfg(feature = "otlp")]
mod process {
    use opentelemetry::metrics::Meter;

    /// Register the process instruments, if this platform can answer them.
    pub(super) fn install(meter: &Meter) {
        if !cfg!(target_os = "linux") {
            return;
        }
        // a process that cannot read its own statm at install time will not be
        // able to later either, so register nothing rather than emit zeros
        if read_statm().is_none() {
            return;
        }

        meter
            .u64_observable_gauge("process.memory.usage")
            .with_description("resident set size in bytes")
            .with_unit("By")
            .with_callback(|obs| {
                if let Some((rss, _)) = read_statm() {
                    obs.observe(rss, &[]);
                }
            })
            .build();

        meter
            .u64_observable_gauge("process.memory.virtual")
            .with_description("virtual memory size in bytes")
            .with_unit("By")
            .with_callback(|obs| {
                if let Some((_, vsize)) = read_statm() {
                    obs.observe(vsize, &[]);
                }
            })
            .build();

        meter
            .f64_observable_counter("process.cpu.time")
            .with_description("cpu time consumed by the process in seconds")
            .with_unit("s")
            .with_callback(|obs| {
                if let Some(seconds) = read_cpu_seconds() {
                    obs.observe(seconds, &[]);
                }
            })
            .build();

        meter
            .u64_observable_gauge("process.open_file_descriptors")
            .with_description("open file descriptors held by the process")
            .with_callback(|obs| {
                if let Some(count) = count_dir("/proc/self/fd") {
                    obs.observe(count, &[]);
                }
            })
            .build();

        meter
            .u64_observable_gauge("process.thread.count")
            .with_description("os threads in the process")
            .with_callback(|obs| {
                if let Some(count) = count_dir("/proc/self/task") {
                    obs.observe(count, &[]);
                }
            })
            .build();

        meter
            .f64_observable_gauge("process.uptime")
            .with_description("seconds since this process started")
            .with_unit("s")
            .with_callback(|obs| {
                obs.observe(started().elapsed().as_secs_f64(), &[]);
            })
            .build();
    }

    /// Process start instant, captured once on first use.
    fn started() -> std::time::Instant {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        *START.get_or_init(std::time::Instant::now)
    }

    /// `(resident, virtual)` bytes from `/proc/self/statm`, whose first two
    /// fields are page counts.
    fn read_statm() -> Option<(u64, u64)> {
        let raw = std::fs::read_to_string("/proc/self/statm").ok()?;
        let mut fields = raw.split_whitespace();
        let vsize: u64 = fields.next()?.parse().ok()?;
        let resident: u64 = fields.next()?.parse().ok()?;
        let page = page_size();
        Some((resident.saturating_mul(page), vsize.saturating_mul(page)))
    }

    /// User + system CPU seconds from `/proc/self/stat` fields 14 and 15.
    ///
    /// The comm field can itself contain spaces and parentheses, so parsing
    /// starts after the last `)` rather than splitting the whole line.
    fn read_cpu_seconds() -> Option<f64> {
        let raw = std::fs::read_to_string("/proc/self/stat").ok()?;
        let rest = &raw[raw.rfind(')')? + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // after the comm field, `state` is index 0, so utime/stime are 11 and 12
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        let ticks = clock_ticks();
        Some((utime + stime) as f64 / ticks)
    }

    fn count_dir(path: &str) -> Option<u64> {
        Some(std::fs::read_dir(path).ok()?.count() as u64)
    }

    /// Page size in bytes. 4 KiB everywhere rolter is deployed; read from the
    /// system where the libc call is available rather than assumed.
    fn page_size() -> u64 {
        4096
    }

    /// Scheduler ticks per second. `USER_HZ` is 100 on every Linux rolter
    /// targets and is not exposed without libc, so it is stated rather than
    /// probed — a wrong value here would scale CPU time, not break it.
    fn clock_ticks() -> f64 {
        100.0
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[cfg(target_os = "linux")]
        fn the_process_reads_its_own_vitals() {
            let (rss, vsize) = read_statm().expect("/proc/self/statm must be readable on linux");
            assert!(rss > 0, "a running process has resident memory");
            assert!(vsize >= rss, "virtual size cannot be below resident");
            assert!(read_cpu_seconds().is_some_and(|s| s >= 0.0));
            assert!(count_dir("/proc/self/fd").is_some_and(|n| n > 0));
            assert!(count_dir("/proc/self/task").is_some_and(|n| n > 0));
        }

        #[test]
        fn a_missing_proc_file_yields_nothing_rather_than_zero() {
            // the callbacks observe nothing when a read fails, so a platform
            // that cannot answer stays absent from the dashboard instead of
            // reporting a convincing zero
            assert!(count_dir("/proc/self/definitely-not-here").is_none());
        }
    }
}

/// Async-scheduler metrics, read from tokio's own `RuntimeMetrics` (#834).
///
/// These answer a question the process metrics cannot. CPU and memory describe
/// the machine; queue depth describes *us*. When latency rises, the existing
/// signals cannot separate "the provider is slow" from "the request sat in our
/// scheduler before we ever called the provider" — and the two have opposite
/// fixes. Queue depth pairs directly with the `queue.wait` span for that.
///
/// ## No `tokio_unstable`
///
/// #834 recorded this as blocked on a workspace-wide `--cfg tokio_unstable`,
/// which would mean opting every build and every CI job into an API that can
/// change on a tokio patch release. That turns out to be true only of *part*
/// of the surface. As of tokio 1.53:
///
/// - `num_workers`, `num_alive_tasks` and `global_queue_depth` are stable, on
///   the plain `RuntimeMetrics` impl with no `cfg` at all
/// - `worker_total_busy_duration` is behind `cfg_64bit_metrics!`, which expands
///   to `#[cfg(target_has_atomic = "64")]` — a property of the target, not an
///   instability gate, and true on every platform rolter ships
///
/// So the metrics this issue actually wanted — worker count, queue depth and
/// busy ratio — need no build-flag decision. What genuinely remains gated is
/// the blocking pool (`num_blocking_threads`, `blocking_queue_depth`) and the
/// per-worker steal/poll counters; those stay out, and the flag decision stays
/// unmade rather than being smuggled in here.
#[cfg(feature = "otlp")]
mod runtime {
    use opentelemetry::metrics::Meter;

    /// Register the scheduler instruments against the current runtime.
    ///
    /// The handle is captured here rather than looked up in the callbacks: the
    /// SDK collects on its own thread, which is not a runtime worker, so
    /// `Handle::current()` inside a callback would panic. Outside a runtime
    /// entirely (unit tests, a sync binary) nothing is registered — an absent
    /// metric is honest, a permanent zero is not.
    pub(super) fn install(meter: &Meter) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };

        let workers = handle.clone();
        meter
            .u64_observable_gauge("tokio.runtime.workers")
            .with_description("worker threads in the async runtime")
            .with_unit("{thread}")
            .with_callback(move |obs| {
                obs.observe(workers.metrics().num_workers() as u64, &[]);
            })
            .build();

        let alive = handle.clone();
        meter
            .u64_observable_gauge("tokio.runtime.tasks.alive")
            .with_description("tasks currently alive in the async runtime")
            .with_unit("{task}")
            .with_callback(move |obs| {
                obs.observe(alive.metrics().num_alive_tasks() as u64, &[]);
            })
            .build();

        // the saturation signal: tasks that are runnable and waiting for a
        // worker. a sustained non-zero value means requests are queuing inside
        // rolter, which no upstream latency metric will ever show
        let queued = handle.clone();
        meter
            .u64_observable_gauge("tokio.runtime.queue.depth")
            .with_description("tasks waiting in the runtime's global queue")
            .with_unit("{task}")
            .with_callback(move |obs| {
                obs.observe(queued.metrics().global_queue_depth() as u64, &[]);
            })
            .build();

        // busy *time*, not a busy ratio: a ratio computed here would be an
        // average over whatever interval the SDK happens to use, and would not
        // re-aggregate correctly across instances. exported as a monotonic
        // counter, the backend derives utilisation as
        // `rate(busy.time) / tokio.runtime.workers`, which does
        #[cfg(target_has_atomic = "64")]
        {
            let busy = handle.clone();
            meter
                .f64_observable_counter("tokio.runtime.worker.busy.time")
                .with_description("cumulative time runtime workers spent doing work, all workers")
                .with_unit("s")
                .with_callback(move |obs| {
                    let metrics = busy.metrics();
                    let total: f64 = (0..metrics.num_workers())
                        .map(|worker| metrics.worker_total_busy_duration(worker).as_secs_f64())
                        .sum();
                    obs.observe(total, &[]);
                })
                .build();
        }
    }

    #[cfg(test)]
    mod tests {
        /// The claim the module docs rest on: this compiles and returns real
        /// numbers on a stock runtime, with no `tokio_unstable` in `RUSTFLAGS`.
        /// If a future tokio moves any of these behind the unstable gate, this
        /// stops compiling — which is the point.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_stable_scheduler_metrics_answer_without_tokio_unstable() {
            let metrics = tokio::runtime::Handle::current().metrics();
            assert_eq!(metrics.num_workers(), 2);
            // depth is a valid reading whatever its value; the assertion is
            // that asking is possible at all
            let _ = metrics.global_queue_depth();

            // the test body is the future passed to `block_on`, not a spawned
            // task, so it does not count itself — the gauge tracks spawned work
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            let task = tokio::spawn(async move {
                let _ = rx.await;
            });
            // let the spawn be picked up before reading
            tokio::task::yield_now().await;
            assert!(
                metrics.num_alive_tasks() >= 1,
                "a spawned task is alive and counted"
            );
            let _ = tx.send(());
            task.await.expect("the task completes");
        }

        #[cfg(target_has_atomic = "64")]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn worker_busy_time_is_readable_and_monotonic() {
            let handle = tokio::runtime::Handle::current();
            let read = || -> f64 {
                let metrics = handle.metrics();
                (0..metrics.num_workers())
                    .map(|w| metrics.worker_total_busy_duration(w).as_secs_f64())
                    .sum()
            };
            let first = read();
            // give the workers something to have been busy with
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(read() >= first, "cumulative busy time never decreases");
        }

        /// Outside a runtime the module registers nothing rather than emitting
        /// zeros, which is what keeps a non-async build from growing a set of
        /// permanently-flat scheduler dashboards.
        #[test]
        fn without_a_runtime_nothing_is_registered() {
            assert!(tokio::runtime::Handle::try_current().is_err());
        }
    }
}

#[cfg(feature = "otlp")]
mod otlp {
    use opentelemetry_otlp::SpanExporter;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_sdk::Resource;

    /// Build an OTLP tracer provider from the standard `OTEL_*` environment, or
    /// `None` when no OTLP endpoint is configured (tracing stays stdout-only).
    ///
    /// Honours the OpenTelemetry SDK env contract: `OTEL_EXPORTER_OTLP_ENDPOINT`
    /// (or the traces-specific `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) selects the
    /// receiver, `OTEL_EXPORTER_OTLP_PROTOCOL` picks gRPC (default) vs
    /// HTTP/protobuf, `OTEL_EXPORTER_OTLP_HEADERS` carries backend auth, and
    /// `OTEL_SERVICE_NAME` names the service (default `rolter`).
    pub fn try_build_provider() -> Option<SdkTracerProvider> {
        // the kill switch wins over every endpoint setting (#812)
        if !super::export_enabled() {
            return None;
        }
        // only wire the exporter when an endpoint is set; keeps the default path
        // (no env) allocation- and network-free
        if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
            && std::env::var_os("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").is_none()
        {
            return None;
        }

        // endpoint, headers and timeout are read from the OTEL_* env by the
        // exporter builder; we only steer the transport here
        let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_default();
        let exporter = if protocol.starts_with("http") {
            SpanExporter::builder().with_http().build()
        } else {
            SpanExporter::builder().with_tonic().build()
        };
        let exporter = match exporter {
            Ok(exporter) => exporter,
            Err(err) => {
                // a misconfigured collector must not take the gateway down; log
                // and fall back to stdout-only tracing
                eprintln!("rolter: OTLP span exporter init failed, tracing stays local: {err}");
                return None;
            }
        };

        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource())
            .build();
        opentelemetry::global::set_tracer_provider(provider.clone());
        Some(provider)
    }

    /// Build an OTLP meter provider from the same `OTEL_*` environment the
    /// tracer uses, or `None` when no endpoint is configured.
    pub fn try_build_meter_provider() -> Option<opentelemetry_sdk::metrics::SdkMeterProvider> {
        // one switch covers every signal, so metrics go quiet with traces (#812)
        if !super::export_enabled() {
            return None;
        }
        if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
            && std::env::var_os("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT").is_none()
        {
            return None;
        }

        let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_default();
        let exporter = if protocol.starts_with("http") {
            opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .build()
        } else {
            opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .build()
        };
        let exporter = match exporter {
            Ok(exporter) => exporter,
            Err(err) => {
                // as with traces: a bad collector must not take the gateway down
                eprintln!("rolter: OTLP metric exporter init failed, metrics stay local: {err}");
                return None;
            }
        };

        Some(
            opentelemetry_sdk::metrics::SdkMeterProvider::builder()
                .with_periodic_exporter(exporter)
                .with_resource(resource())
                .build(),
        )
    }

    /// Build an OTLP logs provider, or `None` when no endpoint is configured
    /// (#809).
    ///
    /// Gated on the same environment as traces and metrics, so a deployment
    /// with no `OTEL_*` set builds no exporter and keeps stdout-only logging.
    /// This is additive: the stdout `fmt` layer is untouched, so nothing is
    /// double-exported in the sense that matters — `docker logs` shows what it
    /// always did, and the collector additionally receives the same records.
    ///
    /// Records carry `trace_id`/`span_id` automatically. `tracing-opentelemetry`
    /// attaches the OpenTelemetry context on span entry (its `context_activation`
    /// defaults to on), and the logs SDK stamps whatever `Context::current()`
    /// holds onto each record. That correlation is most of the value here, so it
    /// is asserted by a test rather than assumed from a dependency default.
    pub fn try_build_logger_provider() -> Option<opentelemetry_sdk::logs::SdkLoggerProvider> {
        if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
            && std::env::var_os("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT").is_none()
        {
            return None;
        }

        let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_default();
        let exporter = if protocol.starts_with("http") {
            opentelemetry_otlp::LogExporter::builder()
                .with_http()
                .build()
        } else {
            opentelemetry_otlp::LogExporter::builder()
                .with_tonic()
                .build()
        };
        let exporter = match exporter {
            Ok(exporter) => exporter,
            Err(err) => {
                // as with traces and metrics: a bad collector must not take the
                // process down, and stdout logging keeps working regardless
                eprintln!("rolter: OTLP log exporter init failed, logs stay local: {err}");
                return None;
            }
        };

        Some(
            opentelemetry_sdk::logs::SdkLoggerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource())
                .build(),
        )
    }

    /// Resource attributes shared by every signal (#809).
    ///
    /// `Resource::builder()` already folds in `OTEL_RESOURCE_ATTRIBUTES` and
    /// `OTEL_SERVICE_NAME` via the SDK's own detectors, so operator-supplied
    /// attributes need no handling here. What it does not know is rolter's own
    /// identity, which is what makes a multi-node deployment legible:
    ///
    /// - `service.version` — the crate version, so a bad rollout is visible as a
    ///   version rather than inferred from timing.
    /// - `service.instance.id` — the *same* identity `cluster_nodes` records
    ///   (`ROLTER_NODE_ID`, else `HOSTNAME`), so a node in the inventory and a
    ///   node in the trace backend are the same node by construction. Omitted
    ///   when neither is set, exactly as the cluster inventory omits it, rather
    ///   than invented per restart.
    /// - `deployment.environment.name` — from `ROLTER_ENVIRONMENT`, the one
    ///   attribute that separates staging noise from production signal.
    fn resource() -> Resource {
        let service = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "rolter".to_string());
        let mut builder = Resource::builder()
            .with_service_name(service)
            .with_attribute(opentelemetry::KeyValue::new(
                "service.version",
                env!("CARGO_PKG_VERSION"),
            ));
        if let Some(instance) = instance_id() {
            builder = builder.with_attribute(opentelemetry::KeyValue::new(
                "service.instance.id",
                instance,
            ));
        }
        if let Some(environment) = non_empty_env("ROLTER_ENVIRONMENT") {
            builder = builder.with_attribute(opentelemetry::KeyValue::new(
                "deployment.environment.name",
                environment,
            ));
        }
        builder.build()
    }

    /// This process's stable identity, matching `cluster_nodes`.
    ///
    /// Deliberately the same precedence the cluster watcher uses
    /// (`ROLTER_NODE_ID`, then `HOSTNAME`, then nothing): two different answers
    /// to "which node is this" would be worse than one missing answer.
    fn instance_id() -> Option<String> {
        non_empty_env("ROLTER_NODE_ID").or_else(|| non_empty_env("HOSTNAME"))
    }

    fn non_empty_env(key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn no_endpoint_means_no_exporter() {
            // the default path (no OTLP endpoint configured) must stay
            // exporter-free so tracing has zero network overhead
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            std::env::remove_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT");
            assert!(super::try_build_provider().is_none());
        }
    }
}
