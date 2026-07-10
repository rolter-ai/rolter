//! Active upstream health checks. When enabled, a background task periodically
//! probes each provider's `api_base` and records whether it is reachable; the
//! balancer then skips targets whose provider is currently unhealthy. State lives
//! outside the routing snapshot (it must survive config hot-reloads) and is keyed
//! by provider name.
//!
//! Probing is deliberately forgiving: any response that is not a `5xx` (including
//! `401`/`404`) counts as healthy, since upstreams rarely expose a dedicated
//! health route. Only connection failures, timeouts, and server errors mark a
//! provider down. When every target of a route is unhealthy the caller fails open
//! rather than rejecting the request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rolter_core::{GatewayConfig, HealthConfig, ProviderKind};

/// the anthropic messages api rejects requests without a version header, even on
/// the free `GET /v1/models` list endpoint
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Resolve the probe request for a provider: the URL and any header the upstream
/// requires. When the operator left `health.path` at its default (`/`), probe the
/// provider kind's free, non-inference liveness endpoint (`/v1/models` — a list
/// call that burns no tokens) so a healthy result means the API itself is up, not
/// merely that the host answers TCP. An explicit non-default `path` is honoured
/// verbatim for every provider.
fn probe_request(
    kind: ProviderKind,
    api_base: &str,
    configured_path: &str,
) -> (String, Option<(&'static str, &'static str)>) {
    let base = api_base.trim_end_matches('/');
    if configured_path != "/" {
        return (format!("{base}{configured_path}"), None);
    }
    match kind {
        ProviderKind::Openai | ProviderKind::OpenaiCompatible => {
            (format!("{base}/v1/models"), None)
        }
        ProviderKind::Anthropic => (
            format!("{base}/v1/models"),
            Some(("anthropic-version", ANTHROPIC_VERSION)),
        ),
    }
}

/// Map of provider name to its last observed health (`true` = healthy).
type HealthMap = HashMap<String, bool>;

/// Shared, cheaply-cloneable registry of provider health. The derived default
/// (and any instance built from a disabled config) has no backing map and reports
/// every provider healthy — i.e. probing is inert and the balancer never skips.
#[derive(Clone, Default)]
pub struct Health {
    inner: Option<Arc<Mutex<HealthMap>>>,
}

impl Health {
    /// An enabled registry with an empty map. Until the first probe sweep lands,
    /// every provider is treated as healthy.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    /// Whether `provider` may currently receive traffic. Unknown providers (not
    /// yet probed) and a disabled registry both report healthy — fail open.
    pub fn is_healthy(&self, provider: &str) -> bool {
        let Some(inner) = &self.inner else {
            return true;
        };
        inner.lock().unwrap().get(provider).copied().unwrap_or(true)
    }

    /// Record the latest probe result for `provider`.
    pub fn set(&self, provider: &str, healthy: bool) {
        let Some(inner) = &self.inner else {
            return;
        };
        inner.lock().unwrap().insert(provider.to_string(), healthy);
    }
}

/// Spawn the background prober. Sweeps every provider in the current snapshot
/// once per `interval_secs`, issuing a lightweight `GET {api_base}{path}` with a
/// per-probe timeout, and records each provider's health. Runs until the process
/// exits. A no-op (returns without spawning) when probing is disabled.
pub fn spawn_prober(config: &GatewayConfig, state: crate::state::AppState) {
    if !config.health.enabled {
        return;
    }
    let cfg = config.health.clone();
    tokio::spawn(async move {
        run_prober(cfg, state).await;
    });
}

async fn run_prober(cfg: HealthConfig, state: crate::state::AppState) {
    // a dedicated client so probe timeouts never interfere with forward traffic
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
        // newly-added providers are picked up without restarting the prober
        let providers: Vec<(String, String, ProviderKind)> = {
            let snap = state.snapshot.load();
            snap.providers
                .values()
                .map(|p| (p.name.clone(), p.api_base.clone(), p.kind))
                .collect()
        };
        for (name, api_base, kind) in providers {
            let (url, header) = probe_request(kind, &api_base, &cfg.path);
            let mut req = client.get(&url);
            if let Some((k, v)) = header {
                req = req.header(k, v);
            }
            let healthy = match req.send().await {
                Ok(resp) => resp.status().as_u16() < 500,
                Err(_) => false,
            };
            let was = state.health.is_healthy(&name);
            state.health.set(&name, healthy);
            if was != healthy {
                if healthy {
                    state
                        .metrics
                        .health_recovered_total
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    state
                        .metrics
                        .health_down_total
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_reports_healthy() {
        let h = Health::default();
        assert!(h.is_healthy("anything"));
        // set is inert on a disabled registry
        h.set("anything", false);
        assert!(h.is_healthy("anything"));
    }

    #[test]
    fn records_and_reports() {
        let h = Health::new();
        // unknown provider fails open
        assert!(h.is_healthy("p"));
        h.set("p", false);
        assert!(!h.is_healthy("p"));
        h.set("p", true);
        assert!(h.is_healthy("p"));
    }

    #[test]
    fn default_path_uses_kind_free_endpoint() {
        // openai + compatible: /v1/models, no header
        let (url, hdr) = probe_request(ProviderKind::Openai, "https://api.openai.com/", "/");
        assert_eq!(url, "https://api.openai.com/v1/models");
        assert!(hdr.is_none());
        let (url, _) = probe_request(ProviderKind::OpenaiCompatible, "http://vllm:8000", "/");
        assert_eq!(url, "http://vllm:8000/v1/models");
        // anthropic: /v1/models plus the required version header
        let (url, hdr) = probe_request(ProviderKind::Anthropic, "https://api.anthropic.com", "/");
        assert_eq!(url, "https://api.anthropic.com/v1/models");
        assert_eq!(hdr, Some(("anthropic-version", ANTHROPIC_VERSION)));
    }

    #[test]
    fn explicit_path_overrides_kind_default() {
        // a non-default path is honoured verbatim, with no injected header
        let (url, hdr) = probe_request(ProviderKind::Anthropic, "https://x.test", "/healthz");
        assert_eq!(url, "https://x.test/healthz");
        assert!(hdr.is_none());
    }
}
