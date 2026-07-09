use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Lightweight Prometheus counters rendered as text.
///
/// The MVP hand-rolls a few counters to avoid pulling the full metrics stack;
/// the roadmap swaps this for the `metrics` facade plus a prometheus exporter
/// with latency histograms and per-route labels.
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
    /// times a health probe transitioned a provider from healthy to down
    pub health_down_total: AtomicU64,
    /// times a health probe transitioned a provider from down to healthy
    pub health_recovered_total: AtomicU64,
    /// times a circuit breaker tripped a target open after sustained failures
    pub breaker_opened_total: AtomicU64,
    /// times a circuit breaker closed a target after a successful half-open probe
    pub breaker_closed_total: AtomicU64,
}

impl Metrics {
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
        out
    }
}

/// Append one Prometheus metric (HELP + TYPE + value line) to `out`.
fn metric(out: &mut String, kind: &str, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "{name} {value}");
}
