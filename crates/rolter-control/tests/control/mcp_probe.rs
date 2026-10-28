//! MCP connection status and try-it (#951): the four failure states are told
//! apart, the last failure outlives a later success, a tool that errors is its
//! own state, and none of it wakes the gateways.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::support::*;

#[derive(Clone)]
struct Mock {
    /// status every request is answered with, when set
    status: Arc<Mutex<Option<u16>>>,
    bearer: Option<&'static str>,
}

async fn handle(State(mock): State<Mock>, headers: HeaderMap, body: String) -> impl IntoResponse {
    if let Some(status) = *mock.status.lock() {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    if let Some(expected) = mock.bearer {
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        if presented != Some(&format!("Bearer {expected}")) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let message: Value = serde_json::from_str(&body).unwrap();
    let result = match message["method"].as_str().unwrap() {
        "initialize" => json!({
            "protocolVersion": "2025-03-26",
            "serverInfo": {"name": "mock", "version": "9.9"},
        }),
        "tools/list" => json!({"tools": [
            {"name": "echo", "description": "d", "inputSchema": {"type": "object"}},
            {"name": "boom", "description": "d", "inputSchema": {"type": "object"}},
        ]}),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap();
            json!({
                "content": [{"type": "text", "text": format!("ran {name}")}],
                "isError": name == "boom",
            })
        }
        _ => return StatusCode::ACCEPTED.into_response(),
    };
    (
        [(header::CONTENT_TYPE, "application/json")],
        json!({"jsonrpc": "2.0", "id": message["id"], "result": result}).to_string(),
    )
        .into_response()
}

async fn mock(bearer: Option<&'static str>) -> (String, Arc<Mutex<Option<u16>>>) {
    let status = Arc::new(Mutex::new(None));
    let app = Router::new().route("/mcp", post(handle)).with_state(Mock {
        status: status.clone(),
        bearer,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/mcp"), status)
}

struct Fixture {
    client: reqwest::Client,
    base: String,
    org_id: String,
    pool: sqlx::PgPool,
}

impl Fixture {
    async fn server(&self, slug: &str, url: &str, tools: &[&str]) -> String {
        let server: Value = self
            .client
            .post(format!(
                "{}/api/v1/orgs/{}/mcp-servers",
                self.base, self.org_id
            ))
            .json(&json!({"name": slug, "slug": slug, "url": url, "tools": tools}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = server["id"].as_str().unwrap().to_string();
        crate::mcp::wait_for_discovery(&self.pool, id.parse().unwrap()).await;
        id
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let response = self
            .client
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or_default())
    }

    async fn row(&self, id: &str) -> Value {
        let listed: Value = self
            .client
            .get(format!(
                "{}/api/v1/orgs/{}/mcp-servers",
                self.base, self.org_id
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap()
            .clone()
    }
}

async fn fixture() -> (Fixture, rolter_store::postgres::test_schema::TestSchema) {
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    (
        Fixture {
            client,
            base,
            org_id,
            pool,
        },
        db,
    )
}

#[tokio::test]
async fn a_probe_tells_the_four_states_apart_and_remembers_the_last_failure() {
    skip_without_db!();
    let (f, _db) = fixture().await;
    let (url, status) = mock(Some("tok")).await;
    let id = f.server("probe", &url, &["echo"]).await;

    // no credential yet: the server answers 401, which is auth_rejected
    let (code, rejected) = f
        .post(&format!("/api/v1/mcp-servers/{id}/test"), json!({}))
        .await;
    assert_eq!(code, 200);
    assert_eq!(rejected["status"], "auth_rejected", "{rejected}");
    assert_eq!(rejected["reachable"], true);
    assert_eq!(rejected["handshake_completed"], false);

    // with the credential: reachable, and the handshake's facts come back
    f.client
        .put(format!("{}/api/v1/mcp-servers/{id}/auth", f.base))
        .json(&json!({"auth_kind": "bearer", "credential": "tok"}))
        .send()
        .await
        .unwrap();
    let (_, ok) = f
        .post(&format!("/api/v1/mcp-servers/{id}/test"), json!({}))
        .await;
    assert_eq!(ok["status"], "reachable", "{ok}");
    assert_eq!(ok["handshake_completed"], true);
    assert_eq!(ok["protocol_version"], "2025-03-26");
    assert_eq!(ok["server_name"], "mock");
    assert_eq!(ok["server_version"], "9.9");

    // a server that answers but is not an MCP server
    *status.lock() = Some(404);
    let (_, broken) = f
        .post(&format!("/api/v1/mcp-servers/{id}/test"), json!({}))
        .await;
    assert_eq!(broken["status"], "handshake_failed", "{broken}");
    assert_eq!(broken["reachable"], true);

    // and one nothing answers for
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let dead = f.server("dead", &format!("http://{closed}/mcp"), &[]).await;
    let (_, gone) = f
        .post(&format!("/api/v1/mcp-servers/{dead}/test"), json!({}))
        .await;
    assert_eq!(gone["status"], "unreachable", "{gone}");
    assert_eq!(gone["reachable"], false);

    // the row remembers the last contact and the last failure, and the failure
    // outlives the success that followed the first one
    *status.lock() = None;
    let row = f.row(&id).await;
    assert_eq!(row["last_protocol_version"], "2025-03-26");
    assert!(!row["last_contact_at"].is_null());
    assert_eq!(row["last_error_kind"], "handshake_failed");
    assert!(!row["last_error_at"].is_null());
    let dead_row = f.row(&dead).await;
    assert_eq!(dead_row["last_error_kind"], "unreachable");
    assert!(dead_row["last_contact_at"].is_null());
}

#[tokio::test]
async fn a_status_probe_does_not_wake_the_gateways() {
    skip_without_db!();
    let (f, _db) = fixture().await;
    let (url, _status) = mock(None).await;
    let id = f.server("quiet", &url, &["echo"]).await;
    // let any background write of the install land first
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let before = config_version(&f.pool).await;
    for _ in 0..3 {
        f.post(&format!("/api/v1/mcp-servers/{id}/test"), json!({}))
            .await;
    }
    assert_eq!(
        config_version(&f.pool).await,
        before,
        "contact bookkeeping is not configuration"
    );
    // an edit of something the gateway reads still bumps
    let patched = f
        .client
        .patch(format!("{}/api/v1/mcp-servers/{id}", f.base))
        .json(&json!({"tools": ["echo", "boom"]}))
        .send()
        .await
        .unwrap();
    assert!(patched.status().is_success());
    assert!(config_version(&f.pool).await > before);
}

#[tokio::test]
async fn try_it_returns_the_raw_exchange_and_tells_a_tool_error_from_success() {
    skip_without_db!();
    let (f, _db) = fixture().await;
    let (url, _status) = mock(None).await;
    let id = f.server("tryit", &url, &["echo", "boom"]).await;
    let path = format!("/api/v1/mcp-servers/{id}/tools/call");

    let (code, ok) = f
        .post(&path, json!({"tool": "echo", "arguments": {"q": "hi"}}))
        .await;
    assert_eq!(code, 200, "{ok}");
    assert_eq!(ok["outcome"], "success");
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["request"]["method"], "tools/call");
    assert_eq!(ok["request"]["params"]["arguments"]["q"], "hi");
    assert_eq!(ok["response"]["result"]["content"][0]["text"], "ran echo");

    // the server answered and the tool said it failed: its own state
    let (_, errored) = f.post(&path, json!({"tool": "boom"})).await;
    assert_eq!(errored["outcome"], "tool_errored", "{errored}");
    assert_eq!(errored["ok"], false);
    assert_eq!(errored["response"]["result"]["isError"], true);

    // only a tool the manifest declares, with object arguments
    let (code, _) = f.post(&path, json!({"tool": "not-declared"})).await;
    assert_eq!(code, 400);
    let (code, _) = f
        .post(&path, json!({"tool": "echo", "arguments": [1]}))
        .await;
    assert_eq!(code, 400);

    // the audit row names the tool and the outcome and not the arguments
    let audited: Vec<(String, Value)> = sqlx::query_as(
        "select action, detail from audit_log where action = 'mcp_server.tool_call' \
         order by at",
    )
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(audited.len(), 2);
    assert_eq!(audited[0].1["tool"], "echo");
    assert_eq!(audited[1].1["outcome"], "tool_errored");
    assert!(!audited[0].1.to_string().contains("hi"));
}

#[tokio::test]
async fn try_it_against_an_unreachable_server_is_a_failure_state_not_an_error() {
    skip_without_db!();
    let (f, _db) = fixture().await;
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let id = f
        .server("offline", &format!("http://{closed}/mcp"), &["echo"])
        .await;
    let (code, result) = f
        .post(
            &format!("/api/v1/mcp-servers/{id}/tools/call"),
            json!({"tool": "echo"}),
        )
        .await;
    assert_eq!(code, 200);
    assert_eq!(result["outcome"], "unreachable");
    assert_eq!(result["request"], Value::Null);
}
