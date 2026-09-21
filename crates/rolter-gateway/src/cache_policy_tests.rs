//! The post-response policy is the same with the response cache on or off
//! (#1477).
//!
//! Every test serves the gateway in-process over a stub upstream, stub
//! `post_response` plugins and a stub PII sanitizer, with the in-memory cache
//! backend, so none of them needs Redis or a real provider. Each delivery path
//! is exercised: cache off, the cacheable first miss, an exact hit and a
//! semantic hit. Hits are filled while no policy applies and then served after
//! a hot-reload installs one, which is the case a store-time policy would get
//! wrong.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, FailureMode, GatewayConfig, ModelRoute, PiiSanitizerConfig,
    PluginInstanceConfig, PluginStage, PluginsConfig, ProviderConfig, ProviderKind,
    RestorationPolicy, RouteCache, SanitizeDirection, SemanticCacheConfig, Target,
};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::state::AppState;

const PLACEHOLDER: &str = "<EMAIL_1>";

/// Upstream that answers with the first user message it received, so what the
/// provider saw (a placeholder, after sanitization) is what comes back.
async fn echo_chat(State(calls): State<Arc<AtomicUsize>>, Json(body): Json<Value>) -> Json<Value> {
    calls.fetch_add(1, Ordering::SeqCst);
    let seen = body["messages"][0]["content"].as_str().unwrap_or_default();
    Json(json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": seen}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
}

async fn embeddings() -> Json<Value> {
    // one vector for every input: any two compatible requests are a semantic match
    Json(json!({"data": [{"index": 0, "embedding": [1.0, 0.0]}]}))
}

/// A stub sanitizer whose restoration token *is* the address it replaced, so a
/// restore can only produce the address of the request that owns the ticket.
fn sanitizer() -> Router {
    Router::new()
        .route(
            "/sanitize",
            post(|Json(body): Json<Value>| async move {
                let mut content = body["content"].clone();
                let text = content["messages"][0]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let address = text
                    .split_whitespace()
                    .find(|word| word.contains('@'))
                    .map(str::to_string);
                let mut out = json!({"content": content.clone(), "findings": []});
                if let Some(address) = address {
                    content["messages"][0]["content"] =
                        Value::from(text.replace(&address, PLACEHOLDER));
                    out = json!({
                        "content": content,
                        "findings": [{"entity_type": "EMAIL_ADDRESS", "count": 1, "placeholders": [PLACEHOLDER]}],
                        "restoration_token": address,
                    });
                }
                Json(out)
            }),
        )
        .route(
            "/restore",
            post(|Json(body): Json<Value>| async move {
                let address = body["restoration_token"].as_str().unwrap_or_default();
                let restored = body["content"]
                    .to_string()
                    .replace(PLACEHOLDER, address);
                Json(json!({"content": serde_json::from_str::<Value>(&restored).unwrap(), "restored": 1}))
            }),
        )
}

fn plugins() -> Router {
    Router::new()
        .route(
            "/block",
            post(|| async { Json(json!({"action": "block", "reason": "denied by plugin"})) }),
        )
        .route(
            "/transform",
            post(|Json(body): Json<Value>| async move {
                let mut content = body["content"].clone();
                content["choices"][0]["message"]["content"] = Value::from("rewritten by plugin");
                Json(json!({"action": "transform", "content": content}))
            }),
        )
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

struct Harness {
    gateway: SocketAddr,
    state: AppState,
    cache: ResponseCache,
    config: GatewayConfig,
    version: u64,
    plugins: SocketAddr,
    sanitizer: SocketAddr,
    upstream_calls: Arc<AtomicUsize>,
    client: reqwest::Client,
}

#[derive(Debug)]
struct Reply {
    status: u16,
    cache: String,
    body: Value,
}

impl Reply {
    fn content(&self) -> &str {
        self.body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
    }
}

impl Harness {
    /// A route `chat` with the exact cache on (`cache`) and, when `semantic`,
    /// the semantic layer too.
    async fn start(cache: bool, semantic: bool) -> Self {
        let upstream_calls = Arc::new(AtomicUsize::new(0));
        let upstream = serve(
            Router::new()
                .route("/v1/chat/completions", post(echo_chat))
                .route("/v1/embeddings", post(embeddings))
                .with_state(upstream_calls.clone()),
        )
        .await;
        let mut config = GatewayConfig::default();
        config.cache.enabled = cache;
        config.providers.push(ProviderConfig {
            name: "up".to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{upstream}"),
            ..Default::default()
        });
        config.routes.push(ModelRoute {
            model: "chat".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![Target {
                provider: "up".to_string(),
                model: None,
                weight: 1,
            }],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: cache.then(|| RouteCache {
                enabled: true,
                ttl_secs: Some(600),
                per_key: false,
                semantic: semantic.then(|| SemanticCacheConfig {
                    provider: "up".to_string(),
                    model: "embed".to_string(),
                    threshold: 0.9,
                    max_candidates: 16,
                }),
            }),
            variants: Default::default(),
        });

        let mut state = AppState::with_logging(&config, None);
        let response_cache = ResponseCache::in_memory();
        state.response_cache = response_cache.clone();
        let gateway = serve(crate::build_router(
            state.clone(),
            &config.server.metrics_path,
            config.server.max_body_bytes,
        ))
        .await;
        Self {
            gateway,
            state,
            cache: response_cache,
            config,
            version: 1,
            plugins: serve(plugins()).await,
            sanitizer: serve(sanitizer()).await,
            upstream_calls,
            client: reqwest::Client::new(),
        }
    }

    /// Hot-reload the gateway with one `post_response` plugin at `endpoint`
    /// (a path on the stub server, or a full url), or with none.
    fn set_plugin(&mut self, endpoint: Option<&str>, failure_mode: FailureMode) {
        self.config.plugins = PluginsConfig {
            instances: endpoint
                .map(|endpoint| PluginInstanceConfig {
                    slug: "policy".to_string(),
                    org_id: String::new(),
                    project_id: None,
                    stage: PluginStage::PostResponse,
                    position: 0,
                    failure_mode,
                    endpoint: if endpoint.starts_with("http") {
                        endpoint.to_string()
                    } else {
                        format!("http://{}{endpoint}", self.plugins)
                    },
                    auth: None,
                })
                .into_iter()
                .collect(),
        };
        self.reload();
    }

    fn set_restoring_sanitizer(&mut self) {
        self.config.pii_sanitizer = PiiSanitizerConfig {
            enabled: true,
            url: format!("http://{}/sanitize", self.sanitizer),
            restore_url: format!("http://{}/restore", self.sanitizer),
            direction: SanitizeDirection::Request,
            restoration: RestorationPolicy::TrustedDownstream,
            ..Default::default()
        };
        self.reload();
    }

    fn reload(&mut self) {
        self.version += 1;
        self.state.reload(&self.config, self.version);
    }

    async fn ask(&self, text: &str) -> Reply {
        let response = self
            .client
            .post(format!("http://{}/v1/chat/completions", self.gateway))
            .json(&json!({"model": "chat", "messages": [{"role": "user", "content": text}]}))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let cache = response
            .headers()
            .get("x-rolter-cache")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.json().await.unwrap_or(Value::Null);
        Reply {
            status,
            cache,
            body,
        }
    }

    fn upstream_calls(&self) -> usize {
        self.upstream_calls.load(Ordering::SeqCst)
    }
}

fn assert_blocked(reply: &Reply) {
    assert_eq!(reply.status, 403, "{reply:?}");
    assert_eq!(reply.body["error"]["code"], "plugin_blocked", "{reply:?}");
}

#[tokio::test]
async fn a_blocking_plugin_blocks_with_the_cache_off() {
    let mut h = Harness::start(false, false).await;
    h.set_plugin(Some("/block"), FailureMode::FailClosed);
    assert_blocked(&h.ask("hello").await);
}

#[tokio::test]
async fn a_blocking_plugin_blocks_the_first_cacheable_miss() {
    // the reproduction from #1477: this returned 200 once the cache was on
    let mut h = Harness::start(true, false).await;
    h.set_plugin(Some("/block"), FailureMode::FailClosed);
    assert_blocked(&h.ask("hello").await);
    // the upstream answer is still stored as the upstream gave it, so lifting
    // the policy lets the entry serve normally
    h.set_plugin(None, FailureMode::FailClosed);
    let reply = h.ask("hello").await;
    assert_eq!((reply.status, reply.cache.as_str()), (200, "HIT"));
    assert_eq!(reply.content(), "hello");
    assert_eq!(h.upstream_calls(), 1);
}

#[tokio::test]
async fn a_plugin_added_by_hot_reload_blocks_an_exact_hit() {
    let mut h = Harness::start(true, false).await;
    assert_eq!(h.ask("hello").await.status, 200);
    h.set_plugin(Some("/block"), FailureMode::FailClosed);
    assert_blocked(&h.ask("hello").await);
    h.set_plugin(None, FailureMode::FailClosed);
    assert_eq!(h.ask("hello").await.cache, "HIT");
    assert_eq!(h.upstream_calls(), 1);
}

#[tokio::test]
async fn a_plugin_added_by_hot_reload_blocks_a_semantic_hit() {
    let mut h = Harness::start(true, true).await;
    assert_eq!(h.ask("hello there").await.status, 200);
    h.set_plugin(Some("/block"), FailureMode::FailClosed);
    // different words: only the semantic layer could answer this
    assert_blocked(&h.ask("hello friend").await);
    h.set_plugin(None, FailureMode::FailClosed);
    let reply = h.ask("hello friend").await;
    assert_eq!((reply.status, reply.cache.as_str()), (200, "HIT"));
    assert_eq!(reply.content(), "hello there");
    assert_eq!(h.upstream_calls(), 1);
}

#[tokio::test]
async fn a_transforming_plugin_rewrites_every_delivery_but_not_the_entry() {
    let mut h = Harness::start(true, true).await;
    h.set_plugin(Some("/transform"), FailureMode::FailClosed);
    let miss = h.ask("hello there").await;
    assert_eq!((miss.status, miss.cache.as_str()), (200, "MISS"));
    assert_eq!(miss.content(), "rewritten by plugin");
    let exact = h.ask("hello there").await;
    assert_eq!(
        (exact.cache.as_str(), exact.content()),
        ("HIT", "rewritten by plugin")
    );
    let semantic = h.ask("hello friend").await;
    assert_eq!(
        (semantic.cache.as_str(), semantic.content()),
        ("HIT", "rewritten by plugin")
    );
    // the stored entry is the upstream's answer; the rewrite happens per delivery
    h.set_plugin(None, FailureMode::FailClosed);
    assert_eq!(h.ask("hello there").await.content(), "hello there");
    assert_eq!(h.upstream_calls(), 1);
}

#[tokio::test]
async fn an_unreachable_plugin_follows_its_failure_mode_on_a_hit() {
    let mut h = Harness::start(true, false).await;
    assert_eq!(h.ask("hello").await.status, 200);
    // nothing listens on port 1
    h.set_plugin(Some("http://127.0.0.1:1/hook"), FailureMode::FailClosed);
    let closed = h.ask("hello").await;
    assert_blocked(&closed);
    assert!(closed.body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("unavailable")));
    h.set_plugin(Some("http://127.0.0.1:1/hook"), FailureMode::FailOpen);
    let open = h.ask("hello").await;
    assert_eq!((open.status, open.cache.as_str()), (200, "HIT"));
    assert_eq!(h.upstream_calls(), 1);
}

#[tokio::test]
async fn a_hit_restores_pii_for_the_request_it_answers() {
    let mut h = Harness::start(true, true).await;
    h.set_restoring_sanitizer();

    let alice = h.ask("mail alice@corp.com now").await;
    assert_eq!((alice.status, alice.cache.as_str()), (200, "MISS"));
    assert_eq!(alice.content(), "mail alice@corp.com now");

    // same sanitized body, so an exact hit on alice's entry: the restore must
    // use bob's ticket, never alice's
    let bob = h.ask("mail bob@corp.com now").await;
    assert_eq!((bob.status, bob.cache.as_str()), (200, "HIT"), "{bob:?}");
    assert_eq!(bob.content(), "mail bob@corp.com now");

    // a semantic hit is restored per request the same way
    let carol = h.ask("please mail carol@corp.com now").await;
    assert_eq!(
        (carol.status, carol.cache.as_str()),
        (200, "HIT"),
        "{carol:?}"
    );
    assert!(carol.content().contains("carol@corp.com"), "{carol:?}");
    assert!(!carol.content().contains("alice"), "{carol:?}");
    assert_eq!(h.upstream_calls(), 1);

    // what was persisted holds the placeholder, never a restored address
    let stored = h.cache.stored_blobs();
    assert!(!stored.is_empty());
    for blob in stored {
        let blob = String::from_utf8_lossy(&blob);
        assert!(
            !blob.contains("@corp.com"),
            "restored pii persisted: {blob}"
        );
    }
}

#[tokio::test]
async fn pii_restoration_on_a_cacheable_miss_matches_the_uncached_path() {
    for cache in [false, true] {
        let mut h = Harness::start(cache, false).await;
        h.set_restoring_sanitizer();
        let reply = h.ask("mail dave@corp.com now").await;
        assert_eq!(reply.status, 200);
        assert_eq!(reply.content(), "mail dave@corp.com now", "cache={cache}");
    }
}
