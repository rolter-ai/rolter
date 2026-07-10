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

/// What a single probe observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// non-5xx response (401/404 included) — the API is up
    Ok,
    /// the probe itself was rate limited: the API is up, but probing must back
    /// off so the prober never contributes to tripping provider limits
    RateLimited,
    /// connection failure, timeout, or 5xx
    Failed,
}

/// Per-provider probe state machine: consecutive-failure/-success counters
/// gate the healthy flag (no single-probe flips), and a 429 on the probe
/// itself grows an exponential sweep-skipping backoff.
#[derive(Debug, Clone)]
struct ProbeState {
    healthy: bool,
    fails: u32,
    oks: u32,
    /// sweeps left to skip before probing this provider again
    backoff_remaining: u32,
    /// exponent for the next backoff window, capped
    backoff_level: u32,
}

impl Default for ProbeState {
    fn default() -> Self {
        Self {
            healthy: true,
            fails: 0,
            oks: 0,
            backoff_remaining: 0,
            backoff_level: 0,
        }
    }
}

/// longest 429-induced probe pause, in sweeps (2^3)
const MAX_BACKOFF_LEVEL: u32 = 3;

impl ProbeState {
    /// Whether the next sweep should probe this provider; consumes one skipped
    /// sweep from the backoff window when it is active.
    fn should_probe(&mut self) -> bool {
        if self.backoff_remaining > 0 {
            self.backoff_remaining -= 1;
            return false;
        }
        true
    }

    /// Fold one probe result in. Returns `Some(new_health)` when the healthy
    /// flag flipped, `None` otherwise.
    fn on_result(
        &mut self,
        outcome: ProbeOutcome,
        fail_after: u32,
        recover_after: u32,
    ) -> Option<bool> {
        match outcome {
            ProbeOutcome::Ok | ProbeOutcome::RateLimited => {
                if outcome == ProbeOutcome::RateLimited {
                    // pause probing for 2^level sweeps, growing up to the cap
                    self.backoff_remaining = 1 << self.backoff_level;
                    self.backoff_level = (self.backoff_level + 1).min(MAX_BACKOFF_LEVEL);
                } else {
                    self.backoff_level = 0;
                }
                self.fails = 0;
                self.oks = self.oks.saturating_add(1);
                if !self.healthy && self.oks >= recover_after.max(1) {
                    self.healthy = true;
                    return Some(true);
                }
            }
            ProbeOutcome::Failed => {
                self.oks = 0;
                self.fails = self.fails.saturating_add(1);
                if self.healthy && self.fails >= fail_after.max(1) {
                    self.healthy = false;
                    return Some(false);
                }
            }
        }
        None
    }
}

/// Map of provider name to its probe state.
type HealthMap = HashMap<String, ProbeState>;

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
        inner
            .lock()
            .unwrap()
            .get(provider)
            .map(|s| s.healthy)
            .unwrap_or(true)
    }

    /// Force-set a provider's health, resetting its counters. Used by tests and
    /// as an escape hatch; the prober itself goes through [`Health::observe`].
    pub fn set(&self, provider: &str, healthy: bool) {
        let Some(inner) = &self.inner else {
            return;
        };
        inner.lock().unwrap().insert(
            provider.to_string(),
            ProbeState {
                healthy,
                ..Default::default()
            },
        );
    }

    /// Whether the prober should probe `provider` this sweep (false while its
    /// 429 backoff window is active).
    pub fn should_probe(&self, provider: &str) -> bool {
        let Some(inner) = &self.inner else {
            return false;
        };
        inner
            .lock()
            .unwrap()
            .entry(provider.to_string())
            .or_default()
            .should_probe()
    }

    /// Fold a probe outcome into `provider`'s state machine. Returns the new
    /// healthy flag when it flipped.
    pub fn observe(
        &self,
        provider: &str,
        outcome: ProbeOutcome,
        fail_after: u32,
        recover_after: u32,
    ) -> Option<bool> {
        let inner = self.inner.as_ref()?;
        inner
            .lock()
            .unwrap()
            .entry(provider.to_string())
            .or_default()
            .on_result(outcome, fail_after, recover_after)
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
    // probes run concurrently but bounded, so a sweep can never stampede
    // upstreams no matter how many providers are configured
    let limiter = Arc::new(tokio::sync::Semaphore::new(cfg.probe_concurrency.max(1)));
    // spread probes across the first quarter of the interval so sweeps for
    // different providers never align into a synchronized burst
    let jitter_window_ms = (cfg.interval_secs.max(1) * 1000 / 4).min(2000);
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
        let mut sweep = tokio::task::JoinSet::new();
        for (name, api_base, kind) in providers {
            // a provider inside its 429 backoff window sits this sweep out
            if !state.health.should_probe(&name) {
                continue;
            }
            let (url, header) = probe_request(kind, &api_base, &cfg.path);
            let client = client.clone();
            let limiter = limiter.clone();
            let jitter_ms = probe_jitter_ms(&name, jitter_window_ms);
            sweep.spawn(async move {
                let _permit = limiter.acquire_owned().await.ok()?;
                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                let mut req = client.get(&url);
                if let Some((k, v)) = header {
                    req = req.header(k, v);
                }
                let started = std::time::Instant::now();
                let (outcome, status, timed_out) = match req.send().await {
                    Ok(resp) => {
                        let code = resp.status().as_u16();
                        let out = match code {
                            429 => ProbeOutcome::RateLimited,
                            s if s < 500 => ProbeOutcome::Ok,
                            _ => ProbeOutcome::Failed,
                        };
                        (out, Some(code), false)
                    }
                    Err(e) => (ProbeOutcome::Failed, None, e.is_timeout()),
                };
                let latency_ms = started.elapsed().as_millis() as u32;
                Some((name, outcome, status, latency_ms, timed_out))
            });
        }
        while let Some(joined) = sweep.join_next().await {
            let Ok(Some((name, outcome, status, latency_ms, timed_out))) = joined else {
                continue;
            };
            // record a probe health event for every sweep observation (ROL-197)
            state.health_events.emit(probe_health_event(
                &name, outcome, status, latency_ms, timed_out,
            ));
            let flipped = state.health.observe(
                &name,
                outcome,
                cfg.consecutive_failure_threshold,
                cfg.recovery_success_threshold,
            );
            match flipped {
                Some(true) => {
                    state
                        .metrics
                        .health_recovered_total
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Some(false) => {
                    state
                        .metrics
                        .health_down_total
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                None => {}
            }
        }
    }
}

/// Build a probe [`HealthEvent`](crate::health_events::HealthEvent) from a sweep
/// observation. Probes are per-provider, so `target_id` carries the provider
/// name. A 429 is `error`/`rate_limited`; a client timeout is `timeout`; any
/// other failure is `error`.
fn probe_health_event(
    provider: &str,
    outcome: ProbeOutcome,
    status: Option<u16>,
    latency_ms: u32,
    timed_out: bool,
) -> crate::health_events::HealthEvent {
    use crate::health_events::{HealthEvent, HealthOutcome, HealthSource};
    let (health_outcome, error_kind) = match outcome {
        ProbeOutcome::Ok => (HealthOutcome::Ok, None),
        ProbeOutcome::RateLimited => (HealthOutcome::Error, Some("rate_limited".to_string())),
        ProbeOutcome::Failed if timed_out => (HealthOutcome::Timeout, Some("timeout".to_string())),
        ProbeOutcome::Failed => {
            let kind = match status {
                Some(s) if s >= 500 => "upstream_error",
                Some(_) => "error",
                None => "connect_error",
            };
            (HealthOutcome::Error, Some(kind.to_string()))
        }
    };
    HealthEvent {
        target_id: provider.to_string(),
        provider: provider.to_string(),
        source: HealthSource::Probe,
        outcome: health_outcome,
        status_code: status,
        latency_ms,
        error_kind,
    }
}

/// Deterministic, dependency-free per-provider jitter in `[0, window_ms)`,
/// derived from the provider name so each provider keeps a stable offset
/// within the sweep instead of all probes firing at the tick.
fn probe_jitter_ms(name: &str, window_ms: u64) -> u64 {
    if window_ms == 0 {
        return 0;
    }
    let hash: u64 = name
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    hash % window_ms
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
    fn unhealthy_needs_consecutive_failures() {
        let h = Health::new();
        // two failures under a threshold of 3: still healthy, no flip reported
        assert_eq!(h.observe("p", ProbeOutcome::Failed, 3, 2), None);
        assert_eq!(h.observe("p", ProbeOutcome::Failed, 3, 2), None);
        assert!(h.is_healthy("p"));
        // a success in between resets the streak
        assert_eq!(h.observe("p", ProbeOutcome::Ok, 3, 2), None);
        assert_eq!(h.observe("p", ProbeOutcome::Failed, 3, 2), None);
        assert_eq!(h.observe("p", ProbeOutcome::Failed, 3, 2), None);
        assert!(h.is_healthy("p"));
        // the third consecutive failure trips it
        assert_eq!(h.observe("p", ProbeOutcome::Failed, 3, 2), Some(false));
        assert!(!h.is_healthy("p"));
    }

    #[test]
    fn recovery_needs_consecutive_successes() {
        let h = Health::new();
        h.set("p", false);
        // one success under a threshold of 2: still unhealthy
        assert_eq!(h.observe("p", ProbeOutcome::Ok, 3, 2), None);
        assert!(!h.is_healthy("p"));
        // a failure resets the recovery streak
        assert_eq!(h.observe("p", ProbeOutcome::Failed, 3, 2), None);
        assert_eq!(h.observe("p", ProbeOutcome::Ok, 3, 2), None);
        assert!(!h.is_healthy("p"));
        // the second consecutive success recovers
        assert_eq!(h.observe("p", ProbeOutcome::Ok, 3, 2), Some(true));
        assert!(h.is_healthy("p"));
    }

    #[test]
    fn rate_limited_probe_backs_off_exponentially() {
        let h = Health::new();
        // 429 counts as alive, never trips unhealthy
        assert_eq!(h.observe("p", ProbeOutcome::RateLimited, 3, 2), None);
        assert!(h.is_healthy("p"));
        // first backoff window: skip exactly one sweep
        assert!(!h.should_probe("p"));
        assert!(h.should_probe("p"));
        // second consecutive 429 doubles the window
        h.observe("p", ProbeOutcome::RateLimited, 3, 2);
        assert!(!h.should_probe("p"));
        assert!(!h.should_probe("p"));
        assert!(h.should_probe("p"));
        // an ok probe resets the backoff level: next 429 skips one sweep again
        h.observe("p", ProbeOutcome::Ok, 3, 2);
        h.observe("p", ProbeOutcome::RateLimited, 3, 2);
        assert!(!h.should_probe("p"));
        assert!(h.should_probe("p"));
    }

    #[test]
    fn backoff_level_is_capped() {
        let mut s = ProbeState::default();
        for _ in 0..10 {
            s.on_result(ProbeOutcome::RateLimited, 3, 2);
        }
        assert_eq!(s.backoff_remaining, 1 << MAX_BACKOFF_LEVEL);
    }

    #[test]
    fn jitter_is_stable_and_bounded() {
        let a = probe_jitter_ms("openai", 2000);
        assert_eq!(a, probe_jitter_ms("openai", 2000));
        assert!(a < 2000);
        assert_eq!(probe_jitter_ms("anything", 0), 0);
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
