//! Upstream forwarding for rolter.
//!
//! [`Forwarder`] owns pooled HTTP clients (one default, plus one per egress
//! proxy URL) and forwards a request body to a provider, returning the raw
//! [`reqwest::Response`] so the caller can stream it straight back to the client
//! with minimal copying. Cross-protocol requests are normalized by the
//! extensible translation registry before dispatch; responses are translated by
//! the gateway while they stream back to the caller.

mod translation;

pub use translation::{Protocol, TranslatedStream, TranslationPlan};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use reqwest::{Client, Method, Proxy, RequestBuilder, Response};
use rolter_core::{Error, ProviderConfig, ProviderKind, Result, TimeoutConfig};

/// Forwards requests to upstream providers using pooled, reused HTTP clients.
pub struct Forwarder {
    default: Client,
    configured: DashMap<ClientKey, Client>,
    /// connect-establishment timeout baked into every client
    connect_timeout: Option<Duration>,
    /// time-to-response-headers bound applied around each `send()` (0 disables)
    request_timeout: Option<Duration>,
    next_proxy: AtomicUsize,
    proxy_health: DashMap<String, ProxyHealth>,
}

#[derive(Default)]
struct ProxyHealth {
    successes: AtomicU64,
    failures: AtomicU64,
    consecutive_failures: AtomicU64,
    quarantine_until_ms: AtomicU64,
}

/// Redacted per-pool-member counters suitable for Prometheus export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyMetric {
    pub proxy: String,
    pub successes: u64,
    pub failures: u64,
    pub quarantined: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ClientKey {
    proxy: Option<String>,
    ca_bundles: Vec<PathBuf>,
}

impl Default for Forwarder {
    fn default() -> Self {
        Self::new()
    }
}

impl Forwarder {
    /// Create a forwarder with default timeouts.
    pub fn new() -> Self {
        Self::with_timeouts(&TimeoutConfig::default())
    }

    /// Create a forwarder whose clients honour `timeouts`.
    pub fn with_timeouts(timeouts: &TimeoutConfig) -> Self {
        let connect_timeout =
            (timeouts.connect_secs > 0).then(|| Duration::from_secs(timeouts.connect_secs));
        let request_timeout =
            (timeouts.request_secs > 0).then(|| Duration::from_secs(timeouts.request_secs));
        Self {
            default: build_client(None, &[], connect_timeout).unwrap_or_else(|_| Client::new()),
            configured: DashMap::new(),
            connect_timeout,
            request_timeout,
            next_proxy: AtomicUsize::new(0),
            proxy_health: DashMap::new(),
        }
    }

    /// Return the pooled client matching this provider's proxy and trust roots.
    pub fn client_for(&self, provider: &ProviderConfig) -> Result<Client> {
        let proxies = self.proxy_candidates(provider)?;
        let proxy = proxies
            .get(self.next_proxy.fetch_add(1, Relaxed) % proxies.len().max(1))
            .map(|(_, url)| url.as_str());
        self.client_for_proxy(provider, proxy)
    }

    fn client_for_proxy(&self, provider: &ProviderConfig, proxy: Option<&str>) -> Result<Client> {
        let ca_bundles = provider.ca_bundles.clone().unwrap_or_default();
        if proxy.is_none() && ca_bundles.is_empty() {
            return Ok(self.default.clone());
        }
        let key = ClientKey {
            proxy: proxy.map(str::to_string),
            ca_bundles,
        };
        if let Some(client) = self.configured.get(&key) {
            return Ok(client.clone());
        }
        let client = build_client(key.proxy.as_deref(), &key.ca_bundles, self.connect_timeout)?;
        Ok(self.configured.entry(key).or_insert(client).clone())
    }

    fn proxy_candidates(&self, provider: &ProviderConfig) -> Result<Vec<(String, String)>> {
        provider
            .egress_proxy_pool()
            .into_iter()
            .enumerate()
            .map(|(index, reference)| {
                let id = if reference.starts_with("${") {
                    reference.to_string()
                } else {
                    format!("proxy-{index}")
                };
                let url = if let Some(name) = reference
                    .strip_prefix("${")
                    .and_then(|value| value.strip_suffix('}'))
                {
                    std::env::var(name).map_err(|_| {
                        Error::Config(format!(
                            "provider '{}' egress proxy environment variable {name} is unset",
                            provider.name
                        ))
                    })?
                } else {
                    reference.to_string()
                };
                Ok((id, url))
            })
            .collect()
    }

    /// Snapshot redacted success/failure/quarantine counters for every proxy
    /// member observed by this process.
    pub fn proxy_metrics(&self) -> Vec<ProxyMetric> {
        let now = epoch_millis();
        self.proxy_health
            .iter()
            .map(|entry| ProxyMetric {
                proxy: entry.key().clone(),
                successes: entry.successes.load(Relaxed),
                failures: entry.failures.load(Relaxed),
                quarantined: entry.quarantine_until_ms.load(Relaxed) > now,
            })
            .collect()
    }

    async fn send_with_proxy_retry(
        &self,
        provider: &ProviderConfig,
        build: impl Fn(Client) -> RequestBuilder,
    ) -> Result<Response> {
        let mut candidates = self.proxy_candidates(provider)?;
        if candidates.is_empty() {
            let client = self.client_for_proxy(provider, None)?;
            return self.await_send(build(client).send()).await;
        }
        let start = self.next_proxy.fetch_add(1, Relaxed) % candidates.len();
        candidates.rotate_left(start);
        let now = epoch_millis();
        let has_available = candidates.iter().any(|(id, _)| {
            self.proxy_health
                .get(id)
                .map(|h| h.quarantine_until_ms.load(Relaxed) <= now)
                .unwrap_or(true)
        });
        let mut last_error = None;
        for (id, url) in candidates {
            let health = self.proxy_health.entry(id).or_default();
            if has_available && health.quarantine_until_ms.load(Relaxed) > now {
                continue;
            }
            let client = self.client_for_proxy(provider, Some(&url))?;
            let result = match self.request_timeout {
                Some(limit) => match tokio::time::timeout(limit, build(client).send()).await {
                    Ok(result) => result,
                    Err(_) => {
                        health.failures.fetch_add(1, Relaxed);
                        let consecutive = health.consecutive_failures.fetch_add(1, Relaxed) + 1;
                        if consecutive >= 3 {
                            health
                                .quarantine_until_ms
                                .store(now.saturating_add(30_000), Relaxed);
                        }
                        last_error = Some(format!(
                            "upstream request timed out after {}s",
                            limit.as_secs()
                        ));
                        continue;
                    }
                },
                None => build(client).send().await,
            };
            match result {
                Ok(response) => {
                    health.successes.fetch_add(1, Relaxed);
                    health.consecutive_failures.store(0, Relaxed);
                    health.quarantine_until_ms.store(0, Relaxed);
                    return Ok(response);
                }
                Err(error) => {
                    health.failures.fetch_add(1, Relaxed);
                    let consecutive = health.consecutive_failures.fetch_add(1, Relaxed) + 1;
                    if consecutive >= 3 {
                        health
                            .quarantine_until_ms
                            .store(now.saturating_add(30_000), Relaxed);
                    }
                    let retryable = error.is_connect() || error.is_timeout();
                    last_error = Some(error.to_string());
                    if !retryable {
                        break;
                    }
                }
            }
        }
        Err(Error::Upstream(last_error.unwrap_or_else(|| {
            "all egress proxies are quarantined".to_string()
        })))
    }

    /// Drop configured pools after a validated snapshot reload. The next
    /// request rereads CA files, so certificate rotation takes effect without
    /// restarting the gateway.
    pub fn reload(&self) {
        self.configured.clear();
    }

    /// Forward a JSON body to `provider` at `path` and return the raw response.
    ///
    /// `api_key` is injected per provider kind (Bearer for OpenAI-style,
    /// `x-api-key` for Anthropic). When `upstream_model` is set the top-level
    /// `model` field of the body is rewritten to it. `passthrough_headers` are
    /// forwarded verbatim (used to propagate the caller's inbound trace context
    /// — `traceparent`/`b3` — so the upstream continues the same trace); an empty
    /// slice adds nothing, keeping the wire clean for untraced requests.
    pub async fn forward_json(
        &self,
        provider: &ProviderConfig,
        path: &str,
        body: Bytes,
        api_key: Option<&str>,
        upstream_model: Option<&str>,
        passthrough_headers: &[(&str, &str)],
    ) -> Result<Response> {
        if provider.kind == ProviderKind::Openrouter && api_key.is_none() {
            return Err(Error::Config(format!(
                "openrouter provider '{}' requires a resolved api key",
                provider.name
            )));
        }
        if provider.kind == ProviderKind::OllamaCloud && api_key.is_none() {
            return Err(Error::Config(format!(
                "ollama_cloud provider '{}' requires a resolved api key",
                provider.name
            )));
        }
        if matches!(
            provider.kind,
            ProviderKind::Gemini
                | ProviderKind::GeminiNative
                | ProviderKind::Mistral
                | ProviderKind::Groq
                | ProviderKind::Xai
        ) && api_key.is_none()
        {
            return Err(Error::Config(format!(
                "hosted provider '{}' requires a resolved api key",
                provider.name
            )));
        }
        let translation = TranslationPlan::resolve(
            path,
            provider.kind,
            provider.role_profile_for(upstream_model),
        );
        // gemini native embeds the model + method in the path
        // (`/models/{model}:generateContent`), so it is derived from the
        // request rather than the fixed openai/anthropic route path.
        let url = if translation.is_gemini_generate() {
            gemini_generate_url(provider, &body, upstream_model)
        } else {
            provider_url(provider, translation.upstream_path(path))
        };
        let body = translation.translate_request(body)?;
        let body = translation::normalize_prompt_cache_control(body, provider.kind)?;
        // gemini native takes no top-level `model` field (it is in the url)
        let body = if translation.is_gemini_generate() {
            body
        } else {
            maybe_rewrite_model(body, upstream_model)
        };
        self.send_with_proxy_retry(provider, |client| {
            let mut req = apply_provider_auth(
                client
                    .request(Method::POST, &url)
                    .header(reqwest::header::CONTENT_TYPE, "application/json"),
                provider,
                api_key,
            );
            if provider.kind == ProviderKind::Openrouter {
                if let Ok(referer) = std::env::var("OPENROUTER_HTTP_REFERER") {
                    req = req.header("HTTP-Referer", referer);
                }
                if let Ok(title) = std::env::var("OPENROUTER_X_TITLE") {
                    req = req.header("X-Title", title);
                }
            }
            for (name, value) in passthrough_headers {
                req = req.header(*name, *value);
            }
            req.body(body.clone())
        })
        .await
    }

    /// Forward a raw request body verbatim under an explicit `content_type`.
    ///
    /// Unlike [`Self::forward_json`], the body is passed through untouched (no
    /// `model` rewrite, no JSON assumptions) and the caller's `content-type` —
    /// including any multipart boundary — is preserved. Used for multipart
    /// uploads (`/v1/audio/transcriptions`, `/v1/audio/translations`). API-key
    /// injection and trace-context passthrough match `forward_json`.
    pub async fn forward_raw(
        &self,
        provider: &ProviderConfig,
        path: &str,
        body: Bytes,
        content_type: &str,
        api_key: Option<&str>,
        passthrough_headers: &[(&str, &str)],
    ) -> Result<Response> {
        if provider.kind == ProviderKind::OllamaCloud && api_key.is_none() {
            return Err(Error::Config(format!(
                "ollama_cloud provider '{}' requires a resolved api key",
                provider.name
            )));
        }
        let url = provider_url(provider, path);
        self.send_with_proxy_retry(provider, |client| {
            let mut req = apply_provider_auth(
                client
                    .request(Method::POST, &url)
                    .header(reqwest::header::CONTENT_TYPE, content_type),
                provider,
                api_key,
            );
            for (name, value) in passthrough_headers {
                req = req.header(*name, *value);
            }
            req.body(body.clone())
        })
        .await
    }

    /// Forward a model-less OpenAI Responses lifecycle operation. The caller
    /// must resolve the tenant-scoped response route first; this method only
    /// preserves the selected provider credential and native resource path.
    pub async fn forward_resource(
        &self,
        provider: &ProviderConfig,
        method: Method,
        path: &str,
        api_key: Option<&str>,
        passthrough_headers: &[(&str, &str)],
    ) -> Result<Response> {
        let url = provider_url(provider, path);
        self.send_with_proxy_retry(provider, |client| {
            let mut req =
                apply_provider_auth(client.request(method.clone(), &url), provider, api_key);
            for (name, value) in passthrough_headers {
                req = req.header(*name, *value);
            }
            req
        })
        .await
    }

    /// Await an upstream send under the configured time-to-headers budget. The
    /// body stream is left untouched so long/streamed responses aren't cut off.
    async fn await_send(
        &self,
        send: impl std::future::Future<Output = std::result::Result<Response, reqwest::Error>>,
    ) -> Result<Response> {
        match self.request_timeout {
            Some(limit) => match tokio::time::timeout(limit, send).await {
                Ok(res) => res.map_err(|e| Error::Upstream(e.to_string())),
                Err(_) => Err(Error::Upstream(format!(
                    "upstream request timed out after {}s",
                    limit.as_secs()
                ))),
            },
            None => send.await.map_err(|e| Error::Upstream(e.to_string())),
        }
    }
}

fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn apply_provider_auth(
    mut request: RequestBuilder,
    provider: &ProviderConfig,
    api_key: Option<&str>,
) -> RequestBuilder {
    match provider.kind {
        ProviderKind::Anthropic => {
            if let Some(key) = api_key {
                request = request.header("x-api-key", key);
            }
            request.header("anthropic-version", "2023-06-01")
        }
        ProviderKind::AzureOpenai => {
            if let Some(key) = api_key {
                request = request.header("api-key", key);
            }
            request
        }
        ProviderKind::GeminiNative => {
            if let Some(key) = api_key {
                request = request.header("x-goog-api-key", key);
            }
            request
        }
        _ => {
            if let Some(key) = api_key {
                request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
            }
            request
        }
    }
}

fn provider_url(provider: &ProviderConfig, path: &str) -> String {
    let base = provider.api_base.trim_end_matches('/');
    if matches!(
        provider.kind,
        ProviderKind::Openrouter
            | ProviderKind::AzureOpenai
            | ProviderKind::Bedrock
            | ProviderKind::Vertex
            | ProviderKind::Gemini
            | ProviderKind::Mistral
            | ProviderKind::Groq
            | ProviderKind::Xai
    ) {
        let suffix = path.strip_prefix("/v1").unwrap_or(path);
        format!("{base}{suffix}")
    } else {
        format!("{base}{path}")
    }
}

/// build the gemini native generateContent url from the request. the model is
/// taken from the configured upstream override, else the request's `model`
/// field; streaming requests use `:streamGenerateContent?alt=sse`.
fn gemini_generate_url(
    provider: &ProviderConfig,
    body: &Bytes,
    upstream_model: Option<&str>,
) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_slice(body).ok();
    let model = upstream_model
        .map(str::to_string)
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|v| v.get("model"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "gemini-2.5-flash".to_string());
    let stream = parsed
        .as_ref()
        .and_then(|v| v.get("stream"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let base = provider.api_base.trim_end_matches('/');
    if stream {
        format!("{base}/models/{model}:streamGenerateContent?alt=sse")
    } else {
        format!("{base}/models/{model}:generateContent")
    }
}

fn build_client(
    proxy: Option<&str>,
    ca_bundles: &[PathBuf],
    connect_timeout: Option<Duration>,
) -> Result<Client> {
    let mut builder = Client::builder()
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(64)
        .tcp_nodelay(true);
    if let Some(ct) = connect_timeout {
        builder = builder.connect_timeout(ct);
    }
    if let Some(url) = proxy {
        let px = Proxy::all(url)
            .map_err(|error| Error::Config(format!("invalid egress proxy '{url}': {error}")))?;
        builder = builder.proxy(px);
    }
    for path in ca_bundles {
        for certificate in load_ca_bundle(path)? {
            builder = builder.add_root_certificate(certificate);
        }
    }
    builder
        .build()
        .map_err(|error| Error::Config(format!("failed to build upstream TLS client: {error}")))
}

fn load_ca_bundle(path: &Path) -> Result<Vec<reqwest::Certificate>> {
    let bytes = std::fs::read(path).map_err(|error| {
        Error::Config(format!(
            "CA bundle '{}' cannot be read: {error}",
            path.display()
        ))
    })?;
    reqwest::Certificate::from_pem_bundle(&bytes).map_err(|error| {
        Error::Config(format!(
            "CA bundle '{}' contains malformed PEM: {error}",
            path.display()
        ))
    })
}

/// Rewrite the top-level `model` field when an upstream model name is configured.
fn maybe_rewrite_model(body: Bytes, upstream_model: Option<&str>) -> Bytes {
    let Some(model) = upstream_model else {
        return body;
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return body;
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
        if let Ok(rewritten) = serde_json::to_vec(&value) {
            return Bytes::from(rewritten);
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_model_field() {
        let body = Bytes::from(r#"{"model":"public","messages":[]}"#);
        let out = maybe_rewrite_model(body, Some("upstream-model"));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "upstream-model");
    }

    #[test]
    fn leaves_body_untouched_without_model() {
        let body = Bytes::from(r#"{"model":"public"}"#);
        let out = maybe_rewrite_model(body.clone(), None);
        assert_eq!(out, body);
    }

    #[test]
    fn cloud_openai_compatible_bases_do_not_duplicate_v1() {
        let bedrock = provider(
            ProviderKind::Bedrock,
            "https://bedrock-runtime.us-east-1.amazonaws.com/v1".to_string(),
        );
        assert_eq!(
            provider_url(&bedrock, "/v1/chat/completions"),
            "https://bedrock-runtime.us-east-1.amazonaws.com/v1/chat/completions"
        );
    }

    #[test]
    fn hosted_openai_compatible_clouds_strip_v1() {
        let cases = [
            (
                ProviderKind::Gemini,
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
            ),
            (
                ProviderKind::Mistral,
                "https://api.mistral.ai/v1",
                "https://api.mistral.ai/v1/chat/completions",
            ),
            (
                ProviderKind::Groq,
                "https://api.groq.com/openai/v1",
                "https://api.groq.com/openai/v1/chat/completions",
            ),
        ];
        for (kind, base, expected) in cases {
            let p = provider(kind, base.to_string());
            assert_eq!(provider_url(&p, "/v1/chat/completions"), expected);
        }
    }

    /// Accept one connection, capture the raw request head, answer a minimal
    /// 200 and hand the head back for inspection.
    async fn capture_one_request(listener: tokio::net::TcpListener) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            if n == 0 || buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .await
            .unwrap();
        let head_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..head_end]).to_lowercase()
    }

    fn provider(kind: ProviderKind, api_base: String) -> ProviderConfig {
        ProviderConfig {
            name: "p".to_string(),
            slug: None,
            kind,
            api_base,
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

    #[tokio::test]
    async fn openai_wire_carries_no_rolter_signature() {
        // golden guarantee (ROL-100): the upstream sees nothing identifying
        // rolter — no user-agent, no x-* or via headers, no proxy marks
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let p = provider(ProviderKind::OpenaiCompatible, format!("http://{addr}"));
        fwd.forward_json(
            &p,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("sk-test"),
            None,
            &[],
        )
        .await
        .unwrap();
        let head = capture.await.unwrap();

        assert!(!head.contains("rolter"), "rolter mark on the wire:\n{head}");
        for line in head.lines().skip(1).filter(|l| !l.is_empty()) {
            let name = line.split(':').next().unwrap_or("").trim();
            assert!(
                matches!(
                    name,
                    "content-type" | "authorization" | "host" | "content-length" | "accept"
                ),
                "unexpected outbound header `{name}`:\n{head}"
            );
        }
        assert!(!head.contains("user-agent"), "user-agent leaked:\n{head}");
        assert!(head.contains("authorization: bearer sk-test"));
    }

    #[tokio::test]
    async fn ollama_wire_is_keyless() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let p = provider(ProviderKind::Ollama, format!("http://{addr}"));
        fwd.forward_json(
            &p,
            "/v1/chat/completions",
            Bytes::from_static(br#"{"model":"llama"}"#),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
        let head = capture.await.unwrap();

        assert!(!head.contains("authorization:"));
        assert!(head.starts_with("post /v1/chat/completions"));
    }

    #[tokio::test]
    async fn anthropic_wire_carries_only_required_headers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let p = provider(ProviderKind::Anthropic, format!("http://{addr}"));
        fwd.forward_json(
            &p,
            "/v1/messages",
            Bytes::from_static(b"{}"),
            Some("sk-ant"),
            None,
            &[],
        )
        .await
        .unwrap();
        let head = capture.await.unwrap();

        assert!(!head.contains("rolter"), "rolter mark on the wire:\n{head}");
        assert!(!head.contains("user-agent"), "user-agent leaked:\n{head}");
        // the only x-* header is the one anthropic's api itself requires
        for line in head.lines().filter(|l| l.starts_with("x-")) {
            assert!(
                line.starts_with("x-api-key:"),
                "unexpected x-* header:\n{head}"
            );
        }
        assert!(head.contains("anthropic-version: 2023-06-01"));
    }

    #[tokio::test]
    async fn forwards_trace_context_when_provided() {
        // the caller's inbound trace context is propagated verbatim so the
        // upstream continues the same distributed trace (ROL-61)
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let p = provider(ProviderKind::OpenaiCompatible, format!("http://{addr}"));
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0da902b7-01";
        fwd.forward_json(
            &p,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("sk-test"),
            None,
            &[("traceparent", tp)],
        )
        .await
        .unwrap();
        let head = capture.await.unwrap();

        assert!(
            head.contains(&format!("traceparent: {tp}")),
            "traceparent not propagated to upstream:\n{head}"
        );
    }

    #[tokio::test]
    async fn proxy_pool_retries_connect_failure_on_next_member() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));
        let mut provider = provider(ProviderKind::Openai, "http://upstream.invalid".into());
        provider.egress_proxies = vec![
            "http://127.0.0.1:1".to_string(),
            format!("http://{proxy_addr}"),
        ];
        let forwarder = Forwarder::new();
        let response = forwarder
            .forward_json(
                &provider,
                "/v1/chat/completions",
                Bytes::from_static(br#"{"model":"test"}"#),
                Some("key"),
                None,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let head = capture.await.unwrap();
        assert!(
            head.starts_with("post "),
            "unexpected proxy request: {head:?}"
        );
        assert!(head.contains("/v1/chat/completions"));
        let metrics = forwarder.proxy_metrics();
        assert!(metrics
            .iter()
            .any(|m| m.proxy == "proxy-0" && m.failures == 1));
        assert!(metrics
            .iter()
            .any(|m| m.proxy == "proxy-1" && m.successes == 1));
    }

    #[tokio::test]
    async fn request_timeout_fires_on_a_silent_upstream() {
        // a listener that accepts connections but never writes a response,
        // so only the request timeout can unblock the forward
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    // hold the connection open, never respond
                    std::mem::forget(stream);
                }
            }
        });

        let fwd = Forwarder::with_timeouts(&TimeoutConfig {
            connect_secs: 0,
            request_secs: 1,
        });
        let provider = ProviderConfig {
            name: "slow".to_string(),
            slug: None,
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{addr}"),
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
        };
        let err = fwd
            .forward_json(
                &provider,
                "/v1/chat/completions",
                Bytes::from_static(b"{}"),
                None,
                None,
                &[],
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("timed out"),
            "expected a timeout error, got: {err}"
        );
    }

    #[tokio::test]
    async fn ollama_cloud_requires_and_sends_bearer_auth() {
        let fwd = Forwarder::new();
        let missing = provider(ProviderKind::OllamaCloud, "http://127.0.0.1:1".to_string());
        let err = fwd
            .forward_json(
                &missing,
                "/v1/chat/completions",
                Bytes::from_static(b"{}"),
                None,
                None,
                &[],
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("requires a resolved api key"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));
        let cloud = provider(ProviderKind::OllamaCloud, format!("http://{addr}"));
        fwd.forward_json(
            &cloud,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("cloud-secret"),
            None,
            &[],
        )
        .await
        .unwrap();
        assert!(capture
            .await
            .unwrap()
            .contains("authorization: bearer cloud-secret"));
    }

    #[tokio::test]
    async fn azure_openai_strips_v1_and_uses_api_key_auth() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let azure = provider(
            ProviderKind::AzureOpenai,
            format!("http://{addr}/openai/v1"),
        );
        fwd.forward_json(
            &azure,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("azure-secret"),
            None,
            &[],
        )
        .await
        .unwrap();

        let head = capture.await.unwrap();
        assert!(head.starts_with("post /openai/v1/chat/completions"));
        assert!(head.contains("api-key: azure-secret"));
        assert!(!head.contains("authorization:"));
    }

    #[tokio::test]
    async fn vertex_strips_v1_from_gateway_path() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let vertex = provider(
            ProviderKind::Vertex,
            format!("http://{addr}/v1/projects/p/locations/global/endpoints/openapi"),
        );
        fwd.forward_json(
            &vertex,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("oauth-token"),
            None,
            &[],
        )
        .await
        .unwrap();

        let head = capture.await.unwrap();
        assert!(head.starts_with(
            "post /v1/projects/p/locations/global/endpoints/openapi/chat/completions"
        ));
        assert!(head.contains("authorization: bearer oauth-token"));
    }

    #[tokio::test]
    async fn groq_strips_v1_and_uses_bearer_auth() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let groq = provider(ProviderKind::Groq, format!("http://{addr}/openai/v1"));
        fwd.forward_json(
            &groq,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("groq-secret"),
            None,
            &[],
        )
        .await
        .unwrap();

        let head = capture.await.unwrap();
        assert!(head.starts_with("post /openai/v1/chat/completions"));
        assert!(head.contains("authorization: bearer groq-secret"));
    }

    #[tokio::test]
    async fn xai_strips_v1_and_uses_bearer_auth() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let capture = tokio::spawn(capture_one_request(listener));

        let fwd = Forwarder::new();
        let xai = provider(ProviderKind::Xai, format!("http://{addr}/v1"));
        fwd.forward_json(
            &xai,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            Some("xai-secret"),
            None,
            &[],
        )
        .await
        .unwrap();

        let head = capture.await.unwrap();
        assert!(head.starts_with("post /v1/chat/completions"));
        assert!(head.contains("authorization: bearer xai-secret"));
    }

    #[tokio::test]
    async fn hosted_cloud_without_key_is_rejected() {
        let fwd = Forwarder::new();
        let mistral = provider(ProviderKind::Mistral, "http://127.0.0.1:1".to_string());
        let err = fwd
            .forward_json(
                &mistral,
                "/v1/chat/completions",
                Bytes::from_static(b"{}"),
                None,
                None,
                &[],
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    struct TestTlsServer {
        addr: std::net::SocketAddr,
        ca_path: PathBuf,
    }

    struct TestTlsFixture {
        ca_pem: &'static str,
        server_cert_pem: &'static str,
        server_key_pem: &'static str,
    }

    const ONE: TestTlsFixture = TestTlsFixture {
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIBoTCCAUegAwIBAgIUOywZS1wTTQGvHfTfJs+E/2A374wwCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3Qgb25lIGNhMCAXDTI2MDcxNTEzMDgzMVoY\nDzIxMjYwNjIxMTMwODMxWjAdMRswGQYDVQQDDBJyb2x0ZXIgdGVzdCBvbmUgY2Ew\nWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAAS/DcZgePnQsThk5eArc9ZK4BZ/noeh\nWPxPkrYGX4EkIVUp++Km8WuRLjzJnQGYLtgySAgEjXPb1ID1DHwdJUrJo2MwYTAP\nBgNVHRMBAf8EBTADAQH/MB0GA1UdDgQWBBSltfKu0VJcZPa9FCLJWQqIwU8m0zAf\nBgNVHSMEGDAWgBSltfKu0VJcZPa9FCLJWQqIwU8m0zAOBgNVHQ8BAf8EBAMCAQYw\nCgYIKoZIzj0EAwIDSAAwRQIgIDuLKEw/aG7bEeI2fXGArtFUivAijRCHdAckqhf9\nAHECIQCK+vyFaGiZRuiSlirbCpG/Bj22VaXGWP3SNhcRjAoKtA==\n-----END CERTIFICATE-----\n",
        server_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIBxjCCAW2gAwIBAgIUcq+1etqKMzPPjTDH1vLZr1JJwGkwCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3Qgb25lIGNhMCAXDTI2MDcxNTEzMDgzMVoY\nDzIxMjYwNjIxMTMwODMxWjAhMR8wHQYDVQQDDBZyb2x0ZXIgdGVzdCBvbmUgc2Vy\ndmVyMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE9nm2M1pjXYVgocpnmuhZ6Mx2\nshN3F26dbrdOi1Bnuf/+fBDXiNI7Ul+eVW8BiFj1LwiK1UqbYMqRkbkjz0O7zaOB\nhDCBgTAJBgNVHRMEAjAAMA8GA1UdEQQIMAaHBH8AAAEwEwYDVR0lBAwwCgYIKwYB\nBQUHAwEwDgYDVR0PAQH/BAQDAgWgMB0GA1UdDgQWBBRwXyPXl8GxkEyfmv0Og8WX\nHrt83zAfBgNVHSMEGDAWgBSltfKu0VJcZPa9FCLJWQqIwU8m0zAKBggqhkjOPQQD\nAgNHADBEAiAkOfd7cjmMMzs5cycYE+yK32mTY2a14BZ6gLUDVezcPgIgbtBfBH87\nBP1SjlCjhGlrKwP3V+FMPZ9jX9wDEp0I1V4=\n-----END CERTIFICATE-----\n",
        server_key_pem: concat!(
            "-----BEGIN EC ",
            "PRIVATE KEY-----\nMHcCAQEEIHSJL8ih1JLjEbAt/TeQ1N80q+VIZd3NA0680S5w6cTzoAoGCCqGSM49\nAwEHoUQDQgAE9nm2M1pjXYVgocpnmuhZ6Mx2shN3F26dbrdOi1Bnuf/+fBDXiNI7\nUl+eVW8BiFj1LwiK1UqbYMqRkbkjz0O7zQ==\n-----END EC ",
            "PRIVATE KEY-----\n",
        ),
    };

    const TWO: TestTlsFixture = TestTlsFixture {
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIBoTCCAUegAwIBAgIUb/PGRwbxSceLQXpgs/MeWebvJMIwCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3QgdHdvIGNhMCAXDTI2MDcxNTEzMDgzMVoY\nDzIxMjYwNjIxMTMwODMxWjAdMRswGQYDVQQDDBJyb2x0ZXIgdGVzdCB0d28gY2Ew\nWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAARk1P9GXr+2YdsbdEUtmEdGpnhvGH5p\nWkmDekne56IfEdvoyRqwRuQeaEbJH9VX6aynP2Ln8R2GEkIx1qgVUcBwo2MwYTAP\nBgNVHRMBAf8EBTADAQH/MB0GA1UdDgQWBBSMmIuW8upym2nXRBwrW5lqL9MEvDAf\nBgNVHSMEGDAWgBSMmIuW8upym2nXRBwrW5lqL9MEvDAOBgNVHQ8BAf8EBAMCAQYw\nCgYIKoZIzj0EAwIDSAAwRQIgT1J77HJlltep0qr6u7jwkyFtAWCg7Wcfxl80pfpb\nQL4CIQDfdjscsGHwRrBMN0Js8SK2y08GrnGTpHhpwgCIKllSCg==\n-----END CERTIFICATE-----\n",
        server_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIByDCCAW2gAwIBAgIUUUn86mPVojDWs8O+tR/ZogJPphgwCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3QgdHdvIGNhMCAXDTI2MDcxNTEzMDgzMVoY\nDzIxMjYwNjIxMTMwODMxWjAhMR8wHQYDVQQDDBZyb2x0ZXIgdGVzdCB0d28gc2Vy\ndmVyMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEvMwYoXPeMjBpta56AhjH9UVI\nKxqtOK3xp/s8+vgHuIzqnN5jKc2Fng5WqMm9TmzkTQJR4S8raOxOl92WeJzO+aOB\nhDCBgTAJBgNVHRMEAjAAMA8GA1UdEQQIMAaHBH8AAAEwEwYDVR0lBAwwCgYIKwYB\nBQUHAwEwDgYDVR0PAQH/BAQDAgWgMB0GA1UdDgQWBBQbfJhkqT7Ppws9+p/ZKaLx\nwmsKajAfBgNVHSMEGDAWgBSMmIuW8upym2nXRBwrW5lqL9MEvDAKBggqhkjOPQQD\nAgNJADBGAiEA/AaH7vUsNK06p1RydKSWZQU1EXxgZDz7vf54YEOdI7QCIQCYRNXF\nIuXJHr9h3naQ/b+Sc8jk3liqsB3t9mFtnd0Anw==\n-----END CERTIFICATE-----\n",
        server_key_pem: concat!(
            "-----BEGIN EC ",
            "PRIVATE KEY-----\nMHcCAQEEIEcxFCSFxrCf9mszmnKbv3ODG8xd5RTvNm/74yum0lM0oAoGCCqGSM49\nAwEHoUQDQgAEvMwYoXPeMjBpta56AhjH9UVIKxqtOK3xp/s8+vgHuIzqnN5jKc2F\nng5WqMm9TmzkTQJR4S8raOxOl92WeJzO+Q==\n-----END EC ",
            "PRIVATE KEY-----\n",
        ),
    };

    impl Drop for TestTlsServer {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.ca_path);
        }
    }

    async fn spawn_tls_server(fixture: &TestTlsFixture) -> TestTlsServer {
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};

        let certificates = CertificateDer::pem_slice_iter(fixture.server_cert_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(CertificateDer::into_owned)
            .collect();
        let key = PrivateKeyDer::from_pem_slice(fixture.server_key_pem.as_bytes()).unwrap();
        let tls = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let Ok(mut stream) = acceptor.accept(stream).await else {
                return;
            };
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
                .await
                .unwrap();
        });

        let ca_path = std::env::temp_dir().join(format!(
            "rolter-test-ca-{}-{}.pem",
            std::process::id(),
            addr.port()
        ));
        std::fs::write(&ca_path, fixture.ca_pem).unwrap();
        TestTlsServer { addr, ca_path }
    }

    async fn tls_request_with(
        fwd: &Forwarder,
        server: &TestTlsServer,
        host: &str,
        trusted: bool,
    ) -> Result<Response> {
        let mut upstream = provider(
            ProviderKind::OpenaiCompatible,
            format!("https://{host}:{}", server.addr.port()),
        );
        if trusted {
            upstream.ca_bundles = Some(vec![server.ca_path.clone()]);
        }
        fwd.forward_json(
            &upstream,
            "/v1/chat/completions",
            Bytes::from_static(b"{}"),
            None,
            None,
            &[],
        )
        .await
    }

    async fn tls_request(server: &TestTlsServer, host: &str, trusted: bool) -> Result<Response> {
        tls_request_with(&Forwarder::new(), server, host, trusted).await
    }

    #[tokio::test]
    async fn custom_ca_allows_private_tls_upstream() {
        let server = spawn_tls_server(&ONE).await;
        let response = tls_request(&server, "127.0.0.1", true).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }

    #[tokio::test]
    async fn private_tls_upstream_is_rejected_without_custom_ca() {
        let server = spawn_tls_server(&ONE).await;
        let error = tls_request(&server, "127.0.0.1", false).await.unwrap_err();
        assert!(matches!(error, Error::Upstream(_)));
    }

    #[tokio::test]
    async fn custom_ca_keeps_hostname_verification_enabled() {
        let server = spawn_tls_server(&ONE).await;
        let error = tls_request(&server, "localhost", true).await.unwrap_err();
        assert!(matches!(error, Error::Upstream(_)));
    }

    #[tokio::test]
    async fn reload_rereads_rotated_ca_bundle() {
        let first = spawn_tls_server(&ONE).await;
        let fwd = Forwarder::new();
        tls_request_with(&fwd, &first, "127.0.0.1", true)
            .await
            .unwrap();

        let second = spawn_tls_server(&TWO).await;
        std::fs::copy(&second.ca_path, &first.ca_path).unwrap();
        fwd.reload();
        let mut provider = provider(
            ProviderKind::OpenaiCompatible,
            format!("https://127.0.0.1:{}", second.addr.port()),
        );
        provider.ca_bundles = Some(vec![first.ca_path.clone()]);
        let response = fwd
            .forward_json(
                &provider,
                "/v1/chat/completions",
                Bytes::from_static(b"{}"),
                None,
                None,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
}
