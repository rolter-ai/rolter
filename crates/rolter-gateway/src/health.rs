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

use rolter_core::{HealthConfig, ProviderKind};

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
) -> (String, Vec<(String, String)>) {
    let base = api_base.trim_end_matches('/');
    if configured_path != "/" {
        return (format!("{base}{configured_path}"), Vec::new());
    }
    match kind {
        ProviderKind::Openai
        | ProviderKind::OpenaiCompatible
        | ProviderKind::Ollama
        | ProviderKind::OllamaCloud
        | ProviderKind::LlamaCpp => (format!("{base}/v1/models"), Vec::new()),
        ProviderKind::Openrouter
        | ProviderKind::Gemini
        | ProviderKind::GeminiNative
        | ProviderKind::Mistral
        | ProviderKind::Groq
        | ProviderKind::Xai => (format!("{base}/models"), Vec::new()),
        ProviderKind::Tei => (format!("{base}/health"), Vec::new()),
        ProviderKind::AzureOpenai => (format!("{base}/models"), Vec::new()),
        ProviderKind::Bedrock => (bedrock_models_url(base), Vec::new()),
        ProviderKind::Vertex => (vertex_models_url(base), Vec::new()),
        ProviderKind::Anthropic => (
            format!("{base}/v1/models"),
            vec![(
                "anthropic-version".to_string(),
                ANTHROPIC_VERSION.to_string(),
            )],
        ),
    }
}

fn bedrock_models_url(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if let Some(control) = base.strip_prefix("https://bedrock-runtime.") {
        let host = control.split('/').next().unwrap_or(control);
        return format!("https://bedrock.{host}/foundation-models");
    }
    format!("{}/foundation-models", base.trim_end_matches("/v1"))
}

fn vertex_models_url(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if let Some(prefix) = base.strip_suffix("/endpoints/openapi") {
        return format!("{prefix}/publishers/google/models");
    }
    format!("{base}/models")
}

/// A fully-resolved probe request for one provider: either a free liveness GET
/// or an opt-in minimal completion POST (`also_track_via_llm_call`, ROL-199).
/// Owns every value so the spawned sweep task needs no borrow of the config.
enum ProbePlan {
    /// free, non-inference liveness GET
    Free {
        url: String,
        headers: Vec<(String, String)>,
    },
    /// a real `max_tokens = 1` completion that proves inference works end to end
    LlmCall {
        url: String,
        headers: Vec<(String, String)>,
        body: String,
    },
}

impl ProbePlan {
    fn build(self, client: &reqwest::Client) -> reqwest::RequestBuilder {
        match self {
            ProbePlan::Free { url, headers } => {
                let mut req = client.get(&url);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                req
            }
            ProbePlan::LlmCall { url, headers, body } => {
                let mut req = client
                    .post(&url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json");
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                req.body(body)
            }
        }
    }
}

/// Decide how to probe `provider`: the opt-in llm-call check when enabled and
/// fully configured, else the free liveness probe. Falls back to the free probe
/// (with a warning) when the flag is on but the model or api key is missing, so
/// a misconfiguration degrades to the free signal rather than silently reporting
/// the provider down.
fn build_probe_plan(
    provider: &rolter_core::ProviderConfig,
    configured_path: &str,
) -> (ProbePlan, crate::health_events::HealthSource) {
    use crate::health_events::HealthSource;
    let base = provider.api_base.trim_end_matches('/');
    if provider.also_track_via_llm_call {
        let key = provider.resolve_api_key();
        if let Some(model) = &provider.llm_probe_model {
            if key.is_some() || provider.kind == ProviderKind::Ollama {
                let (path, headers) = match provider.kind {
                    ProviderKind::Anthropic => (
                        "/v1/messages",
                        vec![
                            ("x-api-key".to_string(), key.unwrap_or_default()),
                            (
                                "anthropic-version".to_string(),
                                ANTHROPIC_VERSION.to_string(),
                            ),
                        ],
                    ),
                    ProviderKind::Openrouter => {
                        let headers = key
                            .map(|key| vec![("authorization".to_string(), format!("Bearer {key}"))])
                            .unwrap_or_default();
                        ("/chat/completions", headers)
                    }
                    ProviderKind::AzureOpenai => {
                        let headers = key
                            .map(|key| vec![("api-key".to_string(), key)])
                            .unwrap_or_default();
                        ("/chat/completions", headers)
                    }
                    ProviderKind::Bedrock | ProviderKind::Vertex => {
                        let headers = key
                            .map(|key| vec![("authorization".to_string(), format!("Bearer {key}"))])
                            .unwrap_or_default();
                        ("/chat/completions", headers)
                    }
                    _ => {
                        let headers = key
                            .map(|key| vec![("authorization".to_string(), format!("Bearer {key}"))])
                            .unwrap_or_default();
                        ("/v1/chat/completions", headers)
                    }
                };
                let body = format!(
                    "{{\"model\":{},\"messages\":[{{\"role\":\"user\",\"content\":\"ping\"}}],\"max_tokens\":1}}",
                    serde_json::Value::String(model.clone())
                );
                return (
                    ProbePlan::LlmCall {
                        url: format!("{base}{path}"),
                        headers,
                        body,
                    },
                    HealthSource::LlmCall,
                );
            }
        }
        tracing::warn!(
            provider = %provider.name,
            "also_track_via_llm_call is set but llm_probe_model or required api key is missing; \
             falling back to the free liveness probe"
        );
    }
    let (url, mut headers) = probe_request(provider.kind, &provider.api_base, configured_path);
    if let Some(key) = provider.resolve_api_key() {
        match provider.kind {
            ProviderKind::AzureOpenai => headers.push(("api-key".to_string(), key)),
            ProviderKind::OllamaCloud
            | ProviderKind::Openrouter
            | ProviderKind::Bedrock
            | ProviderKind::Vertex
            | ProviderKind::Gemini
            | ProviderKind::GeminiNative
            | ProviderKind::Mistral
            | ProviderKind::Groq
            | ProviderKind::Xai => {
                headers.push(("authorization".to_string(), format!("Bearer {key}")));
            }
            _ => {}
        }
    }
    (ProbePlan::Free { url, headers }, HealthSource::Probe)
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

    /// Drop all per-provider probe state, so every provider reads healthy again.
    /// Called when a hot-reload disables probing: a target parked unhealthy by a
    /// now-disabled prober must not stay skipped forever.
    pub fn clear(&self) {
        if let Some(inner) = &self.inner {
            inner.lock().unwrap().clear();
        }
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
/// exits.
///
/// The prober is always spawned; it reads its enable-state and tuning
/// (interval/timeout/path/concurrency/thresholds) off the live snapshot each
/// sweep, so a config hot-reload can turn probing on or off and re-tune it without
/// a restart (ROL-125). While disabled it idles and leaves every provider healthy.
pub fn spawn_prober(state: crate::state::AppState) {
    tokio::spawn(run_prober(state));
}

async fn run_prober(state: crate::state::AppState) {
    // tracks the last observed enable-state so a disable transition can clear any
    // provider left parked unhealthy by the now-stopped prober
    let mut was_enabled = false;
    loop {
        // re-read the health tuning off the current snapshot every iteration so a
        // hot-reload of enabled/interval/timeout/path/concurrency takes effect on
        // the next sweep without restarting the task
        let cfg = state.snapshot.load().health.clone();
        let idle = Duration::from_secs(cfg.interval_secs.max(1));
        if !cfg.enabled {
            if was_enabled {
                state.health.clear();
                was_enabled = false;
            }
            tokio::time::sleep(idle).await;
            continue;
        }
        was_enabled = true;
        run_sweep(&cfg, &state).await;
        tokio::time::sleep(idle).await;
    }
}

/// Run a single probe sweep across every provider in the current snapshot using
/// the supplied (live) tuning.
async fn run_sweep(cfg: &HealthConfig, state: &crate::state::AppState) {
    // probes run concurrently but bounded, so a sweep can never stampede
    // upstreams no matter how many providers are configured
    let limiter = Arc::new(tokio::sync::Semaphore::new(cfg.probe_concurrency.max(1)));
    // spread probes across the first quarter of the interval so sweeps for
    // different providers never align into a synchronized burst
    let jitter_window_ms = (cfg.interval_secs.max(1) * 1000 / 4).min(2000);
    {
        // read providers off the current snapshot so hot-reloads and newly-added
        // providers are picked up. resolve the whole probe plan here (while we
        // hold the config) so the spawned tasks own everything they need
        let plans: Vec<(
            String,
            ProbePlan,
            crate::health_events::HealthSource,
            reqwest::Client,
        )> = {
            let snap = state.snapshot.load();
            snap.providers
                .values()
                .filter_map(|p| {
                    let (plan, source) = build_probe_plan(p, &cfg.path);
                    match state.forwarder.client_for(p) {
                        Ok(client) => Some((p.name.clone(), plan, source, client)),
                        Err(error) => {
                            tracing::warn!(provider = %p.name, %error, "cannot build health-probe TLS client");
                            None
                        }
                    }
                })
                .collect()
        };
        let mut sweep = tokio::task::JoinSet::new();
        for (name, plan, source, client) in plans {
            // a provider inside its 429 backoff window sits this sweep out
            if !state.health.should_probe(&name) {
                continue;
            }
            let limiter = limiter.clone();
            let jitter_ms = probe_jitter_ms(&name, jitter_window_ms);
            let timeout = Duration::from_secs(cfg.timeout_secs.max(1));
            sweep.spawn(async move {
                let _permit = limiter.acquire_owned().await.ok()?;
                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                let req = plan.build(&client);
                let started = std::time::Instant::now();
                let (outcome, status, timed_out) =
                    match tokio::time::timeout(timeout, req.send()).await {
                        Ok(Ok(resp)) => {
                            let code = resp.status().as_u16();
                            let out = match code {
                                429 => ProbeOutcome::RateLimited,
                                s if s < 500 => ProbeOutcome::Ok,
                                _ => ProbeOutcome::Failed,
                            };
                            (out, Some(code), false)
                        }
                        Ok(Err(e)) => (ProbeOutcome::Failed, None, e.is_timeout()),
                        Err(_) => (ProbeOutcome::Failed, None, true),
                    };
                let latency_ms = started.elapsed().as_millis() as u32;
                Some((name, source, outcome, status, latency_ms, timed_out))
            });
        }
        while let Some(joined) = sweep.join_next().await {
            let Ok(Some((name, source, outcome, status, latency_ms, timed_out))) = joined else {
                continue;
            };
            // record a health event for every sweep observation (ROL-197); the
            // source distinguishes free probes from llm-call checks (ROL-199)
            state.health_events.emit(probe_health_event(
                &name, source, outcome, status, latency_ms, timed_out,
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
    source: crate::health_events::HealthSource,
    outcome: ProbeOutcome,
    status: Option<u16>,
    latency_ms: u32,
    timed_out: bool,
) -> crate::health_events::HealthEvent {
    use crate::health_events::{HealthEvent, HealthOutcome};
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
        source,
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
    fn clear_resets_parked_providers_to_healthy() {
        let h = Health::new();
        h.set("p", false);
        assert!(!h.is_healthy("p"));
        // disabling the prober clears state so a parked provider is not stuck
        // unhealthy forever
        h.clear();
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
        assert!(hdr.is_empty());
        let (url, _) = probe_request(ProviderKind::OpenaiCompatible, "http://vllm:8000", "/");
        assert_eq!(url, "http://vllm:8000/v1/models");
        // anthropic: /v1/models plus the required version header
        let (url, hdr) = probe_request(ProviderKind::Anthropic, "https://api.anthropic.com", "/");
        assert_eq!(url, "https://api.anthropic.com/v1/models");
        assert_eq!(
            hdr,
            vec![(
                "anthropic-version".to_string(),
                ANTHROPIC_VERSION.to_string()
            )]
        );

        let (url, _) = probe_request(
            ProviderKind::Bedrock,
            "https://bedrock-runtime.us-east-1.amazonaws.com/v1",
            "/",
        );
        assert_eq!(
            url,
            "https://bedrock.us-east-1.amazonaws.com/foundation-models"
        );

        let (url, _) = probe_request(
            ProviderKind::Vertex,
            "https://aiplatform.googleapis.com/v1/projects/p/locations/global/endpoints/openapi",
            "/",
        );
        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/projects/p/locations/global/publishers/google/models"
        );
    }

    #[test]
    fn explicit_path_overrides_kind_default() {
        // a non-default path is honoured verbatim, with no injected header
        let (url, hdr) = probe_request(ProviderKind::Anthropic, "https://x.test", "/healthz");
        assert_eq!(url, "https://x.test/healthz");
        assert!(hdr.is_empty());
    }

    fn provider(kind: ProviderKind) -> rolter_core::ProviderConfig {
        rolter_core::ProviderConfig {
            name: "p".to_string(),
            slug: None,
            kind,
            api_base: "https://api.test".to_string(),
            api_key: None,
            api_key_env: None,
            egress_proxy: None,
            egress_proxies: Vec::new(),
            kv_events: None,
            lmcache: None,
            ca_bundles: None,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
        }
    }

    #[test]
    fn plan_defaults_to_free_probe() {
        let (plan, source) = build_probe_plan(&provider(ProviderKind::Openai), "/");
        assert!(matches!(plan, ProbePlan::Free { .. }));
        assert_eq!(source, crate::health_events::HealthSource::Probe);
    }

    #[test]
    fn plan_uses_llm_call_when_enabled_and_configured() {
        let mut p = provider(ProviderKind::Openai);
        p.also_track_via_llm_call = true;
        p.llm_probe_model = Some("gpt-4o-mini".to_string());
        p.api_key = Some("sk-test".to_string());
        let (plan, source) = build_probe_plan(&p, "/");
        assert_eq!(source, crate::health_events::HealthSource::LlmCall);
        match plan {
            ProbePlan::LlmCall { url, headers, body } => {
                assert_eq!(url, "https://api.test/v1/chat/completions");
                assert!(headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && v == "Bearer sk-test"));
                assert!(body.contains("\"model\":\"gpt-4o-mini\""));
                assert!(body.contains("\"max_tokens\":1"));
            }
            _ => panic!("expected an llm-call plan"),
        }
    }

    #[test]
    fn plan_anthropic_llm_call_targets_messages_with_version() {
        let mut p = provider(ProviderKind::Anthropic);
        p.also_track_via_llm_call = true;
        p.llm_probe_model = Some("claude-3-5-haiku".to_string());
        p.api_key = Some("sk-ant".to_string());
        let (plan, _) = build_probe_plan(&p, "/");
        match plan {
            ProbePlan::LlmCall { url, headers, .. } => {
                assert_eq!(url, "https://api.test/v1/messages");
                assert!(headers
                    .iter()
                    .any(|(k, v)| k == "x-api-key" && v == "sk-ant"));
                assert!(headers
                    .iter()
                    .any(|(k, v)| k == "anthropic-version" && v == ANTHROPIC_VERSION));
            }
            _ => panic!("expected an llm-call plan"),
        }
    }

    #[test]
    fn plan_falls_back_to_free_when_llm_misconfigured() {
        // flag on but no model/key: degrade to the free probe rather than fail
        let mut p = provider(ProviderKind::Openai);
        p.also_track_via_llm_call = true;
        let (plan, source) = build_probe_plan(&p, "/");
        assert!(matches!(plan, ProbePlan::Free { .. }));
        assert_eq!(source, crate::health_events::HealthSource::Probe);
    }

    #[test]
    fn ollama_cloud_probe_uses_models_endpoint_and_bearer_auth() {
        let mut p = provider(ProviderKind::OllamaCloud);
        p.api_base = "https://ollama.com".to_string();
        p.api_key = Some("test-cloud-key".to_string());
        let (plan, source) = build_probe_plan(&p, "/");
        assert_eq!(source, crate::health_events::HealthSource::Probe);
        match plan {
            ProbePlan::Free { url, headers } => {
                assert_eq!(url, "https://ollama.com/v1/models");
                assert_eq!(
                    headers,
                    vec![(
                        "authorization".to_string(),
                        "Bearer test-cloud-key".to_string()
                    )]
                );
            }
            _ => panic!("expected a free probe"),
        }
    }

    #[test]
    fn openrouter_probe_uses_api_v1_models_and_bearer_auth() {
        let mut p = provider(ProviderKind::Openrouter);
        p.api_base = "https://openrouter.ai/api/v1".to_string();
        p.api_key = Some("test-openrouter-key".to_string());
        let (plan, source) = build_probe_plan(&p, "/");
        assert_eq!(source, crate::health_events::HealthSource::Probe);
        match plan {
            ProbePlan::Free { url, headers } => {
                assert_eq!(url, "https://openrouter.ai/api/v1/models");
                assert_eq!(
                    headers,
                    vec![(
                        "authorization".to_string(),
                        "Bearer test-openrouter-key".to_string()
                    )]
                );
            }
            _ => panic!("expected a free probe"),
        }
    }

    #[test]
    fn tei_uses_native_health_endpoint_without_auth_by_default() {
        let (url, headers) = probe_request(ProviderKind::Tei, "http://tei:80/", "/");
        assert_eq!(url, "http://tei:80/health");
        assert!(headers.is_empty());
    }

    #[test]
    fn azure_probe_uses_models_endpoint_and_api_key_auth() {
        let mut p = provider(ProviderKind::AzureOpenai);
        p.api_base = "https://example.openai.azure.com/openai/v1".to_string();
        p.api_key = Some("azure-secret".to_string());
        let (plan, _) = build_probe_plan(&p, "/");
        match plan {
            ProbePlan::Free { url, headers } => {
                assert_eq!(url, "https://example.openai.azure.com/openai/v1/models");
                assert_eq!(
                    headers,
                    vec![("api-key".to_string(), "azure-secret".to_string())]
                );
            }
            _ => panic!("expected a free probe"),
        }
    }
}
