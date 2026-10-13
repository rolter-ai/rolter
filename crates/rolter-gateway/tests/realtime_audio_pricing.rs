//! Realtime audio pricing and input-transcription metering (#1882, #1883).
//!
//! A realtime session bills the audio tokens of a response apart from its text
//! ones, and the upstream runs a second model over the caller's audio whose
//! usage arrives on its own event. These tests drive real WebSocket sessions
//! through the gateway against a mock Realtime upstream that reports both, and
//! read the rows the gateway sends to an in-process stand-in for ClickHouse.
//! The one budget assertion needs Redis and skips without `ROLTER_TEST_REDIS_URL`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use rolter_core::{GatewayConfig, ProviderConfig, ProviderKind};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const KEY: &str = "sk-realtime-audio-pricing";
const MODEL: &str = "gpt-realtime";

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

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
        // one more interval, so a row that should not exist can fail the count
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows = self.0.lock().clone();
        assert_eq!(rows.len(), count, "expected {count} rows, saw {rows:?}");
        rows
    }
}

/// A Realtime upstream. Each `response.create` is answered with a response
/// whose `response.done` carries `usage`; each `input_audio_buffer.commit` with
/// `transcription` (a `conversation.item.input_audio_transcription.completed`
/// event) when one is given.
async fn upstream(usage: Value, transcription: Option<Value>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let usage = usage.clone();
            let transcription = transcription.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(message)) = socket.next().await {
                    let Message::Text(text) = message else {
                        if message.is_close() {
                            break;
                        }
                        continue;
                    };
                    let frames = if text.contains("\"response.create\"") {
                        vec![
                            json!({"type": "response.created", "event_id": "ev_1",
                                   "response": {"id": "resp_1", "status": "in_progress"}}),
                            json!({"type": "response.done", "event_id": "ev_2",
                                   "response": {"id": "resp_1", "status": "completed",
                                                "output": [], "usage": usage}}),
                        ]
                    } else if text.contains("\"input_audio_buffer.commit\"") {
                        transcription.iter().cloned().collect()
                    } else {
                        continue;
                    };
                    for frame in frames {
                        if socket
                            .send(Message::Text(frame.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    });
    addr
}

/// A route over `upstream` whose `gpt-realtime` price is `price`, plus the
/// prices of any `extra` models. Turns flush as they finish.
fn config(
    upstream: SocketAddr,
    clickhouse: SocketAddr,
    org: &str,
    price: Value,
    extra: Vec<Value>,
) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
    config.logging.flush_ms = 20;
    config.logging.batch_max = 1;
    config.realtime.usage_flush_secs = 0;
    config.providers.push(ProviderConfig {
        name: "up".into(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(
        serde_json::from_value(json!({
            "model": MODEL,
            "strategy": "round_robin",
            "targets": [{"provider": "up", "model": "gpt-realtime-upstream", "weight": 1}]
        }))
        .unwrap(),
    );
    config.db_virtual_keys.push(
        serde_json::from_value(json!({
            "key_hash": rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
            "id": format!("key-{org}"),
            "org_id": org,
        }))
        .unwrap(),
    );
    let mut price = price;
    price["model"] = json!(MODEL);
    for price in std::iter::once(price).chain(extra) {
        config
            .model_prices
            .push(serde_json::from_value(price).unwrap());
    }
    config
}

async fn open(config: &GatewayConfig, redis: Option<&str>) -> Client {
    let state = rolter_gateway::AppState::with_logging(config, redis);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{gw}/v1/realtime?model={MODEL}"
        ))
        .unwrap();
    request.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {KEY}").parse().unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

async fn next_type(client: &mut Client) -> String {
    let message = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("a frame within 5s")
        .expect("the socket is open")
        .expect("a well-formed frame");
    match message {
        Message::Text(text) => serde_json::from_str::<Value>(text.as_str()).unwrap()["type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        other => panic!("expected a text event, got {other:?}"),
    }
}

async fn send(client: &mut Client, event: Value) {
    client
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
}

async fn run_turn(client: &mut Client) {
    send(client, json!({"type": "response.create"})).await;
    assert_eq!(next_type(client).await, "response.created");
    assert_eq!(next_type(client).await, "response.done");
}

fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

// ── #1882: audio and text tokens priced apart ────────────────────────────────

/// 100 input tokens (90 audio, 20 of the prompt served from the cache, 15 of
/// those audio) and 50 output tokens (45 audio).
fn split_usage() -> Value {
    json!({
        "total_tokens": 150, "input_tokens": 100, "output_tokens": 50,
        "input_token_details": {
            "cached_tokens": 20, "text_tokens": 10, "audio_tokens": 90,
            "cached_tokens_details": {"text_tokens": 5, "audio_tokens": 15}
        },
        "output_token_details": {"text_tokens": 5, "audio_tokens": 45}
    })
}

/// A dollar per token as the text input rate and two for output, so the
/// expected figure can be added up by hand.
fn text_rates() -> Value {
    json!({"input_per_mtok": 1_000_000, "output_per_mtok": 2_000_000})
}

/// Each modality is charged at its own rate:
/// 5 fresh text x 1 + 5 cached text x 1 (no cached rate, so the input rate)
/// + 75 fresh audio x 10 + 15 cached audio x 5 + 5 text out x 2 + 45 audio out x 20.
#[tokio::test]
async fn audio_tokens_are_priced_at_the_audio_rates() {
    let org = unique("org-audio-rates");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut price = text_rates();
    price["audio_input_per_mtok"] = json!(10_000_000);
    price["audio_output_per_mtok"] = json!(20_000_000);
    price["cached_audio_input_per_mtok"] = json!(5_000_000);
    let upstream = upstream(split_usage(), None).await;
    let mut client = open(&config(upstream, clickhouse, &org, price, vec![]), None).await;

    run_turn(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    assert_eq!(row["cost_usd"], 1745.0, "{row}");
    assert_eq!(row["prompt_tokens"], 100);
    assert_eq!(row["completion_tokens"], 50);
    assert_eq!(row["unpriced"], 0);
}

/// A row that sets no audio rate prices the same response as it always has:
/// every token at the text rate.
#[tokio::test]
async fn a_row_without_audio_rates_prices_audio_as_text() {
    let org = unique("org-no-audio-rates");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let upstream = upstream(split_usage(), None).await;
    let mut client = open(
        &config(upstream, clickhouse, &org, text_rates(), vec![]),
        None,
    )
    .await;

    run_turn(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    // 100 x 1 + 50 x 2
    assert_eq!(row["cost_usd"], 200.0, "{row}");
}

/// A response that names no split is priced at the text rates even when the
/// row has audio ones: there is nothing to apply them to.
#[tokio::test]
async fn a_response_without_a_split_ignores_the_audio_rates() {
    let org = unique("org-no-split");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut price = text_rates();
    price["audio_input_per_mtok"] = json!(10_000_000);
    price["audio_output_per_mtok"] = json!(20_000_000);
    let usage = json!({"total_tokens": 150, "input_tokens": 100, "output_tokens": 50});
    let upstream = upstream(usage, None).await;
    let mut client = open(&config(upstream, clickhouse, &org, price, vec![]), None).await;

    run_turn(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    assert_eq!(row["cost_usd"], 200.0, "{row}");
}

/// The budget counter takes the figure the row carries, audio rates included.
#[tokio::test]
async fn the_budget_is_charged_the_audio_priced_cost() {
    let Some(redis_url) = std::env::var("ROLTER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("skipping: ROLTER_TEST_REDIS_URL not set");
        return;
    };
    use redis::AsyncCommands;
    let org = unique("org-audio-budget");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let mut price = text_rates();
    price["audio_input_per_mtok"] = json!(10_000_000);
    price["audio_output_per_mtok"] = json!(20_000_000);
    price["cached_audio_input_per_mtok"] = json!(5_000_000);
    let upstream = upstream(split_usage(), None).await;
    let mut config = config(upstream, clickhouse, &org, price, vec![]);
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": 100_000, "period": "monthly"
        }))
        .unwrap(),
    );
    let mut client = open(&config, Some(&redis_url)).await;

    run_turn(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();
    assert_eq!(row["cost_usd"], 1745.0, "{row}");

    let key = format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    );
    let mut conn = redis::Client::open(redis_url.as_str())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let mut spent = None;
    for _ in 0..100 {
        let value: Option<String> = conn.get(&key).await.unwrap();
        spent = value.and_then(|value| value.parse::<f64>().ok());
        if spent == Some(1745.0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(spent, Some(1745.0));
}

// ── #1883: input-audio transcription ─────────────────────────────────────────

fn session_update(model: &str) -> Value {
    json!({"type": "session.update",
           "session": {"audio": {"input": {"transcription": {"model": model}}}}})
}

fn tokens_completed() -> Value {
    json!({
        "type": "conversation.item.input_audio_transcription.completed",
        "event_id": "ev_t", "item_id": "item_1", "content_index": 0,
        "transcript": "hello",
        "usage": {
            "type": "tokens", "total_tokens": 22, "input_tokens": 13, "output_tokens": 9,
            "input_token_details": {"text_tokens": 3, "audio_tokens": 10}
        }
    })
}

fn duration_completed(seconds: f64) -> Value {
    json!({
        "type": "conversation.item.input_audio_transcription.completed",
        "event_id": "ev_t", "item_id": "item_1", "content_index": 0,
        "transcript": "hello",
        "usage": {"type": "duration", "seconds": seconds}
    })
}

async fn commit_and_read(client: &mut Client) {
    send(client, json!({"type": "input_audio_buffer.commit"})).await;
    assert_eq!(
        next_type(client).await,
        "conversation.item.input_audio_transcription.completed"
    );
}

/// The usage lands in a row of its own, priced against the transcription model
/// the session named: 3 text x 2 + 10 audio x 6 + 9 out x 10.
#[tokio::test]
async fn token_billed_transcription_is_its_own_priced_row() {
    let org = unique("org-transcribe-tokens");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let transcribe = json!({"model": "gpt-4o-transcribe", "input_per_mtok": 2_000_000,
        "output_per_mtok": 10_000_000, "audio_input_per_mtok": 6_000_000});
    let upstream = upstream(split_usage(), Some(tokens_completed())).await;
    let mut client = open(
        &config(upstream, clickhouse, &org, text_rates(), vec![transcribe]),
        None,
    )
    .await;

    send(&mut client, session_update("gpt-4o-transcribe")).await;
    commit_and_read(&mut client).await;
    run_turn(&mut client).await;
    let rows = rows.wait_for(2).await;
    client.close(None).await.unwrap();

    let transcription = rows
        .iter()
        .find(|row| row["model"] == "gpt-4o-transcribe")
        .unwrap_or_else(|| panic!("no transcription row in {rows:?}"));
    assert_eq!(transcription["cost_usd"], 156.0, "{transcription}");
    assert_eq!(transcription["prompt_tokens"], 13);
    assert_eq!(transcription["completion_tokens"], 9);
    assert_eq!(transcription["total_tokens"], 22);
    assert_eq!(transcription["unpriced"], 0);
    assert_eq!(transcription["usage_unknown"], 0);
    assert_eq!(transcription["provider"], "up");
    let response = rows.iter().find(|row| row["model"] == MODEL).unwrap();
    assert_eq!(
        response["cost_usd"], 200.0,
        "the response keeps its own row"
    );
    assert_ne!(transcription["request_id"], response["request_id"]);
}

/// A duration-billed model (`whisper-1`) is priced per minute: 30 s at $0.006.
#[tokio::test]
async fn duration_billed_transcription_is_priced_per_minute() {
    let org = unique("org-transcribe-seconds");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let whisper = json!({"model": "whisper-1", "input_per_mtok": 0, "output_per_mtok": 0,
        "audio_input_per_minute": 0.006});
    let upstream = upstream(split_usage(), Some(duration_completed(30.0))).await;
    let mut client = open(
        &config(upstream, clickhouse, &org, text_rates(), vec![whisper]),
        None,
    )
    .await;

    send(&mut client, session_update("whisper-1")).await;
    commit_and_read(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    assert_eq!(row["model"], "whisper-1", "{row}");
    assert_eq!(row["cost_usd"], 0.003, "{row}");
    assert_eq!(row["total_tokens"], 0);
    assert_eq!(row["unpriced"], 0);
    assert_eq!(row["usage_unknown"], 0);
}

/// A transcription model with no price is recorded unpriced, never as free
/// and priced; so is a session whose `session.update` never named one.
#[tokio::test]
async fn a_transcription_without_a_price_is_unpriced() {
    let org = unique("org-transcribe-unpriced");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let upstream = upstream(split_usage(), Some(tokens_completed())).await;
    let mut client = open(
        &config(upstream, clickhouse, &org, text_rates(), vec![]),
        None,
    )
    .await;

    send(&mut client, session_update("gpt-4o-mini-transcribe")).await;
    commit_and_read(&mut client).await;
    // a later update switches transcription off, so the next one names nothing
    send(
        &mut client,
        json!({"type": "session.update",
               "session": {"audio": {"input": {"transcription": null}}}}),
    )
    .await;
    commit_and_read(&mut client).await;
    let rows = rows.wait_for(2).await;
    client.close(None).await.unwrap();

    let named = rows
        .iter()
        .find(|row| row["model"] == "gpt-4o-mini-transcribe")
        .unwrap_or_else(|| panic!("no row for the named model in {rows:?}"));
    assert_eq!(named["unpriced"], 1, "{named}");
    assert_eq!(named["cost_usd"], 0.0);
    assert_eq!(named["total_tokens"], 22, "the tokens are still counted");
    let unnamed = rows.iter().find(|row| row["model"] == MODEL).unwrap();
    assert_eq!(unnamed["unpriced"], 1, "{unnamed}");
    assert_eq!(unnamed["cost_usd"], 0.0);
}

/// A duration-billed model whose row sets no per-minute rate is unpriced too.
#[tokio::test]
async fn a_duration_without_a_minute_rate_is_unpriced() {
    let org = unique("org-transcribe-norate");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let whisper = json!({"model": "whisper-1", "input_per_mtok": 1, "output_per_mtok": 1});
    let upstream = upstream(split_usage(), Some(duration_completed(30.0))).await;
    let mut client = open(
        &config(upstream, clickhouse, &org, text_rates(), vec![whisper]),
        None,
    )
    .await;

    send(&mut client, session_update("whisper-1")).await;
    commit_and_read(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    assert_eq!(row["unpriced"], 1, "{row}");
    assert_eq!(row["cost_usd"], 0.0);
}

/// An event that carries no usage is a spend the gateway cannot count.
#[tokio::test]
async fn a_transcription_without_usage_is_unknown_not_free() {
    let org = unique("org-transcribe-nousage");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let whisper = json!({"model": "whisper-1", "input_per_mtok": 0, "output_per_mtok": 0,
        "audio_input_per_minute": 0.006});
    let event = json!({"type": "conversation.item.input_audio_transcription.completed",
                       "item_id": "i", "content_index": 0, "transcript": "hi"});
    let upstream = upstream(split_usage(), Some(event)).await;
    let mut client = open(
        &config(upstream, clickhouse, &org, text_rates(), vec![whisper]),
        None,
    )
    .await;

    send(&mut client, session_update("whisper-1")).await;
    commit_and_read(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    assert_eq!(row["usage_unknown"], 1, "{row}");
    assert_eq!(row["cost_usd"], 0.0);
}
