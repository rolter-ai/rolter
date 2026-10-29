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
                ..Default::default()
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
            key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), &key_of(org)),
            id: format!("key-{org}"),
            org_id: org.to_string(),
            ..Default::default()
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

/// An org's own `chat` and `openai` shadow the deployment-wide rows of the
/// same names (a control plane's config file) for that org's keys only
/// (#3516): every other org and a key with no org are served the global rows.
#[tokio::test]
async fn an_orgs_own_row_shadows_a_global_row_of_the_same_name_for_its_keys_only() {
    let (a, a_calls) = upstream("from-org-a").await;
    let (global, global_calls) = upstream("from-file").await;
    let mut config = config([format!("http://{a}"), format!("http://{global}")], false);
    config.cache.enabled = false;
    // org-b's rows become the file's: no tenancy
    config.providers[1].tenancy = None;
    config.routes[1].tenancy = None;
    config.db_virtual_keys.push(VirtualKeyRecord {
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), &key_of("")),
        id: "key-global".to_string(),
        ..Default::default()
    });
    let (gw, _) = gateway(&config).await;

    assert_eq!(
        ask(gw, "org-a", "hello").await,
        (200, "from-org-a".to_string())
    );
    assert_eq!(
        ask(gw, "org-b", "hello").await,
        (200, "from-file".to_string())
    );
    assert_eq!(ask(gw, "", "hello").await, (200, "from-file".to_string()));
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
    assert_eq!(global_calls.load(Ordering::SeqCst), 2);
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
    assert_eq!(&*squatter.namespace, "faq@org-b");
    assert!(snapshot.routes.owned_by("org-b", "faq").is_none());
    let namespaces: std::collections::HashSet<&str> = snapshot
        .routes
        .values()
        .map(|entry| &*entry.namespace)
        .collect();
    assert_eq!(namespaces.len(), snapshot.routes.len());
}

/// #3462: a cache hit feeds the same metric series as the miss before it,
/// the route's runtime namespace, so two orgs' `chat` hits never land in one
/// bare `chat` series. The integration test of the same name covers misses,
/// variants and complexity; this one needs the in-memory cache.
#[tokio::test]
async fn cache_hits_report_into_their_own_orgs_metric_series() {
    let (a, _) = upstream("from-a").await;
    let (b, _) = upstream("from-b").await;
    let config = config([format!("http://{a}"), format!("http://{b}")], false);
    let (gw, _) = gateway(&config).await;
    for org in ORGS {
        // a miss, then a hit
        assert_eq!(ask(gw, org, "hello").await.0, 200);
        assert_eq!(ask(gw, org, "hello").await.0, 200);
    }
    let expected: Vec<String> = ORGS
        .iter()
        .map(|org| format!("rolter_request_latency_ms_count{{model=\"chat@{org}\"}} 2"))
        .collect();
    let mut body = String::new();
    // the row that feeds the histograms is written as the answer finishes
    for _ in 0..50 {
        body = reqwest::get(format!("http://{gw}{}", config.server.metrics_path))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        if expected.iter().all(|line| body.contains(line.as_str())) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    for line in &expected {
        assert!(body.contains(line.as_str()), "missing {line} in:\n{body}");
    }
    assert!(
        !body.contains("model=\"chat\""),
        "no bare series merges the two orgs:\n{body}"
    );
}

/// A key of `org`, minted in `project`.
fn project_key(org: &str, project: &str) -> String {
    key_of(&format!("{org}-{project}"))
}

/// A control plane's file serves `chat`, the provider `edge` and the group
/// `pool` to every org; org-a has its own row of each name, narrowed to its
/// project `proj-a1` (a `project_only` route, a provider and a group scoped to
/// that project). Each has its own upstream model, so a listing shows which
/// row a key resolved.
fn narrowed_shadows(org_base: String, file_base: String) -> GatewayConfig {
    let narrowed = Some(Tenancy {
        org_id: "org-a".to_string(),
        project_id: Some("proj-a1".to_string()),
    });
    let mut config: GatewayConfig = serde_json::from_value(json!({
        "providers": [
            {"name": "edge", "slug": "edge", "kind": "openai_compatible", "api_base": file_base},
            {"name": "edge", "slug": "edge", "kind": "openai_compatible", "api_base": org_base,
             "tenancy": narrowed},
        ],
        "provider_groups": [
            {"name": "pool", "slug": "pool", "members": [{"provider": "edge"}]},
            {"name": "pool", "slug": "pool", "members": [{"provider": "edge"}],
             "tenancy": narrowed},
        ],
        "routes": [
            {"model": "chat", "targets": [{"provider": "edge", "model": "file-model"}]},
            {"model": "chat", "targets": [{"provider": "edge", "model": "org-model"}],
             "advanced": {"visibility": {"project_only": true}}, "tenancy": narrowed},
        ],
    }))
    .unwrap();
    config.retry.max_retries = 0;
    let pepper = config.server.resolve_key_pepper();
    for (org, project) in [
        ("org-a", "proj-a1"),
        ("org-a", "proj-a2"),
        ("org-b", "proj-b1"),
    ] {
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&pepper, &project_key(org, project)),
            id: format!("key-{org}-{project}"),
            org_id: org.to_string(),
            project_id: project.to_string(),
            ..Default::default()
        });
    }
    config
}

/// Which upstream answers `model` for `key`, or the status when none does.
async fn answer(gw: SocketAddr, key: &str, model: &str) -> Result<String, u16> {
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(key)
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or_default();
    match body["choices"][0]["message"]["content"].as_str() {
        Some(content) if status == 200 => Ok(content.to_string()),
        _ => Err(status),
    }
}

/// An org's row narrowed to one project shadows the global row of its name
/// for that project only: a sibling project of the same org, like another
/// org, resolves as if the org had no such row and reaches the global one
/// (#3546).
#[tokio::test]
async fn a_project_narrowed_org_row_shadows_the_global_row_for_its_project_only() {
    let (org, _) = upstream("from-org-a").await;
    let (file, _) = upstream("from-file").await;
    let config = narrowed_shadows(format!("http://{org}"), format!("http://{file}"));
    let (gw, _) = gateway(&config).await;

    // a `project_only` route, a project-scoped provider and a project-scoped
    // group, each shadowing a global row of the same name
    for model in ["chat", "edge/m", "pool/m"] {
        assert_eq!(
            answer(gw, &project_key("org-a", "proj-a1"), model).await,
            Ok("from-org-a".to_string()),
            "{model}: the narrowed project gets its org's row"
        );
        assert_eq!(
            answer(gw, &project_key("org-a", "proj-a2"), model).await,
            Ok("from-file".to_string()),
            "{model}: a sibling project falls through to the global row"
        );
        assert_eq!(
            answer(gw, &project_key("org-b", "proj-b1"), model).await,
            Ok("from-file".to_string()),
            "{model}: another org gets the global row"
        );
    }
}

/// `GET /v1/models` lists the row each key resolves, so a sibling project
/// sees the global `chat`, `edge` and `pool` its requests reach (#3546).
#[tokio::test]
async fn the_model_listing_follows_a_project_narrowed_shadow() {
    let config = narrowed_shadows("http://127.0.0.1:9".into(), "http://127.0.0.1:9".into());
    let (gw, _) = gateway(&config).await;
    let listed = |key: String| async move {
        let body: Value = reqwest::Client::new()
            .get(format!("http://{gw}/v1/models"))
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut ids: Vec<String> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["id"].as_str())
            .filter(|id| *id != crate::fake_llm::MODEL_NAME)
            .map(str::to_string)
            .collect();
        ids.sort();
        ids
    };

    // the narrowed project lists its org's provider and group
    assert_eq!(
        listed(project_key("org-a", "proj-a1")).await,
        ["chat", "edge/org-model", "pool/org-model"]
    );
    for (org, project) in [("org-a", "proj-a2"), ("org-b", "proj-b1")] {
        assert_eq!(
            listed(project_key(org, project)).await,
            ["chat", "edge/file-model", "pool/file-model"],
            "{org}/{project}"
        );
    }
}

/// Only a row outside the key's tenancy falls through (#3546). An org's own
/// route the key may not use for any other reason (its visibility, the key's
/// model allow-list, the key's provider allow-list) is refused as it is, and
/// never answered by the global row of the same name.
#[tokio::test]
async fn a_refusal_that_is_not_about_tenancy_does_not_fall_through() {
    let (org, org_calls) = upstream("from-org-a").await;
    let (file, file_calls) = upstream("from-file").await;
    let mut config: GatewayConfig = serde_json::from_value(json!({
        "providers": [
            {"name": "file", "kind": "openai_compatible", "api_base": format!("http://{file}")},
            {"name": "orgp", "kind": "openai_compatible", "api_base": format!("http://{org}"),
             "tenancy": {"org_id": "org-a"}},
        ],
        "routes": [
            {"model": "chat", "targets": [{"provider": "file"}]},
            {"model": "chat", "targets": [{"provider": "orgp"}], "tenancy": {"org_id": "org-a"},
             "advanced": {"visibility": {
                 "allowed_key_ids": ["key-ok", "key-models", "key-providers"]}}},
        ],
    }))
    .unwrap();
    config.retry.max_retries = 0;
    let pepper = config.server.resolve_key_pepper();
    for (id, models, providers) in [
        ("key-ok", vec![], vec![]),
        ("key-hidden", vec![], vec![]),
        ("key-models", vec!["other".to_string()], vec![]),
        // the global route's provider: a fall-through would be served
        ("key-providers", vec![], vec!["file".to_string()]),
    ] {
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&pepper, &key_of(id)),
            id: id.to_string(),
            org_id: "org-a".to_string(),
            project_id: "proj-a1".to_string(),
            models,
            providers,
            ..Default::default()
        });
    }
    let (gw, _) = gateway(&config).await;

    assert_eq!(
        answer(gw, &key_of("key-ok"), "chat").await,
        Ok("from-org-a".to_string())
    );
    for id in ["key-hidden", "key-models", "key-providers"] {
        let refused = answer(gw, &key_of(id), "chat").await;
        assert!(
            matches!(refused, Err(status) if status >= 400),
            "{id}: {refused:?}"
        );
    }
    assert_eq!(
        file_calls.load(Ordering::SeqCst),
        0,
        "a refusal fell through"
    );
    assert_eq!(org_calls.load(Ordering::SeqCst), 1);
}

/// An upstream that accepts realtime sockets and counts them.
async fn realtime_upstream() -> (SocketAddr, Arc<AtomicUsize>) {
    use futures_util::{SinkExt, StreamExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sockets = Arc::new(AtomicUsize::new(0));
    let counter = sockets.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let counter = counter.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                while let Some(Ok(message)) = socket.next().await {
                    let close = message.is_close();
                    if socket.send(message).await.is_err() || close {
                        break;
                    }
                }
            });
        }
    });
    (addr, sockets)
}

/// The realtime upgrade resolves its model through the same rule: a sibling
/// project of an org whose `chat` is `project_only` opens its session on the
/// global `chat` (#3546).
#[tokio::test]
async fn a_realtime_session_falls_through_a_project_narrowed_route() {
    let (org, org_sockets) = realtime_upstream().await;
    let (file, file_sockets) = realtime_upstream().await;
    let config = narrowed_shadows(format!("http://{org}"), format!("http://{file}"));
    let (gw, _) = gateway(&config).await;
    let open = |key: String| {
        let (org_sockets, file_sockets) = (org_sockets.clone(), file_sockets.clone());
        async move {
            let mut request =
                tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                    format!("ws://{gw}/v1/realtime?model=chat"),
                )
                .unwrap();
            request.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {key}").parse().unwrap(),
            );
            let (socket, _) = tokio_tungstenite::connect_async(request)
                .await
                .expect("the session opens");
            // the gateway dials the upstream before it answers the first frame
            for _ in 0..200 {
                if org_sockets.load(Ordering::SeqCst) + file_sockets.load(Ordering::SeqCst) > 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            drop(socket);
        }
    };

    open(project_key("org-a", "proj-a2")).await;
    assert_eq!(
        file_sockets.load(Ordering::SeqCst),
        1,
        "the sibling project"
    );
    assert_eq!(org_sockets.load(Ordering::SeqCst), 0);
    open(project_key("org-a", "proj-a1")).await;
    assert_eq!(
        org_sockets.load(Ordering::SeqCst),
        1,
        "the narrowed project"
    );
    assert_eq!(file_sockets.load(Ordering::SeqCst), 1);
}

/// An org's provider and a global group may share a slug (they share one
/// namespace per org, not across orgs). A project-scoped org provider gives way
/// to the global group for the org's other projects, and serves its own (#3546).
#[test]
fn a_project_scoped_org_provider_gives_way_to_a_global_group_of_its_slug() {
    let narrowed = json!({"org_id": "org-a", "project_id": "proj-a1"});
    let config: GatewayConfig = serde_json::from_value(json!({
        "providers": [
            {"name": "file", "kind": "openai_compatible", "api_base": "https://file.example"},
            {"name": "edge", "slug": "edge", "kind": "openai_compatible",
             "api_base": "https://org.example", "tenancy": narrowed},
        ],
        "provider_groups": [
            {"name": "edge", "slug": "edge", "members": [{"provider": "file"}]},
        ],
    }))
    .unwrap();
    let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::new());
    let target = |org: &str, project: &str| {
        let key = crate::state::KeyMeta {
            org_id: org.to_string(),
            project_id: project.to_string(),
            ..Default::default()
        };
        snapshot
            .resolve_for("edge/m", Some(&key))
            .map(|entry| entry.route.targets[0].provider.clone())
    };
    assert_eq!(target("org-a", "proj-a1").as_deref(), Some("edge"));
    assert_eq!(target("org-a", "proj-a2").as_deref(), Some("file"));
    assert_eq!(target("org-b", "proj-b1").as_deref(), Some("file"));
}
