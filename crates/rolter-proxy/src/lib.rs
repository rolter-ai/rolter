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
use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use reqwest::{Client, Method, Proxy, Response};
use rolter_core::{Error, ProviderConfig, ProviderKind, Result, TimeoutConfig};

/// Forwards requests to upstream providers using pooled, reused HTTP clients.
pub struct Forwarder {
    default: Client,
    configured: DashMap<ClientKey, Client>,
    /// connect-establishment timeout baked into every client
    connect_timeout: Option<Duration>,
    /// time-to-response-headers bound applied around each `send()` (0 disables)
    request_timeout: Option<Duration>,
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
        }
    }

    /// Return the pooled client matching this provider's proxy and trust roots.
    pub fn client_for(&self, provider: &ProviderConfig) -> Result<Client> {
        let ca_bundles = provider.ca_bundles.clone().unwrap_or_default();
        if provider.egress_proxy.is_none() && ca_bundles.is_empty() {
            return Ok(self.default.clone());
        }
        let key = ClientKey {
            proxy: provider.egress_proxy.clone(),
            ca_bundles,
        };
        if let Some(client) = self.configured.get(&key) {
            return Ok(client.clone());
        }
        let client = build_client(key.proxy.as_deref(), &key.ca_bundles, self.connect_timeout)?;
        Ok(self.configured.entry(key).or_insert(client).clone())
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
        let translation = TranslationPlan::resolve(
            path,
            provider.kind,
            provider.role_profile_for(upstream_model),
        );
        let url = provider_url(provider, translation.upstream_path(path));
        let body = translation.translate_request(body)?;
        let body = maybe_rewrite_model(body, upstream_model);
        let client = self.client_for(provider)?;
        let mut req = client
            .request(Method::POST, &url)
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        match provider.kind {
            ProviderKind::Anthropic => {
                if let Some(key) = api_key {
                    req = req.header("x-api-key", key);
                }
                req = req.header("anthropic-version", "2023-06-01");
            }
            ProviderKind::AzureOpenai => {
                if let Some(key) = api_key {
                    req = req.header("api-key", key);
                }
            }
            _ => {
                if let Some(key) = api_key {
                    req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
                }
            }
        }
        if provider.kind == ProviderKind::Openrouter {
            if let Ok(referer) = std::env::var("OPENROUTER_HTTP_REFERER") {
                req = req.header("HTTP-Referer", referer);
            }
            if let Ok(title) = std::env::var("OPENROUTER_X_TITLE") {
                req = req.header("X-Title", title);
            }
        }
        // propagate the caller's trace context verbatim (nothing when empty)
        for (name, value) in passthrough_headers {
            req = req.header(*name, *value);
        }
        let send = req.body(body).send();
        // bound time-to-response-headers only; the body stream is untouched so
        // long SSE responses are never cut short
        self.await_send(send).await
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
        let client = self.client_for(provider)?;
        let mut req = client
            .request(Method::POST, &url)
            .header(reqwest::header::CONTENT_TYPE, content_type.to_string());
        match provider.kind {
            ProviderKind::Anthropic => {
                if let Some(key) = api_key {
                    req = req.header("x-api-key", key);
                }
                req = req.header("anthropic-version", "2023-06-01");
            }
            ProviderKind::AzureOpenai => {
                if let Some(key) = api_key {
                    req = req.header("api-key", key);
                }
            }
            _ => {
                if let Some(key) = api_key {
                    req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
                }
            }
        }
        for (name, value) in passthrough_headers {
            req = req.header(*name, *value);
        }
        let send = req.body(body).send();
        self.await_send(send).await
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
        let client = self.client_for(provider)?;
        let mut req = client.request(method, &url);
        match provider.kind {
            ProviderKind::Anthropic => {
                if let Some(key) = api_key {
                    req = req.header("x-api-key", key);
                }
                req = req.header("anthropic-version", "2023-06-01");
            }
            ProviderKind::AzureOpenai => {
                if let Some(key) = api_key {
                    req = req.header("api-key", key);
                }
            }
            _ => {
                if let Some(key) = api_key {
                    req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
                }
            }
        }
        for (name, value) in passthrough_headers {
            req = req.header(*name, *value);
        }
        self.await_send(req.send()).await
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

fn provider_url(provider: &ProviderConfig, path: &str) -> String {
    let base = provider.api_base.trim_end_matches('/');
    if matches!(
        provider.kind,
        ProviderKind::Openrouter
            | ProviderKind::AzureOpenai
            | ProviderKind::Bedrock
            | ProviderKind::Vertex
    ) {
        let suffix = path.strip_prefix("/v1").unwrap_or(path);
        format!("{base}{suffix}")
    } else {
        format!("{base}{path}")
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
            kind,
            api_base,
            api_key: None,
            api_key_env: None,
            egress_proxy: None,
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
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{addr}"),
            api_key: None,
            api_key_env: None,
            egress_proxy: None,
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
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIBnjCCAUWgAwIBAgIUfxgZ9JoymX8Sv3T49mqG64zKBtwwCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3Qgb25lIGNhMB4XDTI2MDcxMzEyNTg1N1oX\nDTI2MDcxNTEyNTg1N1owHTEbMBkGA1UEAwwScm9sdGVyIHRlc3Qgb25lIGNhMFkw\nEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEEWTrGZoOOKndIPjyI02jOBHpkCkTeeF+\nbyJVYvgCv9VpWhkGSAMaXuLrJJ2y0JiCH3E4DSyNbGJCuUi4HeaUxqNjMGEwHQYD\nVR0OBBYEFKpTnXjkmXVb/0jAer87tnJgSEHHMB8GA1UdIwQYMBaAFKpTnXjkmXVb\n/0jAer87tnJgSEHHMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgEGMAoG\nCCqGSM49BAMCA0cAMEQCIEQbIclicbU2JAbgYIqHIzDjVAKmKFLr7POSBd79PgoC\nAiBjWxEJ4UnWU6hlQmQEZjIrYRPXOiTh0pxl/CEeYrui6g==\n-----END CERTIFICATE-----\n",
        server_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIByTCCAW6gAwIBAgIUHnkBmxgKbit461qgZKJXjHF/B58wCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3Qgb25lIGNhMB4XDTI2MDcxMzEyNTg1N1oX\nDTI2MDcxNTEyNTg1N1owITEfMB0GA1UEAwwWcm9sdGVyIHRlc3Qgb25lIHNlcnZl\ncjBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABLivU90aOMFkuoQc0k/Q6J80NsxX\nUiU0J52pPLtdUjFGmK1wGOaSbhjprbIw/rt5uw7CjPhdqpnyZ9OrikdGXnujgYcw\ngYQwDAYDVR0TAQH/BAIwADAPBgNVHREECDAGhwR/AAABMBMGA1UdJQQMMAoGCCsG\nAQUFBwMBMA4GA1UdDwEB/wQEAwIFoDAdBgNVHQ4EFgQUk8wnUC4SAYSm/NlgB/io\nLhuvYZgwHwYDVR0jBBgwFoAUqlOdeOSZdVv/SMB6vzu2cmBIQccwCgYIKoZIzj0E\nAwIDSQAwRgIhAKLPtM9Qa1AWbs925IAHXbeo22o39jY59LtUQS9O2/9qAiEAkqn/\neHIzg843rP5KA3qQGC1DC5J088HxRvTml7Yspcw=\n-----END CERTIFICATE-----\n",
        server_key_pem: concat!(
            "-----BEGIN EC ",
            "PRIVATE KEY-----\nMHcCAQEEINyQJcJG4kxH2Gnn+M1yPMf3FA5EjQAsMf7oPViJtStaoAoGCCqGSM49\nAwEHoUQDQgAEuK9T3Ro4wWS6hBzST9DonzQ2zFdSJTQnnak8u11SMUaYrXAY5pJu\nGOmtsjD+u3m7DsKM+F2qmfJn06uKR0Zeew==\n-----END EC ",
            "PRIVATE KEY-----\n",
        ),
    };

    const TWO: TestTlsFixture = TestTlsFixture {
        ca_pem: "-----BEGIN CERTIFICATE-----\nMIIBnjCCAUWgAwIBAgIUS82iOLqT3bwuTd1pxkHyQZSJ4lQwCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3QgdHdvIGNhMB4XDTI2MDcxMzEyNTg1N1oX\nDTI2MDcxNTEyNTg1N1owHTEbMBkGA1UEAwwScm9sdGVyIHRlc3QgdHdvIGNhMFkw\nEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEUJTQ+CM+LCWCBMkpEljwFpRTi4d7sjrz\nud8VFXebsgqQtsZspofKP4hxWDEOi6On+AgVKtfWC3RyljgCDocXaaNjMGEwHQYD\nVR0OBBYEFD7y986Rvr1eTjKiDwEsM++Dc9LeMB8GA1UdIwQYMBaAFD7y986Rvr1e\nTjKiDwEsM++Dc9LeMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgEGMAoG\nCCqGSM49BAMCA0cAMEQCIFih1b9BMxrL6qpV4QTyJaqcZDSDX/XRyU5Ntx3V4+LF\nAiAyPHBfB33aaVoTjFYEvhwGQy2yzrfYoPnYGtBHUb6OHg==\n-----END CERTIFICATE-----\n",
        server_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIBxzCCAW6gAwIBAgIUIhgTtyo3XM5UwPqGuTTIPY3BmsswCgYIKoZIzj0EAwIw\nHTEbMBkGA1UEAwwScm9sdGVyIHRlc3QgdHdvIGNhMB4XDTI2MDcxMzEyNTg1N1oX\nDTI2MDcxNTEyNTg1N1owITEfMB0GA1UEAwwWcm9sdGVyIHRlc3QgdHdvIHNlcnZl\ncjBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABHTvMKtS57HTOCf46JpM92Rh+CXp\n8Sb2lFZDrQuoJ9dmLWl1vUSpvsHcqry8oPspGwUkbL3LI+Af2EaLGluC23+jgYcw\ngYQwDAYDVR0TAQH/BAIwADAPBgNVHREECDAGhwR/AAABMBMGA1UdJQQMMAoGCCsG\nAQUFBwMBMA4GA1UdDwEB/wQEAwIFoDAdBgNVHQ4EFgQUX5bbipqVwbskoa1iV/jZ\nGdlTSd8wHwYDVR0jBBgwFoAUPvL3zpG+vV5OMqIPASwz74Nz0t4wCgYIKoZIzj0E\nAwIDRwAwRAIgNGsYYkXFFB0P1rMG/eanIq7I+P6oFPcgRibW83CDD2cCIEmmUJA5\nyLAi4x88OBsW3K0PJeQVbqklHJIQn3+mJi3+\n-----END CERTIFICATE-----\n",
        server_key_pem: concat!(
            "-----BEGIN EC ",
            "PRIVATE KEY-----\nMHcCAQEEIC7XcAAQFdluitOuNe9Z8Tt0PP/IEpYypkn2Dk3MvUDWoAoGCCqGSM49\nAwEHoUQDQgAEdO8wq1LnsdM4J/jomkz3ZGH4JenxJvaUVkOtC6gn12YtaXW9RKm+\nwdyqvLyg+ykbBSRsvcsj4B/YRosaW4Lbfw==\n-----END EC ",
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
