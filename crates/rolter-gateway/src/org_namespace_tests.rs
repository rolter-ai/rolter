//! Two orgs' `chat` routes never share per-route state on the request path
//! (#1857, ADR-0040): not the response cache, exact or semantic, and not a
//! cooldown parked after a connection failure.
//!
//! The gateway is served in-process with the in-memory cache backend, so none
//! of these needs Redis.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, RouteCache,
    SemanticCacheConfig, Target, Tenancy, VirtualKeyRecord,
};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::state::AppState;

const ORGS: [&str; 2] = ["org-a", "org-b"];

fn key_of(org: &str) -> String {
    format!("rk-org-namespace-tests-{org}")
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// An upstream that names itself in every completion and counts its calls,
/// and answers every embedding with one vector, so any two requests are a
/// semantic match.
async fn upstream(name: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "id": "chatcmpl-1",
                        "object": "chat.completion",
                        "choices": [{"index": 0, "finish_reason": "stop",
                                     "message": {"role": "assistant", "content": name}}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    }))
                }
            }),
        )
        .route(
            "/v1/embeddings",
            post(|| async { Json(json!({"data": [{"index": 0, "embedding": [1.0, 0.0]}]})) }),
        );
    (serve(app).await, calls)
}

/// Each org's `openai` provider at `bases[i]` and its `chat` route on it, with
/// the response cache (and, when `semantic`, its semantic layer) on, plus a
/// key per org.
fn config(bases: [String; 2], semantic: bool) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.cache.enabled = true;
    config.retry.max_retries = 0;
    for (org, base) in ORGS.into_iter().zip(bases) {
        let tenancy = Some(Tenancy {
            org_id: org.to_string(),
            project_id: None,
        });
        config.providers.push(ProviderConfig {
            name: "openai".to_string(),
            slug: Some("openai".to_string()),
            kind: ProviderKind::OpenaiCompatible,
            api_base: base,
            tenancy: tenancy.clone(),
            ..Default::default()
        });
        config.routes.push(ModelRoute {
            model: "chat".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![Target {
                provider: "openai".to_string(),
                model: None,
                weight: 1,
                takes_over: Vec::new(),
            }],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: Some(RouteCache {
                enabled: true,
                ttl_secs: Some(600),
                per_key: false,
                semantic: semantic.then(|| SemanticCacheConfig {
                    provider: "openai".to_string(),
                    model: "embed".to_string(),
                    threshold: 0.9,
                    max_candidates: 16,
                }),
            }),
            variants: Default::default(),
            tenancy,
        });
        config.db_virtual_keys.push(VirtualKeyRecord {
            previous: None,
            access_policy: None,
            key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), &key_of(org)),
            id: format!("key-{org}"),
            org_id: org.to_string(),
            team_id: String::new(),
            project_id: String::new(),
            user_id: String::new(),
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
            business_unit_id: String::new(),
            customer_id: String::new(),
        });
    }
    config
}

async fn gateway(config: &GatewayConfig) -> (SocketAddr, AppState) {
    let mut state = AppState::with_logging(config, None);
    state.response_cache = ResponseCache::in_memory();
    let addr = serve(crate::build_router(
        state.clone(),
        &config.server.metrics_path,
        config.server.max_body_bytes,
    ))
    .await;
    (addr, state)
}

async fn ask(gw: SocketAddr, org: &str, prompt: &str) -> (u16, String) {
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(key_of(org))
        .json(&json!({"model": "chat", "messages": [{"role": "user", "content": prompt}]}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or_default();
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (status, content)
}

#[tokio::test]
async fn one_orgs_cached_answer_is_never_served_to_another_org() {
    let (a, a_calls) = upstream("from-a").await;
    let (b, b_calls) = upstream("from-b").await;
    let (gw, _) = gateway(&config(
        [format!("http://{a}"), format!("http://{b}")],
        false,
    ))
    .await;

    assert_eq!(ask(gw, "org-a", "hello").await, (200, "from-a".to_string()));
    // the same body from org-b misses org-a's entry and reaches its own
    assert_eq!(ask(gw, "org-b", "hello").await, (200, "from-b".to_string()));
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
    // and each org's own entry still hits
    assert_eq!(ask(gw, "org-a", "hello").await, (200, "from-a".to_string()));
    assert_eq!(ask(gw, "org-b", "hello").await, (200, "from-b".to_string()));
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn one_orgs_semantic_candidates_are_never_matched_for_another_org() {
    let (a, _) = upstream("from-a").await;
    let (b, b_calls) = upstream("from-b").await;
    let (gw, _) = gateway(&config(
        [format!("http://{a}"), format!("http://{b}")],
        true,
    ))
    .await;

    assert_eq!(ask(gw, "org-a", "hello").await, (200, "from-a".to_string()));
    // a different body is an exact miss and, with one embedding for every
    // input, a semantic match: but only against org-b's own candidates
    assert_eq!(
        ask(gw, "org-b", "hello there").await,
        (200, "from-b".to_string())
    );
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_refused_connection_parks_only_its_own_orgs_target() {
    // nothing listens on org-b's provider: the connect is refused
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let (a, _) = upstream("from-a").await;
    let mut config = config([format!("http://{a}"), format!("http://{closed}")], false);
    config.cache.enabled = false;
    config.cooldown.base_secs = 60;
    config.cooldown.max_secs = 60;
    let (gw, state) = gateway(&config).await;

    let (status, _) = ask(gw, "org-b", "hello").await;
    assert!(status >= 500, "{status}");
    assert!(state.cooldowns.is_parked("chat@org-b", 0));
    assert!(!state.cooldowns.is_parked("chat@org-a", 0));
    assert!(!state.cooldowns.is_parked("chat", 0));
    assert_eq!(ask(gw, "org-a", "hello").await, (200, "from-a".to_string()));
}

/// A snapshot of `routes` and `providers`, given as JSON rows.
fn snapshot_of(providers: Value, routes: Value) -> crate::state::Snapshot {
    let config: GatewayConfig =
        serde_json::from_value(json!({"providers": providers, "routes": routes})).unwrap();
    crate::state::Snapshot::build(&config, &crate::load::LoadTracker::new())
}

fn faq(org: Option<&str>) -> Value {
    let mut route = json!({"model": "faq", "targets": [{"provider": "p"}]});
    if let Some(org) = org {
        route["tenancy"] = json!({"org_id": org});
    }
    route
}

fn provider(org: Option<&str>) -> Value {
    let mut provider =
        json!({"name": "p", "kind": "openai_compatible", "api_base": "https://p.example"});
    if let Some(org) = org {
        provider["tenancy"] = json!({"org_id": org});
    }
    provider
}

/// The exact-cache key of the route `org` resolves `faq` to.
fn faq_key(snapshot: &crate::state::Snapshot, org: &str) -> String {
    let entry = snapshot.routes.get(org, "faq").unwrap();
    ResponseCache::make_key(
        "rolter",
        "/v1/chat/completions",
        crate::handlers::cache_scope(entry, ""),
        b"{}",
    )
}

#[test]
fn an_org_routes_cache_key_does_not_depend_on_whether_its_name_is_shared() {
    let both = json!([provider(Some("org-a")), provider(Some("org-b"))]);
    let a_alone = snapshot_of(both.clone(), json!([faq(Some("org-a"))]));
    let shared = snapshot_of(
        both.clone(),
        json!([faq(Some("org-a")), faq(Some("org-b"))]),
    );
    let b_alone = snapshot_of(both, json!([faq(Some("org-b"))]));
    let global = snapshot_of(json!([provider(None)]), json!([faq(None)]));

    // org-a keeps one key whether or not org-b shares the name, so its entries
    // never sit under a key a later org-b route would read once org-a's route
    // is gone and the name is unshared again
    assert_eq!(faq_key(&a_alone, "org-a"), faq_key(&shared, "org-a"));
    assert_eq!(faq_key(&b_alone, "org-b"), faq_key(&shared, "org-b"));
    assert_ne!(faq_key(&a_alone, "org-a"), faq_key(&b_alone, "org-b"));
    // a global route keeps the key it always had, which no org route takes
    let global_key = faq_key(&global, "");
    assert_eq!(
        global_key,
        ResponseCache::make_key("rolter", "/v1/chat/completions", "", b"{}")
    );
    assert_ne!(global_key, faq_key(&a_alone, "org-a"));
}

#[test]
fn a_reference_that_resolves_to_nothing_never_names_another_orgs_provider() {
    // org-a's `p` is the only provider of that name, so its runtime id is the
    // bare name; a global route naming `p` must not land on it
    let snapshot = snapshot_of(json!([provider(Some("org-a"))]), json!([faq(None)]));
    let entry = snapshot.routes.get("", "faq").unwrap();
    let target = &entry.route.targets[0].provider;
    assert!(target.starts_with('\u{0}'), "{target:?}");
    assert!(!snapshot.providers.contains_key(target));
    assert!(snapshot.providers.contains_key("p"));
}

#[test]
fn a_route_name_spelling_another_orgs_namespace_does_not_share_its_state() {
    // org-a names a route `faq@org-b`, which is exactly the namespace org-b's
    // `faq` takes once a global `faq` shares the name: the first keeps it
    let snapshot = snapshot_of(
        json!([
            provider(None),
            provider(Some("org-a")),
            provider(Some("org-b"))
        ]),
        json!([
            {"model": "faq@org-b", "targets": [{"provider": "p"}], "tenancy": {"org_id": "org-a"}},
            faq(None),
            faq(Some("org-b")),
        ]),
    );
    let squatter = snapshot.routes.get("org-a", "faq@org-b").unwrap();
    assert_eq!(squatter.namespace, "faq@org-b");
    assert!(snapshot.routes.owned_by("org-b", "faq").is_none());
    let namespaces: std::collections::HashSet<&str> = snapshot
        .routes
        .values()
        .map(|entry| entry.namespace.as_str())
        .collect();
    assert_eq!(namespaces.len(), snapshot.routes.len());
}
