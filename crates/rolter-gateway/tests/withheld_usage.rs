//! Billed-but-withheld accounting (#1478).
//!
//! A post-call policy — an output guardrail or a post-response plugin — decides
//! what the caller receives. It does not decide what the provider charged: by
//! the time it runs, the upstream has already generated and billed the answer.
//! These tests drive the gateway over HTTP against mock upstreams and assert
//! that such a response is metered exactly once, from the provider's own usage,
//! and logged as withheld rather than disappearing.
//!
//! Request-log rows go to an in-process stand-in for the ClickHouse HTTP
//! interface, so those tests always run. Budget counters and the response cache
//! live in Redis; the tests that need them read `ROLTER_TEST_REDIS_URL` and skip
//! when it is unset, the same contract the postgres suites keep.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use parking_lot::Mutex;
use rolter_core::{
    BalancingStrategy, GatewayConfig, GuardAction, ModelRoute, ProviderConfig, ProviderKind,
    Target, VirtualKeyRecord,
};
use serde_json::{json, Value};

const KEY: &str = "sk-withheld-usage";

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A stand-in for the ClickHouse HTTP interface keeping every `JSONEachRow`
/// line, split by table.
#[derive(Clone, Default)]
struct Rows {
    logs: Arc<Mutex<Vec<Value>>>,
    /// raw bodies posted to `request_payloads`, kept verbatim so a test can
    /// assert the rejected content never reached them
    payloads: Arc<Mutex<Vec<String>>>,
}

impl Rows {
    async fn serve(&self) -> SocketAddr {
        async fn ingest(
            State(rows): State<Rows>,
            axum::extract::RawQuery(query): axum::extract::RawQuery,
            body: String,
        ) -> &'static str {
            let query = query.unwrap_or_default();
            if query.contains("request_payloads") {
                rows.payloads.lock().push(body);
            } else if query.contains("request_logs") {
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

    /// Wait until `count` request-log rows have arrived, then return them.
    async fn wait_for(&self, count: usize) -> Vec<Value> {
        for _ in 0..200 {
            if self.logs.lock().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // one more flush interval, so a row that should not exist has had its
        // chance to arrive and fail the count below
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows = self.logs.lock().clone();
        assert_eq!(rows.len(), count, "expected {count} rows, saw {rows:?}");
        rows
    }
}

/// The completion every leaky upstream returns: an address for a guardrail to
/// catch, and one prompt plus one completion token.
fn leaky_completion() -> Value {
    json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "write to ops@corp.com"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })
}

/// A leaky upstream that counts its calls and fails the first `fail_first`
/// with a retryable 503.
async fn leaky_upstream(fail_first: u32) -> (SocketAddr, Arc<AtomicU32>) {
    async fn handler(
        State((calls, fail_first)): State<(Arc<AtomicU32>, u32)>,
    ) -> axum::response::Response {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        if n < fail_first {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": {"message": "overloaded"}})),
            )
                .into_response();
        }
        Json(leaky_completion()).into_response()
    }
    let calls = Arc::new(AtomicU32::new(0));
    let addr = serve(
        Router::new()
            .route("/v1/chat/completions", post(handler))
            .with_state((calls.clone(), fail_first)),
    )
    .await;
    (addr, calls)
}

/// A config over `upstream` with one virtual key in `org`, a price of one
/// dollar per token, a monthly org budget, and request logs going to
/// `clickhouse`.
fn config(upstream: SocketAddr, clickhouse: SocketAddr, org: &str) -> GatewayConfig {
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
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config.db_virtual_keys.push(VirtualKeyRecord {
        access_policy: None,
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), KEY),
        id: format!("key-{org}"),
        org_id: org.into(),
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
    config.model_prices.push(
        serde_json::from_value(json!({
            "model": "test-model",
            "input_per_mtok": 1_000_000,
            "output_per_mtok": 1_000_000
        }))
        .unwrap(),
    );
    config.budgets.push(
        serde_json::from_value(json!({
            "scope": "org", "id": org, "limit_usd": 100, "period": "monthly"
        }))
        .unwrap(),
    );
    config
}

fn with_output_rule(mut config: GatewayConfig, action: GuardAction) -> GatewayConfig {
    config.guardrails = rolter_core::GuardrailsConfig {
        enabled: true,
        max_scan_bytes: None,
        streaming_post_call: Default::default(),
        rules: vec![rolter_core::GuardrailRule {
            name: "email".to_string(),
            builtin: Some(rolter_core::BuiltinRule::Email),
            pattern: None,
            stage: rolter_core::GuardStage::PostCall,
            action,
            replacement: None,
            include_system: false,
        }],
    };
    config
}

fn with_post_response_plugin(
    mut config: GatewayConfig,
    org: &str,
    hook: SocketAddr,
) -> GatewayConfig {
    config.plugins = rolter_core::PluginsConfig {
        instances: vec![rolter_core::PluginInstanceConfig {
            slug: "audit".to_string(),
            org_id: org.to_string(),
            project_id: None,
            stage: rolter_core::PluginStage::PostResponse,
            position: 0,
            failure_mode: rolter_core::FailureMode::FailClosed,
            endpoint: format!("http://{hook}/hook"),
            auth: None,
        }],
    };
    config
}

/// A post-response plugin answering every call with `answer`.
async fn plugin(answer: Value) -> SocketAddr {
    serve(Router::new().route(
        "/hook",
        post(move || {
            let answer = answer.clone();
            async move { Json(answer) }
        }),
    ))
    .await
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

async fn ask(gw: SocketAddr, prompt: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(KEY)
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": prompt}]}))
        .send()
        .await
        .unwrap()
}

async fn metric(gw: SocketAddr, name: &str) -> f64 {
    let body = reqwest::get(format!("http://{gw}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    body.lines()
        .find_map(|line| line.strip_prefix(name)?.trim().parse().ok())
        .unwrap_or_else(|| panic!("no {name} sample in:\n{body}"))
}

fn assert_billed(row: &Value) {
    assert_eq!(row["prompt_tokens"], 1, "{row}");
    assert_eq!(row["completion_tokens"], 1, "{row}");
    assert_eq!(row["total_tokens"], 2, "{row}");
    assert_eq!(row["cost_usd"], 2.0, "{row}");
    assert_eq!(row["unpriced"], 0, "{row}");
    assert_eq!(row["usage_unknown"], 0, "{row}");
}

// ── request-log rows: always run ────────────────────────────────────────────

#[tokio::test]
async fn a_redacted_answer_is_billed_and_delivered() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = leaky_upstream(0).await;
    let gw = gateway(
        &with_output_rule(
            config(upstream, clickhouse, "org-redact"),
            GuardAction::Redact,
        ),
        None,
    )
    .await;

    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 200);
    let _ = response.bytes().await.unwrap();

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["status"], 200);
    assert_eq!(row["withheld"], 0);
    assert_billed(row);
}

#[tokio::test]
async fn a_blocked_answer_is_billed_logged_as_withheld_and_never_captured() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = leaky_upstream(0).await;
    let mut config = with_output_rule(
        config(upstream, clickhouse, "org-block"),
        GuardAction::Block,
    );
    // payload capture on, so a leak of the rejected body would have somewhere
    // to land
    config.logging.payload_capture.enabled = true;
    let gw = gateway(&config, None).await;

    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "guardrail_blocked");

    let row = &rows.wait_for(1).await[0];
    // the caller's outcome and the provider's bill, side by side on one row
    assert_eq!(row["status"], 403);
    assert_eq!(row["withheld"], 1);
    assert_eq!(row["error"], "guardrail_blocked: email");
    assert_billed(row);
    assert!(
        !row.to_string().contains("ops@corp.com"),
        "the rejected content reached the log row: {row}"
    );
    for payload in rows.payloads.lock().iter() {
        assert!(
            !payload.contains("ops@corp.com"),
            "the rejected content reached payload capture: {payload}"
        );
    }
    assert_eq!(metric(gw, "rolter_withheld_responses_total").await, 1.0);
}

#[tokio::test]
async fn a_blocking_plugin_is_billed_and_logged_as_withheld() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = leaky_upstream(0).await;
    let hook = plugin(json!({"action": "block", "reason": "denied by plugin"})).await;
    let gw = gateway(
        &with_post_response_plugin(
            config(upstream, clickhouse, "org-plugin"),
            "org-plugin",
            hook,
        ),
        None,
    )
    .await;

    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "plugin_blocked");

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["status"], 403);
    assert_eq!(row["withheld"], 1);
    assert_eq!(row["error"], "plugin_blocked: denied by plugin");
    assert_billed(row);
}

/// A plugin that rewrites the body — here dropping the usage object and
/// replacing it with a zero — changes what the caller sees, never the bill.
#[tokio::test]
async fn a_plugin_rewriting_usage_does_not_change_the_bill() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = leaky_upstream(0).await;
    let hook = plugin(json!({
        "action": "transform",
        "content": {
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "[redacted]"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}
        }
    }))
    .await;
    let gw = gateway(
        &with_post_response_plugin(
            config(upstream, clickhouse, "org-rewrite"),
            "org-rewrite",
            hook,
        ),
        None,
    )
    .await;

    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["usage"]["total_tokens"], 0,
        "the caller sees the rewrite"
    );

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["withheld"], 0);
    assert_billed(row);
}

/// An upstream that reports no usage leaves the row's zeros marked unknown.
#[tokio::test]
async fn an_answer_without_usage_is_logged_as_unknown_not_free() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let upstream = serve(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(json!({
                "id": "chatcmpl-silent",
                "object": "chat.completion",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "write to ops@corp.com"}}]
            }))
        }),
    ))
    .await;
    let gw = gateway(
        &with_output_rule(
            config(upstream, clickhouse, "org-silent"),
            GuardAction::Block,
        ),
        None,
    )
    .await;

    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 403);
    let _ = response.bytes().await.unwrap();

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["withheld"], 1);
    assert_eq!(row["usage_unknown"], 1);
    assert_eq!(row["total_tokens"], 0);
}

/// An upstream that answers `200` and promises a body it never finishes: the
/// connection closes a few bytes in, the way a crashed replica or a reset
/// proxy leaves a response (#1775).
async fn truncating_upstream() -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Read one whole request, so the answer below never races the gateway
    /// still writing its body — that would fail the send rather than the
    /// body read this upstream exists to fail.
    async fn read_request(socket: &mut tokio::net::TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let Ok(n) = socket.read(&mut chunk).await else {
                return;
            };
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&chunk[..n]);
            let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                return;
            }
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                read_request(&mut socket).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                          content-length: 4096\r\n\r\n{\"id\":\"chatcmpl-cut\",\"choices\":[",
                    )
                    .await;
                let _ = socket.shutdown().await;
            });
        }
    });
    addr
}

/// What a body read that failed after a `200` must leave behind (#1775): one
/// row naming the target, a `502` with the error, and usage marked unknown —
/// the provider may have billed an answer nobody received — plus the failure
/// counted against the target.
async fn assert_body_read_failure_is_logged(gw: SocketAddr, rows: &Rows) {
    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 502);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("body could not be read")),
        "{body}"
    );

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["status"], 502, "{row}");
    assert_eq!(row["provider"], "up", "{row}");
    assert_eq!(row["target"], "test-model", "{row}");
    assert_eq!(row["usage_unknown"], 1, "{row}");
    assert_eq!(row["total_tokens"], 0, "{row}");
    assert!(
        row["error"]
            .as_str()
            .is_some_and(|e| e.contains("body could not be read")),
        "{row}"
    );
    assert_eq!(metric(gw, "rolter_upstream_errors_total").await, 1.0);
    assert_eq!(
        metric(
            gw,
            "rolter_target_requests_total{provider=\"up\",target=\"test-model\",outcome=\"error\"}"
        )
        .await,
        1.0
    );
}

/// A guarded route buffers the answer to inspect it, and a body that never
/// finishes arriving used to leave no row at all.
#[tokio::test]
async fn a_body_that_fails_to_arrive_is_logged_as_a_502_with_unknown_usage() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let upstream = truncating_upstream().await;
    let gw = gateway(
        &with_output_rule(
            config(upstream, clickhouse, "org-truncated"),
            GuardAction::Redact,
        ),
        None,
    )
    .await;
    assert_body_read_failure_is_logged(gw, &rows).await;
}

/// A retried request is billed for the attempt that answered, once — the
/// failed attempt produced nothing to bill and writes no row of its own.
#[tokio::test]
async fn a_retried_then_blocked_answer_is_billed_once() {
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    // `down` always 503s, so every request fails over to `up`
    let (down, down_calls) = leaky_upstream(u32::MAX).await;
    let (upstream, calls) = leaky_upstream(0).await;
    let mut config = with_output_rule(
        config(upstream, clickhouse, "org-retry"),
        GuardAction::Block,
    );
    config.providers.insert(
        0,
        ProviderConfig {
            name: "down".into(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{down}"),
            ..Default::default()
        },
    );
    config.routes[0].targets.insert(
        0,
        Target {
            provider: "down".into(),
            model: Some("test-model".into()),
            weight: 1,
        },
    );
    let gw = gateway(&config, None).await;

    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 403);
    let _ = response.bytes().await.unwrap();
    assert_eq!(down_calls.load(Ordering::SeqCst), 1, "the 503 was tried");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "and retried once");

    let row = &rows.wait_for(1).await[0];
    assert_eq!(row["withheld"], 1);
    assert_eq!(row["provider"], "up");
    assert_billed(row);
}

// ── budget counters and the response cache: need redis ──────────────────────

fn redis_url() -> Option<String> {
    let url = std::env::var("ROLTER_TEST_REDIS_URL").ok()?;
    (!url.is_empty()).then_some(url)
}

/// A per-run suffix, so tests sharing one redis never read each other's
/// counters or cache entries.
fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

/// Read the org's monthly spend counter, waiting for the asynchronous
/// recorder to land it. `None` once `settle` passes with nothing written.
async fn org_spend(redis: &str, org: &str, settle: Duration) -> Option<f64> {
    use redis::AsyncCommands;
    let client = redis::Client::open(redis).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let key = format!(
        "rolter:budget:org:{org}:{}",
        chrono::Utc::now().format("%Y%m")
    );
    let deadline = tokio::time::Instant::now() + settle;
    loop {
        let value: Option<String> = conn.get(&key).await.unwrap();
        if let Some(value) = value {
            return value.parse().ok();
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The audit's regression (#1478): the same billed answer advances the org's
/// spend whether the output rule redacts it or blocks it.
#[tokio::test]
async fn a_blocked_answer_advances_budget_spend_like_a_delivered_one() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let (upstream, _) = leaky_upstream(0).await;
    for (action, status) in [(GuardAction::Redact, 200), (GuardAction::Block, 403)] {
        let org = unique("org-budget");
        let rows = Rows::default();
        let clickhouse = rows.serve().await;
        let gw = gateway(
            &with_output_rule(config(upstream, clickhouse, &org), action),
            Some(&redis),
        )
        .await;
        let response = ask(gw, "ping").await;
        assert_eq!(response.status(), status);
        let _ = response.bytes().await.unwrap();
        assert_eq!(
            org_spend(&redis, &org, Duration::from_secs(5)).await,
            Some(2.0),
            "{action:?}: the provider's charge must reach the budget"
        );
    }
}

#[tokio::test]
async fn a_blocking_plugin_advances_budget_spend() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-plugin-budget");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, _) = leaky_upstream(0).await;
    let hook = plugin(json!({"action": "block", "reason": "denied"})).await;
    let gw = gateway(
        &with_post_response_plugin(config(upstream, clickhouse, &org), &org, hook),
        Some(&redis),
    )
    .await;
    let response = ask(gw, "ping").await;
    assert_eq!(response.status(), 403);
    let _ = response.bytes().await.unwrap();
    assert_eq!(
        org_spend(&redis, &org, Duration::from_secs(5)).await,
        Some(2.0)
    );
}

/// On a cache-enabled route the miss that reached the upstream is billed once
/// and the hit that replays it costs nothing, even when both are withheld.
#[tokio::test]
async fn a_withheld_cache_miss_is_billed_once_and_its_hit_is_free() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let org = unique("org-cache");
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let (upstream, calls) = leaky_upstream(0).await;
    let mut config = with_output_rule(config(upstream, clickhouse, &org), GuardAction::Block);
    config.cache.enabled = true;
    config.cache.namespace = unique("withheld-cache");
    config.routes[0].cache = Some(rolter_core::RouteCache {
        enabled: true,
        ttl_secs: Some(60),
        per_key: false,
        semantic: None,
    });
    let gw = gateway(&config, Some(&redis)).await;

    for _ in 0..2 {
        let response = ask(gw, "ping").await;
        assert_eq!(response.status(), 403);
        let _ = response.bytes().await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the second ask was a hit");

    let logged = rows.wait_for(2).await;
    let miss = logged
        .iter()
        .find(|row| row["cache_hit"] == 0)
        .expect("a miss row");
    let hit = logged
        .iter()
        .find(|row| row["cache_hit"] == 1)
        .expect("a hit row");
    assert_eq!(miss["withheld"], 1);
    assert_billed(miss);
    assert_eq!(hit["withheld"], 1);
    assert_eq!(hit["status"], 403);
    assert_eq!(hit["total_tokens"], 0);
    assert_eq!(hit["cost_usd"], 0.0);

    // a hit adds nothing, so once the miss's spend has landed it stays put
    assert_eq!(
        org_spend(&redis, &org, Duration::from_secs(5)).await,
        Some(2.0)
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        org_spend(&redis, &org, Duration::from_millis(0)).await,
        Some(2.0),
        "the cache hit must not be billed"
    );
}

/// The cache-miss path buffers the answer to store it, and failed the same
/// way: a bare `502` and no row (#1775).
#[tokio::test]
async fn a_cache_miss_whose_body_fails_to_arrive_is_logged() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let rows = Rows::default();
    let clickhouse = rows.serve().await;
    let upstream = truncating_upstream().await;
    let mut config = config(upstream, clickhouse, &unique("org-truncated-cache"));
    config.cache.enabled = true;
    config.cache.namespace = unique("truncated-cache");
    config.routes[0].cache = Some(rolter_core::RouteCache {
        enabled: true,
        ttl_secs: Some(60),
        per_key: false,
        semantic: None,
    });
    let gw = gateway(&config, Some(&redis)).await;
    assert_body_read_failure_is_logged(gw, &rows).await;
}
