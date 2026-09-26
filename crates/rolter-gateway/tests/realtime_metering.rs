//! Realtime session admission and metering (#1396).
//!
//! A `/v1/realtime` session used to be admitted against the process-local
//! `[realtime]` caps and nothing else, so a tenant could spend without limit
//! and without a trace. These tests drive real WebSocket sessions through the
//! gateway against a mock Realtime upstream and assert the session meets the
//! HTTP path's budgets, rate limits and request log.
//!
//! Request-log rows go to an in-process stand-in for the ClickHouse HTTP
//! interface, so those tests always run. Budget and rate-limit counters live in
//! Redis; the tests that need them read `ROLTER_TEST_REDIS_URL` and skip when it
//! is unset, the same contract the other Redis suites keep.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use rolter_core::{GatewayConfig, ProviderConfig, ProviderKind};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;

const KEY: &str = "sk-realtime-metering";
const OTHER_KEY: &str = "sk-realtime-metering-other";
const MODEL: &str = "gpt-realtime";

/// What every mock turn reports: 150 tokens, which at the test price of one
/// dollar per token is a $150 turn.
const USAGE: &str = r#"{"total_tokens":150,"input_tokens":100,"output_tokens":50,"input_token_details":{"cached_tokens":0,"text_tokens":10,"audio_tokens":90},"output_token_details":{"text_tokens":5,"audio_tokens":45}}"#;

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A stand-in for the ClickHouse HTTP interface keeping every `request_logs`
/// row.
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

    /// Wait until `count` rows have arrived, then return them.
    async fn wait_for(&self, count: usize) -> Vec<Value> {
        for _ in 0..200 {
            if self.0.lock().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // one more flush interval, so a row that should not exist has had its
        // chance to arrive and fail the count below
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows = self.0.lock().clone();
        assert_eq!(rows.len(), count, "expected {count} rows, saw {rows:?}");
        rows
    }
}

/// A Realtime upstream answering each `response.create` with
/// `response.created`, one audio delta and, when `finish` is set,
/// `response.done` carrying [`USAGE`]. Counts the sessions it accepted.
async fn realtime_upstream(finish: bool) -> (SocketAddr, Arc<AtomicU32>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sessions = Arc::new(AtomicU32::new(0));
    let accepted = sessions.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let accepted = accepted.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                accepted.fetch_add(1, Ordering::SeqCst);
                let mut turn = 0;
                while let Some(Ok(message)) = socket.next().await {
                    let Message::Text(text) = message else {
                        if message.is_close() {
                            break;
                        }
                        continue;
                    };
                    if !text.contains("\"response.create\"") {
                        continue;
                    }
                    turn += 1;
                    let id = format!("resp_{turn}");
                    let mut frames = vec![
                        json!({"type": "response.created", "event_id": "ev_1",
                               "response": {"id": id, "status": "in_progress"}})
                        .to_string(),
                        format!(
                            r#"{{"type":"response.output_audio.delta","event_id":"ev_2","response_id":"{id}","delta":"UklGRiQAAABXQVZF"}}"#
                        ),
                    ];
                    if finish {
                        frames.push(format!(
                            r#"{{"type":"response.done","event_id":"ev_3","response":{{"id":"{id}","status":"completed","output":[],"usage":{USAGE}}}}}"#
                        ));
                    }
                    for frame in frames {
                        if socket.send(Message::Text(frame.into())).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    (addr, sessions)
}

/// One route over `upstream`, priced at a dollar per token, with two virtual
/// keys in `org` and rows going to `clickhouse` when given. Turns are flushed
/// as they finish, so tests need not wait out a timer.
fn config(upstream: SocketAddr, clickhouse: Option<SocketAddr>, org: &str) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    if let Some(clickhouse) = clickhouse {
        config.logging.clickhouse_url = Some(format!("http://{clickhouse}"));
        config.logging.flush_ms = 20;
        config.logging.batch_max = 1;
    }
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
    for (key, id) in [
        (KEY, format!("key-{org}")),
        (OTHER_KEY, format!("other-{org}")),
    ] {
        config.db_virtual_keys.push(
            serde_json::from_value(json!({
                "key_hash": rolter_auth::hash_key(&config.server.resolve_key_pepper(), key),
                "id": id,
                "org_id": org,
            }))
            .unwrap(),
        );
    }
    config.model_prices.push(
        serde_json::from_value(json!({
            "model": MODEL,
            "input_per_mtok": 1_000_000,
            "output_per_mtok": 1_000_000
        }))
        .unwrap(),
    );
    config
}

fn with_org_budget(mut config: GatewayConfig, org: &str, limit_usd: u32) -> GatewayConfig {
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": limit_usd, "period": "monthly"
        }))
        .unwrap(),
    );
    config
}

async fn gateway(config: &GatewayConfig, redis: Option<&str>) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, redis);
    serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await
}

fn request(
    gw: SocketAddr,
    key: &str,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "ws://{gw}/v1/realtime?model={MODEL}"
        ))
        .unwrap();
    request.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {key}").parse().unwrap(),
    );
    request
}

/// A refused upgrade: its status, JSON body and `Retry-After`.
type Refusal = (u16, Value, Option<String>);

async fn try_open(gw: SocketAddr, key: &str) -> Result<Client, Refusal> {
    match tokio_tungstenite::connect_async(request(gw, key)).await {
        Ok((client, _)) => Ok(client),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err((
            response.status().as_u16(),
            response
                .body()
                .as_deref()
                .and_then(|body| serde_json::from_slice(body).ok())
                .unwrap_or(Value::Null),
            response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        )),
        Err(other) => panic!("expected an admission or an HTTP refusal, got {other}"),
    }
}

async fn open(gw: SocketAddr, key: &str) -> Client {
    match try_open(gw, key).await {
        Ok(client) => client,
        Err(refusal) => panic!("the session was refused: {refusal:?}"),
    }
}

async fn refused(gw: SocketAddr, key: &str) -> Refusal {
    match try_open(gw, key).await {
        Ok(_) => panic!("expected the upgrade to be refused"),
        Err(refusal) => refusal,
    }
}

/// The next frame, failing the test rather than hanging it.
async fn next(client: &mut Client) -> Message {
    tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("a frame within 5s")
        .expect("the socket is open")
        .expect("a well-formed frame")
}

/// The `type` of the next text frame.
async fn next_event(client: &mut Client) -> Value {
    match next(client).await {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text event, got {other:?}"),
    }
}

/// Ask for one response and read it to its `response.done`.
async fn run_turn(client: &mut Client) {
    client
        .send(Message::Text(r#"{"type":"response.create"}"#.into()))
        .await
        .unwrap();
    for expected in [
        "response.created",
        "response.output_audio.delta",
        "response.done",
    ] {
        assert_eq!(next_event(client).await["type"], expected);
    }
}

fn redis_url() -> Option<String> {
    let url = std::env::var("ROLTER_TEST_REDIS_URL").ok()?;
    (!url.is_empty()).then_some(url)
}

/// A per-run suffix, so tests sharing one redis never read each other's
/// counters.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

fn spend_key(org: &str) -> String {
    format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    )
}

async fn redis(url: &str) -> redis::aio::MultiplexedConnection {
    redis::Client::open(url)
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap()
}

/// The org's spend counter once it reaches `expected`, or whatever it holds
/// after five seconds.
async fn org_spend(url: &str, org: &str, expected: f64) -> Option<f64> {
    use redis::AsyncCommands;
    let mut conn = redis(url).await;
    let mut spent = None;
    for _ in 0..100 {
        let value: Option<String> = conn.get(spend_key(org)).await.unwrap();
        spent = value.and_then(|value| value.parse().ok());
        if spent == Some(expected) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    spent
}

// ── the request log: always runs ─────────────────────────────────────────────

/// A finished turn reaches the request log with the upstream's usage, priced
/// and attributed like an HTTP request, so it lands in cost attribution and
/// analytics.
#[tokio::test]
async fn a_completed_turn_is_logged_with_its_usage_and_cost() {
    let org = unique("org-logged");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = realtime_upstream(true).await;
    let gw = gateway(&config(upstream, Some(clickhouse), &org), None).await;

    let mut upgrade = request(gw, KEY);
    upgrade
        .headers_mut()
        .insert("x-request-id", "rt-logged-upgrade".parse().unwrap());
    let (mut client, _) = tokio_tungstenite::connect_async(upgrade).await.unwrap();
    run_turn(&mut client).await;
    let row = &rows.wait_for(1).await[0];
    client.close(None).await.unwrap();

    assert_eq!(row["model"], MODEL, "{row}");
    assert_eq!(row["provider"], "up");
    assert_eq!(row["target"], "gpt-realtime-upstream");
    assert_eq!(row["org_id"], org.as_str());
    assert_eq!(row["virtual_key_id"], format!("key-{org}"));
    assert_eq!(row["status"], 200);
    assert_eq!(row["stream"], 1);
    assert_eq!(row["prompt_tokens"], 100);
    assert_eq!(row["completion_tokens"], 50);
    assert_eq!(row["total_tokens"], 150);
    assert_eq!(row["cost_usd"], 150.0);
    assert_eq!(row["unpriced"], 0);
    assert_eq!(row["usage_unknown"], 0);
    assert_eq!(
        row["request_id"], "rt-logged-upgrade:1",
        "each turn's row carries the upgrade's id plus its ordinal"
    );
}

/// The metering model's point: a session that never closes is accounted as it
/// goes, flush by flush, rather than when it ends.
#[tokio::test]
async fn a_session_that_stays_open_is_accounted_on_the_flush_timer() {
    let org = unique("org-timer");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = realtime_upstream(true).await;
    let mut config = config(upstream, Some(clickhouse), &org);
    config.realtime.usage_flush_secs = 1;
    let gw = gateway(&config, None).await;

    let mut client = open(gw, KEY).await;
    run_turn(&mut client).await;
    run_turn(&mut client).await;
    // still open: both rows arrive on the timer, not on close
    let rows = rows.wait_for(2).await;
    let mut ids: Vec<&str> = rows
        .iter()
        .map(|row| row["request_id"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    assert!(ids[0].ends_with(":1") && ids[1].ends_with(":2"), "{ids:?}");
    assert!(ids[0].strip_suffix(":1") == ids[1].strip_suffix(":2"));
    client.close(None).await.unwrap();
}

/// A response the client walked away from never reports usage, but the
/// upstream still generated it. It is logged as unknown rather than dropped.
#[tokio::test]
async fn a_response_cut_short_is_logged_as_unknown_usage() {
    let org = unique("org-cut");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = realtime_upstream(false).await;
    let gw = gateway(&config(upstream, Some(clickhouse), &org), None).await;

    let mut client = open(gw, KEY).await;
    client
        .send(Message::Text(r#"{"type":"response.create"}"#.into()))
        .await
        .unwrap();
    assert_eq!(next_event(&mut client).await["type"], "response.created");
    assert_eq!(
        next_event(&mut client).await["type"],
        "response.output_audio.delta"
    );
    client.close(None).await.unwrap();

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["status"], 499, "{row}");
    assert_eq!(row["error"], "client disconnected");
    assert_eq!(row["usage_unknown"], 1);
    assert_eq!(row["total_tokens"], 0);
    assert_eq!(row["org_id"], org.as_str());
}

/// `unpriced_policy = "block"` refuses a realtime session the way it refuses a
/// chat request: before any upstream is dialled.
#[tokio::test]
async fn an_unpriced_model_is_refused_under_a_block_policy() {
    let org = unique("org-unpriced");
    let (upstream, sessions) = realtime_upstream(true).await;
    let mut config = config(upstream, None, &org);
    config.model_prices.clear();
    config.unpriced_policy = rolter_core::UnpricedPolicy::Block;
    let gw = gateway(&config, None).await;

    let (status, body, _) = refused(gw, KEY).await;
    assert_eq!(status, 402);
    assert_eq!(body["error"]["code"], "model_unpriced");
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        0,
        "no upstream was dialled"
    );
}

// ── budgets and rate limits: need redis ──────────────────────────────────────

/// A budget already at its limit refuses a new session with the HTTP path's
/// 402, and no upstream session is opened for it.
#[tokio::test]
async fn a_spent_budget_refuses_a_new_session() {
    let Some(url) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-spent");
    let (upstream, sessions) = realtime_upstream(true).await;
    let gw = gateway(
        &with_org_budget(config(upstream, None, &org), &org, 100),
        Some(&url),
    )
    .await;
    {
        use redis::AsyncCommands;
        let _: () = redis(&url).await.set(spend_key(&org), "100").await.unwrap();
    }

    let (status, body, _) = refused(gw, KEY).await;
    assert_eq!(status, 402);
    assert_eq!(body["error"]["code"], "insufficient_quota");
    assert_eq!(body["error"]["type"], "insufficient_quota");
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        0,
        "no upstream was dialled"
    );
}

/// Usage reaches the budget counter, and when a turn spends the last of the
/// budget the session is closed with an `error` event and a policy-violation
/// close frame rather than left to keep spending.
#[tokio::test]
async fn a_session_is_closed_when_its_budget_runs_out() {
    let Some(url) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-closed");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = realtime_upstream(true).await;
    // the first turn costs $150: it is served, and it spends the budget
    let gw = gateway(
        &with_org_budget(config(upstream, Some(clickhouse), &org), &org, 100),
        Some(&url),
    )
    .await;

    let mut client = open(gw, KEY).await;
    run_turn(&mut client).await;
    let error = next_event(&mut client).await;
    assert_eq!(error["type"], "error", "{error}");
    assert_eq!(error["error"]["code"], "insufficient_quota");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("budget exceeded"),
        "{error}"
    );
    assert!(error["event_id"].as_str().is_some());
    match next(&mut client).await {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Policy),
        other => panic!("expected a policy close, got {other:?}"),
    }

    assert_eq!(org_spend(&url, &org, 150.0).await, Some(150.0));
    assert_eq!(rows.wait_for(1).await[0]["cost_usd"], 150.0);
    // and the next session is refused at the door
    assert_eq!(refused(gw, KEY).await.0, 402);
}

/// A budget is shared by the whole scope chain, so a session is closed on the
/// flush timer when something else spends it, even if the session itself has
/// not finished a turn since.
#[tokio::test]
async fn a_budget_spent_elsewhere_closes_an_idle_session_on_the_timer() {
    let Some(url) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-elsewhere");
    let (upstream, _) = realtime_upstream(true).await;
    let mut config = with_org_budget(config(upstream, None, &org), &org, 100);
    config.realtime.usage_flush_secs = 1;
    let gw = gateway(&config, Some(&url)).await;

    let mut client = open(gw, KEY).await;
    // other traffic in the same org spends the budget while the session is
    // open and quiet
    {
        use redis::AsyncCommands;
        let _: () = redis(&url).await.set(spend_key(&org), "100").await.unwrap();
    }
    let error = next_event(&mut client).await;
    assert_eq!(error["type"], "error", "{error}");
    assert_eq!(error["error"]["code"], "insufficient_quota");
    match next(&mut client).await {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Policy),
        other => panic!("expected a policy close, got {other:?}"),
    }
}

/// A session with room in its budget is charged and left open.
#[tokio::test]
async fn usage_is_charged_to_the_budget_while_the_session_continues() {
    let Some(url) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-charged");
    let (upstream, _) = realtime_upstream(true).await;
    let gw = gateway(
        &with_org_budget(config(upstream, None, &org), &org, 1_000),
        Some(&url),
    )
    .await;

    let mut client = open(gw, KEY).await;
    run_turn(&mut client).await;
    assert_eq!(org_spend(&url, &org, 150.0).await, Some(150.0));
    run_turn(&mut client).await;
    assert_eq!(org_spend(&url, &org, 300.0).await, Some(300.0));
    client.close(None).await.unwrap();
}

/// `rpm` is enforced per key, not per gateway process: a key with one slot
/// gets one session while the process cap has room for a thousand, and a
/// sibling key in the same org is unaffected.
#[tokio::test]
async fn rate_limits_apply_per_key_rather_than_per_process() {
    let Some(url) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-rpm");
    let (upstream, sessions) = realtime_upstream(true).await;
    let mut config = config(upstream, None, &org);
    assert!(config.realtime.max_connections >= 1_000);
    config.rate_limits.push(
        serde_json::from_value(json!({"scope": "key", "id": format!("key-{org}"), "rpm": 1}))
            .unwrap(),
    );
    let gw = gateway(&config, Some(&url)).await;

    let mut first = open(gw, KEY).await;
    let (status, body, retry_after) = refused(gw, KEY).await;
    assert_eq!(status, 429);
    assert_eq!(body["error"]["code"], "rate_limit_exceeded");
    assert!(retry_after.is_some(), "a 429 says when to come back");
    let mut sibling = open(gw, OTHER_KEY).await;
    assert_eq!(sessions.load(Ordering::SeqCst), 2);

    first.close(None).await.unwrap();
    sibling.close(None).await.unwrap();
}

/// A session's turns count against `tpm` as they are metered, so the tokens a
/// key spends over a socket hold back its next session like any other usage.
#[tokio::test]
async fn turn_tokens_fill_the_key_tpm_window() {
    let Some(url) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-tpm");
    let (upstream, _) = realtime_upstream(true).await;
    let mut config = config(upstream, None, &org);
    config.rate_limits.push(
        serde_json::from_value(json!({"scope": "key", "id": format!("key-{org}"), "tpm": 100}))
            .unwrap(),
    );
    let gw = gateway(&config, Some(&url)).await;

    let mut client = open(gw, KEY).await;
    run_turn(&mut client).await;
    client.close(None).await.unwrap();

    // the tokens land asynchronously; the refusal follows once they have
    let mut refusal = None;
    for _ in 0..100 {
        match try_open(gw, KEY).await {
            Ok(mut admitted) => {
                let _ = admitted.close(None).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(refused) => {
                refusal = Some(refused);
                break;
            }
        }
    }
    let (status, body, _) = refusal.expect("150 tokens against a 100 tpm cap refuse the key");
    assert_eq!(status, 429);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("tpm"),
        "{body}"
    );
}
