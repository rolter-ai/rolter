//! Background scrape of upstream engine `/metrics`. When enabled, a task pulls
//! each provider's Prometheus text exposition periodically, parses out the
//! queue depth (requests waiting to be scheduled), and publishes it into a
//! lock-free [`arc_swap`] snapshot. The request path folds this per-provider
//! depth into the balancer's in-flight load view so load-aware strategies steer
//! away from backed-up engines.
//!
//! State lives outside the routing snapshot so it survives config hot-reloads,
//! and reads never take a lock (honoring the "no locks on the hot path" rule).
//! A disabled registry (the derived default) reports zero depth for everything,
//! leaving the balancer's own in-flight counts untouched.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use rolter_core::{GatewayConfig, MetricsScrapeConfig};

/// Per-provider scraped signals. Extend with latency/utilization as more
/// scorers land; today it carries the scheduler queue depth.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderMetrics {
    /// requests waiting to be scheduled upstream (higher = more backed up)
    pub queue_depth: u64,
}

/// Provider name to its latest scraped metrics.
type MetricsMap = HashMap<String, ProviderMetrics>;

/// Shared, cheaply-cloneable snapshot of upstream metrics. The derived default
/// has no backing store and reports zero depth for every provider — i.e.
/// scraping is inert and contributes nothing to the load view.
#[derive(Clone, Default)]
pub struct UpstreamMetrics {
    inner: Option<Arc<ArcSwap<MetricsMap>>>,
}

impl UpstreamMetrics {
    /// An enabled registry with an empty snapshot. Until the first scrape lands,
    /// every provider reports zero depth.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(ArcSwap::from_pointee(HashMap::new()))),
        }
    }

    /// Latest scraped queue depth for `provider`. Unknown providers and a
    /// disabled registry both report `0`.
    pub fn queue_depth(&self, provider: &str) -> u64 {
        let Some(inner) = &self.inner else {
            return 0;
        };
        inner
            .load()
            .get(provider)
            .map(|m| m.queue_depth)
            .unwrap_or(0)
    }

    /// Atomically replace the whole snapshot with a freshly scraped sweep.
    fn store(&self, map: MetricsMap) {
        if let Some(inner) = &self.inner {
            inner.store(Arc::new(map));
        }
    }
}

/// Prometheus metric names that expose the scheduler queue depth, in priority
/// order (vLLM, then generic/SGLang/TGI fallbacks). The first one present in a
/// scrape wins.
const QUEUE_DEPTH_METRICS: &[&str] = &[
    "vllm:num_requests_waiting",
    "sglang:num_queue_reqs",
    "num_requests_waiting",
    "tgi_queue_size",
];

/// Sum the samples of the first present queue-depth metric in a Prometheus text
/// exposition. Comment lines (`# HELP`/`# TYPE`) are ignored; labelled series
/// (`name{...} value`) are summed across all label sets. Returns `0` when none
/// of the known metrics appear.
fn parse_queue_depth(body: &str) -> u64 {
    for metric in QUEUE_DEPTH_METRICS {
        let mut total = 0f64;
        let mut seen = false;
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // a sample line is `name value` or `name{labels} value`; match the
            // metric name up to a space or the opening brace
            let name_end = line.find([' ', '{']).unwrap_or(line.len());
            if &line[..name_end] != *metric {
                continue;
            }
            if let Some(val) = line.rsplit(char::is_whitespace).next() {
                if let Ok(v) = val.parse::<f64>() {
                    total += v;
                    seen = true;
                }
            }
        }
        if seen {
            // depth is a count; clamp negatives/NaN to 0 and round to whole reqs
            return if total.is_finite() && total > 0.0 {
                total.round() as u64
            } else {
                0
            };
        }
    }
    0
}

/// Spawn the background scraper. Sweeps every provider in the current snapshot
/// once per `interval_secs`, issuing `GET {api_base}{path}`, parses the queue
/// depth, and publishes a fresh snapshot. Runs until the process exits. A no-op
/// (returns without spawning) when scraping is disabled.
pub fn spawn_scraper(config: &GatewayConfig, state: crate::state::AppState) {
    if !config.metrics_scrape.enabled {
        return;
    }
    let cfg = config.metrics_scrape.clone();
    tokio::spawn(async move {
        run_scraper(cfg, state).await;
    });
}

async fn run_scraper(cfg: MetricsScrapeConfig, state: crate::state::AppState) {
    // a dedicated client so scrape timeouts never interfere with forward traffic
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut ticker = tokio::time::interval(Duration::from_secs(cfg.interval_secs.max(1)));
    loop {
        ticker.tick().await;
        // read providers off the current snapshot each sweep so hot-reloads and
        // newly-added providers are picked up without restarting the scraper
        let providers: Vec<(String, String)> = {
            let snap = state.snapshot.load();
            snap.providers
                .values()
                .map(|p| (p.name.clone(), p.api_base.clone()))
                .collect()
        };
        let mut map = MetricsMap::with_capacity(providers.len());
        for (name, api_base) in providers {
            let url = format!("{}{}", api_base.trim_end_matches('/'), cfg.path);
            let depth = match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => resp
                    .text()
                    .await
                    .map(|b| parse_queue_depth(&b))
                    .unwrap_or(0),
                _ => 0,
            };
            map.insert(name, ProviderMetrics { queue_depth: depth });
        }
        state.upstream_metrics.store(map);
        state
            .metrics
            .metrics_scrapes_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_reports_zero() {
        let m = UpstreamMetrics::default();
        assert_eq!(m.queue_depth("anything"), 0);
        // store is inert on a disabled registry
        m.store(HashMap::from([(
            "p".to_string(),
            ProviderMetrics { queue_depth: 9 },
        )]));
        assert_eq!(m.queue_depth("p"), 0);
    }

    #[test]
    fn stores_and_reports_depth() {
        let m = UpstreamMetrics::new();
        // unknown provider reads zero
        assert_eq!(m.queue_depth("p"), 0);
        m.store(HashMap::from([(
            "p".to_string(),
            ProviderMetrics { queue_depth: 4 },
        )]));
        assert_eq!(m.queue_depth("p"), 4);
    }

    #[test]
    fn parses_vllm_queue_depth() {
        let body = "\
# HELP vllm:num_requests_waiting Number of requests waiting to be processed.
# TYPE vllm:num_requests_waiting gauge
vllm:num_requests_waiting{model_name=\"llama\"} 7.0
";
        assert_eq!(parse_queue_depth(body), 7);
    }

    #[test]
    fn sums_labelled_series() {
        let body = "\
vllm:num_requests_waiting{engine=\"0\"} 3
vllm:num_requests_waiting{engine=\"1\"} 5
";
        assert_eq!(parse_queue_depth(body), 8);
    }

    #[test]
    fn falls_back_to_generic_metric() {
        let body = "num_requests_waiting 2\n";
        assert_eq!(parse_queue_depth(body), 2);
    }

    #[test]
    fn missing_metric_is_zero() {
        assert_eq!(parse_queue_depth("some_other_metric 5\n"), 0);
        assert_eq!(parse_queue_depth(""), 0);
    }

    #[test]
    fn first_present_metric_wins_over_later() {
        // vllm metric present -> the later generic fallback is not summed in
        let body = "\
vllm:num_requests_waiting 1
num_requests_waiting 100
";
        assert_eq!(parse_queue_depth(body), 1);
    }
}
