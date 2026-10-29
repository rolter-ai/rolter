//! Image, transcription and speech requests are priced by their unit (#2937).
//!
//! Each test sends one request through a stand-in upstream that answers with a
//! fixed body, and asserts `cost_usd`, `unpriced` and `usage_unknown` on the
//! row the gateway logs. The rates are the ones OpenAI publishes for its
//! per-unit models (dall-e-3 standard 1024x1024 at $0.04 an image, whisper-1 at
//! $0.006 a minute, tts-1 at $15 per million characters), which keeps the
//! arithmetic recognisable; they are test data, not a price list.
//!
//! The tests drive the gateway over HTTP against a mock upstream and an
//! in-process stand-in for the ClickHouse HTTP interface, like
//! `cache_write_pricing.rs`. The budget test needs redis and skips without
//! `ROLTER_TEST_REDIS_URL`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind, Target,
    VirtualKeyRecord,
};
use serde_json::{json, Value};

fn key() -> String {
    format!("sk-unit-pricing-{:x}", std::process::id())
}

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

    async fn row_for(&self, request_id: &str) -> Value {
        for _ in 0..200 {
            let found: Vec<Value> = self
                .logs
                .lock()
                .iter()
                .filter(|row| row["request_id"] == request_id)
                .cloned()
                .collect();
            if let Some(row) = found.first() {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no request-log row arrived for {request_id}");
    }
}

/// An upstream that answers every request with `body` as `content_type`.
async fn upstream(content_type: &'static str, body: &'static str) -> SocketAddr {
    async fn handler(
        State((content_type, body)): State<(&'static str, &'static str)>,
    ) -> impl IntoResponse {
        ([(header::CONTENT_TYPE, content_type)], body)
    }
    serve(
        Router::new()
            .fallback(handler)
            .with_state((content_type, body)),
    )
    .await
}

/// One `test-model` route over one provider, a key in `org`, and `price` as the
/// model's row (or none).
fn config(
    upstream: SocketAddr,
    clickhouse: SocketAddr,
    org: &str,
    price: Option<Value>,
) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.retry.base_backoff_ms = 0;
    config.retry.max_backoff_ms = 0;
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "test-model".into(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "up".into(),
            model: Some("test-model".into()),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config.db_virtual_keys.push(VirtualKeyRecord {
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), &key()),
        id: format!("key-{org}"),
        org_id: org.into(),
        ..Default::default()
    });
    if let Some(mut price) = price {
        price["model"] = json!("test-model");
        config
            .model_prices
            .push(serde_json::from_value(price).unwrap());
    }
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": 100, "period": "monthly"
        }))
        .unwrap(),
    );
    config
}

/// What the caller sends.
enum Call {
    Json(&'static str, Value),
    /// a multipart transcription or translation of a tiny `file`
    Upload(&'static str),
}

struct Answer {
    content_type: &'static str,
    body: &'static str,
}

async fn gateway_for(config: &GatewayConfig, redis: Option<&str>) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, redis);
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

async fn send(gw: SocketAddr, request_id: &str, call: Call) {
    let client = reqwest::Client::new();
    let response = match call {
        Call::Json(path, body) => client
            .post(format!("http://{gw}{path}"))
            .bearer_auth(key())
            .header("x-request-id", request_id)
            .json(&body)
            .send()
            .await
            .unwrap(),
        Call::Upload(path) => {
            let boundary = "UNITBOUND";
            let body = format!(
                "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ntest-model\r\n\
                 --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
                 Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
                b = boundary
            );
            client
                .post(format!("http://{gw}{path}"))
                .bearer_auth(key())
                .header("x-request-id", request_id)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body)
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(response.status(), 200, "{request_id}");
    // read to the end so the stream has finished and its row has been emitted
    let _ = response.bytes().await.unwrap();
}

/// One request to a gateway whose only model has `price`; returns its row.
async fn exchange(request_id: &str, price: Option<Value>, answer: Answer, call: Call) -> Value {
    let upstream = upstream(answer.content_type, answer.body).await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let gw = gateway_for(
        &config(upstream, clickhouse, "org-unit-pricing", price),
        None,
    )
    .await;
    send(gw, request_id, call).await;
    rows.row_for(request_id).await
}

fn images(n: u64) -> Call {
    Call::Json(
        "/v1/images/generations",
        json!({"model": "test-model", "prompt": "a cat", "n": n}),
    )
}

fn speech(input: &str) -> Call {
    Call::Json(
        "/v1/audio/speech",
        json!({"model": "test-model", "input": input, "voice": "alloy"}),
    )
}

const THREE_IMAGES: Answer = Answer {
    content_type: "application/json",
    body: r#"{"created":1,"data":[{"url":"a"},{"url":"b"},{"url":"c"}]}"#,
};

const IMAGE_RATE: fn() -> Option<Value> = || Some(json!({"image_per_unit": 0.04}));

#[tokio::test]
async fn an_image_request_is_priced_by_the_images_returned() {
    // asked for two, got three: the provider bills what it returned
    let row = exchange("up-images", IMAGE_RATE(), THREE_IMAGES, images(2)).await;
    assert_eq!(row["cost_usd"], 0.12, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
    // no prompt-cache figure was stated, so the row leaves the column out and
    // a ClickHouse without it accepts the row (#3410)
    assert!(row.get("cache_usage_reported").is_none(), "{row}");
}

#[tokio::test]
async fn an_image_request_without_a_data_array_is_priced_by_n() {
    let row = exchange(
        "up-images-n",
        IMAGE_RATE(),
        Answer {
            content_type: "application/json",
            body: r#"{"created":1}"#,
        },
        images(2),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.08, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
}

#[tokio::test]
async fn an_image_request_on_a_model_with_no_price_is_unpriced() {
    let row = exchange("up-images-none", None, THREE_IMAGES, images(1)).await;
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    assert_eq!(row["unpriced"], 1, "{row}");
}

#[tokio::test]
async fn an_image_request_on_a_price_with_only_token_rates_is_unpriced() {
    let row = exchange(
        "up-images-tokens-only",
        Some(json!({"input_per_mtok": 5, "output_per_mtok": 15})),
        THREE_IMAGES,
        images(1),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    assert_eq!(row["unpriced"], 1, "{row}");
}

/// `gpt-image-1` reports tokens; a row that sets both kinds of rate charges
/// both, the image count on top of the tokens.
#[tokio::test]
async fn an_image_request_adds_its_unit_cost_to_its_token_cost() {
    let row = exchange(
        "up-images-both",
        Some(json!({
            "input_per_mtok": 1_000_000, "output_per_mtok": 1_000_000, "image_per_unit": 0.5
        })),
        Answer {
            content_type: "application/json",
            body: r#"{"data":[{"b64_json":"x"}],"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}"#,
        },
        images(1),
    )
    .await;
    // 10 + 20 tokens at a dollar, and one image at fifty cents
    assert_eq!(row["cost_usd"], 30.5, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

#[tokio::test]
async fn a_transcription_is_priced_by_the_duration_it_states() {
    let row = exchange(
        "up-transcribe",
        Some(json!({"audio_input_per_minute": 0.006})),
        Answer {
            content_type: "application/json",
            body: r#"{"text":"hi","duration":90.0}"#,
        },
        Call::Upload("/v1/audio/transcriptions"),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.009, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

#[tokio::test]
async fn a_translation_reads_a_duration_usage_object() {
    let row = exchange(
        "up-translate",
        Some(json!({"audio_input_per_minute": 0.006})),
        Answer {
            content_type: "application/json",
            body: r#"{"text":"hi","usage":{"type":"duration","seconds":120}}"#,
        },
        Call::Upload("/v1/audio/translations"),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.012, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

/// A transcript an output guardrail withholds was still transcribed, and its
/// minutes are billed from the upstream's answer, not the refusal (#3331).
#[tokio::test]
async fn a_withheld_transcription_is_billed_its_minutes() {
    use rolter_core::{BuiltinRule, GuardAction, GuardStage, GuardrailRule, GuardrailsConfig};
    let upstream = upstream(
        "application/json",
        r#"{"text":"card 4111 1111 1111 1111","duration":90.0}"#,
    )
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut config = config(
        upstream,
        clickhouse,
        "org-unit-pricing",
        Some(json!({"audio_input_per_minute": 0.006})),
    );
    config.guardrails = GuardrailsConfig {
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
    };
    let gw = gateway_for(&config, None).await;
    let boundary = "UNITBOUND";
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/audio/transcriptions"))
        .bearer_auth(key())
        .header("x-request-id", "up-transcribe-withheld")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ntest-model\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\nRIFFxxxxWAVE\r\n--{b}--\r\n",
            b = boundary
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let _ = response.bytes().await.unwrap();
    let row = rows.row_for("up-transcribe-withheld").await;
    assert_eq!(row["withheld"], 1, "{row}");
    assert_eq!(row["cost_usd"], 0.009, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

/// A `json` transcription carries the text and nothing else: the minutes are
/// not known, which is not the same as free.
#[tokio::test]
async fn a_transcription_with_no_stated_duration_is_unknown_not_free() {
    let row = exchange(
        "up-transcribe-bare",
        Some(json!({"audio_input_per_minute": 0.006})),
        Answer {
            content_type: "application/json",
            body: r#"{"text":"hi"}"#,
        },
        Call::Upload("/v1/audio/transcriptions"),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    assert_eq!(row["usage_unknown"], 1, "{row}");
}

#[tokio::test]
async fn a_transcription_on_a_price_with_no_audio_rate_is_unpriced() {
    let row = exchange(
        "up-transcribe-norate",
        Some(json!({"input_per_mtok": 5})),
        Answer {
            content_type: "application/json",
            body: r#"{"text":"hi","duration":90.0}"#,
        },
        Call::Upload("/v1/audio/transcriptions"),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.0, "{row}");
    assert_eq!(row["unpriced"], 1, "{row}");
}

#[tokio::test]
async fn speech_is_priced_by_the_characters_of_its_input() {
    let row = exchange(
        "up-speech",
        Some(json!({"speech_per_mchar": 15})),
        Answer {
            content_type: "audio/mpeg",
            body: "ID3-not-really-audio",
        },
        // twenty characters, a multi-byte one among them
        speech("héllo wörld, hello 世"),
    )
    .await;
    assert_eq!(row["cost_usd"], 0.0003, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

#[tokio::test]
async fn a_chat_request_is_untouched_by_the_unit_rates() {
    let row = exchange(
        "up-chat",
        Some(json!({
            "input_per_mtok": 1_000_000, "output_per_mtok": 1_000_000,
            "image_per_unit": 9, "audio_input_per_minute": 9, "speech_per_mchar": 9
        })),
        Answer {
            content_type: "application/json",
            body: r#"{"id":"c","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#,
        },
        Call::Json(
            "/v1/chat/completions",
            json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}),
        ),
    )
    .await;
    assert_eq!(row["cost_usd"], 7.0, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
}

/// #1882: an audio chat model reports the audio share of its tokens under
/// `prompt_tokens_details` / `completion_tokens_details`, and each is charged at
/// the row's audio rate: 4 text x 1 + 6 audio x 10 + 1 text out x 2 + 9 audio
/// out x 20.
#[tokio::test]
async fn a_chat_audio_model_is_priced_by_modality() {
    let row = exchange(
        "up-chat-audio",
        Some(json!({
            "input_per_mtok": 1_000_000, "output_per_mtok": 2_000_000,
            "audio_input_per_mtok": 10_000_000, "audio_output_per_mtok": 20_000_000
        })),
        Answer {
            content_type: "application/json",
            body: r#"{"id":"c","model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":10,"total_tokens":20,"prompt_tokens_details":{"audio_tokens":6,"text_tokens":4},"completion_tokens_details":{"audio_tokens":9,"text_tokens":1}}}"#,
        },
        Call::Json(
            "/v1/chat/completions",
            json!({"model": "test-model", "messages": [{"role": "user", "content": "ping"}]}),
        ),
    )
    .await;
    assert_eq!(row["cost_usd"], 4.0 + 60.0 + 2.0 + 180.0, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
}

/// #1882: a token-billed transcription model (`gpt-4o-transcribe`) reports its
/// audio and text input apart in `usage.input_token_details`, and the audio is
/// charged at the audio rate: 3 text x 2 + 10 audio x 6 + 9 out x 10.
#[tokio::test]
async fn a_token_billed_transcription_is_priced_by_modality() {
    let row = exchange(
        "up-transcribe-tokens",
        Some(json!({
            "input_per_mtok": 2_000_000, "output_per_mtok": 10_000_000,
            "audio_input_per_mtok": 6_000_000
        })),
        Answer {
            content_type: "application/json",
            body: r#"{"text":"hi","usage":{"type":"tokens","input_tokens":13,"input_token_details":{"text_tokens":3,"audio_tokens":10},"output_tokens":9,"total_tokens":22}}"#,
        },
        Call::Upload("/v1/audio/transcriptions"),
    )
    .await;
    assert_eq!(row["cost_usd"], 156.0, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

fn redis_url() -> Option<String> {
    let url = std::env::var("ROLTER_TEST_REDIS_URL").ok()?;
    (!url.is_empty()).then_some(url)
}

/// An image, a transcription and a speech request each advance the org's
/// budget by their unit cost, as a token request advances it by its tokens.
#[tokio::test]
async fn unit_priced_requests_advance_the_budget() {
    use redis::AsyncCommands;
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let org = format!("org-unit-pricing-{}-{nanos}", std::process::id());
    let image_upstream = upstream("application/json", THREE_IMAGES.body).await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let price = json!({"image_per_unit": 0.5});
    let gw = gateway_for(
        &config(image_upstream, clickhouse, &org, Some(price)),
        Some(&redis),
    )
    .await;
    send(gw, "budget-images", images(3)).await;

    let client = redis::Client::open(redis.as_str()).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let key = format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let spend = loop {
        let value: Option<String> = conn.get(&key).await.unwrap();
        if let Some(value) = value {
            break value.parse::<f64>().unwrap();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no spend reached the budget"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // three images at fifty cents
    assert_eq!(spend, 1.5);
}

/// A cached image route whose `post_response` plugin blocks the reply still
/// bills the images the upstream returned: the count is read from the
/// upstream body on the cache-miss path, not from the refusal the caller
/// receives (#3404). Needs redis for the response cache.
#[tokio::test]
async fn a_withheld_cache_miss_bills_its_images() {
    use rolter_core::{FailureMode, PluginInstanceConfig, PluginStage, PluginsConfig};
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let upstream = upstream("application/json", THREE_IMAGES.body).await;
    let hook = serve(Router::new().route(
        "/hook",
        post(|| async { axum::Json(json!({"action": "block"})) }),
    ))
    .await;
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let org = "org-unit-pricing-cached";
    let mut config = config(upstream, clickhouse, org, IMAGE_RATE());
    config.cache.enabled = true;
    // a namespace of its own so an entry from an earlier run is never a hit
    config.cache.namespace = format!("rolter-test-3404-{}", uuid::Uuid::new_v4());
    config.routes[0].cache = Some(serde_json::from_value(json!({"enabled": true})).unwrap());
    config.plugins = PluginsConfig {
        instances: vec![PluginInstanceConfig {
            slug: "veto".to_string(),
            org_id: org.to_string(),
            project_id: None,
            stage: PluginStage::PostResponse,
            position: 0,
            failure_mode: FailureMode::FailOpen,
            endpoint: format!("http://{hook}/hook"),
            auth: None,
        }],
    };
    let gw = gateway_for(&config, Some(&redis)).await;
    // asked for one, got three: a refusal body carries no `data`, so reading
    // it would bill the one asked for
    let response = reqwest::Client::new()
        .post(format!("http://{gw}/v1/images/generations"))
        .bearer_auth(key())
        .header("x-request-id", "up-images-cached-withheld")
        .json(&json!({"model": "test-model", "prompt": "a cat", "n": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let _ = response.bytes().await.unwrap();
    let row = rows.row_for("up-images-cached-withheld").await;
    assert_eq!(row["withheld"], 1, "{row}");
    assert_eq!(row["cache_hit"], 0, "{row}");
    assert_eq!(row["cost_usd"], 0.12, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}
