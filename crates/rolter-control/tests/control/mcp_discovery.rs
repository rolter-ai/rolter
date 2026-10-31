//! MCP tool discovery (#1283): the manifest comes from the server's own
//! `tools/list`, a tool that disappears is kept and flagged, and a server that
//! cannot be asked leaves a note instead of failing anything.

use std::sync::Arc;
use std::time::Duration;

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
    tools: Arc<Mutex<Vec<String>>>,
    bearer: Option<&'static str>,
}

async fn handle(State(mock): State<Mock>, headers: HeaderMap, body: String) -> impl IntoResponse {
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
        "initialize" => json!({"protocolVersion": "2025-06-18", "serverInfo": {"name": "mock"}}),
        "tools/list" => json!({"tools": mock.tools.lock().iter().map(|name| json!({
            "name": name,
            "description": format!("does {name}"),
            "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}},
        })).collect::<Vec<_>>()}),
        _ => return StatusCode::ACCEPTED.into_response(),
    };
    (
        [(header::CONTENT_TYPE, "application/json")],
        json!({"jsonrpc": "2.0", "id": message["id"], "result": result}).to_string(),
    )
        .into_response()
}

async fn mock(tools: &[&str], bearer: Option<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
    let live = Arc::new(Mutex::new(tools.iter().map(|t| (*t).to_string()).collect()));
    let app = Router::new().route("/mcp", post(handle)).with_state(Mock {
        tools: live.clone(),
        bearer,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/mcp"), live)
}

fn names(server: &Value, field: &str) -> Vec<String> {
    server[field]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// Poll the server row until the background discovery has run.
async fn wait_attempted(client: &reqwest::Client, base: &str, org: &str, slug: &str) -> Value {
    for _ in 0..100 {
        let listed: Value = client
            .get(format!("{base}/api/v1/orgs/{org}/mcp-servers"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(server) = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["slug"] == slug && !s["tools_discovery_attempted_at"].is_null())
        {
            return server.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("discovery never ran for {slug}");
}

#[tokio::test]
async fn an_installed_server_gains_its_manifest_from_tools_list_and_keeps_what_vanishes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
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

    let (url, live) = mock(&["search", "fetch"], None).await;
    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Docs", "slug": "docs", "url": url,
            // the fallback list: `legacy` is not something the server has
            "tools": ["search", "legacy"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // the install answers at once with the fallback and an undiscovered row
    assert_eq!(created["tools_discovery_status"], "pending", "{created}");
    assert!(created["tools_discovered_at"].is_null());

    let server = wait_attempted(&client, &base, &org_id, "docs").await;
    assert_eq!(server["tools_discovery_status"], "discovered", "{server}");
    assert_eq!(names(&server, "tools"), ["search", "fetch", "legacy"]);
    assert_eq!(names(&server, "tools_stale"), ["legacy"]);
    assert!(!server["tools_discovered_at"].is_null());
    assert!(server["tools_discovery_error"].is_null());
    let id = server["id"].as_str().unwrap();

    // on demand, after the server drops `fetch` and gains `index`
    *live.lock() = vec!["search".into(), "index".into()];
    let again: Value = client
        .post(format!("{base}/api/v1/mcp-servers/{id}/discover"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        names(&again, "tools"),
        ["search", "index", "fetch", "legacy"],
        "{again}"
    );
    assert_eq!(names(&again, "tools_stale"), ["fetch", "legacy"]);

    // the schemas come from their own endpoint, with the flag per tool
    let tools: Value = client
        .get(format!("{base}/api/v1/mcp-servers/{id}/tools"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let by_name = |name: &str| {
        tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("index")["description"], "does index");
    assert_eq!(by_name("index")["input_schema"]["type"], "object");
    assert_eq!(by_name("index")["stale"], false);
    assert_eq!(by_name("fetch")["stale"], true);
    assert!(
        by_name("fetch")["input_schema"].is_null()
            || by_name("fetch")["input_schema"]["type"] == "object"
    );
}

#[tokio::test]
async fn a_server_that_cannot_be_reached_is_noted_not_emptied() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
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
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };

    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Down", "slug": "down", "url": format!("http://{closed}/mcp"),
            "tools": ["a", "b"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["tools_discovery_status"], "pending");
    let server = wait_attempted(&client, &base, &org_id, "down").await;
    // never discovered stays "not discovered yet", with the reason beside it
    assert_eq!(server["tools_discovery_status"], "pending", "{server}");
    assert_eq!(server["tools_discovery_error"], "unreachable");
    assert_eq!(names(&server, "tools"), ["a", "b"]);
    assert!(names(&server, "tools_stale").is_empty());
    assert!(server["tools_discovered_at"].is_null());
}

#[tokio::test]
async fn discovery_authenticates_with_the_stored_credential_and_waits_for_an_oauth_session() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
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

    let (url, _live) = mock(&["whoami"], Some("upstream-token")).await;
    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({"name": "Auth", "slug": "auth", "url": url}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    // installed with no credential: the mock refuses, which is auth_rejected
    let rejected = wait_attempted(&client, &base, &org_id, "auth").await;
    assert_eq!(
        rejected["tools_discovery_error"], "auth_rejected",
        "{rejected}"
    );

    // setting the credential triggers discovery with it
    client
        .put(format!("{base}/api/v1/mcp-servers/{id}/auth"))
        .json(&json!({"auth_kind": "bearer", "credential": "upstream-token"}))
        .send()
        .await
        .unwrap();
    let mut discovered = Value::Null;
    for _ in 0..100 {
        let now: Value = client
            .post(format!("{base}/api/v1/mcp-servers/{id}/discover"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if now["tools_discovery_status"] == "discovered" {
            discovered = now;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(names(&discovered, "tools"), ["whoami"], "{discovered}");
    assert!(!discovered.to_string().contains("upstream-token"));

    // a server behind OAuth has no session to ask with
    let oauth: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({"name": "O", "slug": "o", "url": "http://127.0.0.1:9/mcp", "tools": ["x"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let oauth_id = oauth["id"].as_str().unwrap();
    client
        .put(format!("{base}/api/v1/mcp-servers/{oauth_id}/auth"))
        .json(&json!({"auth_kind": "oauth"}))
        .send()
        .await
        .unwrap();
    let waiting: Value = client
        .post(format!("{base}/api/v1/mcp-servers/{oauth_id}/discover"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(waiting["tools_discovery_status"], "pending", "{waiting}");
    assert_eq!(waiting["tools_discovery_error"], "needs_session");
    assert_eq!(names(&waiting, "tools"), ["x"]);
}
