//! Upstream forwarding for rolter.
//!
//! [`Forwarder`] owns pooled HTTP clients (one default, plus one per egress
//! proxy URL) and forwards a request body to a provider, returning the raw
//! [`reqwest::Response`] so the caller can stream it straight back to the client
//! with minimal copying. Native request/response translation between OpenAI and
//! Anthropic schemas is a follow-up; today matching schemas pass through.

use std::time::Duration;

use bytes::Bytes;
use dashmap::DashMap;
use reqwest::{Client, Method, Proxy, Response};
use rolter_core::{Error, ProviderConfig, ProviderKind, Result};

/// Forwards requests to upstream providers using pooled, reused HTTP clients.
pub struct Forwarder {
    default: Client,
    proxied: DashMap<String, Client>,
}

impl Default for Forwarder {
    fn default() -> Self {
        Self::new()
    }
}

impl Forwarder {
    /// Create a forwarder with a default pooled client.
    pub fn new() -> Self {
        Self {
            default: build_client(None),
            proxied: DashMap::new(),
        }
    }

    fn client_for(&self, provider: &ProviderConfig) -> Client {
        match &provider.egress_proxy {
            None => self.default.clone(),
            Some(proxy) => self
                .proxied
                .entry(proxy.clone())
                .or_insert_with(|| build_client(Some(proxy)))
                .clone(),
        }
    }

    /// Forward a JSON body to `provider` at `path` and return the raw response.
    ///
    /// `api_key` is injected per provider kind (Bearer for OpenAI-style,
    /// `x-api-key` for Anthropic). When `upstream_model` is set the top-level
    /// `model` field of the body is rewritten to it.
    pub async fn forward_json(
        &self,
        provider: &ProviderConfig,
        path: &str,
        body: Bytes,
        api_key: Option<&str>,
        upstream_model: Option<&str>,
    ) -> Result<Response> {
        let base = provider.api_base.trim_end_matches('/');
        let url = format!("{base}{path}");
        let body = maybe_rewrite_model(body, upstream_model);
        let client = self.client_for(provider);
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
            _ => {
                if let Some(key) = api_key {
                    req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
                }
            }
        }
        req.body(body)
            .send()
            .await
            .map_err(|e| Error::Upstream(e.to_string()))
    }
}

fn build_client(proxy: Option<&str>) -> Client {
    let mut builder = Client::builder()
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(64)
        .tcp_nodelay(true);
    if let Some(url) = proxy {
        if let Ok(px) = Proxy::all(url) {
            builder = builder.proxy(px);
        }
    }
    builder.build().unwrap_or_else(|_| Client::new())
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
}
