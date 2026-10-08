//! The `also_track_via_llm_call` health check against real upstream stand-ins.
//!
//! The unit tests in `health.rs` pin the request each kind is sent; these run
//! the sweep itself, so a status the check reads as healthy or failed is
//! proven end to end and not only in the function that classifies it (#2818).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{GatewayConfig, ProviderConfig, ProviderKind};
use serde_json::{json, Value};

/// What a stand-in upstream saw on its chat completions route.
#[derive(Default)]
struct Seen {
    authorization: Vec<String>,
    bodies: Vec<Value>,
}

type Shared = Arc<Mutex<Seen>>;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// An upstream that serves chat completions at `/v1/chat/completions` only,
/// the way Mistral does, and answers anything else `404`.
async fn mistral_like() -> (SocketAddr, Shared) {
    async fn completions(
        State(seen): State<Shared>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let mut seen = seen.lock();
        seen.authorization.push(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
        seen.bodies.push(body);
        Json(json!({"choices": [{"message": {"role": "assistant", "content": "p"}}]}))
    }
    let seen = Shared::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(seen.clone());
    (serve(app).await, seen)
}

/// An upstream whose chat completions answer the given status, and how many
/// times it was asked.
async fn answering(status: StatusCode) -> (SocketAddr, Arc<AtomicU32>) {
    let hits = Arc::new(AtomicU32::new(0));
    let counted = hits.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                (status, "no")
            }
        }),
    );
    (serve(app).await, hits)
}

/// Wait until every upstream was asked at least twice. Sweeps run a second
/// apart and a result is folded in only when its sweep ends, so a second hit
/// proves the first one's result has been judged.
async fn judged_at_least_once(hits: &[&Arc<AtomicU32>]) {
    for _ in 0..150 {
        if hits.iter().all(|hits| hits.load(Ordering::SeqCst) >= 2) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the sweep never asked every upstream twice");
}

fn llm_call_provider(name: &str, kind: ProviderKind, api_base: String) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        kind,
        api_base,
        api_key: Some("sk-probe".to_string()),
        also_track_via_llm_call: true,
        llm_probe_model: Some("probe-model".to_string()),
        ..Default::default()
    }
}

/// Start the prober over `providers` and return the registry it fills in.
fn start_sweeps(providers: Vec<ProviderConfig>) -> rolter_gateway::health::Health {
    let mut config = GatewayConfig {
        providers,
        ..Default::default()
    };
    config.health.enabled = true;
    config.health.interval_secs = 1;
    config.health.timeout_secs = 2;
    // one failed check is enough to park a provider, so the test does not wait
    // out several sweeps
    config.health.consecutive_failure_threshold = 1;
    let state = rolter_gateway::AppState::with_logging(&config, None);
    rolter_gateway::health::spawn_prober(state.clone());
    state.health.clone()
}

/// Run sweeps over `providers` until `done` holds for the registry, or panic.
async fn sweep_until(
    providers: Vec<ProviderConfig>,
    done: impl Fn(&rolter_gateway::health::Health) -> bool,
) -> rolter_gateway::health::Health {
    let health = start_sweeps(providers);
    for _ in 0..150 {
        if done(&health) {
            return health;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the sweep never reached the expected state");
}

/// A `/v1`-carrying kind is called at `{base}/chat/completions` with its key
/// and a one-token body. The check used to post to `/v1/v1/chat/completions`,
/// which this stand-in answers `404`, and read that as healthy.
#[tokio::test]
async fn the_check_reaches_the_endpoint_a_v1_carrying_kind_serves() {
    let (right, seen) = mistral_like().await;
    // a base that is wrong for an openai-shaped kind: the check appends `/v1`
    // to a base that already has one, the way a misconfigured provider would
    let doubled = mistral_like().await.0;
    let providers = vec![
        llm_call_provider(
            "mistral",
            ProviderKind::Mistral,
            format!("http://{right}/v1"),
        ),
        llm_call_provider(
            "doubled",
            ProviderKind::OpenaiCompatible,
            format!("http://{doubled}/v1"),
        ),
    ];
    let health = sweep_until(providers, |health| !health.is_healthy("doubled")).await;
    // a 404 from the endpoint the check was aimed at is a failed check
    assert!(!health.is_healthy("doubled"));
    // let the mistral probe land at least once before asserting on it
    for _ in 0..50 {
        if !seen.lock().bodies.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(health.is_healthy("mistral"));
    let seen = seen.lock();
    assert!(
        !seen.bodies.is_empty(),
        "the check never reached /v1/chat/completions"
    );
    assert_eq!(seen.authorization[0], "Bearer sk-probe");
    assert_eq!(seen.bodies[0]["model"], "probe-model");
    assert_eq!(seen.bodies[0]["max_tokens"], 1);
}

/// A `401`/`403` from the completion means the configured key cannot call the
/// model, which the free probe's "any answer below 500" rule would call healthy.
#[tokio::test]
async fn a_refused_key_fails_the_check() {
    let (unauthorized, _) = answering(StatusCode::UNAUTHORIZED).await;
    let (forbidden, _) = answering(StatusCode::FORBIDDEN).await;
    let providers = vec![
        llm_call_provider(
            "unauthorized",
            ProviderKind::Openai,
            format!("http://{unauthorized}"),
        ),
        llm_call_provider(
            "forbidden",
            ProviderKind::Openai,
            format!("http://{forbidden}"),
        ),
    ];
    sweep_until(providers, |health| {
        !health.is_healthy("unauthorized") && !health.is_healthy("forbidden")
    })
    .await;
}

/// Statuses that are not about where or whether the check may call stay
/// healthy: the model answered, or declined for a reason that is not the
/// check's to judge. A `429` also backs the prober off rather than failing it.
#[tokio::test]
async fn a_rate_limited_or_rejected_prompt_is_still_a_reachable_model() {
    let (limited, limited_hits) = answering(StatusCode::TOO_MANY_REQUESTS).await;
    let (invalid, invalid_hits) = answering(StatusCode::UNPROCESSABLE_ENTITY).await;
    let providers = vec![
        llm_call_provider("limited", ProviderKind::Openai, format!("http://{limited}")),
        llm_call_provider("invalid", ProviderKind::Openai, format!("http://{invalid}")),
    ];
    let health = start_sweeps(providers);
    judged_at_least_once(&[&limited_hits, &invalid_hits]).await;
    assert!(health.is_healthy("limited"));
    assert!(health.is_healthy("invalid"));
}
