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
}

impl Metrics {
    /// Render the counters in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut counter = |name: &str, help: &str, value: u64| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} counter");
            let _ = writeln!(out, "{name} {value}");
        };
        counter(
            "rolter_requests_total",
            "total proxied requests",
            self.requests_total.load(Relaxed),
        );
        counter(
            "rolter_upstream_errors_total",
            "upstream request failures",
            self.upstream_errors_total.load(Relaxed),
        );
        counter(
            "rolter_auth_failures_total",
            "requests rejected due to auth",
            self.auth_failures_total.load(Relaxed),
        );
        out
    }
}
