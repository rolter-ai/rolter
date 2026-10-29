//! Variant routing on the multipart upload path (#3494).
//!
//! A route with variants dials only the variants' targets: the routing checks
//! count them alone (#3430), and the chat path never consults the classic pool
//! then. `/v1/audio/transcriptions` used to dial the pool regardless, so an
//! audio route's variants were inert and a route whose variants held only
//! fallbacks was dropped although an upload could have served it. These tests
//! drive the upload path against a route with two variants, mock upstreams and
//! an in-process stand-in for the ClickHouse HTTP interface, and check which
//! target was dialled and which variant the row names.

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
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target, Variant,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-multipart-variants";
const PATH: &str = "/v1/audio/transcriptions";

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A stand-in for the ClickHouse HTTP interface keeping every `request_logs`
/// `JSONEachRow` line.
#[derive(Clone, Default)]
struct Rows {
    logs: Arc<Mutex<Vec<Value>>>,
}

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
                        rows.logs.lock().push(row);
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

    /// The row logged for `request_id`, waiting for it to arrive.
    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            if let Some(row) = self
                .logs
                .lock()
                .iter()
                .find(|row| row["request_id"] == request_id)
            {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "no request-log row for {request_id}; saw {:?}",
            self.logs.lock()
        );
    }
}

/// A transcription upstream answering `status`, counting the uploads it gets
/// and keeping the last body, to check the form went through untouched.
#[derive(Clone)]
struct Upstream {
    addr: SocketAddr,
    calls: Arc<AtomicU32>,
    last_body: Arc<Mutex<String>>,
}

#[derive(Clone)]
struct Answer {
    status: StatusCode,
    calls: Arc<AtomicU32>,
    last_body: Arc<Mutex<String>>,
}

async fn transcribe(State(answer): State<Answer>, body: String) -> axum::response::Response {
    answer.calls.fetch_add(1, Ordering::SeqCst);
    *answer.last_body.lock() = body;
    if answer.status.is_success() {
        (answer.status, Json(json!({"text": "hello"}))).into_response()
    } else {
        (
            answer.status,
            Json(json!({"error": {"message": "overloaded"}})),
        )
            .into_response()
    }
}

async fn upstream(status: StatusCode) -> Upstream {
    let calls = Arc::new(AtomicU32::new(0));
    let last_body = Arc::new(Mutex::new(String::new()));
    let addr = serve(
        Router::new()
            .route(PATH, post(transcribe))
            .with_state(Answer {
                status,
                calls: calls.clone(),
                last_body: last_body.clone(),
            }),
    )
    .await;
    Upstream {
        addr,
        calls,
        last_body,
    }
}

fn provider(name: &str, up: &Upstream) -> ProviderConfig {
    ProviderConfig {
        name: name.into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{}", up.addr),
        ..Default::default()
    }
}

fn target(provider: &str) -> Target {
    Target {
        provider: provider.into(),
        model: Some("whisper-1".into()),
        weight: 1,
        takes_over: Vec::new(),
    }
}

fn variant(name: &str, weight: u32, provider: &str) -> Variant {
    Variant {
        name: name.into(),
        weight,
        targets: vec![target(provider)],
        params: Default::default(),
    }
}

/// A route named `model` over the classic `pool` and `variants`.
fn route(model: &str, pool: Vec<Target>, variants: Vec<Variant>) -> ModelRoute {
    ModelRoute {
        model: model.into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: pool,
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants,
        tenancy: None,
    }
}

async fn gateway(config: &GatewayConfig) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, None);
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

fn config(clickhouse: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.retry.base_backoff_ms = 0;
    config.retry.max_backoff_ms = 0;
    config.db_virtual_keys.push(VirtualKeyRecord {
        previous: None,
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: "key-multipart".into(),
        org_id: "org-multipart".into(),
        team_id: "team-multipart".into(),
        project_id: "project-multipart".into(),
        user_id: String::new(),
        models: vec![],
        providers: vec![],
        disabled: false,
        expires_at: None,
        cache: None,
        business_unit_id: String::new(),
        customer_id: String::new(),
    });
    config
}

fn audio_form(model: &str) -> String {
    format!(
        "--VARIANTS\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n\
         --VARIANTS\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFF\r\n--VARIANTS--\r\n"
    )
}

async fn upload(gw: SocketAddr, id: &str, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}{PATH}"))
        .bearer_auth(KEY)
        .header("x-request-id", id)
        .header(
            reqwest::header::CONTENT_TYPE,
            "multipart/form-data; boundary=VARIANTS",
        )
        .body(audio_form(model))
        .send()
        .await
        .unwrap()
}

/// A transcription route with two variants dials the sampled variant's target,
/// never the classic pool, and the row names the variant. `idle` has weight 0,
/// so while `chosen` has a positive weight it takes no first attempts and the
/// pick is deterministic.
#[tokio::test]
async fn a_transcription_dials_the_sampled_variants_target_and_logs_it() {
    let pool = upstream(StatusCode::OK).await;
    let chosen = upstream(StatusCode::OK).await;
    let idle = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let mut config = config(rows.serve().await);
    config.providers = vec![
        provider("pool", &pool),
        provider("chosen", &chosen),
        provider("idle", &idle),
    ];
    config.routes.push(route(
        "whisper",
        vec![target("pool")],
        vec![variant("idle", 0, "idle"), variant("chosen", 1, "chosen")],
    ));
    // a route served only through variants, with no pool of its own
    config.routes.push(route(
        "whisper-variants-only",
        Vec::new(),
        vec![variant("idle", 0, "idle"), variant("chosen", 1, "chosen")],
    ));
    let gw = gateway(&config).await;

    for i in 0..4 {
        let id = format!("transcribe-{i}");
        let response = upload(gw, &id, "whisper").await;
        assert_eq!(response.status(), 200, "{:?}", response.text().await);
        let row = rows.row_for(&id).await;
        assert_eq!(row["variant"], "chosen", "{row}");
        assert_eq!(row["provider"], "chosen", "{row}");
        assert_eq!(row["target"], "whisper-1", "{row}");
        assert_eq!(row["attempts"], 1, "{row}");
    }
    assert_eq!(chosen.calls.load(Ordering::SeqCst), 4);
    assert_eq!(pool.calls.load(Ordering::SeqCst), 0, "the pool was dialled");
    assert_eq!(idle.calls.load(Ordering::SeqCst), 0);
    // the form is forwarded as it came: no variant param reaches it
    assert_eq!(*chosen.last_body.lock(), audio_form("whisper"));

    let response = upload(gw, "variants-only", "whisper-variants-only").await;
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
    let row = rows.row_for("variants-only").await;
    assert_eq!(row["variant"], "chosen", "{row}");
    assert_eq!(chosen.calls.load(Ordering::SeqCst), 5);
}

/// A failing primary variant falls over to the next one in declared order, as
/// on the chat path, and the row names the variant that served.
#[tokio::test]
async fn a_transcription_falls_over_to_the_next_variant() {
    let down = upstream(StatusCode::SERVICE_UNAVAILABLE).await;
    let spare = upstream(StatusCode::OK).await;
    let rows = Rows::default();
    let mut config = config(rows.serve().await);
    config.providers = vec![provider("down", &down), provider("spare", &spare)];
    config.routes.push(route(
        "whisper",
        Vec::new(),
        vec![variant("primary", 1, "down"), variant("spare", 0, "spare")],
    ));
    let gw = gateway(&config).await;

    let response = upload(gw, "failover", "whisper").await;
    assert_eq!(response.status(), 200, "{:?}", response.text().await);
    let row = rows.row_for("failover").await;
    assert_eq!(row["variant"], "spare", "{row}");
    assert_eq!(row["provider"], "spare", "{row}");
    assert_eq!(row["attempts"], 2, "{row}");
    assert_eq!(down.calls.load(Ordering::SeqCst), 1);
    assert_eq!(spare.calls.load(Ordering::SeqCst), 1);
}
