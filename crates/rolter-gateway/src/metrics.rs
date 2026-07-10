use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use dashmap::DashMap;

/// Upper bounds (milliseconds) of the request-latency / TTFT histogram buckets.
/// The implicit `+Inf` bucket catches everything above the last boundary and is
/// represented by the observation `count`.
const LATENCY_BUCKETS_MS: [u32; 13] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000];

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
    /// request-log rows successfully written to clickhouse
    pub logs_written_total: AtomicU64,
    /// request-log rows dropped (queue full or write failed)
    pub logs_dropped_total: AtomicU64,
    /// requests rejected because a matching budget was exhausted
    pub budget_blocks_total: AtomicU64,
    /// requests rejected because a matching rpm/tpm rate limit was exhausted
    pub rate_limit_blocks_total: AtomicU64,
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
    /// times a circuit breaker tripped a target open after sustained failures
    pub breaker_opened_total: AtomicU64,
    /// times a circuit breaker closed a target after a successful half-open probe
    pub breaker_closed_total: AtomicU64,
    /// upstream `/metrics` scrape sweeps completed
    pub metrics_scrapes_total: AtomicU64,
    /// per-model latency + TTFT histograms, keyed by public model name
    by_model: DashMap<String, ModelHist>,
    /// passive per-target success/error tally, keyed by (provider, target)
    by_target: DashMap<(String, String), TargetStats>,
    /// per-variant request tally for A/B attribution, keyed by (model, variant)
    by_variant: DashMap<(String, String), AtomicU64>,
}

impl Metrics {
    /// Record one completed request's total latency and time-to-first-token
    /// against the `model` label. Called once per request from the log sink.
    pub fn observe_request(&self, model: &str, latency_ms: u32, ttft_ms: u32) {
        let hist = self
            .by_model
            .entry(model.to_string())
            .or_insert_with(ModelHist::new);
        hist.latency.observe(latency_ms);
        hist.ttft.observe(ttft_ms);
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

    /// Render the counters in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::new();
        metric(
            &mut out,
            "counter",
            "rolter_requests_total",
            "total proxied requests",
            self.requests_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_upstream_errors_total",
            "upstream request failures",
            self.upstream_errors_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_auth_failures_total",
            "requests rejected due to auth",
            self.auth_failures_total.load(Relaxed),
        );
        metric(
            &mut out,
            "gauge",
            "rolter_config_version",
            "config version applied to the live snapshot",
            self.config_version.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_config_reloads_total",
            "successful config hot-reloads applied",
            self.config_reloads_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_config_reload_failures_total",
            "failed config snapshot fetches",
            self.config_reload_failures_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_logs_written_total",
            "request-log rows written to clickhouse",
            self.logs_written_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_logs_dropped_total",
            "request-log rows dropped (queue full or write failed)",
            self.logs_dropped_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_budget_blocks_total",
            "requests rejected due to an exhausted budget",
            self.budget_blocks_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_rate_limit_blocks_total",
            "requests rejected due to an exhausted rate limit",
            self.rate_limit_blocks_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_retries_total",
            "upstream attempts retried after a transient failure",
            self.retries_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_cooldowns_tripped_total",
            "targets parked on a cooldown after a transient failure",
            self.cooldowns_tripped_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_key_cooldowns_tripped_total",
            "api keys parked on a cooldown after a key-level failure",
            self.key_cooldowns_tripped_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_health_down_total",
            "providers marked unhealthy by an active health probe",
            self.health_down_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_health_recovered_total",
            "providers restored to healthy by an active health probe",
            self.health_recovered_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_breaker_opened_total",
            "targets tripped open by the circuit breaker after sustained failures",
            self.breaker_opened_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_breaker_closed_total",
            "targets closed by the circuit breaker after a successful half-open probe",
            self.breaker_closed_total.load(Relaxed),
        );
        metric(
            &mut out,
            "counter",
            "rolter_metrics_scrapes_total",
            "upstream /metrics scrape sweeps completed",
            self.metrics_scrapes_total.load(Relaxed),
        );
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
        out
    }

    /// Append the per-variant request counter, one `{model,variant}` series per
    /// (model, variant). Lets Prometheus/Grafana show A/B traffic splits without
    /// querying ClickHouse.
    fn render_variant_counters(&self, out: &mut String) {
        let name = "rolter_variant_requests_total";
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
        let name = "rolter_target_requests_total";
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
    use super::*;

    #[test]
    fn histogram_observe_and_render() {
        let m = Metrics::default();
        // three requests on one model: 3ms, 40ms, 8000ms (last > 5000 bucket)
        m.observe_request("gpt-4o", 3, 2);
        m.observe_request("gpt-4o", 40, 10);
        m.observe_request("gpt-4o", 8000, 200);
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
}
