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

/// What a mock transcription upstream answers.
#[derive(Clone)]
enum Reply {
    /// `{"text": …}` as JSON, whatever the form's `stream` field says
    Text(&'static str),
    /// `transcript.text.*` events, as a model that streams answers
    Stream(&'static str),
    /// an error with this status
    Fail(StatusCode),
}

/// A transcription upstream answering with a [`Reply`], counting the uploads
/// it gets and keeping the last body and content type, to check the form went
/// through untouched.
#[derive(Clone)]
struct Upstream {
    addr: SocketAddr,
    calls: Arc<AtomicU32>,
    last_body: Arc<Mutex<String>>,
    last_content_type: Arc<Mutex<String>>,
}

#[derive(Clone)]
struct Answer {
    reply: Reply,
    calls: Arc<AtomicU32>,
    last_body: Arc<Mutex<String>>,
    last_content_type: Arc<Mutex<String>>,
}

async fn transcribe(
    State(answer): State<Answer>,
    headers: axum::http::HeaderMap,
    body: String,
) -> axum::response::Response {
    answer.calls.fetch_add(1, Ordering::SeqCst);
    *answer.last_body.lock() = body;
    *answer.last_content_type.lock() = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    match answer.reply {
        Reply::Text(text) => (StatusCode::OK, Json(json!({"text": text}))).into_response(),
        Reply::Stream(text) => {
            let events = format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"type": "transcript.text.delta", "delta": text}),
                json!({"type": "transcript.text.done", "text": text}),
            );
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                events,
            )
                .into_response()
        }
        Reply::Fail(status) => {
            (status, Json(json!({"error": {"message": "overloaded"}}))).into_response()
        }
    }
}

async fn upstream(reply: Reply) -> Upstream {
    let calls = Arc::new(AtomicU32::new(0));
    let last_body = Arc::new(Mutex::new(String::new()));
    let last_content_type = Arc::new(Mutex::new(String::new()));
    let addr = serve(
        Router::new()
            .route(PATH, post(transcribe))
            .with_state(Answer {
                reply,
                calls: calls.clone(),
                last_body: last_body.clone(),
                last_content_type: last_content_type.clone(),
            }),
    )
    .await;
    Upstream {
        addr,
        calls,
        last_body,
        last_content_type,
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
        ..Default::default()
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
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: "key-multipart".into(),
        org_id: "org-multipart".into(),
        team_id: "team-multipart".into(),
        project_id: "project-multipart".into(),
        ..Default::default()
    });
    config
}

fn audio_form(model: &str) -> String {
    audio_form_with(model, &[])
}

/// The upload form for `model`, with extra text `fields` before the file.
fn audio_form_with(model: &str, fields: &[(&str, &str)]) -> String {
    let mut form =
        format!("--VARIANTS\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n");
    for (name, value) in fields {
        form.push_str(&format!(
            "--VARIANTS\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    form.push_str(
        "--VARIANTS\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\nRIFF\r\n--VARIANTS--\r\n",
    );
    form
}

async fn upload(gw: SocketAddr, id: &str, model: &str) -> reqwest::Response {
    upload_with(gw, id, model, &[]).await
}

async fn upload_with(
    gw: SocketAddr,
    id: &str,
    model: &str,
    fields: &[(&str, &str)],
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}{PATH}"))
        .bearer_auth(KEY)
        .header("x-request-id", id)
        .header(
            reqwest::header::CONTENT_TYPE,
            "multipart/form-data; boundary=VARIANTS",
        )
        .body(audio_form_with(model, fields))
        .send()
        .await
        .unwrap()
}

/// The `error.code` of a refusal.
async fn error_code(response: reqwest::Response) -> String {
    let body: Value = response.json().await.unwrap();
    body["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A blocking built-in output rule for payment card numbers.
fn card_rule() -> rolter_core::GuardrailsConfig {
    use rolter_core::{BuiltinRule, GuardAction, GuardStage, GuardrailRule, GuardrailsConfig};
    GuardrailsConfig {
        enabled: true,
        rules: vec![GuardrailRule {
            name: "card".into(),
            builtin: Some(BuiltinRule::PaymentCard),
            pattern: None,
            stage: GuardStage::PostCall,
            action: GuardAction::Block,
            replacement: None,
            include_system: false,
        }],
        ..Default::default()
    }
}

/// A route named `model` with an idle variant on `idle` and the variant every
/// first attempt takes, `chosen`, on `provider`.
fn split_route(model: &str, provider: &str) -> ModelRoute {
    route(
        model,
        Vec::new(),
        vec![variant("idle", 0, "idle"), variant("chosen", 1, provider)],
    )
}

/// A transcription route with two variants dials the sampled variant's target,
/// never the classic pool, and the row names the variant. `idle` has weight 0,
/// so while `chosen` has a positive weight it takes no first attempts and the
/// pick is deterministic.
#[tokio::test]
async fn a_transcription_dials_the_sampled_variants_target_and_logs_it() {
    let pool = upstream(Reply::Text("hello")).await;
    let chosen = upstream(Reply::Text("hello")).await;
    let idle = upstream(Reply::Text("hello")).await;
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
        assert_eq!(row["first_variant"], "chosen", "{row}");
        assert_eq!(row["provider"], "chosen", "{row}");
        assert_eq!(row["target"], "whisper-1", "{row}");
        assert_eq!(row["attempts"], 1, "{row}");
    }
    assert_eq!(chosen.calls.load(Ordering::SeqCst), 4);
    assert_eq!(pool.calls.load(Ordering::SeqCst), 0, "the pool was dialled");
    assert_eq!(idle.calls.load(Ordering::SeqCst), 0);
    // the form is forwarded as it came, boundary included: no variant param
    // reaches it
    assert_eq!(*chosen.last_body.lock(), audio_form("whisper"));
    assert_eq!(
        *chosen.last_content_type.lock(),
        "multipart/form-data; boundary=VARIANTS"
    );

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
    let down = upstream(Reply::Fail(StatusCode::SERVICE_UNAVAILABLE)).await;
    let spare = upstream(Reply::Text("hello")).await;
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
    // the variant the split assigned it, which failed (#3505)
    assert_eq!(row["first_variant"], "primary", "{row}");
    // and the variant it sampled, on the upload path as on chat (#3547)
    assert_eq!(row["sampled_variant"], "primary", "{row}");
    assert_eq!(row["provider"], "spare", "{row}");
    assert_eq!(row["attempts"], 2, "{row}");
    assert_eq!(down.calls.load(Ordering::SeqCst), 1);
    assert_eq!(spare.calls.load(Ordering::SeqCst), 1);
}

/// The response stage of the guardrails judges a transcript served through a
/// variant as it judges one from the classic pool (#3331): a blocking output
/// rule withholds it with `403 guardrail_blocked`, and the row names the
/// variant.
#[tokio::test]
async fn an_output_rule_withholds_a_transcript_served_through_a_variant() {
    let idle = upstream(Reply::Text("unused")).await;
    let carded = upstream(Reply::Text("card 4111 1111 1111 1111")).await;
    let rows = Rows::default();
    let mut config = config(rows.serve().await);
    config.providers = vec![provider("idle", &idle), provider("carded", &carded)];
    config.routes.push(split_route("whisper", "carded"));
    config.guardrails = card_rule();
    let gw = gateway(&config).await;

    let response = upload(gw, "withheld", "whisper").await;
    assert_eq!(response.status(), 403);
    assert_eq!(error_code(response).await, "guardrail_blocked");
    assert_eq!(carded.calls.load(Ordering::SeqCst), 1);
    let row = rows.row_for("withheld").await;
    assert_eq!(row["variant"], "chosen", "{row}");
    assert_eq!(row["withheld"], 1, "{row}");
}

/// A `stream=true` upload through a variant keeps #3432's rules. With the
/// default `streaming_post_call = "reject"` it is refused before dialling. With
/// `passthrough` the response legs are still built whatever the form field
/// says: a model that ignores `stream` and answers JSON is judged and withheld,
/// and only a real event stream goes through with the legs set aside.
#[tokio::test]
async fn a_streamed_transcription_through_a_variant_keeps_the_stream_rules() {
    let idle = upstream(Reply::Text("unused")).await;
    let json_reply = upstream(Reply::Text("card 4111 1111 1111 1111")).await;
    let events = upstream(Reply::Stream("card 4111 1111 1111 1111")).await;
    let rows = Rows::default();
    let mut config = config(rows.serve().await);
    config.providers = vec![
        provider("idle", &idle),
        provider("json", &json_reply),
        provider("events", &events),
    ];
    config.routes.push(split_route("whisper-json", "json"));
    config.routes.push(split_route("whisper-events", "events"));
    config.guardrails = card_rule();
    let stream = [("stream", "true")];

    let gw = gateway(&config).await;
    let refused = upload_with(gw, "stream-rejected", "whisper-events", &stream).await;
    assert_eq!(refused.status(), 400);
    assert_eq!(error_code(refused).await, "guardrail_streaming_unsupported");
    assert_eq!(
        events.calls.load(Ordering::SeqCst),
        0,
        "refused before dialling"
    );

    config.guardrails.streaming_post_call = rolter_core::StreamingPostCall::Passthrough;
    let gw = gateway(&config).await;
    // the model ignored `stream`: its JSON answer is judged like any other
    let judged = upload_with(gw, "stream-ignored", "whisper-json", &stream).await;
    assert_eq!(judged.status(), 403);
    assert_eq!(error_code(judged).await, "guardrail_blocked");
    assert_eq!(json_reply.calls.load(Ordering::SeqCst), 1);
    let row = rows.row_for("stream-ignored").await;
    assert_eq!(row["variant"], "chosen", "{row}");

    // a real event stream is served with the output legs set aside
    let streamed = upload_with(gw, "stream-served", "whisper-events", &stream).await;
    assert_eq!(streamed.status(), 200);
    let body = streamed.text().await.unwrap();
    assert!(body.contains("transcript.text.delta"), "{body}");
    assert_eq!(events.calls.load(Ordering::SeqCst), 1);
    let row = rows.row_for("stream-served").await;
    assert_eq!(row["variant"], "chosen", "{row}");
}
