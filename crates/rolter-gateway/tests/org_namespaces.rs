//! Two orgs that each name a route `gpt-4o` and a provider `openai` (#1857,
//! ADR-0040).
//!
//! Each org's keys must reach only their own provider, by the route's name and
//! by `openai/<model>`; the per-target state behind the two routes (breakers,
//! cooldowns, in-flight counts) must never be shared; and the request log must
//! name the provider by its display name and record whose it was.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target, Tenancy,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

const ORG_A: &str = "org-a";
const ORG_B: &str = "org-b";

fn key_of(org: &str) -> String {
    format!("rk-namespaces-{org}")
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// An upstream that answers every chat call with `status` and names itself in
/// the completion, counting the calls it got.
async fn upstream(name: &'static str, status: StatusCode) -> (SocketAddr, Arc<AtomicU32>) {
    let hits = Arc::new(AtomicU32::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let body = json!({
                    "id": "chatcmpl-1",
                    "object": "chat.completion",
                    "created": 0,
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": name},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                });
                (status, Json(body)).into_response()
            }
        }),
    );
    (serve(app).await, hits)
}

/// A stand-in for ClickHouse keeping every `request_logs` row.
#[derive(Clone, Default)]
struct Rows(Arc<Mutex<Vec<Value>>>);

impl Rows {
    async fn serve(&self) -> SocketAddr {
        async fn ingest(
            State(rows): State<Rows>,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
            body: String,
        ) -> &'static str {
            if query.unwrap_or_default().contains("request_logs") {
                for line in body.lines().filter(|line| !line.trim().is_empty()) {
                    if let Ok(row) = serde_json::from_str::<Value>(line) {
                        rows.0.lock().push(row);
                    }
                }
            }
            "ok"
        }
        serve(
            Router::new()
                .route("/", post(ingest))
                .with_state(self.clone()),
        )
        .await
    }

    async fn wait_for(&self, count: usize) -> Vec<Value> {
        for _ in 0..200 {
            if self.0.lock().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.0.lock().clone()
    }
}

fn owned_by(org: &str) -> Option<Tenancy> {
    Some(Tenancy {
        org_id: org.to_string(),
        project_id: None,
    })
}

/// Both orgs' `openai` provider and `gpt-4o` route, as the store composes
/// them, with a key per org.
fn config(a: SocketAddr, b: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.retry.max_retries = 0;
    config.breaker.enabled = true;
    config.breaker.failure_threshold = 2;
    config.breaker.open_secs = 60;
    for (org, upstream) in [(ORG_A, a), (ORG_B, b)] {
        config.providers.push(ProviderConfig {
            name: "openai".into(),
            slug: Some("openai".into()),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{upstream}"),
            tenancy: owned_by(org),
            ..Default::default()
        });
        config.routes.push(ModelRoute {
            model: "gpt-4o".into(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![Target {
                provider: "openai".into(),
                ..Default::default()
            }],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            tenancy: owned_by(org),
        });
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), &key_of(org)),
            id: format!("key-{org}"),
            org_id: org.into(),
            providers: vec!["openai".into()],
            ..Default::default()
        });
    }
    config
}

async fn gateway(state: rolter_gateway::AppState) -> SocketAddr {
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

async fn ask(gw: SocketAddr, org: &str, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(key_of(org))
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

async fn answer(response: reqwest::Response) -> (u16, String) {
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or_default();
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (status, content)
}

#[tokio::test]
async fn each_orgs_key_reaches_only_its_own_route_and_provider() {
    let (a, a_hits) = upstream("from-a", StatusCode::OK).await;
    let (b, b_hits) = upstream("from-b", StatusCode::OK).await;
    let config = config(a, b);
    config.validate().unwrap();
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = gateway(state).await;

    // by the route's name and by the provider's address alike
    for model in ["gpt-4o", "openai/gpt-4o"] {
        assert_eq!(
            answer(ask(gw, ORG_A, model).await).await,
            (200, "from-a".into()),
            "{model}"
        );
        assert_eq!(
            answer(ask(gw, ORG_B, model).await).await,
            (200, "from-b".into()),
            "{model}"
        );
    }
    assert_eq!(a_hits.load(Ordering::SeqCst), 2);
    assert_eq!(b_hits.load(Ordering::SeqCst), 2);

    // each org lists its own route once, never the other's as a second entry
    for org in [ORG_A, ORG_B] {
        let models: Value = reqwest::Client::new()
            .get(format!("http://{gw}/v1/models"))
            .bearer_auth(key_of(org))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let ids: Vec<&str> = models["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["id"].as_str())
            .collect();
        assert_eq!(
            ids.iter().filter(|id| **id == "gpt-4o").count(),
            1,
            "{org}: {ids:?}"
        );
        assert_eq!(
            ids.iter().filter(|id| **id == "openai/gpt-4o").count(),
            1,
            "{org}: {ids:?}"
        );
    }
}

#[tokio::test]
async fn a_failing_provider_in_one_org_never_trips_the_other_orgs_breaker() {
    let (a, a_hits) = upstream("from-a", StatusCode::OK).await;
    let (b, b_hits) = upstream("from-b", StatusCode::SERVICE_UNAVAILABLE).await;
    let config = config(a, b);
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = gateway(state.clone()).await;

    // org-b's provider fails until its breaker opens
    for _ in 0..3 {
        let (status, _) = answer(ask(gw, ORG_B, "gpt-4o").await).await;
        assert!(status >= 500, "{status}");
    }
    let views = state.breaker.snapshot();
    let open: Vec<&str> = views
        .iter()
        .filter(|view| view.state == rolter_gateway::breaker::BreakerPhase::Open)
        .map(|view| view.model.as_str())
        .collect();
    assert_eq!(open, vec!["gpt-4o@org-b"], "{views:?}");
    assert!(state.breaker.allows("gpt-4o@org-a", 0));
    assert!(!state.breaker.allows("gpt-4o@org-b", 0));

    // org-a's route keeps serving from its own provider throughout
    for _ in 0..3 {
        assert_eq!(
            answer(ask(gw, ORG_A, "gpt-4o").await).await,
            (200, "from-a".into())
        );
    }
    assert_eq!(a_hits.load(Ordering::SeqCst), 3);
    // the open breaker stopped org-b's third call before it went upstream:
    // with every target open the gateway fails open, so it may have tried once
    assert!(b_hits.load(Ordering::SeqCst) <= 3);
    assert!(state
        .breaker
        .snapshot()
        .iter()
        .all(|view| view.model != "gpt-4o@org-a"
            || view.state == rolter_gateway::breaker::BreakerPhase::Closed));
}

#[tokio::test]
async fn the_request_log_names_the_provider_and_the_org_that_owns_it() {
    let (a, _) = upstream("from-a", StatusCode::OK).await;
    let (b, _) = upstream("from-b", StatusCode::OK).await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut config = config(a, b);
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = gateway(state).await;

    let response = ask(gw, ORG_B, "gpt-4o").await;
    assert_eq!(
        response
            .headers()
            .get("x-rolter-provider")
            .and_then(|v| v.to_str().ok()),
        Some("openai"),
        "the header shows the display name, never the runtime id"
    );
    assert_eq!(answer(response).await, (200, "from-b".into()));
    let logged = rows.wait_for(1).await;
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert_eq!(logged[0]["provider"], "openai");
    assert_eq!(logged[0]["provider_org_id"], ORG_B);
    assert_eq!(logged[0]["org_id"], ORG_B);
}

#[tokio::test]
async fn a_global_provider_keeps_its_name_and_serves_an_org_without_its_own() {
    let (a, _) = upstream("from-a", StatusCode::OK).await;
    let (global, global_hits) = upstream("from-global", StatusCode::OK).await;
    let mut config = config(a, a);
    // org-b's rows go; a deployment-wide `openai` from the gateway's own file
    // takes their place, and org-a's row still shadows it for org-a
    config
        .providers
        .retain(|p| p.tenancy.as_ref().is_some_and(|t| t.org_id == ORG_A));
    config
        .routes
        .retain(|r| r.tenancy.as_ref().is_some_and(|t| t.org_id == ORG_A));
    config.providers.push(ProviderConfig {
        name: "openai".into(),
        slug: Some("openai".into()),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{global}"),
        ..Default::default()
    });
    config.validate().unwrap();
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let snapshot = state.snapshot.load();
    assert!(snapshot.providers.contains_key("openai"));
    assert!(snapshot.providers.contains_key("openai@org-a"));
    drop(snapshot);
    let gw = gateway(state).await;

    assert_eq!(
        answer(ask(gw, ORG_A, "openai/gpt-4o").await).await,
        (200, "from-a".into())
    );
    assert_eq!(
        answer(ask(gw, ORG_B, "openai/gpt-4o").await).await,
        (200, "from-global".into())
    );
    // org-b has no route of that name and gets no other org's
    assert_eq!(answer(ask(gw, ORG_B, "gpt-4o").await).await.0, 404);
    assert_eq!(global_hits.load(Ordering::SeqCst), 1);
}
