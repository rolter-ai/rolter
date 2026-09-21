use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use dashmap::DashMap;

/// Upper bounds (milliseconds) of the request-latency / TTFT histogram buckets.
/// The implicit `+Inf` bucket catches everything above the last boundary and is
/// represented by the observation `count`.
const LATENCY_BUCKETS_MS: [u32; 13] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000];

/// Labelled counter family names, shared by the Prometheus renderers and the
/// OTLP exporter so the two cannot name the same series differently.
const LABELLED_TARGET: &str = "rolter_target_requests_total";
const LABELLED_VARIANT: &str = "rolter_variant_requests_total";
const LABELLED_COMPLEXITY: &str = "rolter_complexity_route_requests_total";

/// The labelled families, declared up front for the OTLP exporter. At install
/// time the gateway has served nothing, so every family is legitimately empty
/// and the set cannot be inferred from a first collection.
pub const LABELLED_FAMILIES: &[rolter_core::telemetry::LabelledFamily] = &[
    (
        LABELLED_TARGET,
        "proxied requests per upstream target by outcome",
    ),
    (
        LABELLED_VARIANT,
        "proxied requests per A/B variant by model",
    ),
    (
        LABELLED_COMPLEXITY,
        "complexity routing decisions by requested model, tier, selected route and outcome",
    ),
];

/// A hand-rolled Prometheus histogram: non-cumulative bucket counters plus a
/// running sum and total count. Buckets are cumulated only at render time so the
/// observe path does a single atomic add.
struct Histogram {
    // one counter per `LATENCY_BUCKETS_MS` boundary, holding the number of
    // observations that fell in `(prev_bound, bound]`
    buckets: [AtomicU64; LATENCY_BUCKETS_MS.len()],
    sum_ms: AtomicU64,
    count: AtomicU64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            sum_ms: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Record one observation of `ms` milliseconds.
    fn observe(&self, ms: u32) {
        self.sum_ms.fetch_add(ms as u64, Relaxed);
        self.count.fetch_add(1, Relaxed);
        for (i, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if ms <= bound {
                self.buckets[i].fetch_add(1, Relaxed);
                return;
            }
        }
        // above the last boundary: lands in the implicit +Inf bucket, which is
        // reconstructed from `count` at render time
    }
}

/// Per-model latency + time-to-first-token histograms.
struct ModelHist {
    latency: Histogram,
    ttft: Histogram,
}

impl ModelHist {
    fn new() -> Self {
        Self {
            latency: Histogram::new(),
            ttft: Histogram::new(),
        }
    }
}

/// Passive per-target request-outcome tally, derived from real traffic (no extra
/// upstream calls). Feeds provider stability / uptime SLA views.
#[derive(Default)]
struct TargetStats {
    ok: AtomicU64,
    err: AtomicU64,
}

/// Lightweight Prometheus metrics rendered as text.
///
/// Hand-rolled counters/gauges plus per-model latency and TTFT histograms — this
/// avoids pulling the full `metrics` facade + global recorder, which would not
/// fit the lock-free `arc-swap` design where an explicit `Arc<Metrics>` is
/// threaded through the request path.
#[derive(Default)]
pub struct Metrics {
    pub requests_total: AtomicU64,
    pub upstream_errors_total: AtomicU64,
    pub auth_failures_total: AtomicU64,
    /// config version currently applied to the live snapshot
    pub config_version: AtomicU64,
    /// successful hot-reloads applied since start
    pub config_reloads_total: AtomicU64,
    /// failed snapshot fetches/parses since start
    pub config_reload_failures_total: AtomicU64,
    /// live per-(model, target) circuit-breaker entries after the last reload,
    /// so registry growth across config generations is observable (#1053)
    pub breaker_entries: AtomicU64,
    /// request-log rows successfully written to clickhouse
    pub logs_written_total: AtomicU64,
    /// request-log rows dropped (queue full or write failed)
    pub logs_dropped_total: AtomicU64,
    /// budget/rate-limit usage records dropped because the recording queue was
    /// full — the counter store is not keeping up, and this request's spend is
    /// missing from its counters (#1051)
    pub usage_records_dropped_total: AtomicU64,
    /// usage records waiting to be written, sampled at scrape time
    pub usage_records_queued: AtomicU64,
    /// provider-health-event rows successfully written to clickhouse
    pub health_events_written_total: AtomicU64,
    /// provider-health-event rows dropped (queue full or write failed)
    pub health_events_dropped_total: AtomicU64,
    /// requests rejected because a matching budget was exhausted
    pub budget_blocks_total: AtomicU64,
    /// requests refused because the model had no price and the unpriced policy
    /// is `block` — spend that would never have reached a budget (#974)
    pub unpriced_blocks_total: AtomicU64,
    /// requests served against a model with no price row. Non-zero means the
    /// spend figures for this window are a floor, not a total (#969)
    pub unpriced_served_total: AtomicU64,
    /// requests rejected because a matching rpm/tpm rate limit was exhausted
    pub rate_limit_blocks_total: AtomicU64,
    /// requests rejected by a built-in guardrail block rule
    pub guardrail_blocks_total: AtomicU64,
    /// individual guardrail redactions applied to request content
    pub guardrail_redactions_total: AtomicU64,
    /// responses withheld because a post-call guardrail rule matched
    pub guardrail_output_blocks_total: AtomicU64,
    /// individual guardrail redactions applied to response content
    pub guardrail_output_redactions_total: AtomicU64,
    /// streamed requests refused because the route has post-call rules
    pub guardrail_stream_rejections_total: AtomicU64,
    /// streamed requests refused because a fail-closed `post_response` plugin
    /// applies to them and cannot inspect a stream (#1776)
    pub plugin_stream_rejections_total: AtomicU64,
    /// decorator messages injected by prompt templates at admission
    pub prompt_template_decorations_total: AtomicU64,
    /// requests rejected for invalid prompt-template variables
    pub prompt_template_rejections_total: AtomicU64,
    /// requests rejected by the custom guardrail webhook
    pub guardrail_webhook_blocks_total: AtomicU64,
    /// request bodies transformed by the custom guardrail webhook
    pub guardrail_webhook_transforms_total: AtomicU64,
    /// custom-guardrail-webhook calls that failed transport (timeout/connect/non-2xx)
    pub guardrail_webhook_errors_total: AtomicU64,
    /// entities replaced with placeholders by the external PII sanitizer (#848).
    /// a count of *replacements*, never an entity type or a matched value
    pub pii_entities_redacted_total: AtomicU64,
    /// sanitizer calls that failed transport (timeout/connect/non-2xx)
    pub pii_sanitizer_errors_total: AtomicU64,
    /// requests rejected because the sanitizer was unreachable and the
    /// deployment is fail-closed
    pub pii_sanitizer_blocks_total: AtomicU64,
    /// responses whose placeholders were turned back into plaintext
    pub pii_restorations_total: AtomicU64,
    /// streamed requests refused because the sanitizer's response leg is active
    pub pii_stream_rejections_total: AtomicU64,
    /// restore calls that failed. distinct from a sanitizer failure: the
    /// content is already safe, so this is a usability failure and not a
    /// disclosure one, and the two must not share a counter
    pub pii_restore_errors_total: AtomicU64,
    /// requests rejected by a webhook plugin instance (#509)
    pub plugin_blocks_total: AtomicU64,
    /// request/response bodies transformed by a webhook plugin instance
    pub plugin_transforms_total: AtomicU64,
    /// webhook plugin calls that failed transport (timeout/connect/non-2xx)
    pub plugin_errors_total: AtomicU64,
    /// in-flight requests across every target, sampled at scrape time. A value
    /// that never returns to zero on an idle gateway means a request exited
    /// without releasing its slot (#1083)
    pub inflight_requests: AtomicU64,
    /// requests whose client went away before the response finished — a
    /// browser tab closed, a `ctrl-c` in an agent CLI, a proxy timeout. Not an
    /// error: it is ordinary traffic for an agent gateway, and the reason it is
    /// counted is that the tokens generated before the caller vanished were
    /// still billed (#1083)
    pub client_disconnects_total: AtomicU64,
    /// responses the upstream produced and billed but a post-call policy (an
    /// output guardrail or a post-response plugin) refused to deliver. Their
    /// spend is still recorded; this is how an operator sees how much of it
    /// bought nothing (#1478)
    pub withheld_responses_total: AtomicU64,
    /// requests rejected because their selected provider's queue was full
    pub provider_queue_rejections_total: AtomicU64,
    /// requests that timed out waiting for a provider queue slot
    pub provider_queue_timeouts_total: AtomicU64,
    /// upstream attempts retried after a transient failure (408/429/5xx/connect)
    pub retries_total: AtomicU64,
    /// times a target was parked on a cooldown after a transient failure
    pub cooldowns_tripped_total: AtomicU64,
    /// times an api key was parked on a cooldown after a 429/401 on a
    /// multi-key provider
    pub key_cooldowns_tripped_total: AtomicU64,
    /// times a health probe transitioned a provider from healthy to down
    pub health_down_total: AtomicU64,
    /// times a health probe transitioned a provider from down to healthy
    pub health_recovered_total: AtomicU64,
    /// status-page polls that observed a non-operational provider (secondary
    /// signal only; never gates routing)
    pub status_page_degraded_total: AtomicU64,
    /// times a circuit breaker tripped a target open after sustained failures
    pub breaker_opened_total: AtomicU64,
    /// times a circuit breaker closed a target after a successful half-open probe
    pub breaker_closed_total: AtomicU64,
    /// upstream `/metrics` scrape sweeps completed
    pub metrics_scrapes_total: AtomicU64,
    /// requests served from the response cache without contacting an upstream
    pub cache_hits_total: AtomicU64,
    /// cache-eligible requests that were not found in the cache
    pub cache_misses_total: AtomicU64,
    /// successful responses written into the response cache
    pub cache_stores_total: AtomicU64,
    /// cacheable responses skipped for exceeding `cache.max_entry_bytes`
    pub cache_too_large_total: AtomicU64,
    /// semantic candidates served above the configured cosine threshold
    pub semantic_cache_hits_total: AtomicU64,
    /// semantic lookups with no candidate above threshold
    pub semantic_cache_misses_total: AtomicU64,
    /// responses added to bounded semantic candidate indexes
    pub semantic_cache_stores_total: AtomicU64,
    pub kv_events_total: AtomicU64,
    pub kv_events_malformed_total: AtomicU64,
    pub kv_event_stream_failures_total: AtomicU64,
    pub kv_cache_decisions_total: AtomicU64,
    pub lmcache_refreshes_total: AtomicU64,
    pub lmcache_refresh_failures_total: AtomicU64,
    pub lmcache_decisions_total: AtomicU64,
    /// OTLP histogram recorder, installed at startup when an OTLP endpoint is
    /// configured (#805). `None` on every other deployment, which is what keeps
    /// the untraced request path free of extra work
    otlp_histograms: std::sync::OnceLock<rolter_core::telemetry::RequestHistograms>,
    /// per-model latency + TTFT histograms, keyed by public model name
    by_model: DashMap<String, ModelHist>,
    /// passive per-target success/error tally, keyed by (provider, target)
    by_target: DashMap<(String, String), TargetStats>,
    /// per-variant request tally for A/B attribution, keyed by (model, variant)
    by_variant: DashMap<(String, String), AtomicU64>,
    /// complexity-policy selections keyed by (requested model, tier, selected
    /// route, outcome). Every label is configuration-bounded (16 tiers/route).
    by_complexity: DashMap<(String, String, String, String), AtomicU64>,
}

/// One scalar metric: its Prometheus type, name, help text, and current value.
///
/// Deliberately flat and owned-free — building the list is a walk over atomics,
/// cheap enough to do on every scrape and on every OTLP collection interval.
#[derive(Debug, Clone, Copy)]
pub struct Scalar {
    /// prometheus metric type: `counter` or `gauge`
    pub kind: &'static str,
    /// fully-qualified metric name, e.g. `rolter_requests_total`
    pub name: &'static str,
    /// one-line description, used as the Prometheus HELP text
    pub help: &'static str,
    /// value at the moment the list was built
    pub value: u64,
}

impl Metrics {
    /// Record one completed request's total latency and time-to-first-token
    /// against the `model` label. Called once per request from the log sink.
    pub fn observe_request(
        &self,
        provider: &str,
        model: &str,
        latency_ms: u32,
        ttft_ms: u32,
        output_tokens: u32,
    ) {
        let hist = self
            .by_model
            .entry(model.to_string())
            .or_insert_with(ModelHist::new);
        hist.latency.observe(latency_ms);
        hist.ttft.observe(ttft_ms);
        // and to OTLP, when an endpoint is configured. a histogram cannot be
        // observable — OTel builds one from individual measurements — so this
        // is the only metric that costs the request path anything, and only
        // where the operator asked for it
        if let Some(otlp) = self.otlp_histograms.get() {
            otlp.record(provider, model, latency_ms, ttft_ms, output_tokens);
        }
    }

    /// Record one completed request's outcome against its upstream target.
    /// `ok` is a 2xx response; anything else counts as an error. Rows with no
    /// resolved target (builtin fake-llm, pre-routing rejects) are skipped.
    pub fn observe_target(&self, provider: &str, target: &str, ok: bool) {
        if provider.is_empty() && target.is_empty() {
            return;
        }
        let stats = self
            .by_target
            .entry((provider.to_string(), target.to_string()))
            .or_default();
        if ok {
            stats.ok.fetch_add(1, Relaxed);
        } else {
            stats.err.fetch_add(1, Relaxed);
        }
    }

    /// Record one completed request against the A/B variant that served it.
    /// Classic single-pool routes pass an empty variant and are not counted.
    pub fn observe_variant(&self, model: &str, variant: &str) {
        if variant.is_empty() {
            return;
        }
        self.by_variant
            .entry((model.to_string(), variant.to_string()))
            .or_default()
            .fetch_add(1, Relaxed);
    }

    /// Record an opt-in complexity-routing decision. `fallback` means the
    /// selected tier route was unavailable or not permitted, so the request
    /// kept its originally requested route.
    pub fn observe_complexity(&self, model: &str, tier: &str, route: &str, fallback: bool) {
        self.by_complexity
            .entry((
                model.to_string(),
                tier.to_string(),
                route.to_string(),
                if fallback {
                    "fallback".to_string()
                } else {
                    "selected".to_string()
                },
            ))
            .or_default()
            .fetch_add(1, Relaxed);
    }

    /// Every scalar counter and gauge, in a stable order.
    ///
    /// Single source of truth for the two exporters that read these numbers:
    /// the Prometheus text endpoint below and the OTLP metrics exporter (#805).
    /// A counter added here reaches both without a second edit — the point of
    /// #805 being "a second exporter over the same numbers, not a rewrite".
    ///
    /// Per-model histograms and label-bearing counters are not scalars and are
    /// rendered separately.
    pub fn scalars(&self) -> Vec<Scalar> {
        vec![
            Scalar {
                kind: "counter",
                name: "rolter_requests_total",
                help: "total proxied requests",
                value: self.requests_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_upstream_errors_total",
                help: "upstream request failures",
                value: self.upstream_errors_total.load(Relaxed),
            },
            // read from process-global counters in rolter-proxy rather than
            // fields here: the connector that increments them lives inside the
            // pooled reqwest client, which has no handle to this struct. one
            // gateway owns one forwarder, so a global is the same scope (#834)
            Scalar {
                kind: "counter",
                name: "rolter_upstream_connections_total",
                help: "connections established to upstream providers",
                value: rolter_proxy::pool::connections_total(),
            },
            Scalar {
                kind: "counter",
                name: "rolter_upstream_connect_errors_total",
                help: "failed attempts to connect to an upstream provider",
                value: rolter_proxy::pool::connect_errors_total(),
            },
            Scalar {
                kind: "counter",
                name: "rolter_auth_failures_total",
                help: "requests rejected due to auth",
                value: self.auth_failures_total.load(Relaxed),
            },
            Scalar {
                kind: "gauge",
                name: "rolter_config_version",
                help: "config version applied to the live snapshot",
                value: self.config_version.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_config_reloads_total",
                help: "successful config hot-reloads applied",
                value: self.config_reloads_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_usage_records_dropped_total",
                help: "budget/rate-limit usage records dropped because the recording queue was full",
                value: self.usage_records_dropped_total.load(Relaxed),
            },
            Scalar {
                kind: "gauge",
                name: "rolter_usage_records_queued",
                help: "usage records waiting to be written to the counter store",
                value: self.usage_records_queued.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_config_reload_failures_total",
                help: "failed config snapshot fetches",
                value: self.config_reload_failures_total.load(Relaxed),
            },
            Scalar {
                kind: "gauge",
                name: "rolter_breaker_entries",
                help: "per-target circuit-breaker entries held, measured at the last config reload",
                value: self.breaker_entries.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_logs_written_total",
                help: "request-log rows written to clickhouse",
                value: self.logs_written_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_logs_dropped_total",
                help: "request-log rows dropped (queue full or write failed)",
                value: self.logs_dropped_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_health_events_written_total",
                help: "provider-health-event rows written to clickhouse",
                value: self.health_events_written_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_health_events_dropped_total",
                help: "provider-health-event rows dropped (queue full or write failed)",
                value: self.health_events_dropped_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_budget_blocks_total",
                help: "requests rejected due to an exhausted budget",
                value: self.budget_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_unpriced_blocks_total",
                help: "requests refused because the model has no price (unpriced policy = block)",
                value: self.unpriced_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_unpriced_served_total",
                help: "requests served against a model with no price row",
                value: self.unpriced_served_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_rate_limit_blocks_total",
                help: "requests rejected due to an exhausted rate limit",
                value: self.rate_limit_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_blocks_total",
                help: "requests rejected by a built-in guardrail block rule",
                value: self.guardrail_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_redactions_total",
                help: "guardrail redactions applied to request content",
                value: self.guardrail_redactions_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_output_blocks_total",
                help: "responses withheld by a post-call guardrail block rule",
                value: self.guardrail_output_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_output_redactions_total",
                help: "guardrail redactions applied to response content",
                value: self.guardrail_output_redactions_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_stream_rejections_total",
                help: "streamed requests refused because the route has post-call rules",
                value: self.guardrail_stream_rejections_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_plugin_stream_rejections_total",
                help: "streamed requests refused because a fail-closed post_response plugin applies",
                value: self.plugin_stream_rejections_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_prompt_template_decorations_total",
                help: "decorator messages injected by prompt templates at admission",
                value: self.prompt_template_decorations_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_prompt_template_rejections_total",
                help: "requests rejected for invalid prompt-template variables",
                value: self.prompt_template_rejections_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_pii_entities_redacted_total",
                help: "entities replaced with placeholders by the external PII sanitizer",
                value: self.pii_entities_redacted_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_pii_sanitizer_errors_total",
                help: "PII sanitizer calls that failed transport",
                value: self.pii_sanitizer_errors_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_pii_sanitizer_blocks_total",
                help: "requests rejected because the PII sanitizer was unavailable and the deployment is fail-closed",
                value: self.pii_sanitizer_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_pii_restorations_total",
                help: "responses whose PII placeholders were restored to plaintext",
                value: self.pii_restorations_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_pii_stream_rejections_total",
                help: "streamed requests refused because the PII sanitizer's response leg is active",
                value: self.pii_stream_rejections_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_pii_restore_errors_total",
                help: "PII restore calls that failed",
                value: self.pii_restore_errors_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_webhook_blocks_total",
                help: "requests rejected by the custom guardrail webhook",
                value: self.guardrail_webhook_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_webhook_transforms_total",
                help: "request bodies transformed by the custom guardrail webhook",
                value: self.guardrail_webhook_transforms_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_guardrail_webhook_errors_total",
                help: "custom-guardrail-webhook calls that failed transport",
                value: self.guardrail_webhook_errors_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_plugin_blocks_total",
                help: "requests rejected by a webhook plugin instance",
                value: self.plugin_blocks_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_plugin_transforms_total",
                help: "request/response bodies transformed by a webhook plugin instance",
                value: self.plugin_transforms_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_plugin_errors_total",
                help: "webhook plugin calls that failed transport",
                value: self.plugin_errors_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_provider_queue_rejections_total",
                help: "requests rejected because a provider queue was full or unavailable",
                value: self.provider_queue_rejections_total.load(Relaxed),
            },
            Scalar {
                kind: "gauge",
                name: "rolter_inflight_requests",
                help: "requests currently in flight to upstream providers",
                value: self.inflight_requests.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_client_disconnects_total",
                help: "requests whose client disconnected before the response completed",
                value: self.client_disconnects_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_withheld_responses_total",
                help: "billed upstream responses a post-call policy refused to deliver",
                value: self.withheld_responses_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_provider_queue_timeouts_total",
                help: "requests that timed out waiting for a provider queue slot",
                value: self.provider_queue_timeouts_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_retries_total",
                help: "upstream attempts retried after a transient failure",
                value: self.retries_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_cooldowns_tripped_total",
                help: "targets parked on a cooldown after a transient failure",
                value: self.cooldowns_tripped_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_key_cooldowns_tripped_total",
                help: "api keys parked on a cooldown after a key-level failure",
                value: self.key_cooldowns_tripped_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_health_down_total",
                help: "providers marked unhealthy by an active health probe",
                value: self.health_down_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_health_recovered_total",
                help: "providers restored to healthy by an active health probe",
                value: self.health_recovered_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_status_page_degraded_total",
                help: "status-page polls that saw a non-operational provider (secondary signal)",
                value: self.status_page_degraded_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_breaker_opened_total",
                help: "targets tripped open by the circuit breaker after sustained failures",
                value: self.breaker_opened_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_breaker_closed_total",
                help: "targets closed by the circuit breaker after a successful half-open probe",
                value: self.breaker_closed_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_metrics_scrapes_total",
                help: "upstream /metrics scrape sweeps completed",
                value: self.metrics_scrapes_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_cache_hits_total",
                help: "requests served from the response cache",
                value: self.cache_hits_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_cache_misses_total",
                help: "cache-eligible requests not found in the response cache",
                value: self.cache_misses_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_cache_stores_total",
                help: "successful responses written into the response cache",
                value: self.cache_stores_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_semantic_cache_hits_total",
                help: "responses served from semantic cache",
                value: self.semantic_cache_hits_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_semantic_cache_misses_total",
                help: "semantic cache lookups below the similarity threshold",
                value: self.semantic_cache_misses_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_semantic_cache_stores_total",
                help: "responses stored in semantic cache indexes",
                value: self.semantic_cache_stores_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_kv_events_total",
                help: "validated vLLM KV events consumed",
                value: self.kv_events_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_kv_events_malformed_total",
                help: "malformed vLLM KV event batches ignored",
                value: self.kv_events_malformed_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_kv_event_stream_failures_total",
                help: "vLLM KV event stream disconnects and connection failures",
                value: self.kv_event_stream_failures_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_kv_cache_decisions_total",
                help: "routing decisions using fresh exact KV residency",
                value: self.kv_cache_decisions_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_lmcache_refreshes_total",
                help: "successful LMCache controller refreshes",
                value: self.lmcache_refreshes_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_lmcache_refresh_failures_total",
                help: "failed or malformed LMCache controller refreshes",
                value: self.lmcache_refresh_failures_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_lmcache_decisions_total",
                help: "routing decisions using fresh LMCache signals",
                value: self.lmcache_decisions_total.load(Relaxed),
            },
            Scalar {
                kind: "counter",
                name: "rolter_cache_too_large_total",
                help: "cacheable responses skipped for exceeding cache.max_entry_bytes",
                value: self.cache_too_large_total.load(Relaxed),
            },
        ]
    }
    /// The scalar metrics as plain tuples, for the OTLP exporter (#805).
    ///
    /// Same list `render()` walks — the two exporters cannot drift apart.
    pub fn scalars_for_export(&self) -> Vec<rolter_core::telemetry::ScalarMetric> {
        self.scalars()
            .into_iter()
            .map(|s| (s.kind, s.name, s.help, s.value))
            .collect()
    }

    /// Every labelled counter series, for the OTLP exporter (#805).
    ///
    /// The counterpart of `render_target_counters`, `render_variant_counters`
    /// and `render_complexity_counters`, which write the same numbers in
    /// Prometheus text. Labels are unescaped here on purpose: escaping exists
    /// for the Prometheus text format, and OTLP carries attribute values as
    /// typed strings that need no quoting.
    pub fn labelled_for_export(&self) -> Vec<rolter_core::telemetry::LabelledMetric> {
        let mut out = Vec::new();
        for entry in self.by_target.iter() {
            let (provider, target) = entry.key();
            let stats = entry.value();
            for (outcome, value) in [
                ("ok", stats.ok.load(Relaxed)),
                ("error", stats.err.load(Relaxed)),
            ] {
                out.push((
                    LABELLED_TARGET,
                    vec![
                        ("provider", provider.clone()),
                        ("target", target.clone()),
                        ("outcome", outcome.to_string()),
                    ],
                    value,
                ));
            }
        }
        for entry in self.by_variant.iter() {
            let (model, variant) = entry.key();
            out.push((
                LABELLED_VARIANT,
                vec![("model", model.clone()), ("variant", variant.clone())],
                entry.value().load(Relaxed),
            ));
        }
        for entry in self.by_complexity.iter() {
            let (model, tier, route, outcome) = entry.key();
            out.push((
                LABELLED_COMPLEXITY,
                vec![
                    ("model", model.clone()),
                    ("tier", tier.clone()),
                    ("route", route.clone()),
                    ("outcome", outcome.clone()),
                ],
                entry.value().load(Relaxed),
            ));
        }
        out
    }

    /// Hand the request path an OTLP histogram recorder.
    ///
    /// Called once at startup, after the meter provider exists. Before this —
    /// and forever, on a deployment with no OTLP endpoint — `observe_request`
    /// sees `None` and does nothing extra.
    pub fn set_otlp_histograms(&self, histograms: rolter_core::telemetry::RequestHistograms) {
        let _ = self.otlp_histograms.set(histograms);
    }

    /// The Prometheus bucket boundaries as `f64`, so the OTLP histogram can be
    /// built with the same ones. Two exporters of one measurement disagreeing
    /// about bucket edges is worse than either alone: the numbers look
    /// comparable and are not.
    pub fn latency_buckets_ms() -> Vec<f64> {
        LATENCY_BUCKETS_MS.iter().map(|&b| f64::from(b)).collect()
    }

    /// Render the counters in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for scalar in self.scalars() {
            metric(
                &mut out,
                scalar.kind,
                scalar.name,
                scalar.help,
                scalar.value,
            );
        }
        self.render_histogram(
            &mut out,
            "rolter_request_latency_ms",
            "total request latency in milliseconds",
            |m| &m.latency,
        );
        self.render_histogram(
            &mut out,
            "rolter_request_ttft_ms",
            "time to first token in milliseconds",
            |m| &m.ttft,
        );
        self.render_target_counters(&mut out);
        self.render_variant_counters(&mut out);
        self.render_complexity_counters(&mut out);
        out
    }

    /// Append one series per configured tier decision. These counters combine
    /// with existing request latency/cost telemetry to show policy impact
    /// without retaining prompt content.
    fn render_complexity_counters(&self, out: &mut String) {
        let name = LABELLED_COMPLEXITY;
        let _ = writeln!(
            out,
            "# HELP {name} complexity routing decisions by requested model, tier, selected route and outcome"
        );
        let _ = writeln!(out, "# TYPE {name} counter");
        for entry in self.by_complexity.iter() {
            let (model, tier, route, outcome) = entry.key();
            let (model, tier, route, outcome) = (
                escape_label(model),
                escape_label(tier),
                escape_label(route),
                escape_label(outcome),
            );
            let _ = writeln!(
                out,
                "{name}{{model=\"{model}\",tier=\"{tier}\",route=\"{route}\",outcome=\"{outcome}\"}} {}",
                entry.value().load(Relaxed)
            );
        }
    }

    /// Append the per-variant request counter, one `{model,variant}` series per
    /// (model, variant). Lets Prometheus/Grafana show A/B traffic splits without
    /// querying ClickHouse.
    fn render_variant_counters(&self, out: &mut String) {
        let name = LABELLED_VARIANT;
        let _ = writeln!(
            out,
            "# HELP {name} proxied requests per A/B variant by model"
        );
        let _ = writeln!(out, "# TYPE {name} counter");
        for entry in self.by_variant.iter() {
            let (model, variant) = entry.key();
            let (model, variant) = (escape_label(model), escape_label(variant));
            let _ = writeln!(
                out,
                "{name}{{model=\"{model}\",variant=\"{variant}\"}} {}",
                entry.value().load(Relaxed)
            );
        }
    }

    /// Append the passive per-target outcome counter, one `{provider,target,
    /// outcome}` series per (target, outcome). A per-target error rate / uptime
    /// is `sum(rate(..{outcome="error"})) / sum(rate(..))` in Prometheus.
    fn render_target_counters(&self, out: &mut String) {
        let name = LABELLED_TARGET;
        let _ = writeln!(
            out,
            "# HELP {name} proxied requests per upstream target by outcome"
        );
        let _ = writeln!(out, "# TYPE {name} counter");
        for entry in self.by_target.iter() {
            let (provider, target) = entry.key();
            let (provider, target) = (escape_label(provider), escape_label(target));
            let stats = entry.value();
            let _ = writeln!(
                out,
                "{name}{{provider=\"{provider}\",target=\"{target}\",outcome=\"ok\"}} {}",
                stats.ok.load(Relaxed)
            );
            let _ = writeln!(
                out,
                "{name}{{provider=\"{provider}\",target=\"{target}\",outcome=\"error\"}} {}",
                stats.err.load(Relaxed)
            );
        }
    }

    /// Append a per-model histogram (one `{model=...}` series per model) in the
    /// Prometheus histogram exposition format: cumulative `_bucket` lines, a
    /// `_sum` and a `_count`. `pick` selects which histogram of the pair to emit.
    fn render_histogram(
        &self,
        out: &mut String,
        name: &str,
        help: &str,
        pick: impl Fn(&ModelHist) -> &Histogram,
    ) {
        let _ = writeln!(out, "# HELP {name} {help}");
        let _ = writeln!(out, "# TYPE {name} histogram");
        for entry in self.by_model.iter() {
            let model = escape_label(entry.key());
            let hist = pick(entry.value());
            let mut cumulative = 0u64;
            for (i, bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
                cumulative += hist.buckets[i].load(Relaxed);
                let _ = writeln!(
                    out,
                    "{name}_bucket{{model=\"{model}\",le=\"{bound}\"}} {cumulative}"
                );
            }
            let count = hist.count.load(Relaxed);
            let _ = writeln!(
                out,
                "{name}_bucket{{model=\"{model}\",le=\"+Inf\"}} {count}"
            );
            let _ = writeln!(
                out,
                "{name}_sum{{model=\"{model}\"}} {}",
                hist.sum_ms.load(Relaxed)
            );
            let _ = writeln!(out, "{name}_count{{model=\"{model}\"}} {count}");
        }
    }
}

/// Append one Prometheus metric (HELP + TYPE + value line) to `out`.
fn metric(out: &mut String, kind: &str, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "{name} {value}");
}

/// Escape a Prometheus label value: backslash, double-quote and newline per the
/// exposition format spec.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Every labelled series the Prometheus text carries must also be exported
    /// over OTLP, and with the same value.
    ///
    /// This is the invariant the whole design rests on: two exporters over one
    /// set of numbers. If they can drift, a dashboard built on one and an alert
    /// built on the other disagree, and the disagreement is invisible.
    #[test]
    fn otlp_labelled_export_matches_the_prometheus_text() {
        let m = Metrics::default();
        m.observe_target("openai", "gpt-4o", true);
        m.observe_target("openai", "gpt-4o", false);
        m.observe_target("anthropic", "sonnet", true);
        m.observe_variant("gpt-4o", "b");
        m.observe_complexity("gpt-4o", "cheap", "mini", false);
        m.observe_complexity("gpt-4o", "cheap", "mini", true);

        let text = m.render();
        for (family, attrs, value) in m.labelled_for_export() {
            // rebuild the Prometheus series this OTLP series corresponds to
            let labels = attrs.iter().fold(String::new(), |mut acc, (k, v)| {
                if !acc.is_empty() {
                    acc.push(',');
                }
                use std::fmt::Write;
                let _ = write!(acc, "{k}=\"{v}\"");
                acc
            });
            let line = format!("{family}{{{labels}}} {value}");
            assert!(
                text.contains(&line),
                "OTLP series absent from prometheus text: {line}"
            );
        }
    }

    /// Escaping is a Prometheus text-format concern and must not leak into OTLP.
    ///
    /// The parity test above only uses values that need no escaping, so this
    /// pins the one place the two exporters are legitimately allowed to differ:
    /// Prometheus quotes the value into its line, OTLP carries it as a typed
    /// attribute string. Escaping it twice would show `gpt\\"4o` in the backend.
    #[test]
    fn otlp_attributes_are_not_prometheus_escaped() {
        let m = Metrics::default();
        m.observe_variant("gpt\"4o", "b");

        let text = m.render();
        assert!(
            text.contains(r#"model="gpt\"4o""#),
            "prometheus text must escape"
        );

        let (_, attrs, _) = m
            .labelled_for_export()
            .into_iter()
            .next()
            .expect("one series");
        let model = attrs
            .iter()
            .find(|(k, _)| *k == "model")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            model,
            Some("gpt\"4o"),
            "otlp attribute must carry the raw value"
        );
    }

    /// ... and nothing is exported over OTLP that the text does not carry.
    #[test]
    fn otlp_labelled_export_has_no_extra_series() {
        let m = Metrics::default();
        m.observe_target("openai", "gpt-4o", true);
        m.observe_variant("gpt-4o", "b");
        m.observe_complexity("gpt-4o", "cheap", "mini", false);

        // one ok + one error per target, one per variant, one per complexity key
        assert_eq!(m.labelled_for_export().len(), 2 + 1 + 1);
    }

    /// Every family the exporter registers must be one the gateway actually
    /// produces, or the backend shows a permanently empty metric.
    #[test]
    fn every_declared_family_is_produced() {
        let m = Metrics::default();
        m.observe_target("openai", "gpt-4o", true);
        m.observe_variant("gpt-4o", "b");
        m.observe_complexity("gpt-4o", "cheap", "mini", false);

        let produced: Vec<&str> = m
            .labelled_for_export()
            .into_iter()
            .map(|(family, _, _)| family)
            .collect();
        for (family, help) in LABELLED_FAMILIES {
            assert!(
                produced.contains(family),
                "declared but never produced: {family}"
            );
            assert!(!help.is_empty(), "{family} has no help text");
        }
    }

    /// The OTLP histogram must use the boundaries the Prometheus one uses.
    /// Two exporters of one measurement disagreeing about bucket edges is worse
    /// than either alone: the numbers look comparable and are not.
    #[test]
    fn otlp_histogram_buckets_match_the_prometheus_boundaries() {
        let buckets = Metrics::latency_buckets_ms();
        assert_eq!(buckets.len(), LATENCY_BUCKETS_MS.len());
        for (got, want) in buckets.iter().zip(LATENCY_BUCKETS_MS.iter()) {
            assert!((got - f64::from(*want)).abs() < f64::EPSILON);
        }
    }

    /// A deployment with no OTLP endpoint never installs a recorder, and
    /// `observe_request` must stay exactly as cheap as it was.
    #[test]
    fn request_observation_works_without_an_otlp_recorder() {
        let m = Metrics::default();
        // the OnceLock is empty here, which is the default deployment
        m.observe_request("openai", "gpt-4o", 42, 7, 0);
        assert!(m.render().contains("rolter_request_latency_ms"));

        // and an inert recorder is safe to install and record into
        m.set_otlp_histograms(rolter_core::telemetry::RequestHistograms::default());
        m.observe_request("openai", "gpt-4o", 9, 3, 0);
        assert!(!rolter_core::telemetry::RequestHistograms::default().is_active());
    }

    use super::*;

    #[test]
    fn histogram_observe_and_render() {
        let m = Metrics::default();
        // three requests on one model: 3ms, 40ms, 8000ms (last > 5000 bucket)
        m.observe_request("openai", "gpt-4o", 3, 2, 0);
        m.observe_request("openai", "gpt-4o", 40, 10, 0);
        m.observe_request("openai", "gpt-4o", 8000, 200, 0);
        let out = m.render();

        // type header emitted once
        assert!(out.contains("# TYPE rolter_request_latency_ms histogram"));
        // cumulative buckets: le=5 has the 3ms obs, le=50 has 3ms+40ms
        assert!(out.contains("rolter_request_latency_ms_bucket{model=\"gpt-4o\",le=\"5\"} 1"));
        assert!(out.contains("rolter_request_latency_ms_bucket{model=\"gpt-4o\",le=\"50\"} 2"));
        // 8000ms sits above the 5000 boundary but below +Inf
        assert!(out.contains("rolter_request_latency_ms_bucket{model=\"gpt-4o\",le=\"5000\"} 2"));
        assert!(out.contains("rolter_request_latency_ms_bucket{model=\"gpt-4o\",le=\"+Inf\"} 3"));
        assert!(out.contains("rolter_request_latency_ms_sum{model=\"gpt-4o\"} 8043"));
        assert!(out.contains("rolter_request_latency_ms_count{model=\"gpt-4o\"} 3"));
        // ttft rendered as its own series
        assert!(out.contains("rolter_request_ttft_ms_count{model=\"gpt-4o\"} 3"));
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(escape_label("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn target_counters_tally_by_outcome() {
        let m = Metrics::default();
        m.observe_target("openai", "gpt-4o", true);
        m.observe_target("openai", "gpt-4o", true);
        m.observe_target("openai", "gpt-4o", false);
        // empty provider+target (builtin/pre-routing) is skipped
        m.observe_target("", "", false);
        let out = m.render();

        assert!(out.contains("# TYPE rolter_target_requests_total counter"));
        assert!(out.contains(
            "rolter_target_requests_total{provider=\"openai\",target=\"gpt-4o\",outcome=\"ok\"} 2"
        ));
        assert!(out.contains(
            "rolter_target_requests_total{provider=\"openai\",target=\"gpt-4o\",outcome=\"error\"} 1"
        ));
        // the empty-label observation created no series
        assert!(!out.contains("provider=\"\",target=\"\""));
    }

    #[test]
    fn variant_counter_tallies_and_skips_empty() {
        let m = Metrics::default();
        m.observe_variant("chat", "control");
        m.observe_variant("chat", "control");
        m.observe_variant("chat", "canary");
        // classic single-pool route (no variant) is not counted
        m.observe_variant("gpt-4o", "");
        let out = m.render();

        assert!(out.contains("# TYPE rolter_variant_requests_total counter"));
        assert!(out.contains("rolter_variant_requests_total{model=\"chat\",variant=\"control\"} 2"));
        assert!(out.contains("rolter_variant_requests_total{model=\"chat\",variant=\"canary\"} 1"));
        assert!(!out.contains("model=\"gpt-4o\""));
    }

    #[test]
    fn complexity_counter_distinguishes_selected_and_fallback_routes() {
        let metrics = Metrics::default();
        metrics.observe_complexity("router", "complex", "capable-route", false);
        metrics.observe_complexity("router", "complex", "router", true);
        let out = metrics.render();

        assert!(out.contains("# TYPE rolter_complexity_route_requests_total counter"));
        assert!(out.contains(
            "rolter_complexity_route_requests_total{model=\"router\",tier=\"complex\",route=\"capable-route\",outcome=\"selected\"} 1"
        ));
        assert!(out.contains(
            "rolter_complexity_route_requests_total{model=\"router\",tier=\"complex\",route=\"router\",outcome=\"fallback\"} 1"
        ));
    }
}
