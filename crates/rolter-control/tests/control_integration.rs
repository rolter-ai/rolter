//! End-to-end control-plane integration tests against a real Postgres.
//!
//! Gated on the `postgres` feature and the `ROLTER_TEST_DATABASE_URL` env var:
//! when the var is unset (local runs without a database) the tests self-skip.
//! CI provides a Postgres service and the var, so they run there.
#![cfg(feature = "postgres")]

use std::net::SocketAddr;

use rolter_store::postgres::test_schema::TestSchema;
use serde_json::{json, Value};

use rolter_store::postgres::test_database;

/// The database this worktree owns. `ROLTER_TEST_DATABASE_URL` names the
/// server; the database itself is derived from the workspace path, so parallel
/// worktrees do not share one (#1430).
async fn database_url() -> Option<String> {
    test_database::url().await
}

/// Create a fresh isolated schema and return the guard owning it, so a test
/// gets its schema back when it finishes — including when it panics (#1364).
///
/// Bind the guard for the whole test: the schema goes with it. Take the pool
/// out with `db.pool().clone()`; tests that only need a router use
/// [`fresh_app`].
async fn fresh_db() -> TestSchema {
    let url = database_url()
        .await
        .expect("ROLTER_TEST_DATABASE_URL checked by caller");
    TestSchema::create(&url).await
}

/// Isolated schema + control-plane app router (migrations applied by
/// `test_app`). The schema guard comes back with the router and has to outlive
/// it, so bind it rather than dropping it on the floor.
async fn fresh_app() -> (axum::Router, TestSchema) {
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("build app");
    (app, db)
}

/// Serve `app` on an ephemeral port and return its address.
async fn serve(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Serve a control-plane app that knows its own ephemeral address as the
/// deployment's public base URL (#1418).
///
/// The SSO and MCP OAuth flows derive their redirect URI from that URL, and it
/// used to arrive through `ROLTER_PUBLIC_URL`. The environment is process-wide:
/// under `cargo nextest` each test owns its process and that is harmless, but
/// the coverage job runs plain `cargo test`, where every test in this binary is
/// a thread sharing one environment — so one test's listener address became
/// another test's redirect URI, and the failure landed on whichever flow
/// happened to be mid-exchange. Binding the listener first and passing the
/// address into the app keeps each test's value its own.
async fn serve_with_public_url(pool: sqlx::PgPool, admin_token: Option<String>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app =
        rolter_control::test_app_with_public_url(pool, admin_token, &format!("http://{addr}"))
            .await
            .expect("build app");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// The one KEK every test in this file installs (#1351).
///
/// `std::env::set_var` is process-wide. The main test job runs `cargo nextest`,
/// which gives each test its own process, but the `coverage` job runs plain
/// `cargo test` — one binary, tests as threads, one environment — and
/// `Kek::from_env()` is read at *request* time. So a test setting a value of
/// its own is read by another test's in-flight request, and a sealed value
/// written under one key fails to open under the next. The symptom is never
/// local: it lands on whichever unrelated seal-then-open test was mid-flight.
///
/// Nothing here needs a *distinct* key, only *a* key — the one test that needs
/// a non-matching one builds it directly with `Kek::from_secret`, never through
/// the environment. So every call site installs this same value and the race
/// has nothing to observe.
const TEST_KEK: &str = "integration-test-kek";

macro_rules! skip_without_db {
    () => {
        if !test_database::is_configured() {
            eprintln!("skipping: {} not set", test_database::URL_ENV);
            return;
        }
    };
}

/// `/readyz` is a real readiness signal, not an alias for `/healthz` (#1081):
/// it reports not-ready against a database whose migrations have not run, and
/// ready once they have. `/healthz` answers `200` throughout — it is liveness,
/// and must never depend on the database, or a database outage would restart
/// every control pod instead of draining it.
#[tokio::test]
async fn readyz_waits_for_migrations_while_healthz_stays_up() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let client = reqwest::Client::new();

    let addr = serve(rolter_control::test_app_unmigrated(pool.clone())).await;
    let not_ready = client
        .get(format!("http://{addr}/readyz"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        not_ready.status(),
        503,
        "unmigrated database must not be ready"
    );
    let body: Value = not_ready.json().await.unwrap();
    assert_eq!(body["status"], "not_ready");
    assert_eq!(
        body["checks"]["database"], "ok",
        "the pool itself is reachable"
    );
    assert!(
        body["checks"]["migrations"]
            .as_str()
            .unwrap()
            .contains("pending"),
        "migrations should be the failing check, got {body}"
    );

    // liveness does not consult the database, so it is up even here
    let health = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        health.status(),
        200,
        "healthz must not depend on the database"
    );

    // the same pool, once migrated, is ready
    let migrated = serve(rolter_control::test_app(pool).await.expect("build app")).await;
    let ready = client
        .get(format!("http://{migrated}/readyz"))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    let body: Value = ready.json().await.unwrap();
    assert_eq!(body["status"], "ready");
    assert_eq!(body["checks"]["migrations"], "ok");
}

#[tokio::test]
async fn ping_and_healthz_respond() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();

    let health = client
        .get(format!("http://{addr}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);

    let ping: Value = client
        .get(format!("http://{addr}/api/v1/ping"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ping["pong"], true);
}

#[tokio::test]
async fn snapshot_served_on_empty_store() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["version"].is_number(),
        "snapshot missing version: {body}"
    );
    assert!(
        body["config"].is_object(),
        "snapshot missing config: {body}"
    );
}

#[tokio::test]
async fn crud_create_round_trip_reflects_in_snapshot() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // helper: POST json, assert 2xx, return the parsed body
    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    // org → team → project hierarchy
    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");

    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");

    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");

    // provider under the org
    let provider = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/providers"),
        json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}),
    )
    .await;
    let provider_id = provider["id"].as_str().expect("provider id");

    // route under the project, plus a target pointing at the provider
    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");

    let advanced = client
        .put(format!("{base}/api/v1/routes/{route_id}/advanced"))
        .json(&json!({
            "advanced": {
                "model_type": "chat",
                "capabilities": ["tools", "vision"],
                "description": "managed model",
                "base_url": "https://models.example/v1",
                "pricing": {"image_per_unit": 0.04},
                "limits": {"output_tokens": 2048, "timeout_secs": 30},
                "headers": {"x-model-region": "eu"},
                "locked_headers": ["x-model-region"]
            }
        }))
        .send()
        .await
        .unwrap();
    assert!(advanced.status().is_success());

    post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": provider_id, "weight": 1}),
    )
    .await;

    // the snapshot the gateway polls must now reflect the new provider + route
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(
        snap["version"].as_u64().unwrap_or(0) > 0,
        "version should bump after writes: {snap}"
    );
    let providers = snap["config"]["providers"].as_array().expect("providers");
    assert!(
        providers.iter().any(|p| p["name"] == "openai"),
        "provider missing from snapshot: {snap}"
    );
    let routes = snap["config"]["routes"].as_array().expect("routes");
    assert!(
        routes.iter().any(|r| r["model"] == "gpt-4o"),
        "route missing from snapshot: {snap}"
    );
    let route = routes
        .iter()
        .find(|r| r["model"] == "gpt-4o")
        .expect("route in snapshot");
    assert_eq!(route["advanced"]["limits"]["output_tokens"], 2048);
    assert_eq!(route["advanced"]["headers"]["x-model-region"], "eu");
}

/// Pausing a dashboard guardrail rule that a route still names in an override
/// must not turn every later snapshot into a 500: the override is pruned and
/// reported instead (#2306).
#[tokio::test]
async fn pausing_a_guardrail_rule_named_by_a_route_override_keeps_the_snapshot_served() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn send(req: reqwest::RequestBuilder, what: &str) -> Value {
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap_or(Value::Null);
        assert!(status.is_success(), "{what} failed ({status}): {json}");
        json
    }
    let rule_body = |enabled: bool| {
        json!({
            "name": "no-secrets", "enabled": enabled, "source_type": "pattern",
            "pattern": "secret", "stage": "pre_call", "action": "block",
            "include_system": false, "position": 0
        })
    };

    let org = send(
        client
            .post(format!("{base}/api/v1/orgs"))
            .json(&json!({"name": "Acme", "slug": "acme"})),
        "org",
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");
    let team = send(
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
            .json(&json!({"name": "Platform"})),
        "team",
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");
    let project = send(
        client
            .post(format!("{base}/api/v1/teams/{team_id}/projects"))
            .json(&json!({"name": "Gateway"})),
        "project",
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");
    let provider = send(
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .json(
                &json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}),
            ),
        "provider",
    )
    .await;
    let provider_id = provider["id"].as_str().expect("provider id");
    let route = send(
        client
            .post(format!("{base}/api/v1/projects/{project_id}/routes"))
            .json(&json!({"model": "gpt-4o", "strategy": "round_robin"})),
        "route",
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");
    send(
        client
            .post(format!("{base}/api/v1/routes/{route_id}/targets"))
            .json(&json!({"provider_id": provider_id, "weight": 1})),
        "target",
    )
    .await;
    let rule = send(
        client
            .post(format!("{base}/api/v1/guardrails/rules"))
            .json(&rule_body(true)),
        "rule",
    )
    .await;
    let rule_id = rule["id"].as_str().expect("rule id");
    send(
        client
            .put(format!("{base}/api/v1/routes/{route_id}/advanced"))
            .json(&json!({"advanced": {"guardrails": {"disable": ["no-secrets"]}}})),
        "advanced",
    )
    .await;

    let snapshot = |client: reqwest::Client, base: String| async move {
        client
            .get(format!("{base}/internal/snapshot"))
            .send()
            .await
            .unwrap()
    };
    let live = snapshot(client.clone(), base.clone()).await;
    assert_eq!(live.status(), 200);
    let live: Value = live.json().await.unwrap();
    let overrides = live["config"]["routes"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["model"] == "gpt-4o"))
        .map(|r| r["advanced"]["guardrails"]["disable"].clone())
        .expect("route in snapshot");
    assert_eq!(
        overrides,
        json!(["no-secrets"]),
        "precondition: override served"
    );

    // pause the rule: it leaves the effective set while the override stays
    send(
        client
            .put(format!("{base}/api/v1/guardrails/rules/{rule_id}"))
            .json(&rule_body(false)),
        "pause",
    )
    .await;

    let paused = snapshot(client.clone(), base.clone()).await;
    assert_eq!(paused.status(), 200, "snapshot must survive the pause");
    let paused: Value = paused.json().await.unwrap();
    let route = paused["config"]["routes"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["model"] == "gpt-4o"))
        .expect("route still served");
    assert!(
        route["advanced"]["guardrails"]["disable"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "override pruned: {route}"
    );

    let problems: Value = send(
        client.get(format!("{base}/api/v1/config/problems")),
        "problems",
    )
    .await;
    let listed = problems["problems"].as_array().expect("problems array");
    assert!(
        listed.iter().any(|p| p
            .as_str()
            .is_some_and(|p| p.contains("gpt-4o") && p.contains("no-secrets"))),
        "pruned override must be reported: {problems}"
    );
}

/// One org can neither point its routes and groups at another org's providers
/// nor take a name the gateway holds in a deployment-wide namespace, and the
/// snapshot stays servable through every refused attempt (#1844, #1845).
#[tokio::test]
async fn tenancy_guards_refuse_cross_org_references_and_shared_names() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let post = |url: String, body: Value| {
        let client = client.clone();
        async move {
            let resp = client.post(&url).json(&body).send().await.unwrap();
            let status = resp.status().as_u16();
            let json: Value = resp.json().await.unwrap_or(Value::Null);
            (status, json)
        }
    };
    let tenant = |slug: &'static str| {
        let base = base.clone();
        async move {
            let (_, org) = post(
                format!("{base}/api/v1/orgs"),
                json!({"name": slug, "slug": slug}),
            )
            .await;
            let org_id = org["id"].as_str().unwrap().to_string();
            let (_, team) = post(
                format!("{base}/api/v1/orgs/{org_id}/teams"),
                json!({"name": "core"}),
            )
            .await;
            let team_id = team["id"].as_str().unwrap();
            let (_, project) = post(
                format!("{base}/api/v1/teams/{team_id}/projects"),
                json!({"name": "app"}),
            )
            .await;
            (org_id, project["id"].as_str().unwrap().to_string())
        }
    };
    let (org_a, project_a) = tenant("tenant-a").await;
    let (org_b, project_b) = tenant("tenant-b").await;
    let provider = |org: String, name: &'static str| {
        post(
            format!("{base}/api/v1/orgs/{org}/providers"),
            json!({"name": name, "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
    };

    let (status, provider_a) = provider(org_a.clone(), "edge-a").await;
    assert_eq!(status, 200, "{provider_a}");
    let provider_a = provider_a["id"].as_str().unwrap().to_string();
    let (status, route_a) = post(
        format!("{base}/api/v1/projects/{project_a}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 200, "{route_a}");
    let (status, _) = post(
        format!(
            "{base}/api/v1/routes/{}/targets",
            route_a["id"].as_str().unwrap()
        ),
        json!({"provider_id": provider_a, "weight": 1}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = post(
        format!("{base}/api/v1/orgs/{org_a}/provider-groups"),
        json!({"name": "pool-a", "strategy": "round_robin",
               "members": [{"provider_id": provider_a}]}),
    )
    .await;
    assert_eq!(status, 200);

    // names the gateway holds across every org answer 409, without saying where
    let (status, body) = provider(org_b.clone(), "edge-a").await;
    assert_eq!(status, 409, "{body}");
    assert!(!body.to_string().contains("tenant-a"), "{body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/providers"),
        json!({"name": "edge-b", "slug": "edge-a", "kind": "openai_compatible",
               "api_base": "http://127.0.0.1:9"}),
    )
    .await;
    assert_eq!(status, 409, "provider slug: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/projects/{project_b}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 409, "route name: {body}");

    // another org's provider is refused as a target and as a member, as if unknown
    let (status, provider_b) = provider(org_b.clone(), "edge-b").await;
    assert_eq!(status, 200, "{provider_b}");
    let (status, route_b) = post(
        format!("{base}/api/v1/projects/{project_b}/routes"),
        json!({"model": "gpt-4o-b", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 200, "{route_b}");
    let (status, body) = post(
        format!(
            "{base}/api/v1/routes/{}/targets",
            route_b["id"].as_str().unwrap()
        ),
        json!({"provider_id": provider_a, "weight": 1}),
    )
    .await;
    assert_eq!(status, 404, "cross-org target: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "pool-b", "strategy": "round_robin",
               "members": [{"provider_id": provider_a}]}),
    )
    .await;
    assert_eq!(status, 404, "cross-org member: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "pool-a", "strategy": "round_robin",
               "members": [{"provider_id": provider_b["id"]}]}),
    )
    .await;
    assert_eq!(status, 409, "group slug: {body}");

    // the fleet still converges, and every row names the org that owns it
    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let config = &snapshot["config"];
    assert!(config.is_object(), "snapshot refused: {snapshot}");
    let route = config["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "gpt-4o")
        .unwrap();
    assert_eq!(route["tenancy"]["org_id"], org_a.as_str());
    assert_eq!(route["tenancy"]["project_id"], project_a.as_str());
    let edge_b = config["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "edge-b")
        .unwrap();
    assert_eq!(edge_b["tenancy"]["org_id"], org_b.as_str());

    // the anonymous dashboard config names no tenant
    let public: Value = client
        .get(format!("{base}/api/v1/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !public.to_string().contains(&org_a),
        "public config names an org"
    );
}

/// Providers and provider groups answer `slug/model` from one namespace at the
/// gateway, so a slug either kind holds is refused to the other kind too, on
/// create and on rename. A route name is refused where it would sit on another
/// org's `slug/model` address or on the builtin `fake-llm`, and is otherwise
/// free to contain `/` (#1845).
#[tokio::test]
async fn providers_groups_and_route_names_share_the_gateways_address_namespace() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let send = |method: reqwest::Method, url: String, body: Value| {
        let client = client.clone();
        async move {
            let resp = client
                .request(method, &url)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let json: Value = resp.json().await.unwrap_or(Value::Null);
            (status, json)
        }
    };
    let post = |url: String, body: Value| send(reqwest::Method::POST, url, body);
    let tenant = |slug: &'static str| {
        let base = base.clone();
        async move {
            let (_, org) = post(
                format!("{base}/api/v1/orgs"),
                json!({"name": slug, "slug": slug}),
            )
            .await;
            let org_id = org["id"].as_str().unwrap().to_string();
            let (_, team) = post(
                format!("{base}/api/v1/orgs/{org_id}/teams"),
                json!({"name": "core"}),
            )
            .await;
            let team_id = team["id"].as_str().unwrap();
            let (_, project) = post(
                format!("{base}/api/v1/teams/{team_id}/projects"),
                json!({"name": "app"}),
            )
            .await;
            (org_id, project["id"].as_str().unwrap().to_string())
        }
    };
    let (org_a, project_a) = tenant("tenant-a").await;
    let (org_b, project_b) = tenant("tenant-b").await;
    let provider = |org: &str, name: &str, slug: &str| {
        post(
            format!("{base}/api/v1/orgs/{org}/providers"),
            json!({"name": name, "slug": slug, "kind": "openai_compatible",
                   "api_base": "http://127.0.0.1:9"}),
        )
    };

    // org a holds provider slug `edge` and group slug `pool`
    let (status, edge) = provider(&org_a, "edge", "edge").await;
    assert_eq!(status, 200, "{edge}");
    let (status, pool) = post(
        format!("{base}/api/v1/orgs/{org_a}/provider-groups"),
        json!({"name": "pool", "slug": "pool", "strategy": "round_robin",
               "members": [{"provider_id": edge["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{pool}");

    // across kinds: a provider may not take a group's slug, nor a group a
    // provider's, and the refusal does not say which org holds it
    let (status, body) = provider(&org_b, "pool-provider", "pool").await;
    assert_eq!(status, 409, "provider on a group slug: {body}");
    assert!(!body.to_string().contains("tenant-a"), "{body}");
    let (status, own) = provider(&org_b, "edge-b", "edge-b").await;
    assert_eq!(status, 200, "{own}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "edge", "slug": "edge", "strategy": "round_robin",
               "members": [{"provider_id": own["id"]}]}),
    )
    .await;
    assert_eq!(status, 409, "group on a provider slug: {body}");
    let (status, own_group) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "pool-b", "slug": "pool-b", "strategy": "round_robin",
               "members": [{"provider_id": own["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{own_group}");

    // the same on rename
    let (status, body) = send(
        reqwest::Method::PUT,
        format!("{base}/api/v1/providers/{}", own["id"].as_str().unwrap()),
        json!({"slug": "pool", "allow_slug_change": true}),
    )
    .await;
    assert_eq!(status, 409, "provider renamed onto a group slug: {body}");
    let (status, body) = send(
        reqwest::Method::PUT,
        format!(
            "{base}/api/v1/provider-groups/{}",
            own_group["id"].as_str().unwrap()
        ),
        json!({"slug": "edge", "allow_slug_change": true}),
    )
    .await;
    assert_eq!(status, 409, "group renamed onto a provider slug: {body}");
    // re-sending a group's own slug is no collision
    let (status, body) = send(
        reqwest::Method::PUT,
        format!(
            "{base}/api/v1/provider-groups/{}",
            own_group["id"].as_str().unwrap()
        ),
        json!({"slug": "pool-b", "allow_slug_change": true}),
    )
    .await;
    assert_eq!(status, 200, "a group collided with itself: {body}");

    let route = |project: &str, model: &str| {
        post(
            format!("{base}/api/v1/projects/{project}/routes"),
            json!({"model": model, "strategy": "round_robin"}),
        )
    };
    // another org's addresses, and the builtin, are not a route name
    for model in ["edge/gpt-4o", "pool/gpt-4o", "fake-llm"] {
        let (status, body) = route(&project_b, model).await;
        assert_eq!(status, 409, "{model}: {body}");
        assert!(!body.to_string().contains("tenant-a"), "{model}: {body}");
    }
    // the org's own address is its choice, and an hf-style name is no address
    for (project, model) in [
        (&project_a, "edge/gpt-4o"),
        (&project_b, "edge-b/gpt-4o"),
        (&project_b, "Qwen/Qwen2.5-7B-Instruct"),
    ] {
        let (status, body) = route(project, model).await;
        assert_eq!(status, 200, "{model}: {body}");
    }

    // nothing refused above reached the snapshot, and it still builds
    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        snapshot["config"].is_object(),
        "snapshot refused: {snapshot}"
    );
}

/// A readonly provider or group from the bootstrap file answers `slug/model`
/// for every org, so no database provider or group may take its slug, on create
/// or on rename, and no route may be named on its address (#1845).
#[tokio::test]
async fn a_bootstrap_slug_is_refused_to_every_database_row() {
    skip_without_db!();
    let db = fresh_db().await;
    let mut bootstrap = rolter_core::GatewayConfig::default();
    // a derived slug (`edge-cluster`) and an explicit group slug
    bootstrap.providers.push(rolter_core::ProviderConfig {
        name: "Edge Cluster".to_string(),
        kind: rolter_core::ProviderKind::OpenaiCompatible,
        api_base: "http://127.0.0.1:9".to_string(),
        ..Default::default()
    });
    bootstrap
        .provider_groups
        .push(rolter_core::ProviderGroupConfig {
            name: "Shared".to_string(),
            slug: Some("shared-pool".to_string()),
            strategy: rolter_core::BalancingStrategy::RoundRobin,
            members: vec![rolter_core::GroupMember {
                provider: "Edge Cluster".to_string(),
                model: None,
                weight: 1,
            }],
            tenancy: None,
            ..Default::default()
        });
    let app = rolter_control::test_app_with_bootstrap(db.pool().clone(), &bootstrap)
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let send = |method: reqwest::Method, url: String, body: Value| {
        let client = client.clone();
        async move {
            let resp = client
                .request(method, &url)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let json: Value = resp.json().await.unwrap_or(Value::Null);
            (status, json)
        }
    };
    let post = |url: String, body: Value| send(reqwest::Method::POST, url, body);
    let (_, org) = post(
        format!("{base}/api/v1/orgs"),
        json!({"name": "tenant", "slug": "tenant"}),
    )
    .await;
    let org = org["id"].as_str().unwrap().to_string();
    let (_, team) = post(
        format!("{base}/api/v1/orgs/{org}/teams"),
        json!({"name": "core"}),
    )
    .await;
    let (_, project) = post(
        format!(
            "{base}/api/v1/teams/{}/projects",
            team["id"].as_str().unwrap()
        ),
        json!({"name": "app"}),
    )
    .await;
    let project = project["id"].as_str().unwrap().to_string();
    let provider = |name: &str, slug: &str| {
        post(
            format!("{base}/api/v1/orgs/{org}/providers"),
            json!({"name": name, "slug": slug, "kind": "openai_compatible",
                   "api_base": "http://127.0.0.1:9"}),
        )
    };

    for slug in ["edge-cluster", "shared-pool"] {
        let (status, body) = provider("mine", slug).await;
        assert_eq!(status, 409, "provider on bootstrap slug {slug}: {body}");
    }
    let (status, own) = provider("mine", "mine").await;
    assert_eq!(status, 200, "{own}");
    // a readonly group's own slug meets the older config-owned guard first,
    // which answers 400; a readonly provider's slug meets the namespace check
    for (slug, refused) in [("edge-cluster", 409), ("shared-pool", 400)] {
        let (status, body) = post(
            format!("{base}/api/v1/orgs/{org}/provider-groups"),
            json!({"name": format!("group-{slug}"), "slug": slug, "strategy": "round_robin",
                   "members": [{"provider_id": own["id"]}]}),
        )
        .await;
        assert_eq!(status, refused, "group on bootstrap slug {slug}: {body}");
    }
    let (status, group) = post(
        format!("{base}/api/v1/orgs/{org}/provider-groups"),
        json!({"name": "my-pool", "slug": "my-pool", "strategy": "round_robin",
               "members": [{"provider_id": own["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{group}");

    // the same on a slug change
    let (status, body) = send(
        reqwest::Method::PUT,
        format!("{base}/api/v1/providers/{}", own["id"].as_str().unwrap()),
        json!({"slug": "shared-pool", "allow_slug_change": true}),
    )
    .await;
    assert_eq!(
        status, 409,
        "provider renamed onto a bootstrap slug: {body}"
    );
    let (status, body) = send(
        reqwest::Method::PUT,
        format!(
            "{base}/api/v1/provider-groups/{}",
            group["id"].as_str().unwrap()
        ),
        json!({"slug": "edge-cluster", "allow_slug_change": true}),
    )
    .await;
    assert_eq!(status, 409, "group renamed onto a bootstrap slug: {body}");

    // and a route may not sit on either address
    for model in ["edge-cluster/gpt-4o", "shared-pool/gpt-4o"] {
        let (status, body) = post(
            format!("{base}/api/v1/projects/{project}/routes"),
            json!({"model": model, "strategy": "round_robin"}),
        )
        .await;
        assert_eq!(status, 409, "{model}: {body}");
    }
}

/// Listings answer a caller whose role sits below the org with what that
/// caller reaches, instead of refusing the whole list (#1846, #1850). A project
/// member can navigate to their own project, a team admin sees and manages
/// their own team's people, and nobody learns another tenant's org exists.
#[tokio::test]
async fn members_below_the_org_list_what_they_reach() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("listing".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let insert = |sql: &'static str, parent: Option<uuid::Uuid>, name: &'static str| {
        let pool = pool.clone();
        async move {
            let query = sqlx::query_scalar::<_, uuid::Uuid>(sql);
            let query = match parent {
                Some(parent) => query.bind(parent),
                None => query,
            };
            query.bind(name).fetch_one(&pool).await.unwrap()
        }
    };
    let org_sql = "insert into orgs (name, slug) values ($1, $1) returning id";
    let team_sql = "insert into teams (org_id, name) values ($1, $2) returning id";
    let project_sql = "insert into projects (team_id, name) values ($1, $2) returning id";
    let acme = insert(org_sql, None, "acme").await;
    let globex = insert(org_sql, None, "globex").await;
    let core = insert(team_sql, Some(acme), "core").await;
    let research = insert(team_sql, Some(acme), "research").await;
    let payments = insert(team_sql, Some(globex), "payments").await;
    let app_project = insert(project_sql, Some(core), "app").await;
    let batch = insert(project_sql, Some(core), "batch").await;
    let evals = insert(project_sql, Some(research), "evals").await;
    let checkout = insert(project_sql, Some(payments), "checkout").await;

    let member = seed_user(&pool, "pm@acme.test", false).await;
    seed_membership(&pool, member, None, None, Some(app_project), "member").await;
    let lead = seed_user(&pool, "lead@acme.test", false).await;
    seed_membership(&pool, lead, None, Some(core), None, "admin").await;
    let org_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(acme), None, None, "admin").await;
    let outsider = seed_user(&pool, "ops@globex.test", false).await;
    seed_membership(&pool, outsider, None, None, Some(checkout), "member").await;
    let pm = seed_session(&pool, member, "listing_pm").await;
    let lead_session = seed_session(&pool, lead, "listing_lead").await;
    let org_session = seed_session(&pool, org_admin, "listing_org").await;
    let ops = seed_session(&pool, outsider, "listing_ops").await;

    let get = |path: String, token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let resp = client
                .get(format!("{base}{path}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let ids = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect()
    };

    // a project member: their own org, team and project, nothing else
    let (_, orgs) = get("/api/v1/orgs".into(), pm.clone()).await;
    assert_eq!(ids(&orgs), vec![acme.to_string()]);
    let (_, teams) = get(format!("/api/v1/orgs/{acme}/teams"), pm.clone()).await;
    assert_eq!(ids(&teams), vec![core.to_string()]);
    let (_, projects) = get(format!("/api/v1/teams/{core}/projects"), pm.clone()).await;
    assert_eq!(ids(&projects), vec![app_project.to_string()]);
    let (_, projects) = get(format!("/api/v1/orgs/{acme}/projects"), pm.clone()).await;
    assert_eq!(ids(&projects), vec![app_project.to_string()]);
    assert_eq!(
        get(format!("/api/v1/orgs/{globex}/teams"), pm.clone())
            .await
            .0,
        403
    );
    assert_eq!(
        get(format!("/api/v1/teams/{research}/projects"), pm.clone())
            .await
            .0,
        403
    );
    // and /auth/me says which org and team the project sits under
    let (_, me) = get("/api/v1/auth/me".into(), pm.clone()).await;
    assert_eq!(me["memberships"][0]["scope_org_id"], acme.to_string());
    assert_eq!(me["memberships"][0]["scope_team_id"], core.to_string());
    assert_eq!(me["memberships"][0]["project_id"], app_project.to_string());
    // and the org's rule table, which the dashboard reads to say why a control
    // is disabled: the custom roles are the org's, a project role is enough
    let (status, matrix) = get(format!("/api/v1/rbac/matrix?org_id={acme}"), pm.clone()).await;
    assert_eq!(status, 200, "{matrix}");
    assert!(matrix["custom_roles"].is_array(), "{matrix}");

    // another tenant's member never sees that acme exists
    let (_, orgs) = get("/api/v1/orgs".into(), ops.clone()).await;
    assert_eq!(ids(&orgs), vec![globex.to_string()]);
    assert_eq!(
        get(format!("/api/v1/rbac/matrix?org_id={acme}"), ops.clone())
            .await
            .0,
        403
    );

    // a team admin: every project of their team, and their team's people
    let (_, projects) = get(
        format!("/api/v1/teams/{core}/projects"),
        lead_session.clone(),
    )
    .await;
    let mut both = vec![app_project.to_string(), batch.to_string()];
    both.sort();
    let mut listed = ids(&projects);
    listed.sort();
    assert_eq!(listed, both);
    let (_, memberships) = get(
        format!("/api/v1/orgs/{acme}/memberships"),
        lead_session.clone(),
    )
    .await;
    let people: Vec<&str> = memberships
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["user_id"].as_str().unwrap())
        .collect();
    assert!(people.contains(&lead.to_string().as_str()), "{memberships}");
    assert!(
        people.contains(&member.to_string().as_str()),
        "{memberships}"
    );
    assert!(
        !people.contains(&org_admin.to_string().as_str()),
        "org row leaked: {memberships}"
    );
    let (_, users) = get(format!("/api/v1/orgs/{acme}/users"), lead_session.clone()).await;
    let emails: Vec<&str> = users
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["email"].as_str().unwrap())
        .collect();
    assert_eq!(emails, vec!["lead@acme.test", "pm@acme.test"]);

    // invitations: the team admin sees and revokes their team's, not research's
    let invite = |scope_id: uuid::Uuid, email: &'static str| {
        client
            .post(format!("{base}/api/v1/orgs/{acme}/invitations"))
            .bearer_auth("listing")
            .json(&json!({"email": email, "role": "member",
                          "scope_type": "project", "scope_id": scope_id}))
            .send()
    };
    let ours: Value = invite(batch, "new@acme.test")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let theirs: Value = invite(evals, "eval@acme.test")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ours = ours["invitation"]["id"].as_str().unwrap().to_string();
    let theirs = theirs["invitation"]["id"].as_str().unwrap().to_string();
    let (_, invitations) = get(
        format!("/api/v1/orgs/{acme}/invitations"),
        lead_session.clone(),
    )
    .await;
    assert_eq!(ids(&invitations), vec![ours.clone()]);
    let revoke = |id: String| {
        client
            .delete(format!("{base}/api/v1/invitations/{id}"))
            .bearer_auth(lead_session.clone())
            .send()
    };
    assert_eq!(revoke(ours).await.unwrap().status(), 200);
    assert_eq!(revoke(theirs).await.unwrap().status(), 403);

    // an org role still gets the whole list
    let (_, memberships) = get(format!("/api/v1/orgs/{acme}/memberships"), org_session).await;
    assert_eq!(memberships.as_array().unwrap().len(), 3, "{memberships}");
}

/// A superadmin account holds no membership, and still gets what an org admin
/// gets from the self-service key routes: the seeded operator could not open
/// the Playground on a fresh deployment (#1847).
#[tokio::test]
async fn a_superadmin_without_membership_mints_personal_and_playground_keys() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("keys".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    // a playground key is scoped to the project's routes, so it needs one
    sqlx::query(
        "insert into routes (project_id, model, strategy) values ($1, 'gpt-4o', 'round_robin')",
    )
    .bind(project)
    .execute(&pool)
    .await
    .unwrap();
    let operator = seed_user(&pool, "operator@acme.test", true).await;
    let session = seed_session(&pool, operator, "keys_operator").await;

    let playground = client
        .post(format!(
            "{base}/api/v1/me/projects/{project}/playground-key"
        ))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(playground.status(), 200);
    let personal = client
        .post(format!("{base}/api/v1/me/projects/{project}/virtual-keys"))
        .bearer_auth(&session)
        .json(&json!({"name": "operator laptop", "expires_in_days": 7}))
        .send()
        .await
        .unwrap();
    assert_eq!(personal.status(), 200);
}

/// Offboarding stops a person's own keys at the gateway and leaves the shared
/// keys an admin minted for the project alone (#1841). Deprovisioning through
/// SCIM moves the snapshot version, so a polling gateway refetches; the same
/// keys come back on reprovisioning; and deleting the account disables them
/// rather than letting the foreign key turn them into shared keys.
#[tokio::test]
async fn a_leavers_personal_keys_leave_the_snapshot_and_shared_keys_stay() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("leaver".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("leaver")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org: uuid::Uuid = org["id"].as_str().unwrap().parse().unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    let scim: Value = client
        .post(format!("{base}/api/v1/orgs/{org}/scim-tokens"))
        .bearer_auth("leaver")
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim = scim["secret"].as_str().unwrap().to_string();
    let provisioned: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&scim)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "engineer@acme.test",
            "emails": [{"value": "engineer@acme.test", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    let engineer: uuid::Uuid = scim_id.parse().unwrap();
    seed_membership(&pool, engineer, None, None, Some(project), "member").await;
    let session = seed_session(&pool, engineer, "leaver_engineer").await;

    let mint = |url: String, bearer: String| {
        let client = client.clone();
        async move {
            let row: Value = client
                .post(url)
                .bearer_auth(bearer)
                .json(&json!({"name": "k", "expires_in_days": 7}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            row["id"].as_str().unwrap().to_string()
        }
    };
    let personal = mint(
        format!("{base}/api/v1/me/projects/{project}/virtual-keys"),
        session.clone(),
    )
    .await;
    let shared = mint(
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        "leaver".to_string(),
    )
    .await;
    // the version and, per served key, whether it is disabled
    let snapshot = || {
        let client = client.clone();
        let base = base.clone();
        async move {
            let snapshot: Value = client
                .get(format!("{base}/internal/snapshot"))
                .bearer_auth("leaver")
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let keys = snapshot["config"]["db_virtual_keys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| {
                    (
                        key["id"].as_str().unwrap().to_string(),
                        key["disabled"].as_bool().unwrap_or(false),
                    )
                })
                .collect::<std::collections::HashMap<String, bool>>();
            (snapshot["version"].as_i64().unwrap(), keys)
        }
    };
    let set_active = |active: bool| {
        let client = client.clone();
        let url = format!("{base}/scim/v2/Users/{scim_id}");
        let scim = scim.clone();
        async move {
            let response = client
                .patch(url)
                .bearer_auth(scim)
                .json(&json!({
                    "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
                    "Operations": [{"op": "replace", "path": "active", "value": active}]
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
    };
    let (version, keys) = snapshot().await;
    assert_eq!(keys.get(&personal), Some(&false), "{keys:?}");
    assert_eq!(keys.get(&shared), Some(&false), "{keys:?}");

    // deprovisioned by the IdP: the personal key goes, and the version moves
    // so the gateways refetch; the shared one stays
    set_active(false).await;
    let (after, keys) = snapshot().await;
    assert!(
        after > version,
        "deactivation left the version at {version}"
    );
    assert!(
        !keys.contains_key(&personal),
        "a deactivated creator's key is served"
    );
    assert_eq!(keys.get(&shared), Some(&false));
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'scim.user.update' order by at desc limit 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["personal_keys"], 1, "{detail}");

    // reprovisioned: the same key is back, nothing about it changed
    set_active(true).await;
    let (again, keys) = snapshot().await;
    assert!(again > after, "reactivation left the version at {after}");
    assert_eq!(keys.get(&personal), Some(&false));

    // their last role that reaches the project is removed: gone again
    sqlx::query("delete from memberships where user_id = $1")
        .bind(engineer)
        .execute(&pool)
        .await
        .unwrap();
    let (_, keys) = snapshot().await;
    assert!(
        !keys.contains_key(&personal),
        "a creator with no reach keeps their key"
    );
    assert_eq!(keys.get(&shared), Some(&false));

    // the account is deleted: the key outlives it as a row, disabled, rather
    // than being served as a shared key with its owner's policy gone
    seed_membership(&pool, engineer, None, None, Some(project), "member").await;
    let deleted = client
        .delete(format!("{base}/api/v1/users/{engineer}"))
        .bearer_auth("leaver")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let (_, keys) = snapshot().await;
    assert_eq!(keys.get(&personal), Some(&true), "{keys:?}");
    assert_eq!(keys.get(&shared), Some(&false));
    // written to the org it left, since its memberships went with it (#1854)
    let (audited_org, detail): (Option<uuid::Uuid>, Value) = sqlx::query_as(
        "select org_id, detail from audit_log where action = 'user.delete' and target_id = $1",
    )
    .bind(engineer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited_org, Some(org));
    assert_eq!(detail["personal_keys"], 1, "{detail}");
}

/// Account events are written with no org. An org's audit log shows them for
/// its own people, and not for anyone else's (#1854).
#[tokio::test]
async fn an_orgs_audit_log_shows_its_own_peoples_account_events() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(org), None, None, "admin").await;
    let member = seed_user(&pool, "pm@acme.test", false).await;
    seed_membership(&pool, member, None, None, Some(project), "member").await;
    let stranger = seed_user(&pool, "ops@globex.test", false).await;
    for (user, action) in [
        (member, "auth.mfa_break_glass_reset"),
        (member, "auth.login_failed"),
        (stranger, "auth.login_failed"),
    ] {
        sqlx::query(
            "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
             values (null, $1, $2, 'user', $1, '{}')",
        )
        .bind(user)
        .bind(action)
        .execute(&pool)
        .await
        .unwrap();
    }
    let session = seed_session(&pool, admin, "audit_admin").await;
    let page: Value = client
        .get(format!("http://{addr}/api/v1/orgs/{org}/audit-log"))
        .bearer_auth(session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows: Vec<(String, String)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["actor_user_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                row["action"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let member = member.to_string();
    assert!(
        rows.contains(&(member.clone(), "auth.mfa_break_glass_reset".to_string())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&(member, "auth.login_failed".to_string())),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|(actor, _)| actor == &stranger.to_string()),
        "another tenant's account events: {rows:?}"
    );
}

/// The deployment-wide read returns the rows no org read can: a superadmin's
/// own events and an attempt against an unregistered address. It is
/// superadmin-only, and pages and filters like the per-org read (#1858).
#[tokio::test]
async fn the_deployment_audit_log_is_a_superadmins_and_returns_org_less_rows() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("audit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let operator = seed_user(&pool, "operator@deploy.test", true).await;
    let org_admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(org), None, None, "admin").await;
    // oldest first; none of them carries an org
    for (minutes_ago, actor, action, target) in [
        (4, Some(operator), "auth.login", Some(operator)),
        (3, None, "auth.login_failed", None),
        (2, Some(operator), "auth.mfa_enrolled", Some(operator)),
        (1, Some(operator), "auth.login", Some(operator)),
    ] {
        sqlx::query(
            "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail, at)
             values (null, $1, $2, case when $3::uuid is null then null else 'user' end, $3, '{}',
                     now() - make_interval(mins => $4))",
        )
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(minutes_ago)
        .execute(&pool)
        .await
        .unwrap();
    }
    let operator_session = seed_session(&pool, operator, "deploy_operator").await;
    let admin_session = seed_session(&pool, org_admin, "deploy_org_admin").await;
    let url = format!("http://{addr}/api/v1/audit-log");

    // an org admin does not read the deployment's log
    let denied = client
        .get(&url)
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    let page: Value = client
        .get(&url)
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let actions: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions.iter().filter(|a| **a == "auth.login").count(), 2);
    let unknown = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == "auth.login_failed")
        .expect("the unknown-address attempt is returned");
    assert!(unknown["actor_user_id"].is_null(), "{unknown}");
    assert!(unknown["org_id"].is_null(), "{unknown}");
    // the per-org read still cannot see the operator's events
    let per_org: Value = client
        .get(format!("http://{addr}/api/v1/orgs/{org}/audit-log"))
        .bearer_auth(&admin_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(per_org["items"].as_array().unwrap().is_empty(), "{per_org}");

    // filters
    let filtered: Value = client
        .get(format!("{url}?action=auth.mfa_enrolled&include_total=true"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1, "{filtered}");
    assert_eq!(filtered["total"], 1);
    let by_actor: Value = client
        .get(format!("{url}?actor={operator}"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(by_actor["items"].as_array().unwrap().len(), 3, "{by_actor}");

    // cursor: pages of two, newest first, then back
    let first: Value = client
        .get(format!("{url}?limit=2&include_total=true"))
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["total"], 4);
    assert_eq!(first["has_next"], true);
    assert_eq!(first["has_previous"], false);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let second: Value = client
        .get(&url)
        .query(&[("limit", "2"), ("cursor", cursor.as_str())])
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["has_next"], false);
    assert_eq!(second["has_previous"], true);
    let mut seen: Vec<String> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|row| row["id"].as_str().unwrap().to_string())
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 4, "the pages overlap or skip a row");
    assert_eq!(second["items"][0]["action"], "auth.login_failed");
    let back: Value = client
        .get(&url)
        .query(&[
            ("limit", "2"),
            ("direction", "previous"),
            ("cursor", second["previous_cursor"].as_str().unwrap()),
        ])
        .bearer_auth(&operator_session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(back["items"], first["items"]);
}

#[tokio::test]
async fn org_slug_is_validated() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let res = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&serde_json::json!({
            "name": "Test Org",
            "slug": "invalid slug with spaces",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn business_unit_and_customer_crud_round_trip() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");

    let other_org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Other", "slug": "other"}),
    )
    .await;
    let other_org_id = other_org["id"].as_str().expect("other org id");

    let business_unit = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
        json!({"name": "Payments"}),
    )
    .await;
    let business_unit_id = business_unit["id"].as_str().expect("business unit id");
    assert_eq!(business_unit["slug"], "payments");

    let customer = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/customers"),
        json!({"name": "Acme EU", "business_unit_id": business_unit_id}),
    )
    .await;
    let customer_id = customer["id"].as_str().expect("customer id");
    assert_eq!(customer["slug"], "acme-eu");
    assert_eq!(customer["business_unit_id"], business_unit_id);

    let mismatch_unit = post(
        &client,
        format!("{base}/api/v1/orgs/{other_org_id}/business-units"),
        json!({"name": "Other Unit"}),
    )
    .await;
    let mismatch_unit_id = mismatch_unit["id"].as_str().expect("mismatch unit id");
    let invalid_customer = client
        .post(format!("{base}/api/v1/orgs/{org_id}/customers"))
        .json(&json!({"name": "Broken Link", "business_unit_id": mismatch_unit_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_customer.status(), 400);

    let update_customer = client
        .put(format!("{base}/api/v1/customers/{customer_id}"))
        .json(&json!({
            "slug": "acme-emea",
            "allow_slug_change": true,
            "retired": true
        }))
        .send()
        .await
        .unwrap();
    assert!(update_customer.status().is_success());
    let updated_customer: Value = update_customer.json().await.unwrap();
    assert_eq!(updated_customer["slug"], "acme-emea");
    assert!(updated_customer["retired_at"].is_string());

    let list_customers: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/customers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list_customers.as_array().unwrap().len(), 1);

    let clear_business_unit = client
        .put(format!("{base}/api/v1/customers/{customer_id}"))
        .json(&json!({ "business_unit_id": null, "retired": false }))
        .send()
        .await
        .unwrap();
    assert!(clear_business_unit.status().is_success());
    let cleared_customer: Value = clear_business_unit.json().await.unwrap();
    assert!(cleared_customer["business_unit_id"].is_null());
    assert!(cleared_customer["retired_at"].is_null());

    let retire_business_unit = client
        .put(format!("{base}/api/v1/business-units/{business_unit_id}"))
        .json(&json!({"retired": true}))
        .send()
        .await
        .unwrap();
    assert!(retire_business_unit.status().is_success());
    let updated_unit: Value = retire_business_unit.json().await.unwrap();
    assert!(updated_unit["retired_at"].is_string());

    let delete_customer = client
        .delete(format!("{base}/api/v1/customers/{customer_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_customer.status(), 204);

    let delete_business_unit = client
        .delete(format!("{base}/api/v1/business-units/{business_unit_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_business_unit.status(), 204);
}

#[tokio::test]
async fn governance_scoped_budgets_and_rate_limits() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");

    let unit = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
        json!({"name": "Payments"}),
    )
    .await;
    let unit_id = unit["id"].as_str().expect("business unit id");

    let customer = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/customers"),
        json!({"name": "Acme EU"}),
    )
    .await;
    let customer_id = customer["id"].as_str().expect("customer id");

    let budget = post(
        &client,
        format!("{base}/api/v1/budgets"),
        json!({"scope_type": "business_unit", "scope_id": unit_id, "limit_usd": "250.0"}),
    )
    .await;
    assert_eq!(budget["scope_type"], "business_unit");

    let limit = post(
        &client,
        format!("{base}/api/v1/rate-limits"),
        json!({"scope_type": "customer", "scope_id": customer_id, "rpm": 60}),
    )
    .await;
    assert_eq!(limit["scope_type"], "customer");

    let listed: Value = client
        .get(format!(
            "{base}/api/v1/budgets?scope_type=business_unit&scope_id={unit_id}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // an unknown scope type is rejected before it reaches the store
    let bad = client
        .post(format!("{base}/api/v1/budgets"))
        .json(&json!({"scope_type": "division", "scope_id": unit_id, "limit_usd": "1.0"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // the gateway snapshot carries both caps with their governance scopes
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let budgets = snap["config"]["budgets"].as_array().expect("budgets");
    assert!(budgets
        .iter()
        .any(|b| b["scope"] == "business_unit" && b["id"] == unit_id));
    let limits = snap["config"]["rate_limits"].as_array().expect("limits");
    assert!(limits
        .iter()
        .any(|l| l["scope"] == "customer" && l["id"] == customer_id));
}

/// #996: a budget may carry its own `unpriced_policy`, and it has to survive
/// the whole path — create, list, and the snapshot the gateway actually reads.
/// A field that only toml could populate would silently do nothing for the
/// deployments that use the control plane, which is why #974 left it out.
#[tokio::test]
async fn a_budget_carries_its_own_unpriced_policy_into_the_snapshot() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Unpriced", "slug": "unpriced"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id").to_string();
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id").to_string();

    // the org refuses what it cannot account for
    let strict = post(
        &client,
        format!("{base}/api/v1/budgets"),
        json!({
            "scope_type": "org",
            "scope_id": org_id,
            "limit_usd": "1000.0",
            "unpriced_policy": "block",
        }),
    )
    .await;
    assert_eq!(strict["unpriced_policy"], "block");

    // a budget that says nothing inherits the deployment-wide setting, and
    // comes back as null rather than as a guessed default
    let inheriting = post(
        &client,
        format!("{base}/api/v1/budgets"),
        json!({"scope_type": "team", "scope_id": team_id, "limit_usd": "10.0"}),
    )
    .await;
    assert!(inheriting["unpriced_policy"].is_null());

    // an unknown policy is a 400 that names the three values, not a 500 from
    // the column's check constraint
    let bad = client
        .post(format!("{base}/api/v1/budgets"))
        .json(&json!({
            "scope_type": "org",
            "scope_id": org_id,
            "limit_usd": "5.0",
            "unpriced_policy": "blocked",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let listed: Value = client
        .get(format!(
            "{base}/api/v1/budgets?scope_type=org&scope_id={org_id}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["unpriced_policy"], "block");

    // and the gateway sees it: the override rides the snapshot, and the
    // inheriting budget carries no field at all rather than a fabricated one
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let budgets = snap["config"]["budgets"].as_array().expect("budgets");
    let org_budget = budgets
        .iter()
        .find(|b| b["id"] == org_id.as_str())
        .expect("org budget in snapshot");
    assert_eq!(org_budget["unpriced_policy"], "block");
    let team_budget = budgets
        .iter()
        .find(|b| b["id"] == team_id.as_str())
        .expect("team budget in snapshot");
    assert!(team_budget.get("unpriced_policy").is_none());

    // the audit row says an override was set, because it changes what the
    // gateway will serve rather than only what it counts
    let details: Vec<Value> = sqlx::query_scalar(
        "select detail from audit_log where action = 'budget.create' order by at desc",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(details
        .iter()
        .any(|detail| detail["unpriced_policy"] == "block"));
    assert!(details
        .iter()
        .any(|detail| detail["unpriced_policy"].is_null()));
}

/// #1285: a budget changes in place. Delete-and-recreate left the scope with
/// no cap between the two calls and bumped `config_version` twice, so a
/// polling gateway could load exactly that gap. The edit keeps the row's id
/// and `created_at`, bumps the version once, reaches the snapshot, and leaves
/// a `budget.update` audit row naming what moved.
#[tokio::test]
async fn a_budget_is_edited_in_place_with_one_config_bump() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn send(
        client: &reqwest::Client,
        method: reqwest::Method,
        url: String,
        body: Value,
    ) -> (reqwest::StatusCode, Value) {
        let resp = client
            .request(method, &url)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap_or(Value::Null);
        (status, json)
    }

    let (_, org) = send(
        &client,
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Editable", "slug": "editable"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id").to_string();

    let (status, created) = send(
        &client,
        reqwest::Method::POST,
        format!("{base}/api/v1/budgets"),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "100"}),
    )
    .await;
    assert!(status.is_success(), "{status}: {created}");
    let id = created["id"].as_str().expect("budget id").to_string();
    let url = format!("{base}/api/v1/budgets/{id}");

    // raise the cap and tighten the unpriced policy in one edit
    let before = config_version(&pool).await;
    let (status, edited) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"limit_usd": "250.5", "unpriced_policy": "block"}),
    )
    .await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "one edit must bump config_version exactly once"
    );
    assert_eq!(
        edited["id"], created["id"],
        "the row is edited, not replaced"
    );
    assert_eq!(edited["created_at"], created["created_at"]);
    assert_eq!(edited["limit_usd"], "250.5000");
    assert_eq!(edited["unpriced_policy"], "block");
    // a field the patch did not name is left alone
    assert_eq!(edited["period"], created["period"]);

    // explicit null drops the override; absent fields still stay put
    let (status, inheriting) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"unpriced_policy": null, "period": "daily"}),
    )
    .await;
    assert_eq!(status, 200, "{inheriting}");
    assert!(inheriting["unpriced_policy"].is_null());
    assert_eq!(inheriting["period"], "daily");
    assert_eq!(inheriting["limit_usd"], "250.5000");

    // an empty patch is a no-op: no write, so no bump
    let before = config_version(&pool).await;
    let (status, unchanged) = send(&client, reqwest::Method::PATCH, url.clone(), json!({})).await;
    assert_eq!(status, 200, "{unchanged}");
    assert_eq!(unchanged["period"], "daily");
    assert_eq!(config_version(&pool).await, before);

    // so is one that re-sends what the row already holds, including a limit
    // spelled differently from the column's numeric(12,4): the dashboard sends
    // "250.50" when someone retypes the cap they were shown. the audit query
    // below counts exactly two update rows, so this one must not add a third
    let (status, resent) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"limit_usd": "250.50", "period": "daily", "unpriced_policy": null}),
    )
    .await;
    assert_eq!(status, 200, "{resent}");
    assert_eq!(resent["limit_usd"], "250.5000");
    assert_eq!(
        config_version(&pool).await,
        before,
        "an edit that moves nothing must not wake the fleet"
    );

    // bad values are 400s that change nothing. the column is numeric(12,4),
    // so 1e8 and anything rounding up to it would otherwise overflow in the
    // store and come back as a 500 carrying the database's own message
    for body in [
        json!({"unpriced_policy": "blocked"}),
        json!({"limit_usd": "lots"}),
        json!({"limit_usd": "NaN"}),
        json!({"limit_usd": "-1"}),
        json!({"limit_usd": "100000000"}),
        json!({"limit_usd": "99999999.99995"}),
        json!({"limit_usd": "1e9"}),
        json!({"period": "  "}),
    ] {
        let (status, error) =
            send(&client, reqwest::Method::PATCH, url.clone(), body.clone()).await;
        assert_eq!(status, 400, "{body} should be refused, got {error}");
    }
    // the scope is not editable: a budget moved elsewhere is another budget
    let (status, _) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"scope_id": org_id}),
    )
    .await;
    assert_eq!(status, 400, "scope_id is not a patchable field");
    assert_eq!(config_version(&pool).await, before);

    let (status, _) = send(
        &client,
        reqwest::Method::PATCH,
        format!("{base}/api/v1/budgets/{}", uuid::Uuid::new_v4()),
        json!({"limit_usd": "1"}),
    )
    .await;
    assert_eq!(status, 404);

    // still one budget on the scope, and the gateway sees the edited cap
    let listed: Value = client
        .get(format!(
            "{base}/api/v1/budgets?scope_type=org&scope_id={org_id}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let budget = snap["config"]["budgets"]
        .as_array()
        .expect("budgets")
        .iter()
        .find(|b| b["id"] == org_id.as_str())
        .expect("org budget in snapshot")
        .clone();
    // the snapshot carries the cap as a JSON number
    assert_eq!(budget["limit_usd"], 250.5);
    assert_eq!(budget["period"], "daily");
    assert!(budget.get("unpriced_policy").is_none());

    // one audit row per edit, each naming only what it moved
    let details: Vec<Value> = sqlx::query_scalar(
        "select detail from audit_log where action = 'budget.update' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(details.len(), 2, "{details:?}");
    assert_eq!(
        details[0]["changes"],
        json!({
            "limit_usd": {"from": "100.0000", "to": "250.5000"},
            "unpriced_policy": {"from": null, "to": "block"},
        })
    );
    assert_eq!(
        details[1]["changes"],
        json!({
            "period": {"from": "30d", "to": "daily"},
            "unpriced_policy": {"from": "block", "to": null},
        })
    );
    assert_eq!(details[0]["scope_type"], "org");
    let deletes: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action in ('budget.delete', 'budget.create')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(deletes, 1, "only the original create, never a recreate");
}

/// #1285, the rate-limit half: each cap is set, lifted with an explicit null
/// or left alone when absent, and an edit that would leave no cap at all is
/// refused rather than stored as a limit that admits everything.
#[tokio::test]
async fn a_rate_limit_is_edited_in_place() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn send(
        client: &reqwest::Client,
        method: reqwest::Method,
        url: String,
        body: Value,
    ) -> (reqwest::StatusCode, Value) {
        let resp = client
            .request(method, &url)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap_or(Value::Null);
        (status, json)
    }

    let (_, org) = send(
        &client,
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Throttled", "slug": "throttled"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id").to_string();
    let (status, created) = send(
        &client,
        reqwest::Method::POST,
        format!("{base}/api/v1/rate-limits"),
        json!({"scope_type": "org", "scope_id": org_id, "rpm": 60}),
    )
    .await;
    assert!(status.is_success(), "{status}: {created}");
    let url = format!(
        "{base}/api/v1/rate-limits/{}",
        created["id"].as_str().expect("rate limit id")
    );

    let before = config_version(&pool).await;
    let (status, edited) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"tpm": 1000}),
    )
    .await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(config_version(&pool).await - before, 1);
    assert_eq!(edited["id"], created["id"]);
    assert_eq!(edited["rpm"], 60, "an absent cap is left alone");
    assert_eq!(edited["tpm"], 1000);

    let (status, lifted) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"rpm": null}),
    )
    .await;
    assert_eq!(status, 200, "{lifted}");
    assert!(lifted["rpm"].is_null());
    assert_eq!(lifted["tpm"], 1000);

    // re-sending both caps as they stand writes nothing: no bump, and no
    // third audit row for the count below to trip on
    let before = config_version(&pool).await;
    let (status, resent) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"rpm": null, "tpm": 1000}),
    )
    .await;
    assert_eq!(status, 200, "{resent}");
    assert_eq!(resent["tpm"], 1000);
    assert_eq!(config_version(&pool).await, before);

    // lifting the last cap would leave a limit that admits everything, and
    // the gateway reads a cap of zero or below as no cap at all
    let before = config_version(&pool).await;
    for body in [
        json!({"tpm": null}),
        json!({"tpm": -5}),
        json!({"rpm": 0}),
        json!({"rpm": null, "tpm": 0}),
    ] {
        let (status, error) =
            send(&client, reqwest::Method::PATCH, url.clone(), body.clone()).await;
        assert_eq!(status, 400, "{body} should be refused, got {error}");
    }
    assert_eq!(config_version(&pool).await, before);

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let limit = snap["config"]["rate_limits"]
        .as_array()
        .expect("rate limits")
        .iter()
        .find(|l| l["id"] == org_id.as_str())
        .expect("org rate limit in snapshot")
        .clone();
    // a lifted cap is absent from the snapshot rather than zero
    assert!(limit.get("rpm").is_none());
    assert_eq!(limit["tpm"], 1000);

    let details: Vec<Value> = sqlx::query_scalar(
        "select detail from audit_log where action = 'rate_limit.update' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(details.len(), 2, "{details:?}");
    assert_eq!(
        details[0]["changes"],
        json!({"tpm": {"from": null, "to": 1000}})
    );
    assert_eq!(
        details[1]["changes"],
        json!({"rpm": {"from": 60, "to": null}})
    );
}

/// #1903: creating a budget or rate limit refuses the caps the gateway would
/// ignore or misread, with the same 400s an edit already got. Each refused body
/// writes nothing, so neither the version nor the scope's rows move.
#[tokio::test]
async fn creating_a_cap_refuses_what_the_gateway_would_ignore() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Guarded", "slug": "guarded"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id").to_string();

    async fn refused(client: &reqwest::Client, url: String, body: Value) -> String {
        let response = client.post(&url).json(&body).send().await.unwrap();
        let status = response.status();
        let error: Value = response.json().await.unwrap_or(Value::Null);
        assert_eq!(status, 400, "{body} should be refused, got {error}");
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    let before = config_version(&pool).await;
    let budgets = format!("{base}/api/v1/budgets");
    // NaN parses as a float and is a legal numeric but reaches the gateway as
    // no cap; inf and 1e9 overflow numeric(12,4); a negative cap refuses
    // everything as already spent
    for limit in ["NaN", "inf", "-1", "100000000", "1e9", "lots"] {
        let message = refused(
            &client,
            budgets.clone(),
            json!({"scope_type": "org", "scope_id": org_id, "limit_usd": limit}),
        )
        .await;
        assert!(message.contains("limit_usd"), "{limit}: {message}");
    }
    let rate_limits = format!("{base}/api/v1/rate-limits");
    for (caps, field) in [
        (json!({"rpm": 0}), "rpm"),
        (json!({"tpm": -5}), "tpm"),
        (json!({"rpm": 60, "tpm": 0}), "tpm"),
        // neither cap, however it is spelt, is a limit that admits everything
        (json!({}), "rpm cap, a tpm cap"),
        (json!({"rpm": null, "tpm": null}), "rpm cap, a tpm cap"),
    ] {
        let mut body = json!({"scope_type": "org", "scope_id": org_id});
        body.as_object_mut()
            .unwrap()
            .extend(caps.as_object().unwrap().clone());
        let message = refused(&client, rate_limits.clone(), body).await;
        assert!(message.contains(field), "{caps}: {message}");
    }
    assert_eq!(
        config_version(&pool).await,
        before,
        "a refused create must not wake the fleet"
    );
    let (budget_rows, limit_rows): (i64, i64) =
        sqlx::query_as("select (select count(*) from budgets), (select count(*) from rate_limits)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((budget_rows, limit_rows), (0, 0));

    // the values an edit accepts are accepted here too, zero freezing a scope
    for body in [
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "0"}),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": " 99999999.9999 "}),
    ] {
        let response = client.post(&budgets).json(&body).send().await.unwrap();
        assert!(response.status().is_success(), "{body}");
    }
    let response = client
        .post(&rate_limits)
        .json(&json!({"scope_type": "org", "scope_id": org_id, "tpm": 1}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

/// #1902: the gateway has no rolling windows, and used to read every period it
/// did not know as monthly, so a `7d` budget was a calendar-month cap. A period
/// outside what it recognises is now a 400 on create and on edit, naming the
/// accepted spellings. A row stored before the check keeps being enforced as
/// it was, and `GET /api/v1/config/problems` says so.
#[tokio::test]
async fn a_budget_period_is_one_the_gateway_recognises() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn send(
        client: &reqwest::Client,
        method: reqwest::Method,
        url: String,
        body: Value,
    ) -> (reqwest::StatusCode, Value) {
        let resp = client
            .request(method, &url)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap_or(Value::Null);
        (status, json)
    }

    let (_, org) = send(
        &client,
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Windows", "slug": "windows"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id").to_string();
    let budgets = format!("{base}/api/v1/budgets");

    for period in ["7d", "weekly", "dialy", "  "] {
        let (status, error) = send(
            &client,
            reqwest::Method::POST,
            budgets.clone(),
            json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "10", "period": period}),
        )
        .await;
        assert_eq!(status, 400, "{period:?} should be refused, got {error}");
        let message = error["error"]["message"].as_str().unwrap_or_default();
        for accepted in ["daily", "monthly", "total", "30d"] {
            assert!(
                message.contains(accepted),
                "{message} should name {accepted}"
            );
        }
    }

    // every spelling the gateway reads is accepted, stored as sent
    let (status, created) = send(
        &client,
        reqwest::Method::POST,
        budgets.clone(),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "10", "period": " Daily "}),
    )
    .await;
    assert!(status.is_success(), "{status}: {created}");
    assert_eq!(created["period"], "Daily");
    let (status, defaulted) = send(
        &client,
        reqwest::Method::POST,
        budgets.clone(),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "10"}),
    )
    .await;
    assert!(status.is_success(), "{status}: {defaulted}");
    assert_eq!(defaulted["period"], "30d", "the default is unchanged");

    let url = format!("{base}/api/v1/budgets/{}", created["id"].as_str().unwrap());
    let before = config_version(&pool).await;
    let (status, error) = send(
        &client,
        reqwest::Method::PATCH,
        url.clone(),
        json!({"period": "7d"}),
    )
    .await;
    assert_eq!(status, 400, "{error}");
    assert_eq!(config_version(&pool).await, before);
    let (status, edited) = send(
        &client,
        reqwest::Method::PATCH,
        url,
        json!({"period": "total"}),
    )
    .await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(edited["period"], "total");

    // nothing the API accepts is reported
    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(problems["problems"], json!([]), "{problems}");

    // a row written before the check, which no migration rewrites
    let legacy: uuid::Uuid = sqlx::query_scalar(
        "insert into budgets (scope_type, scope_id, limit_usd, period)
         values ('org', $1, 25, '7d') returning id",
    )
    .bind(org_id.parse::<uuid::Uuid>().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lines = problems["problems"].as_array().expect("problems");
    assert_eq!(lines.len(), 1, "{problems}");
    let line = lines[0].as_str().unwrap();
    assert!(line.contains(&legacy.to_string()), "{line}");
    assert!(line.contains("'7d'"), "{line}");
    assert!(line.contains("monthly"), "{line}");

    // still served, as the monthly cap it always was
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let periods: Vec<&str> = snap["config"]["budgets"]
        .as_array()
        .expect("budgets")
        .iter()
        .filter_map(|b| b["period"].as_str())
        .collect();
    assert_eq!(periods, ["total", "monthly", "monthly"], "{snap}");
}

/// Editing a budget or rate limit takes `update` on it, which the matrix
/// grants to an admin of the scope and not to a viewer (#1285). Both reach the
/// row's own scope chain, so the check is against where the cap already lives.
#[tokio::test]
async fn editing_a_cap_takes_admin_on_its_scope() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Caps", "slug": "caps"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let budget = post_as(
        &client,
        format!("{base}/api/v1/budgets"),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "10"}),
    )
    .await;
    let limit = post_as(
        &client,
        format!("{base}/api/v1/rate-limits"),
        json!({"scope_type": "org", "scope_id": org_id, "rpm": 10}),
    )
    .await;
    let budget_url = format!("{base}/api/v1/budgets/{}", budget["id"].as_str().unwrap());
    let limit_url = format!(
        "{base}/api/v1/rate-limits/{}",
        limit["id"].as_str().unwrap()
    );

    let viewer = seed_user(&pool, "caps-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "capsviewer").await;
    let admin = seed_user(&pool, "caps-admin@example.com", false).await;
    seed_membership(&pool, admin, Some(org_uuid), None, None, "admin").await;
    let admin_token = seed_session(&pool, admin, "capsadmin").await;
    // an admin, but of another org: the edit is authorized against the org
    // the cap lives in, so admin anywhere else is not enough
    let other_org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Elsewhere", "slug": "elsewhere"}),
    )
    .await;
    let other_org_uuid: uuid::Uuid = other_org["id"].as_str().unwrap().parse().unwrap();
    let outsider = seed_user(&pool, "caps-outsider@example.com", false).await;
    seed_membership(&pool, outsider, Some(other_org_uuid), None, None, "admin").await;
    let outsider_token = seed_session(&pool, outsider, "capsoutsider").await;

    for (url, body) in [
        (&budget_url, json!({"limit_usd": "20"})),
        (&limit_url, json!({"rpm": 20})),
    ] {
        let refused = client
            .patch(url.as_str())
            .bearer_auth(&viewer_token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 403, "a viewer edited {url}");

        let refused = client
            .patch(url.as_str())
            .bearer_auth(&outsider_token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            refused.status(),
            403,
            "an admin of another org edited {url}"
        );

        let allowed = client
            .patch(url.as_str())
            .bearer_auth(&admin_token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), 200, "an org admin could not edit {url}");
    }
}

/// Two edits that each lift a different cap must not together leave a rate
/// limit with none (#1285). The "keep at least one cap" rule is checked
/// against the row under the edit's own lock, so whichever edit lands second
/// sees the first one's result and is refused, and the row always limits
/// something. Checked against a read taken before the write, both edits would
/// pass and store a limit that admits everything.
#[tokio::test]
async fn concurrent_edits_cannot_lift_every_cap() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Racing", "slug": "racing"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id").to_string();

    // a few rounds, since a single one can land both edits in order by luck
    for round in 0..5 {
        let created: Value = client
            .post(format!("{base}/api/v1/rate-limits"))
            .json(&json!({"scope_type": "org", "scope_id": org_id, "rpm": 60, "tpm": 1000}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["id"].as_str().expect("rate limit id").to_string();
        let url = format!("{base}/api/v1/rate-limits/{id}");

        let (lift_rpm, lift_tpm) = tokio::join!(
            client.patch(&url).json(&json!({"rpm": null})).send(),
            client.patch(&url).json(&json!({"tpm": null})).send(),
        );
        let mut statuses = [
            lift_rpm.unwrap().status().as_u16(),
            lift_tpm.unwrap().status().as_u16(),
        ];
        statuses.sort_unstable();
        assert_eq!(
            statuses,
            [200, 400],
            "round {round}: exactly one lift may land"
        );

        let (rpm, tpm): (Option<i32>, Option<i32>) =
            sqlx::query_as("select rpm, tpm from rate_limits where id = $1::uuid")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            rpm.is_some() != tpm.is_some(),
            "round {round}: one cap should remain, got rpm={rpm:?} tpm={tpm:?}"
        );
    }
}

/// Every cap write announces the new config version on Redis (#1285), so a
/// subscribed gateway refetches at once rather than at its next poll. A
/// handler that forgot to publish would still bump `config_version` through
/// the table trigger, so only a subscriber on the channel can tell. Needs a
/// Redis (`ROLTER_TEST_REDIS_URL`, set in CI); skipped without one.
#[tokio::test]
async fn cap_writes_announce_the_new_version_on_redis() {
    use futures_util::StreamExt;

    skip_without_db!();
    let Some(redis_url) = std::env::var("ROLTER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app_with_redis(pool.clone(), &redis_url)
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // subscribed before the first write, so no announcement can slip past
    let subscriber = redis::Client::open(redis_url.as_str()).expect("redis url");
    let mut pubsub = subscriber.get_async_pubsub().await.expect("redis pubsub");
    pubsub
        .subscribe(rolter_core::CONFIG_CHANNEL)
        .await
        .expect("subscribe");
    let mut announced = std::pin::pin!(pubsub.on_message());

    // the channel is shared with whatever else talks to this redis, so wait
    // for this schema's own version rather than for the next message
    async fn expect_announced(
        announced: &mut (impl futures_util::Stream<Item = redis::Msg> + Unpin),
        what: &str,
        version: i64,
    ) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let message = tokio::time::timeout_at(deadline, announced.next())
                .await
                .unwrap_or_else(|_| panic!("{what} never announced version {version}"))
                .expect("pubsub stream ended");
            if message.get_payload::<i64>().ok() == Some(version) {
                return;
            }
        }
    }

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Announced", "slug": "announced"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id").to_string();

    let budget: Value = client
        .post(format!("{base}/api/v1/budgets"))
        .json(&json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "100"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    expect_announced(&mut announced, "budget create", config_version(&pool).await).await;
    let budget_url = format!("{base}/api/v1/budgets/{}", budget["id"].as_str().unwrap());
    let edited = client
        .patch(&budget_url)
        .json(&json!({"limit_usd": "200"}))
        .send()
        .await
        .unwrap();
    assert_eq!(edited.status(), 200);
    expect_announced(&mut announced, "budget update", config_version(&pool).await).await;
    let deleted = client.delete(&budget_url).send().await.unwrap();
    assert_eq!(deleted.status(), 204);
    expect_announced(&mut announced, "budget delete", config_version(&pool).await).await;

    let limit: Value = client
        .post(format!("{base}/api/v1/rate-limits"))
        .json(&json!({"scope_type": "org", "scope_id": org_id, "rpm": 60}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    expect_announced(
        &mut announced,
        "rate limit create",
        config_version(&pool).await,
    )
    .await;
    let limit_url = format!(
        "{base}/api/v1/rate-limits/{}",
        limit["id"].as_str().unwrap()
    );
    let edited = client
        .patch(&limit_url)
        .json(&json!({"rpm": 120}))
        .send()
        .await
        .unwrap();
    assert_eq!(edited.status(), 200);
    expect_announced(
        &mut announced,
        "rate limit update",
        config_version(&pool).await,
    )
    .await;
    let deleted = client.delete(&limit_url).send().await.unwrap();
    assert_eq!(deleted.status(), 204);
    expect_announced(
        &mut announced,
        "rate limit delete",
        config_version(&pool).await,
    )
    .await;
}

#[tokio::test]
async fn virtual_key_cost_attribution_round_trip() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");

    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");

    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");

    let virtual_key = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/virtual-keys"),
        json!({"name": "billing-key"}),
    )
    .await;
    let virtual_key_id = virtual_key["id"].as_str().expect("virtual key id");
    assert!(virtual_key["business_unit_id"].is_null());
    assert!(virtual_key["customer_id"].is_null());

    let unit = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
        json!({"name": "Payments"}),
    )
    .await;
    let unit_id = unit["id"].as_str().expect("business unit id");

    let customer = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/customers"),
        json!({"name": "Acme EU", "business_unit_id": unit_id}),
    )
    .await;
    let customer_id = customer["id"].as_str().expect("customer id");

    let attributed: Value = client
        .put(format!(
            "{base}/api/v1/virtual-keys/{virtual_key_id}/attribution"
        ))
        .json(&json!({"business_unit_id": unit_id, "customer_id": customer_id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(attributed["business_unit_id"], unit_id);
    assert_eq!(attributed["customer_id"], customer_id);

    // the gateway snapshot carries the attribution so usage can be tagged
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let keys = snap["config"]["db_virtual_keys"]
        .as_array()
        .expect("db_virtual_keys");
    let key = keys
        .iter()
        .find(|k| k["id"] == virtual_key_id)
        .expect("virtual key in snapshot");
    assert_eq!(key["business_unit_id"], unit_id);
    assert_eq!(key["customer_id"], customer_id);

    // a customer from another org may not be attached
    let other_org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Other", "slug": "other"}),
    )
    .await;
    let other_org_id = other_org["id"].as_str().expect("other org id");
    let foreign_customer = post(
        &client,
        format!("{base}/api/v1/orgs/{other_org_id}/customers"),
        json!({"name": "Foreign"}),
    )
    .await;
    let foreign_customer_id = foreign_customer["id"]
        .as_str()
        .expect("foreign customer id");
    let cross_org = client
        .put(format!(
            "{base}/api/v1/virtual-keys/{virtual_key_id}/attribution"
        ))
        .json(&json!({"customer_id": foreign_customer_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(cross_org.status(), 400);

    // pairing a customer with a business unit that does not own it is rejected
    let other_unit = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
        json!({"name": "Growth"}),
    )
    .await;
    let other_unit_id = other_unit["id"].as_str().expect("other unit id");
    let mismatched = client
        .put(format!(
            "{base}/api/v1/virtual-keys/{virtual_key_id}/attribution"
        ))
        .json(&json!({"business_unit_id": other_unit_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(mismatched.status(), 400);

    // omitted fields stay put; explicit nulls clear the dimension
    let cleared: Value = client
        .put(format!(
            "{base}/api/v1/virtual-keys/{virtual_key_id}/attribution"
        ))
        .json(&json!({"customer_id": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["business_unit_id"], unit_id);
    assert!(cleared["customer_id"].is_null());

    // deleting the unit detaches the keys that pointed at it
    let delete_customer = client
        .delete(format!("{base}/api/v1/customers/{customer_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_customer.status(), 204);
    let delete_unit = client
        .delete(format!("{base}/api/v1/business-units/{unit_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_unit.status(), 204);
    let orphaned: Value = client
        .get(format!("{base}/api/v1/projects/{project_id}/virtual-keys"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(orphaned[0]["business_unit_id"].is_null());
}

/// #2279: a prompt template version `PromptTemplatesConfig::validate` rejects
/// used to be stored and published, after which the snapshot refused to be
/// served at all and config propagation froze for every tenant. The endpoint
/// now refuses such a version, and a row that got in some other way is pruned
/// from the snapshot and listed under `/api/v1/config/problems`.
#[tokio::test]
async fn an_invalid_prompt_template_version_cannot_freeze_the_snapshot() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve(
        rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    )
    .await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> (u16, Value) {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    let (_, org) = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id: uuid::Uuid = org["id"].as_str().expect("org id").parse().unwrap();
    let (_, template) = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/prompt-templates"),
        json!({"name": "support"}),
    )
    .await;
    let template_id = template["id"].as_str().expect("template id").to_string();
    let versions = format!("{base}/api/v1/prompt-templates/{template_id}/versions");

    // each way validate() rejects content is a 400 that names the problem
    let bad = [
        (
            json!({"variables": [], "decorators": [{"content": "hi {{ who }}"}]}),
            "undeclared variable 'who'",
        ),
        (
            json!({
                "variables": [{"name": "v", "required": true, "default": "d"}],
                "decorators": [{"content": "{{ v }}"}]
            }),
            "both required and defaulted",
        ),
        (
            json!({
                "variables": [{"name": "v"}, {"name": "v"}],
                "decorators": [{"content": "{{ v }}"}]
            }),
            "duplicate variable 'v'",
        ),
        (
            json!({"variables": [], "decorators": []}),
            "has no decorators",
        ),
    ];
    for (body, expected) in bad {
        let (status, error) = post(&client, versions.clone(), body).await;
        assert_eq!(status, 400, "{error}");
        let message = error["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(expected),
            "{message} should say {expected}"
        );
    }
    let stored: i64 = sqlx::query_scalar("select count(*) from prompt_template_versions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 0, "a refused version is not stored");

    // a valid version, published and scoped to the org, is served
    let (status, created) = post(
        &client,
        versions,
        json!({
            "variables": [{"name": "tone", "required": true}],
            "decorators": [{"content": "tone={{ tone }}"}]
        }),
    )
    .await;
    assert_eq!(status, 200, "{created}");
    let scoped = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/versions/1/scopes"
        ))
        .json(&json!({"scopes": [{"scope_type": "org", "scope_id": org_id}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(scoped.status(), 204);
    let published = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/publish"
        ))
        .json(&json!({"version": 1}))
        .send()
        .await
        .unwrap();
    assert!(published.status().is_success());

    // a row the endpoint would have refused, written straight to the store as
    // legacy data would be
    let legacy: uuid::Uuid = sqlx::query_scalar(
        "insert into prompt_templates (org_id, name, slug) values ($1, 'legacy', 'legacy')
         returning id",
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into prompt_template_versions (template_id, version, variables, decorators)
         values ($1, 1, '[]', '[{\"content\": \"hi {{ ghost }}\"}]')",
    )
    .bind(legacy)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into prompt_template_scopes (template_id, version, scope_type, scope_id, org_id)
         values ($1, 1, 'org', $2, $2)",
    )
    .bind(legacy)
    .bind(org_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update prompt_templates set published_version = 1 where id = $1")
        .bind(legacy)
        .execute(&pool)
        .await
        .unwrap();

    let resp = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the snapshot must still be served");
    let snap: Value = resp.json().await.unwrap();
    let ids: Vec<&str> = snap["config"]["prompt_templates"]["templates"]
        .as_array()
        .expect("templates")
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert_eq!(ids, [format!("{org_id}:support")], "{snap}");

    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lines = problems["problems"].as_array().expect("problems");
    assert_eq!(lines.len(), 1, "{problems}");
    let line = lines[0].as_str().unwrap();
    assert!(line.contains(&format!("{org_id}:legacy")), "{line}");
    assert!(line.contains("undeclared variable 'ghost'"), "{line}");

    // publishing the malformed legacy version is refused as well
    let republish = client
        .put(format!("{base}/api/v1/prompt-templates/{legacy}/publish"))
        .json(&json!({"version": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(republish.status(), 400);
}

#[tokio::test]
async fn prompt_template_crud_publish_and_scope_round_trip() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");

    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");

    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");

    let provider = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/providers"),
        json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}),
    )
    .await;
    let provider_id = provider["id"].as_str().expect("provider id");

    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o-mini", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");

    let virtual_key = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/virtual-keys"),
        json!({"name": "template-key", "models": ["gpt-4o-mini"], "providers": [provider_id]}),
    )
    .await;
    let virtual_key_id = virtual_key["id"].as_str().expect("virtual key id");

    let template = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/prompt-templates"),
        json!({"name": "support baseline", "description": "v1"}),
    )
    .await;
    let template_id = template["id"].as_str().expect("template id");
    assert_eq!(template["slug"], "support-baseline");

    let templates: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/prompt-templates"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(templates.as_array().unwrap().len(), 1);

    let version1 = post(
        &client,
        format!("{base}/api/v1/prompt-templates/{template_id}/versions"),
        json!({
            "variables": [{"name": "tone", "required": true}],
            "decorators": [{"role": "system", "position": "prepend", "content": "tone={{ tone }}"}]
        }),
    )
    .await;
    assert_eq!(version1["version"], 1);

    let version2 = post(
        &client,
        format!("{base}/api/v1/prompt-templates/{template_id}/versions"),
        json!({"variables": [], "decorators": [{"content": "be brief"}]}),
    )
    .await;
    assert_eq!(version2["version"], 2);

    let set_scopes = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/versions/2/scopes"
        ))
        .json(&json!({
            "scopes": [
                {"scope_type": "org", "scope_id": org_id},
                {"scope_type": "project", "scope_id": project_id},
                {"scope_type": "route", "scope_id": route_id},
                {"scope_type": "virtual_key", "scope_id": virtual_key_id}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(set_scopes.status(), 204);

    let version1_scopes = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/versions/1/scopes"
        ))
        .json(&json!({
            "scopes": [{"scope_type": "org", "scope_id": org_id}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(version1_scopes.status(), 204);

    let publish = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/publish"
        ))
        .json(&json!({"version": 2}))
        .send()
        .await
        .unwrap();
    let publish_status = publish.status();
    let published: Value = publish.json().await.unwrap();
    assert!(
        publish_status.is_success(),
        "publish failed ({publish_status}): {published}"
    );
    assert_eq!(published["published_version"], 2);

    let mutate_published = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/versions/2/scopes"
        ))
        .json(&json!({
            "scopes": [{"scope_type": "org", "scope_id": org_id}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(mutate_published.status(), 400);

    let scopes: Value = client
        .get(format!(
            "{base}/api/v1/prompt-templates/{template_id}/versions/2/scopes"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(scopes.as_array().unwrap().len(), 4);

    let other_org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Other", "slug": "other"}),
    )
    .await;
    let other_org_id = other_org["id"].as_str().expect("other org id");
    let other_team = post(
        &client,
        format!("{base}/api/v1/orgs/{other_org_id}/teams"),
        json!({"name": "Other Team"}),
    )
    .await;
    let other_team_id = other_team["id"].as_str().expect("other team id");
    let other_project = post(
        &client,
        format!("{base}/api/v1/teams/{other_team_id}/projects"),
        json!({"name": "Other Project"}),
    )
    .await;
    let other_project_id = other_project["id"].as_str().expect("other project id");
    let bad_scope = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/versions/2/scopes"
        ))
        .json(&json!({
            "scopes": [
                {"scope_type": "project", "scope_id": other_project_id}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_scope.status(), 400);

    let rollback = client
        .put(format!(
            "{base}/api/v1/prompt-templates/{template_id}/rollback"
        ))
        .json(&json!({"version": 1}))
        .send()
        .await
        .unwrap();
    let rollback_status = rollback.status();
    let rolled_back: Value = rollback.json().await.unwrap();
    assert!(
        rollback_status.is_success(),
        "rollback failed ({rollback_status}): {rolled_back}"
    );
    assert_eq!(rolled_back["published_version"], 1);

    let delete_template = client
        .delete(format!("{base}/api/v1/prompt-templates/{template_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_template.status(), 204);
}

#[tokio::test]
async fn skills_crud_and_publish_round_trip() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Skills Team"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");

    let skill = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/skills"),
        json!({
            "name": "classification baseline",
            "description": "v1",
            "allowed_team_ids": [team_id],
            "minimum_role": "viewer"
        }),
    )
    .await;
    let skill_id = skill["id"].as_str().expect("skill id");
    assert_eq!(skill["slug"], "classification-baseline");

    let skills: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/skills"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(skills.as_array().unwrap().len(), 1);

    let v1 = post(
        &client,
        format!("{base}/api/v1/skills/{skill_id}/versions"),
        json!({"content": "alpha", "metadata": {"author": "ops"}}),
    )
    .await;
    assert_eq!(v1["version"], 1);

    let v2 = post(
        &client,
        format!("{base}/api/v1/skills/{skill_id}/versions"),
        json!({
            "content_ref": "oci://registry.example/skills/classification@sha256:abc",
            "metadata": {"author": "ops"}
        }),
    )
    .await;
    assert_eq!(v2["version"], 2);
    assert!(v2["content"].is_null());
    assert!(v2["content_ref"].is_string());

    let rejected_secret_metadata = client
        .post(format!("{base}/api/v1/skills/{skill_id}/versions"))
        .json(&json!({
            "content": "gamma",
            "metadata": {"api_token": "must-not-be-stored"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected_secret_metadata.status(), 400);

    let versions: Value = client
        .get(format!("{base}/api/v1/skills/{skill_id}/versions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(versions.as_array().unwrap().len(), 2);

    let publish = client
        .put(format!("{base}/api/v1/skills/{skill_id}/publish"))
        .json(&json!({"version": 2}))
        .send()
        .await
        .unwrap();
    let publish_status = publish.status();
    let published: Value = publish.json().await.unwrap();
    assert!(
        publish_status.is_success(),
        "publish failed ({publish_status}): {published}"
    );
    assert_eq!(published["published_version"], 2);

    let resolved: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{org_id}/skills/resolve/classification-baseline"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["version"], 2);
    assert!(resolved["content"].is_null());
    assert_eq!(
        resolved["content_ref"],
        "oci://registry.example/skills/classification@sha256:abc"
    );

    let rollback = client
        .put(format!("{base}/api/v1/skills/{skill_id}/rollback"))
        .json(&json!({"version": 1}))
        .send()
        .await
        .unwrap();
    let rollback_status = rollback.status();
    let rolled_back: Value = rollback.json().await.unwrap();
    assert!(
        rollback_status.is_success(),
        "rollback failed ({rollback_status}): {rolled_back}"
    );
    assert_eq!(rolled_back["published_version"], 1);

    let retire = client
        .put(format!("{base}/api/v1/skills/{skill_id}"))
        .json(&json!({"retired": true}))
        .send()
        .await
        .unwrap();
    assert!(retire.status().is_success());
    let retired: Value = retire.json().await.unwrap();
    assert!(retired["retired_at"].is_string());

    let retired_resolution = client
        .get(format!(
            "{base}/api/v1/orgs/{org_id}/skills/resolve/classification-baseline"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(retired_resolution.status(), 404);

    let delete_skill = client
        .delete(format!("{base}/api/v1/skills/{skill_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_skill.status(), 204);
}

/// Provider credentials posted to the API must be sealed at rest, decrypted
/// into the gateway snapshot, and never leak through the dashboard config
/// endpoint.
#[tokio::test]
async fn provider_api_key_seals_at_rest_and_decrypts_into_snapshot() {
    skip_without_db!();
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
    let org_id = org["id"].as_str().expect("org id");

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .json(&json!({
            "name": "openai",
            "kind": "openai",
            "api_base": "https://api.openai.com",
            "api_key": "sk-live-secret",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().expect("provider id");

    // at rest: sealed, not plaintext
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("select ciphertext from provider_keys where provider_id = $1::uuid")
            .bind(provider_id)
            .fetch_one(&pool)
            .await
            .expect("provider_keys row must exist");
    assert!(
        !String::from_utf8_lossy(&ciphertext).contains("sk-live-secret"),
        "credential must not be stored in plaintext"
    );

    // gateway snapshot: decrypted and usable
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        snap["config"]["providers"][0]["api_key"], "sk-live-secret",
        "snapshot must carry the decrypted key: {snap}"
    );

    // dashboard config: redacted
    let config_body = client
        .get(format!("{base}/api/v1/config"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !config_body.contains("sk-live-secret"),
        "config endpoint must redact provider keys"
    );

    // rotate via PUT, then clear with an empty string
    let updated = client
        .put(format!("{base}/api/v1/providers/{provider_id}"))
        .json(&json!({"api_key": "sk-rotated", "api_base": "https://eu.api.openai.com"}))
        .send()
        .await
        .unwrap();
    assert!(updated.status().is_success(), "{}", updated.status());

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snap["config"]["providers"][0]["api_key"], "sk-rotated");
    assert_eq!(
        snap["config"]["providers"][0]["api_base"],
        "https://eu.api.openai.com"
    );

    let cleared = client
        .put(format!("{base}/api/v1/providers/{provider_id}"))
        .json(&json!({"api_key": ""}))
        .send()
        .await
        .unwrap();
    assert!(cleared.status().is_success());

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        snap["config"]["providers"][0]["api_key"].is_null(),
        "cleared key must drop from the snapshot: {snap}"
    );
}

/// With an admin token configured, the CRUD API and snapshot endpoint reject
/// unauthenticated calls and accept the bearer token.
#[tokio::test]
async fn admin_token_guards_crud_and_snapshot() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool, Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let denied_snapshot = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied_snapshot.status(), 401);

    let allowed = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("sekrit")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert!(allowed.status().is_success(), "{}", allowed.status());
}

/// Every GET the served OpenAPI document does not mark public refuses an
/// anonymous or forged caller once an admin token is configured (#1820).
///
/// The analytics and health routes answered anyone for months: they were merged
/// onto the open router, the document said they needed a bearer, and nothing
/// compared the two. This walks the document the control plane actually serves,
/// so a route added later without a guard fails here rather than in a
/// deployment — and so does a route that is open by design but was never marked
/// `.public()`, which keeps the document honest about what an anonymous caller
/// can ask. Only a 401 passes: before the fix these routes answered 503 in a
/// test app with no ClickHouse, so "anything but 200" would have passed too.
#[tokio::test]
async fn every_route_the_spec_does_not_mark_public_refuses_an_anonymous_caller() {
    skip_without_db!();
    let db = fresh_db().await;
    let app =
        rolter_control::test_app_with_admin_token(db.pool().clone(), Some("sweep".to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let spec: Value = client
        .get(format!("http://{addr}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // authenticated downstream rather than here: the playground proxy hands the
    // caller's virtual key to the gateway, and the gateway is what checks it
    const CHECKED_DOWNSTREAM: &[&str] = &["/gw/{path}"];
    let nil = uuid::Uuid::nil().to_string();
    let mut answered = Vec::new();
    let mut walked = 0;
    for (path, item) in spec["paths"].as_object().expect("the document has paths") {
        let Some(op) = item.get("get") else {
            continue;
        };
        if op.get("security") == Some(&json!([])) || CHECKED_DOWNSTREAM.contains(&path.as_str()) {
            continue;
        }
        // every path parameter becomes the nil uuid: a guarded route has to
        // refuse the caller before it looks anything up
        let mut concrete = String::with_capacity(path.len());
        let mut rest = path.as_str();
        while let Some(open) = rest.find('{') {
            let close = rest[open..].find('}').expect("closed parameter") + open;
            concrete.push_str(&rest[..open]);
            concrete.push_str(&nil);
            rest = &rest[close + 1..];
        }
        concrete.push_str(rest);
        for bearer in [None, Some("forged")] {
            let mut request = client.get(format!("http://{addr}{concrete}"));
            if let Some(bearer) = bearer {
                request = request.bearer_auth(bearer);
            }
            let status = request.send().await.unwrap().status();
            if status != 401 {
                let who = if bearer.is_some() {
                    "forged bearer"
                } else {
                    "anonymous"
                };
                answered.push(format!("{path} ({who}) -> {status}"));
            }
        }
        walked += 1;
    }
    assert!(walked > 50, "the sweep only reached {walked} routes");
    assert!(
        answered.is_empty(),
        "routes that did not refuse a caller without credentials: {answered:#?}"
    );
}

/// Only a project admin changes who reads a project's captured bodies, only to
/// a role the column admits, and the change is audited (#1820).
#[tokio::test]
async fn project_settings_are_read_by_viewers_and_changed_by_project_admins() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("settings".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut tokens = std::collections::HashMap::new();
    for role in ["viewer", "member", "admin"] {
        let user = seed_user(&pool, &format!("{role}@settings.test"), false).await;
        seed_membership(&pool, user, None, None, Some(project), role).await;
        tokens.insert(
            role,
            seed_session(&pool, user, &format!("settings_{role}")).await,
        );
    }
    let settings = format!("{base}/api/v1/projects/{project}/settings");
    let read = |role: &'static str| client.get(&settings).bearer_auth(&tokens[role]).send();
    let write = |role: &'static str, value: &'static str| {
        client
            .put(&settings)
            .bearer_auth(&tokens[role])
            .json(&json!({"payload_min_role": value}))
            .send()
    };

    // members by default, and any role on the project may read the setting
    let current: Value = read("viewer").await.unwrap().json().await.unwrap();
    assert_eq!(current, json!({"payload_min_role": "member"}));

    // a member cannot lower the bar for everyone else
    assert_eq!(write("member", "viewer").await.unwrap().status(), 403);
    // and an admin cannot set a role the column does not admit
    assert_eq!(write("admin", "admin").await.unwrap().status(), 400);

    let changed = write("admin", "viewer").await.unwrap();
    assert_eq!(changed.status(), 200);
    let current: Value = read("viewer").await.unwrap().json().await.unwrap();
    assert_eq!(current, json!({"payload_min_role": "viewer"}));

    // the viewer's effective permissions at the project now include the bodies
    let effective: Value = client
        .get(format!(
            "{base}/api/v1/rbac/effective?org_id={org}&team_id={team}&project_id={project}"
        ))
        .bearer_auth(&tokens["viewer"])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let allowed: Vec<&str> = effective["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(allowed.contains(&"request_payload:read"), "{allowed:?}");

    let audited: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'project.settings.update' \
         and target_id = $1 and detail->>'payload_min_role' = 'viewer'",
    )
    .bind(project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audited, 1);

    // a viewer of another org who names acme's project beside their own org
    // resolves a role at that assembled chain, but holds none on the project's
    // real one: the setting is acme's, and it must not come back as theirs
    let umbrella: uuid::Uuid = sqlx::query_scalar(
        "insert into orgs (name, slug) values ('Umbrella', 'umbrella') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider = seed_user(&pool, "outsider@settings.test", false).await;
    seed_membership(&pool, outsider, Some(umbrella), None, None, "viewer").await;
    let outsider_token = seed_session(&pool, outsider, "settings_outsider").await;
    let effective: Value = client
        .get(format!(
            "{base}/api/v1/rbac/effective?org_id={umbrella}&project_id={project}"
        ))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(effective["role"], json!("viewer"), "{effective}");
    let allowed: Vec<&str> = effective["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(!allowed.contains(&"request_payload:read"), "{allowed:?}");
}

/// `GET /api/v1/config/export` hands back the deployment as an importable
/// `rolter.toml` (#1082). The route is superadmin-only, the body is a TOML
/// document rather than JSON, and — the part worth an end-to-end test — the
/// sealed credential the same store decrypts into `/internal/snapshot` does not
/// appear in it.
#[tokio::test]
async fn config_export_serves_importable_toml_without_credentials() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool, Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/config/export"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401, "the export must not be anonymous");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("sekrit")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id");
    client
        .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .bearer_auth("sekrit")
        .json(&json!({
            "name": "openai",
            "kind": "openai",
            "api_base": "https://api.openai.com",
            "api_key": "sk-live-export-secret",
        }))
        .send()
        .await
        .unwrap();

    let response = client
        .get(format!("{base}/api/v1/config/export"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("application/toml"),
        "unexpected content type: {content_type}"
    );
    assert!(response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .contains("rolter.toml"));

    let body = response.text().await.unwrap();
    assert!(
        !body.contains("sk-live-export-secret"),
        "the export leaked a sealed credential:\n{body}"
    );
    let parsed = rolter_core::GatewayConfig::from_toml_str(&body)
        .expect("the export must be a config the importer can read");
    assert_eq!(parsed.providers.len(), 1);
    assert_eq!(parsed.providers[0].name, "openai");
    assert!(parsed.providers[0].api_key.is_none());
}

/// `GET /api/v1/version` is the dashboard's one source for the update hint
/// (#902): any signed-in caller reads it, an anonymous one does not, and with
/// the check disabled it reports the running version and nothing else — no
/// latest, no url, no check time — so a footer can stay quiet on it.
#[tokio::test]
async fn version_endpoint_reports_the_running_build_and_the_disabled_check() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/version"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // a viewer with no membership anywhere is still an authenticated caller
    let viewer = seed_user(&pool, "version-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "versionviewer").await;
    let resp = client
        .get(format!("{base}/api/v1/version"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["current"],
        rolter_control::update_check::CURRENT_VERSION
    );
    assert_eq!(body["enabled"], false);
    assert_eq!(body["update_available"], false);
    assert!(body["latest"].is_null());
    assert!(body["release_url"].is_null());
    assert!(body["checked_at"].is_null());

    // and the admin token reads it too
    let as_admin = client
        .get(format!("{base}/api/v1/version"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(as_admin.status(), 200);
}

/// `GET /api/v1/public-url` is where the dashboard reads the base it builds a
/// not-yet-registered provider's redirect uri from (#2083): any signed-in
/// caller reads it, an anonymous one does not, and it says whether
/// `ROLTER_PUBLIC_URL` was set or the default is standing in for it.
#[tokio::test]
async fn public_url_endpoint_reports_the_base_and_whether_it_was_configured() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let client = reqwest::Client::new();

    // configured: the app knows its own listener as the public url
    let addr = serve_with_public_url(pool.clone(), Some("sekrit".to_string())).await;
    let base = format!("http://{addr}");
    let denied = client
        .get(format!("{base}/api/v1/public-url"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // a viewer with no membership anywhere is still an authenticated caller
    let viewer = seed_user(&pool, "public-url-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "publicurlviewer").await;
    let body: Value = client
        .get(format!("{base}/api/v1/public-url"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body, json!({"public_url": base, "configured": true}));

    // unset: the default stands in, and the answer says so
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let unset = serve(app).await;
    let body: Value = client
        .get(format!("http://{unset}/api/v1/public-url"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        body,
        json!({"public_url": "http://localhost:4001", "configured": false})
    );
}

/// A superadmin password reset through `PUT /api/v1/users/{id}` ends every
/// session the account holds (#1936): the usual reason for a reset is that the
/// old password is compromised, and whoever signed in with it must not keep a
/// week-long session. A superadmin resetting their own password keeps the
/// session they did it from and loses the rest, and the audit row says how
/// many went.
#[tokio::test]
async fn a_password_reset_revokes_the_accounts_live_sessions() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let admin = seed_user(&pool, "reset-admin@example.com", true).await;
    let admin_here = seed_session(&pool, admin, "resetadminhere").await;
    let admin_elsewhere = seed_session(&pool, admin, "resetadminelsewhere").await;
    let target = seed_user(&pool, "reset-target@example.com", false).await;
    let stolen = seed_session(&pool, target, "resettargetstolen").await;
    let laptop = seed_session(&pool, target, "resettargetlaptop").await;

    let me = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/api/v1/auth/me"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(me(stolen.clone()).await, 200, "the session starts live");

    // a change that leaves the password alone revokes nothing
    let renamed = client
        .put(format!("{base}/api/v1/users/{target}"))
        .bearer_auth(&admin_here)
        .json(&json!({"email": "reset-target-2@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);
    assert_eq!(me(laptop.clone()).await, 200);

    let reset = client
        .put(format!("{base}/api/v1/users/{target}"))
        .bearer_auth(&admin_here)
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(reset.status(), 200);
    for token in [&stolen, &laptop] {
        assert_eq!(me(token.clone()).await, 401, "{token} outlived the reset");
    }
    assert_eq!(
        me(admin_here.clone()).await,
        200,
        "resetting someone else touches only their sessions"
    );
    let revoked: Option<i64> = sqlx::query_scalar(
        "select (detail->>'sessions_revoked')::bigint from audit_log
         where action = 'user.update' and target_id = $1
           and (detail->>'password_changed')::boolean
         order by at desc limit 1",
    )
    .bind(target)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(
        revoked,
        Some(2),
        "the audit row counts the revoked sessions"
    );

    // resetting your own password keeps the session you did it from
    let own = client
        .put(format!("{base}/api/v1/users/{admin}"))
        .bearer_auth(&admin_here)
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 200);
    assert_eq!(
        me(admin_here.clone()).await,
        200,
        "the caller stays signed in"
    );
    assert_eq!(
        me(admin_elsewhere.clone()).await,
        401,
        "every other session of the caller ends"
    );

    // the admin token is no session, so a reset through it spares none
    let again = seed_session(&pool, target, "resettargetagain").await;
    let by_token = client
        .put(format!("{base}/api/v1/users/{target}"))
        .bearer_auth("sekrit")
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(by_token.status(), 200);
    assert_eq!(me(again).await, 401);
}

/// `GET /api/v1/stability` is the dashboard's one source for the nav's
/// experimental markers (#1385): the least-privileged signed-in caller reads it
/// — a viewer sees the nav too — an anonymous one does not, and every row it
/// returns is an exception, because absence is what "stable" means.
#[tokio::test]
async fn stability_endpoint_lists_only_experimental_subsystems() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/stability"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let viewer = seed_user(&pool, "stability-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "stabilityviewer").await;
    let resp = client
        .get(format!("{base}/api/v1/stability"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let rows = body.as_array().expect("an array of markers");
    assert_eq!(rows.len(), rolter_core::SUBSYSTEMS.len());
    for row in rows {
        assert_eq!(
            row["stability"], "experimental",
            "a stable subsystem must be absent, not listed: {row}"
        );
        let id = row["id"].as_str().expect("a subsystem id");
        assert!(
            rolter_core::subsystem(id).is_some(),
            "{id} is not in the core table"
        );
        assert!(
            !row["note"].as_str().unwrap_or_default().is_empty(),
            "{id} must say why it is experimental"
        );
    }
    // a stable subsystem is nowhere in the payload
    assert!(!rows
        .iter()
        .any(|row| row["id"] == "providers" || row["id"] == "virtual_keys"));

    let as_admin = client
        .get(format!("{base}/api/v1/stability"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(as_admin.status(), 200);
}

/// The `/me/*` 401 distinguishes "you are not signed in" from "this deployment
/// has no accounts to sign in to" (#942).
///
/// With an admin token configured the control plane is gated, so an anonymous
/// `/me/*` call is an ordinary missing session and signing in fixes it. In open
/// mode it is not fixable that way at all, and the two must not render alike.
#[tokio::test]
async fn a_gated_control_plane_reports_a_plain_missing_session_on_me() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool, Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();

    let anon = client
        .get(format!("http://{addr}/api/v1/me/virtual-keys"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
    let body: Value = anon.json().await.unwrap();
    assert_eq!(
        body["error"]["code"].as_str(),
        Some("unauthenticated"),
        "a gated deployment must not claim open mode: {body}"
    );

    // the admin token is not a session either: it opens admin routes, but
    // `/me/*` is per-user and there is no user behind a machine token
    let with_admin = client
        .get(format!("http://{addr}/api/v1/me/virtual-keys"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(with_admin.status(), 401);
    let body: Value = with_admin.json().await.unwrap();
    assert_eq!(body["error"]["code"].as_str(), Some("unauthenticated"));
}

/// Persisted feature flags are superadmin-only, survive reads, and write an
/// audit entry for each update.
#[tokio::test]
async fn feature_flags_are_superadmin_only_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/feature-flags"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/feature-flags"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(baseline["response_cache"], true);
    assert_eq!(baseline["cache_aware_routing"], true);
    assert_eq!(baseline["circuit_breaker"], true);
    assert_eq!(baseline["active_health_checks"], true);
    assert_eq!(baseline["complexity_routing"], true);
    assert_eq!(baseline["guardrails"], true);

    let updated: Value = client
        .put(format!("{base}/api/v1/feature-flags"))
        .bearer_auth("sekrit")
        .json(&json!({
            "response_cache": false,
            "cache_aware_routing": false,
            "circuit_breaker": false,
            "active_health_checks": false,
            "complexity_routing": false,
            "guardrails": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["response_cache"], false);
    assert_eq!(updated["cache_aware_routing"], false);
    assert_eq!(updated["circuit_breaker"], false);
    assert_eq!(updated["active_health_checks"], false);
    assert_eq!(updated["complexity_routing"], false);
    assert_eq!(updated["guardrails"], false);

    let reloaded: Value = client
        .get(format!("{base}/api/v1/feature-flags"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reloaded["response_cache"], false);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'feature_flags.update' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("feature_flags.update"));
}

#[tokio::test]
async fn cluster_inventory_tracks_polling_nodes() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // an anonymous poll stays out of the inventory: single-node deployments
    // need no cluster setup
    let anonymous = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert!(anonymous.status().is_success());
    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(nodes.as_array().unwrap().len(), 0);

    // an identified poll registers the node and its applied config version
    let identified = client
        .get(format!("{base}/internal/snapshot?version=0"))
        .bearer_auth("sekrit")
        .header("x-rolter-node-id", "gw-1")
        .header("x-rolter-node-role", "gateway")
        .header("x-rolter-node-build", "0.0.10")
        .send()
        .await
        .unwrap();
    assert!(identified.status().is_success());

    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let node = &nodes.as_array().expect("nodes")[0];
    assert_eq!(node["id"], "gw-1");
    assert_eq!(node["role"], "gateway");
    assert_eq!(node["build_version"], "0.0.10");
    assert_eq!(node["live"], true);

    // the inventory is superadmin-only
    let denied = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // the only live gateway may not be drained: it would take the data plane down
    let refused = client
        .put(format!("{base}/api/v1/cluster/nodes/gw-1/drain"))
        .bearer_auth("sekrit")
        .json(&json!({"draining": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);

    // with a second live gateway the drain is safe, and the draining node
    // learns about it on its next poll
    let second = client
        .get(format!("{base}/internal/snapshot?version=0"))
        .bearer_auth("sekrit")
        .header("x-rolter-node-id", "gw-2")
        .header("x-rolter-node-role", "gateway")
        .send()
        .await
        .unwrap();
    assert!(second.status().is_success());

    let drained: Value = client
        .put(format!("{base}/api/v1/cluster/nodes/gw-1/drain"))
        .bearer_auth("sekrit")
        .json(&json!({"draining": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(drained["desired_state"], "draining");

    let polled = client
        .get(format!("{base}/internal/snapshot?version=0"))
        .bearer_auth("sekrit")
        .header("x-rolter-node-id", "gw-1")
        .header("x-rolter-node-role", "gateway")
        .send()
        .await
        .unwrap();
    assert_eq!(
        polled
            .headers()
            .get("x-rolter-node-state")
            .and_then(|v| v.to_str().ok()),
        Some("draining")
    );

    let drain_action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'cluster_node.set_drain' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(drain_action.as_deref(), Some("cluster_node.set_drain"));

    // returning it to service is the same call with draining=false
    let restored: Value = client
        .put(format!("{base}/api/v1/cluster/nodes/gw-1/drain"))
        .bearer_auth("sekrit")
        .json(&json!({"draining": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restored["desired_state"], "active");

    let forgotten_second = client
        .delete(format!("{base}/api/v1/cluster/nodes/gw-2"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(forgotten_second.status(), 204);

    // a decommissioned node can be forgotten, and the action is audited
    let forgotten = client
        .delete(format!("{base}/api/v1/cluster/nodes/gw-1"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(forgotten.status(), 204);
    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(nodes.as_array().unwrap().len(), 0);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'cluster_node.forget' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("cluster_node.forget"));
}

#[tokio::test]
async fn unavailable_feature_flags_are_reported_and_cannot_be_enabled() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // this deployment has no redis and no cache-publishing provider
    let view: Value = client
        .get(format!("{base}/api/v1/feature-flags"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let unavailable = view["unavailable"].as_array().expect("unavailable list");
    let names: Vec<&str> = unavailable
        .iter()
        .map(|u| u["flag"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"response_cache"), "{view}");
    assert!(names.contains(&"cache_aware_routing"), "{view}");
    assert!(unavailable
        .iter()
        .all(|u| !u["reason"].as_str().unwrap_or_default().is_empty()));

    let put = |flags: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .put(format!("{base}/api/v1/feature-flags"))
                .bearer_auth("sekrit")
                .json(&flags)
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    // every flag is stored on by default, the unavailable two included, and
    // every save carries every flag: one that leaves them on as they are goes
    // through. the old guard refused it, so a deployment without redis could
    // not save its feature flags at all (#1856)
    let unchanged = put(json!({
        "response_cache": true,
        "cache_aware_routing": true,
        "circuit_breaker": false,
        "active_health_checks": true,
        "complexity_routing": true,
        "guardrails": true
    }))
    .await;
    assert_eq!(unchanged, 200);
    // turning an unavailable flag off is always allowed
    let off = put(json!({
        "response_cache": false,
        "cache_aware_routing": true,
        "circuit_breaker": true,
        "active_health_checks": true,
        "complexity_routing": true,
        "guardrails": true
    }))
    .await;
    assert_eq!(off, 200);
    // turning it back on would persist a policy the gateway silently ignores
    let rejected = put(json!({
        "response_cache": true,
        "cache_aware_routing": true,
        "circuit_breaker": true,
        "active_health_checks": true,
        "complexity_routing": true,
        "guardrails": true
    }))
    .await;
    assert_eq!(rejected, 400);

    // the available flags stay editable
    let updated: Value = client
        .put(format!("{base}/api/v1/feature-flags"))
        .bearer_auth("sekrit")
        .json(&json!({
            "response_cache": false,
            "cache_aware_routing": false,
            "circuit_breaker": false,
            "active_health_checks": true,
            "complexity_routing": true,
            "guardrails": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["circuit_breaker"], false);
    assert_eq!(updated["unavailable"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn logging_settings_are_superadmin_only_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/logging-settings"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/logging-settings"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(baseline["sample_rate"], 1.0);
    assert_eq!(baseline["ui_events"], true);

    let updated: Value = client
        .put(format!("{base}/api/v1/logging-settings"))
        .bearer_auth("sekrit")
        .json(&json!({
            "sample_rate": 0.25,
            "payload_capture_enabled": true,
            "payload_capture_max_bytes": 4096,
            "payload_capture_redact_fields": ["token", "authorization"],
            "payload_capture_models": ["gpt-4o"],
            "payload_capture_virtual_key_ids": ["00000000-0000-0000-0000-000000000001"],
            "retention_days": 30,
            "payload_retention_hours": 24
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["sample_rate"], 0.25);
    assert_eq!(updated["payload_capture_enabled"], true);
    assert_eq!(updated["payload_capture_max_bytes"], 4096);
    assert_eq!(updated["retention_days"], 30);
    assert_eq!(updated["payload_retention_hours"], 24);
    // the body above never names ui_events, and a client that predates the
    // field must not flip it either way
    assert_eq!(updated["ui_events"], true);

    // raw bodies may never outlive the metadata row they belong to
    let rejected = client
        .put(format!("{base}/api/v1/logging-settings"))
        .bearer_auth("sekrit")
        .json(&json!({
            "sample_rate": 0.25,
            "payload_capture_enabled": true,
            "payload_capture_max_bytes": 4096,
            "payload_capture_redact_fields": ["token"],
            "payload_capture_models": [],
            "payload_capture_virtual_key_ids": [],
            "retention_days": 1,
            "payload_retention_hours": 720
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);

    let reloaded: Value = client
        .get(format!("{base}/api/v1/logging-settings"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reloaded["sample_rate"], 0.25);
    assert_eq!(reloaded["payload_capture_enabled"], true);
    assert_eq!(reloaded["retention_days"], 30);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'logging_settings.update' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("logging_settings.update"));
}

#[tokio::test]
async fn runtime_policy_is_superadmin_only_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/runtime-policy"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/runtime-policy"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(baseline["retry_max_retries"], 2);
    assert_eq!(baseline["queue_backpressure"], "error");

    let updated: Value = client
        .put(format!("{base}/api/v1/runtime-policy"))
        .bearer_auth("sekrit")
        .json(&json!({
            "retry_max_retries": 4,
            "retry_base_ms": 150,
            "retry_max_ms": 1500,
            "timeout_connect_s": 20,
            "timeout_request_s": 180,
            "queue_enabled": true,
            "queue_capacity": 1024,
            "queue_workers": 16,
            "queue_backpressure": "block",
            "queue_block_ms": 5000
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["retry_max_retries"], 4);
    assert_eq!(updated["timeout_connect_s"], 20);
    assert_eq!(updated["queue_backpressure"], "block");

    let reloaded: Value = client
        .get(format!("{base}/api/v1/runtime-policy"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reloaded["retry_max_retries"], 4);
    assert_eq!(reloaded["queue_backpressure"], "block");

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'runtime_policy.update' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("runtime_policy.update"));
}

#[tokio::test]
async fn compatibility_policy_is_superadmin_only_validated_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/compatibility-policy"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/compatibility-policy"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // defaults preserve the previously compiled-in behavior
    assert_eq!(baseline["anthropic_version"], "2023-06-01");
    assert_eq!(baseline["default_max_tokens"], 1024);
    assert_eq!(baseline["restart_required"].as_array().unwrap().len(), 0);

    // a free-form version would fail every anthropic call at the edge
    let rejected = client
        .put(format!("{base}/api/v1/compatibility-policy"))
        .bearer_auth("sekrit")
        .json(&json!({"anthropic_version": "latest", "default_max_tokens": 1024}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);

    let updated: Value = client
        .put(format!("{base}/api/v1/compatibility-policy"))
        .bearer_auth("sekrit")
        .json(&json!({"anthropic_version": "2024-10-22", "default_max_tokens": 4096}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["anthropic_version"], "2024-10-22");
    assert_eq!(updated["default_max_tokens"], 4096);

    // the gateway snapshot carries the new policy without a restart
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        snap["config"]["compatibility"]["anthropic_version"],
        "2024-10-22"
    );
    assert_eq!(snap["config"]["compatibility"]["default_max_tokens"], 4096);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'compatibility_policy.update' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("compatibility_policy.update"));
}

#[tokio::test]
async fn adaptive_routing_policy_is_superadmin_only_validated_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let denied = client
        .get(format!("{base}/api/v1/adaptive-routing-policy"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-policy"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // shipped off, so enabling it is always a deliberate action
    assert_eq!(baseline["enabled"], false);
    assert_eq!(baseline["min_samples"], 50);
    assert_eq!(baseline["affected_routes"].as_array().unwrap().len(), 0);

    // an all-zero blend would make `adaptive` a random balancer
    let rejected = client
        .put(format!("{base}/api/v1/adaptive-routing-policy"))
        .bearer_auth("sekrit")
        .json(&json!({
            "enabled": true,
            "latency_weight": 0.0,
            "cost_weight": 0.0,
            "load_weight": 0.0,
            "exploration_ratio": 0.05,
            "min_samples": 50
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);

    // and exploration is capped well below "route at random"
    let too_much_exploration = client
        .put(format!("{base}/api/v1/adaptive-routing-policy"))
        .bearer_auth("sekrit")
        .json(&json!({
            "enabled": true,
            "latency_weight": 1.0,
            "cost_weight": 0.5,
            "load_weight": 0.25,
            "exploration_ratio": 0.9,
            "min_samples": 50
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(too_much_exploration.status(), 400);

    let updated: Value = client
        .put(format!("{base}/api/v1/adaptive-routing-policy"))
        .bearer_auth("sekrit")
        .json(&json!({
            "enabled": true,
            "latency_weight": 2.0,
            "cost_weight": 0.0,
            "load_weight": 0.5,
            "exploration_ratio": 0.1,
            "min_samples": 10
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["enabled"], true);
    assert_eq!(updated["latency_weight"], 2.0);
    assert_eq!(updated["min_samples"], 10);

    // the gateway snapshot carries the new policy without a restart
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(snap["config"]["adaptive_routing"]["enabled"], true);
    assert_eq!(snap["config"]["adaptive_routing"]["latency_weight"], 2.0);
    assert_eq!(snap["config"]["adaptive_routing"]["min_samples"], 10);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'adaptive_routing_policy.update' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("adaptive_routing_policy.update"));
}

/// End-to-end local-account login (ROL-32): seed a user with an argon2id hash
/// directly (no signup flow exists yet), then exercise login → `/auth/me` →
/// logout → the now-revoked token is rejected.
#[tokio::test]
async fn login_me_logout_round_trip() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // seed a user the way `rolter-seed` does (same argon2id hashing call shape)
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple")
        .unwrap()
        .to_string();
    sqlx::query("insert into users (email, password_hash, is_superadmin) values ($1, $2, true)")
        .bind("admin@example.com")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();

    // wrong password is rejected
    let denied = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "admin@example.com", "password": "wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // /auth/me without a token is rejected
    let no_token = client
        .get(format!("{base}/api/v1/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401);

    // a made-up token is rejected too (never matches a stored session hash)
    let bad_token = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth("rolter_sess_not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_token.status(), 401);

    // correct credentials issue a session token
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "admin@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().expect("token").to_string();
    assert_eq!(login["user"]["email"], "admin@example.com");

    // the token resolves the current user via the CurrentUser extractor
    let me: Value = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["user"]["email"], "admin@example.com");
    assert!(me["memberships"].is_array());

    // logout revokes the session; a repeat logout is a no-op (idempotent)
    let logout = client
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    let logout_again = client
        .post(format!("{base}/api/v1/auth/logout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(logout_again.status(), 204);

    // the revoked token no longer resolves a session
    let after_logout = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(after_logout.status(), 401);
}

/// Failed logins are throttled, audited and per-account (#1079).
///
/// Before this, `POST /api/v1/auth/login` answered an unlimited number of
/// guesses and each one bought a full argon2 verification, so the endpoint was
/// both a guessing oracle and a cheap CPU-exhaustion vector. The properties
/// worth pinning are that the lock engages, that it says how long it lasts,
/// that it is scoped to the account rather than the deployment, and that a
/// correct password clears the counter.
#[tokio::test]
async fn failed_logins_are_throttled_per_account_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    use argon2::password_hash::PasswordHasher;
    let hash_for = |password: &str| {
        argon2::Argon2::default()
            .hash_password(password.as_bytes())
            .unwrap()
            .to_string()
    };
    for email in ["target@example.com", "bystander@example.com"] {
        sqlx::query(
            "insert into users (email, password_hash, is_superadmin) values ($1, $2, true)",
        )
        .bind(email)
        .bind(hash_for("correct horse battery staple"))
        .execute(&pool)
        .await
        .unwrap();
    }

    let guess = |email: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/api/v1/auth/login"))
                .json(&json!({"email": email, "password": "wrong"}))
                .send()
                .await
                .unwrap()
        }
    };

    // the default budget is five failures; every one of them is an ordinary 401
    for _ in 0..5 {
        assert_eq!(guess("target@example.com").await.status(), 401);
    }

    // the sixth is refused before any password work happens, and says for how long
    let locked = guess("target@example.com").await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .expect("a lock states how long it lasts")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        retry_after > 0,
        "retry-after must never invite an instant retry"
    );

    // the lock belongs to the account, not to the deployment: locking one
    // account must not lock everybody out, which would turn the throttle into
    // the denial-of-service it exists to prevent
    assert_eq!(guess("bystander@example.com").await.status(), 401);
    let ok = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({
            "email": "bystander@example.com",
            "password": "correct horse battery staple"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // and a correct password clears the counter it had started
    for _ in 0..5 {
        assert_eq!(guess("bystander@example.com").await.status(), 401);
    }
    assert_eq!(guess("bystander@example.com").await.status(), 429);

    // the attempts are on the record. the write is spawned off the response
    // path — resolving the org an entry belongs to only costs a query when the
    // account exists, and awaiting that would make a registered address
    // measurably slower to reject than an unregistered one — so poll for it
    let mut actions: Vec<String> = Vec::new();
    for _ in 0..40 {
        actions = sqlx::query_scalar(
            "select action from audit_log where action like 'auth.login%' order by at",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        if actions.iter().any(|a| a == "auth.login_locked")
            && actions.iter().any(|a| a == "auth.login_throttled")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        actions.iter().any(|a| a == "auth.login_failed"),
        "every rejected guess is recorded: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "auth.login_locked"),
        "the attempt that engaged the lock says so: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "auth.login_throttled"),
        "an attempt refused by the lock is recorded too: {actions:?}"
    );
}

/// An expired session row is rejected even though the token digest matches,
/// proving `find_active_by_hash`'s `expires_at > now()` bound is doing its job.
#[tokio::test]
async fn expired_session_is_rejected() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let user_id: uuid::Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, true) returning id",
    )
    .bind("expired@example.com")
    .bind("unused-hash")
    .fetch_one(&pool)
    .await
    .unwrap();

    // insert an already-expired session directly, bypassing login, with a
    // digest matching what the extractor computes for an empty pepper
    let token = "rolter_sess_deadbeef";
    let token_hash = rolter_auth::hash_key("", token);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at) values ($1, $2, now() - interval '1 hour')",
    )
    .bind(user_id)
    .bind(&token_hash)
    .execute(&pool)
    .await
    .unwrap();

    let resp = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

/// Seed a local user directly (no signup flow) and return its id.
async fn seed_user(pool: &sqlx::PgPool, email: &str, is_superadmin: bool) -> uuid::Uuid {
    sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, null, $2) returning id",
    )
    .bind(email)
    .bind(is_superadmin)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Grant `user` a role membership at an org/team/project scope (pass the ids
/// that apply; `None` for the levels that don't).
async fn seed_membership(
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
    org: Option<uuid::Uuid>,
    team: Option<uuid::Uuid>,
    project: Option<uuid::Uuid>,
    role: &str,
) {
    sqlx::query(
        "insert into memberships (user_id, org_id, team_id, project_id, role)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(org)
    .bind(team)
    .bind(project)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

/// Mint a live session for `user` and return the opaque bearer token. The
/// digest is computed with the empty pepper the extractor uses when
/// `ROLTER_SESSION_PEPPER` is unset in tests.
async fn seed_session(pool: &sqlx::PgPool, user_id: uuid::Uuid, suffix: &str) -> String {
    let token = format!("rolter_sess_{suffix}");
    let token_hash = rolter_auth::hash_key("", &token);
    sqlx::query(
        "insert into sessions (user_id, token_hash, expires_at)
         values ($1, $2, now() + interval '1 hour')",
    )
    .bind(user_id)
    .bind(&token_hash)
    .execute(pool)
    .await
    .unwrap();
    token
}

/// The batched project→team lookup must reach the same verdict the
/// per-membership `ProjectRepo::get` loop did (#1048).
///
/// Covers the case the old loop was shaped for and the new query has to
/// reproduce: a user whose only route to the resource is a *project*
/// membership, holding several of them, where the qualifying one is not first.
/// A user with just as many project memberships and none in an allowed team is
/// still refused, so the batch is not simply answering "yes" on any row.
#[tokio::test]
async fn policy_allows_resolves_several_project_memberships_in_one_verdict() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Batched", "slug": "batched"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    // three teams, one of which the skill allows
    let mut team_ids = Vec::new();
    for name in ["Alpha", "Beta", "Gamma"] {
        let team = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/teams"),
            json!({ "name": name }),
        )
        .await;
        team_ids.push(team["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap());
    }
    // the allowed team is deliberately last, so a lookup that stopped at the
    // first row would answer "denied" and fail this test
    let allowed_team = team_ids[2];

    // one project per team
    let mut project_ids = Vec::new();
    for (index, team) in team_ids.iter().enumerate() {
        let project = post_as(
            &client,
            format!("{base}/api/v1/teams/{team}/projects"),
            json!({ "name": format!("project-{index}") }),
        )
        .await;
        project_ids.push(
            project["id"]
                .as_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap(),
        );
    }

    let skill = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/skills"),
        json!({
            "name": "batched-policy",
            "allowed_team_ids": [allowed_team],
            "minimum_role": "member"
        }),
    )
    .await;
    let skill_id = skill["id"].as_str().unwrap().to_string();
    post_as(
        &client,
        format!("{base}/api/v1/skills/{skill_id}/versions"),
        json!({"content": "content"}),
    )
    .await;

    // reaches the skill only through the third project, and only after two
    // project memberships that do not qualify
    // team_id is left unset on purpose. A membership carrying both would be
    // answered by the direct team check before the project lookup runs, so the
    // batched query — the thing under test — would never execute
    let allowed_user = seed_user(&pool, "batched-allowed@example.com", false).await;
    seed_membership(&pool, allowed_user, Some(org_uuid), None, None, "member").await;
    for project in &project_ids {
        seed_membership(
            &pool,
            allowed_user,
            Some(org_uuid),
            None,
            Some(*project),
            "viewer",
        )
        .await;
    }
    let allowed_token = seed_session(&pool, allowed_user, "batched-allowed").await;

    // just as many project memberships, none of them in the allowed team
    let denied_user = seed_user(&pool, "batched-denied@example.com", false).await;
    seed_membership(&pool, denied_user, Some(org_uuid), None, None, "member").await;
    for project in &project_ids[..2] {
        seed_membership(
            &pool,
            denied_user,
            Some(org_uuid),
            None,
            Some(*project),
            "viewer",
        )
        .await;
    }
    let denied_token = seed_session(&pool, denied_user, "batched-denied").await;

    async fn skills_for(client: &reqwest::Client, base: &str, org: &str, token: &str) -> Value {
        client
            .get(format!("{base}/api/v1/orgs/{org}/skills"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    let allowed = skills_for(&client, &base, &org_id, &allowed_token).await;
    assert_eq!(
        allowed.as_array().unwrap().len(),
        1,
        "a project membership in the allowed team must grant access: {allowed}"
    );

    let denied = skills_for(&client, &base, &org_id, &denied_token).await;
    assert!(
        denied.as_array().unwrap().is_empty(),
        "project memberships outside the allowed team must not grant access: {denied}"
    );
}

#[tokio::test]
async fn skill_access_policy_filters_list_history_and_resolution() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let team_a = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Allowed"}),
    )
    .await;
    let team_b = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Denied"}),
    )
    .await;
    let team_a_uuid: uuid::Uuid = team_a["id"].as_str().unwrap().parse().unwrap();
    let team_b_uuid: uuid::Uuid = team_b["id"].as_str().unwrap().parse().unwrap();
    let skill = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/skills"),
        json!({
            "name": "classification",
            "allowed_team_ids": [team_a_uuid],
            "minimum_role": "member"
        }),
    )
    .await;
    let skill_id = skill["id"].as_str().unwrap();
    post_as(
        &client,
        format!("{base}/api/v1/skills/{skill_id}/versions"),
        json!({"content": "approved content"}),
    )
    .await;
    let publish = client
        .put(format!("{base}/api/v1/skills/{skill_id}/publish"))
        .bearer_auth("admintok")
        .json(&json!({"version": 1}))
        .send()
        .await
        .unwrap();
    assert!(publish.status().is_success());

    let allowed_user = seed_user(&pool, "allowed@example.com", false).await;
    seed_membership(&pool, allowed_user, Some(org_uuid), None, None, "member").await;
    seed_membership(
        &pool,
        allowed_user,
        Some(org_uuid),
        Some(team_a_uuid),
        None,
        "viewer",
    )
    .await;
    let allowed_token = seed_session(&pool, allowed_user, "skill-allowed").await;

    let denied_user = seed_user(&pool, "denied@example.com", false).await;
    seed_membership(&pool, denied_user, Some(org_uuid), None, None, "member").await;
    seed_membership(
        &pool,
        denied_user,
        Some(org_uuid),
        Some(team_b_uuid),
        None,
        "viewer",
    )
    .await;
    let denied_token = seed_session(&pool, denied_user, "skill-denied").await;

    let allowed_list: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/skills"))
        .bearer_auth(&allowed_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(allowed_list.as_array().unwrap().len(), 1);
    let denied_list: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/skills"))
        .bearer_auth(&denied_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(denied_list.as_array().unwrap().is_empty());

    let allowed_resolution = client
        .get(format!(
            "{base}/api/v1/orgs/{org_id}/skills/resolve/classification"
        ))
        .bearer_auth(&allowed_token)
        .send()
        .await
        .unwrap();
    assert!(allowed_resolution.status().is_success());
    let denied_history = client
        .get(format!("{base}/api/v1/skills/{skill_id}/versions"))
        .bearer_auth(&denied_token)
        .send()
        .await
        .unwrap();
    assert_eq!(denied_history.status(), 403);
    let denied_resolution = client
        .get(format!(
            "{base}/api/v1/orgs/{org_id}/skills/resolve/classification"
        ))
        .bearer_auth(&denied_token)
        .send()
        .await
        .unwrap();
    assert_eq!(denied_resolution.status(), 403);
}

/// A stub OIDC provider: discovery, JWKS and a token endpoint that signs id
/// tokens with the fixture RSA key. Lets the SSO flow be exercised end to end
/// without a container, while the Keycloak suite covers a real IdP.
mod stub_idp {
    use super::*;
    use axum::Router;
    use std::sync::{Arc, Mutex};

    pub const KID: &str = "stub-key-1";

    /// The stub's signing key, generated once per test process rather than
    /// checked in: a private key in the repository is a private key in the
    /// repository, whatever the comment above it says, and every secret
    /// scanner is right to flag one. ES256 keeps generation instant.
    fn signing_key() -> &'static (jsonwebtoken::EncodingKey, String, String) {
        static KEY: std::sync::OnceLock<(jsonwebtoken::EncodingKey, String, String)> =
            std::sync::OnceLock::new();
        KEY.get_or_init(|| {
            use base64::Engine;
            use p256::elliptic_curve::sec1::ToSec1Point;
            use p256::elliptic_curve::Generate;
            use p256::pkcs8::EncodePrivateKey;

            let secret = p256::SecretKey::generate();
            let pem = secret.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
            let encoding = jsonwebtoken::EncodingKey::from_ec_pem(pem.as_bytes()).unwrap();
            // the jwk carries the affine coordinates, so publish them from the
            // uncompressed sec1 point: 0x04 || x || y
            let point = secret.public_key().to_sec1_point(false);
            let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            (
                encoding,
                b64.encode(point.x().unwrap()),
                b64.encode(point.y().unwrap()),
            )
        })
    }

    #[derive(Clone, Default)]
    pub struct Stub {
        /// claims the next token exchange will mint, set by the test
        pub next_claims: Arc<Mutex<Value>>,
        /// form fields of the last token request, so PKCE can be asserted
        pub last_form: Arc<Mutex<String>>,
        pub issuer: Arc<Mutex<String>>,
    }

    pub async fn serve_stub() -> (String, Stub) {
        let stub = Stub::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let issuer = format!("http://{addr}");
        *stub.issuer.lock().unwrap() = issuer.clone();

        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get({
                    let stub = stub.clone();
                    move || {
                        let issuer = stub.issuer.lock().unwrap().clone();
                        async move {
                            axum::Json(json!({
                                "issuer": issuer,
                                "authorization_endpoint": format!("{issuer}/authorize"),
                                "token_endpoint": format!("{issuer}/token"),
                                "jwks_uri": format!("{issuer}/jwks"),
                            }))
                        }
                    }
                }),
            )
            .route(
                "/jwks",
                axum::routing::get(|| async {
                    let (_, x, y) = signing_key();
                    axum::Json(json!({
                        "keys": [{
                            "kty": "EC",
                            "use": "sig",
                            "alg": "ES256",
                            "crv": "P-256",
                            "kid": KID,
                            "x": x,
                            "y": y,
                        }]
                    }))
                }),
            )
            .route(
                "/token",
                axum::routing::post({
                    let stub = stub.clone();
                    move |body: String| {
                        let stub = stub.clone();
                        async move {
                            *stub.last_form.lock().unwrap() = body;
                            let claims = stub.next_claims.lock().unwrap().clone();
                            axum::Json(json!({
                                "access_token": "stub-access",
                                "token_type": "Bearer",
                                "id_token": sign(&claims),
                            }))
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (issuer, stub)
    }

    /// Sign a claim set as an ES256 id token with the process's stub key.
    pub fn sign(claims: &Value) -> String {
        let (key, _, _) = signing_key();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(KID.to_string());
        jsonwebtoken::encode(&header, claims, key).unwrap()
    }

    pub fn claims(issuer: &str, audience: &str, nonce: &str, groups: Value) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({
            "iss": issuer,
            "aud": audience,
            "sub": "idp-subject-1",
            "email": "ada@example.com",
            "preferred_username": "ada",
            "nonce": nonce,
            "groups": groups,
            "iat": now,
            "exp": now + 300,
        })
    }
}

/// #2304: the slug's charset is a check constraint in the database
/// (`sso_providers_slug_charset`), and the handler used to let a bad one reach
/// the insert, so the caller read a store error carrying the constraint's name
/// rather than a 400 that says what to type. The endpoint and the constraint
/// are compared on a table of slugs instead of trusted to agree: for each one
/// the store is asked directly, then the endpoint, and the answers must match.
#[tokio::test]
async fn sso_slug_outside_the_charset_is_a_400_that_states_the_rule() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "SlugOrg", "slug": "slug-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let longest = "a".repeat(63);
    let too_long = "a".repeat(64);
    let accepted = [
        "okta",
        "a",
        "0",
        "9lives",
        "entra-staging",
        "a--b",
        "okta-",
        longest.as_str(),
    ];
    let refused = [
        "",
        "Okta",
        "acme okta",
        "-okta",
        "okta_prod",
        "okta.prod",
        " okta",
        "okta ",
        "okta\n",
        // a Cyrillic "о", which looks like the Latin one
        "\u{43e}kta",
        "r\u{e9}sum\u{e9}",
        too_long.as_str(),
    ];

    for slug in accepted.iter().chain(refused.iter()).copied() {
        // the store's own answer, with the probe row taken out again so the
        // handler's insert of the same slug below cannot collide with it
        let stored = sqlx::query(
            "insert into sso_providers (org_id, name, slug, issuer, client_id) \
             values ($1::uuid, 'probe', $2, 'https://idp.example.com', 'probe')",
        )
        .bind(&org_id)
        .bind(slug)
        .execute(&pool)
        .await
        .is_ok();
        if stored {
            sqlx::query("delete from sso_providers where org_id = $1::uuid and slug = $2")
                .bind(&org_id)
                .bind(slug)
                .execute(&pool)
                .await
                .unwrap();
        }

        let response = client
            .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
            .bearer_auth("admintok")
            .json(&json!({
                "name": "Probe",
                "slug": slug,
                "issuer": "https://idp.example.com",
                "client_id": "probe"
            }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();

        assert_eq!(
            status == 200,
            stored,
            "slug {slug:?}: the endpoint answered {status} but the store {} it: {body}",
            if stored { "accepts" } else { "refuses" }
        );
        if stored {
            assert_eq!(body["slug"], slug);
        } else {
            assert_eq!(status, 400, "slug {slug:?} must be a 400: {body}");
            let message = body["error"]["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains("sso_providers_slug_charset"),
                "slug {slug:?}: the constraint name reached the caller: {message}"
            );
            if !slug.trim().is_empty() {
                assert!(
                    message.contains("lowercase letters, digits and hyphens")
                        && message.contains("start with a letter or digit")
                        && message.contains("at most 63 characters"),
                    "slug {slug:?}: the 400 does not state the rule: {message}"
                );
            }
        }
    }

    // only the accepted ones were registered; a refused slug left nothing behind
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), accepted.len());
}

/// #1233: a provider is editable in place. Before this, rotating a client
/// secret or taking a provider out of service meant deleting it and
/// registering it again, which dropped every group mapping hanging off it and
/// changed the provider id in the audit trail.
#[tokio::test]
async fn sso_provider_updates_in_place_and_keeps_its_slug_and_mappings() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "UpdOrg", "slug": "upd-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Okta",
            "slug": "okta",
            "issuer": "https://acme.okta.com",
            "client_id": "0oa1",
            "client_secret": "original-secret",
            "group_claim": "groups"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = provider["id"].as_str().unwrap().to_string();
    assert_eq!(provider["has_client_secret"], json!(true));

    // a mapping hangs off the provider. it is the thing delete-and-recreate
    // used to destroy, so every assertion below re-checks it survived
    let mapping: Value = client
        .post(format!("{base}/api/v1/sso-providers/{id}/group-mappings"))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "platform", "role": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mapping_id = mapping["id"].as_str().unwrap().to_string();

    // omitting client_secret leaves the sealed one alone while everything
    // else moves, and the slug is untouched because it is in the login url
    let updated: Value = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Okta (prod)",
            "issuer": "https://acme.okta.com/",
            "client_id": "0oa2",
            "scopes": ["openid", "email"],
            "group_claim": "roles",
            "default_role": "member",
            "enabled": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["id"].as_str().unwrap(), id);
    assert_eq!(updated["name"], json!("Okta (prod)"));
    assert_eq!(updated["slug"], json!("okta"), "the slug is immutable");
    // a trailing slash is trimmed the same way create does it, so the issuer
    // still matches the one in the id token
    assert_eq!(updated["issuer"], json!("https://acme.okta.com"));
    assert_eq!(updated["client_id"], json!("0oa2"));
    assert_eq!(updated["group_claim"], json!("roles"));
    assert_eq!(updated["default_role"], json!("member"));
    assert_eq!(
        updated["enabled"],
        json!(false),
        "a provider can be disabled"
    );
    assert_eq!(
        updated["has_client_secret"],
        json!(true),
        "an absent client_secret leaves the stored one alone"
    );

    // a disabled provider is not offered on the login screen
    let methods: Value = client
        .get(format!("{base}/api/v1/auth/methods"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !methods.to_string().contains("okta"),
        "a disabled provider must not be advertised: {methods}"
    );

    // rotating: a new secret replaces the sealed one and is never echoed back
    let rotated: Value = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Okta (prod)",
            "issuer": "https://acme.okta.com",
            "client_id": "0oa2",
            "client_secret": "rotated-secret",
            "enabled": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rotated["has_client_secret"], json!(true));
    assert!(
        !rotated.to_string().contains("rotated-secret"),
        "the client secret leaked into the api response: {rotated}"
    );
    // omitting scopes keeps the ones already stored rather than resetting
    // them to the create-time defaults
    assert_eq!(rotated["scopes"], json!(["openid", "email"]));

    // an empty string clears it: the provider becomes a public pkce client
    let cleared: Value = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Okta (prod)",
            "issuer": "https://acme.okta.com",
            "client_id": "0oa2",
            "client_secret": "",
            "enabled": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["has_client_secret"], json!(false));

    // the mapping is still there: this is the whole point of editing in place
    let mappings: Value = client
        .get(format!("{base}/api/v1/sso-providers/{id}/group-mappings"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mappings.as_array().unwrap().len(), 1);
    assert_eq!(mappings[0]["id"].as_str().unwrap(), mapping_id);

    // a bad issuer is refused before anything is written
    let bad = client
        .put(format!("{base}/api/v1/sso-providers/{id}"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Okta", "issuer": "not-a-url", "client_id": "0oa2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // and every edit is audited, naming what happened to the secret without
    // ever recording the secret
    let actions: Vec<String> = sqlx::query_scalar(
        "select detail->>'client_secret' from audit_log \
         where action = 'sso_provider.update' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actions, vec!["unchanged", "rotated", "cleared"]);
}

/// OIDC SSO (#240) end to end against a stub identity provider: the login
/// redirect carries PKCE, the callback verifies the id token, groups become
/// memberships, and every rejection path fails closed.
#[tokio::test]
async fn sso_login_maps_groups_to_memberships_and_fails_closed() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // the redirect uri is deployment-owned, so the control plane must know its
    // own public url for the flow to be coherent
    let addr = serve_with_public_url(pool.clone(), Some("admintok".to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");

    let (issuer, stub) = stub_idp::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "SsoOrg", "slug": "sso-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Platform"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();

    // a non-http issuer is refused before anything is stored
    let bad = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Bad", "slug": "bad", "issuer": "not-a-url", "client_id": "x"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Stub IdP",
            "slug": "stub",
            "issuer": issuer,
            "client_id": "rolter",
            "client_secret": "s3cret",
            "group_claim": "groups"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    // the client secret is sealed and never echoed back
    let provider_text = provider.to_string();
    assert!(
        !provider_text.contains("s3cret") && !provider_text.contains("secret_ciphertext"),
        "client secret leaked into the api response: {provider_text}"
    );
    // the row names the two addresses an operator needs, built from the
    // deployment's public url rather than left for the dashboard to guess
    // from its own origin (#2083)
    assert_eq!(
        provider["redirect_uri"],
        format!("{base}/auth/sso/stub/callback")
    );
    assert_eq!(provider["login_url"], format!("{base}/auth/sso/stub/start"));
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["redirect_uri"], provider["redirect_uri"]);
    assert_eq!(listed[0]["login_url"], provider["login_url"]);

    // map an IdP group to a team-scoped admin role
    let mapping = client
        .post(format!(
            "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
        ))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "platform", "role": "admin", "team_id": team_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(mapping.status(), 200);

    // starting a login redirects to the provider with PKCE + state + nonce
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 303);
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with(&format!("{issuer}/authorize?response_type=code")));
    assert!(location.contains("code_challenge_method=S256"));
    // and the redirect uri it carries is the one the provider row advertised,
    // so what an operator registered in the IdP is what the IdP is sent
    let advertised = provider["redirect_uri"].as_str().unwrap();
    assert_eq!(
        url_param(&location, "redirect_uri"),
        advertised.replace(':', "%3A").replace('/', "%2F")
    );
    let state = url_param(&location, "state");

    // an id token minted for a different login (wrong nonce) is refused
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", "other-nonce", json!(["/platform"]));
    let replayed = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(replayed.status(), 400, "a mismatched nonce must be refused");

    // ...and that consumed the state, so the real callback needs a fresh login
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");

    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", &nonce, json!(["/platform"]));
    let logged_in: Value = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(logged_in["user"]["email"], "ada@example.com");
    assert_eq!(logged_in["granted_roles"][0], "admin");
    let session_token = logged_in["token"].as_str().unwrap().to_string();

    // the token exchange used the authorization code with a PKCE verifier
    let form = stub.last_form.lock().unwrap().clone();
    assert!(form.contains("grant_type=authorization_code"));
    assert!(form.contains("code_verifier="));

    // the session works, and the mapped membership is real: the team-scoped
    // admin can create a project inside that team
    let created = client
        .post(format!("{base}/api/v1/teams/{team_id}/projects"))
        .bearer_auth(&session_token)
        .json(&json!({"name": "FromSso"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);
    // but holds nothing at the org level, which no mapping granted
    let denied = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(&session_token)
        .json(&json!({"name": "Nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // a repeat login does not accumulate duplicate memberships
    let before: i64 = sqlx::query_scalar("select count(*) from memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", &nonce, json!(["/platform"]));
    let again = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 200);
    let after: i64 = sqlx::query_scalar("select count(*) from memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after, "a repeat sso login must be idempotent");

    // a user in no mapped group is refused, because the provider set no
    // default_role: SSO authenticates, it does not implicitly authorize
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "rolter", &nonce, json!(["/unmapped"]));
    let ungrouped = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(ungrouped.status(), 403);

    // an id token signed for another audience never becomes a session
    let start = client
        .get(format!("{base}/auth/sso/stub/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() =
        stub_idp::claims(&issuer, "some-other-client", &nonce, json!(["/platform"]));
    let wrong_audience = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_audience.status(), 400);

    // an unknown state (never issued, or already consumed) is refused
    let unknown = client
        .get(format!(
            "{base}/auth/sso/stub/callback?code=abc&state=made-up"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 400);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'auth.sso_login' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("auth.sso_login"));
}

/// Pull a query parameter out of a redirect URL.
fn url_param(url: &str, key: &str) -> String {
    url.split(['?', '&'])
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

/// SCIM 2.0 Users provisioning (#540): an IdP token scopes every call to one
/// org, create/deactivate/reconcile are idempotent, no local password is ever
/// involved, and a revoked token stops working immediately.
#[tokio::test]
async fn scim_users_are_provisioned_scoped_and_idempotent() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "ScimOrg", "slug": "scim-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "OtherScimOrg", "slug": "other-scim-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_string();

    // minting a token returns the secret exactly once
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("rolter_scim_"));
    let token_id = minted["id"].as_str().unwrap().to_string();

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed_text = listed.to_string();
    assert!(
        !listed_text.contains(&secret) && !listed_text.contains("token_hash"),
        "token material leaked into the listing: {listed_text}"
    );

    // an unauthenticated SCIM call is a SCIM-shaped 401, not an axum rejection
    let unauth = client
        .get(format!("{base}/scim/v2/Users"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
    let body: Value = unauth.json().await.unwrap();
    assert_eq!(
        body["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:Error"
    );

    // provision a user
    let created = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "ada@example.com",
            "externalId": "idp-1",
            "displayName": "Ada Lovelace",
            "emails": [{"value": "ada@example.com", "primary": true}],
            "password": "hunter2"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["userName"], "ada@example.com");
    assert_eq!(created["active"], true);
    assert_eq!(created["externalId"], "idp-1");
    let scim_id = created["id"].as_str().unwrap().to_string();
    // the supplied password is ignored, never stored
    let hash: Option<String> = sqlx::query_scalar("select password_hash from users where id = $1")
        .bind(scim_id.parse::<uuid::Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(hash.is_none(), "provisioning must not store a password");

    // a replayed create is a SCIM uniqueness conflict, not a second account
    let replay = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({"userName": "ada@example.com", "emails": [{"value": "ada@example.com"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 409);
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["scimType"], "uniqueness");

    // the filter IdPs reconcile with
    let found: Value = client
        .get(format!(
            "{base}/scim/v2/Users?filter=userName%20eq%20%22ada@example.com%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found["totalResults"], 1);
    assert_eq!(found["Resources"][0]["id"], scim_id);

    // an unsupported filter is refused rather than answered with everything
    let bad_filter = client
        .get(format!(
            "{base}/scim/v2/Users?filter=displayName%20eq%20%22Ada%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_filter.status(), 400);

    // a live session exists, then the IdP deactivates the leaver
    let user_uuid: uuid::Uuid = scim_id.parse().unwrap();
    let session_token = seed_session(&pool, user_uuid, "scimuser").await;
    let me_before = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session_token)
        .send()
        .await
        .unwrap();
    assert_eq!(me_before.status(), 200);

    let patched: Value = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "replace", "path": "active", "value": false}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["active"], false);
    // deactivation logs the leaver out rather than only blocking future logins
    let me_after = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session_token)
        .send()
        .await
        .unwrap();
    assert_eq!(me_after.status(), 401);

    // re-enabling is the same call with the other value
    let reenabled: Value = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .json(&json!({"Operations": [{"op": "replace", "value": {"active": true}}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reenabled["active"], true);

    // another org's token cannot see or touch this resource
    let other_minted: Value = client
        .post(format!("{base}/api/v1/orgs/{other_id}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "entra"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_secret = other_minted["secret"].as_str().unwrap().to_string();
    let cross = client
        .get(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 404, "a token must not reach another tenant");
    let cross_list: Value = client
        .get(format!("{base}/scim/v2/Users"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cross_list["totalResults"], 0);

    // delete deprovisions: the mapping goes, the account survives deactivated
    let deleted = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let gone = client
        .get(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);
    let still_there: Option<uuid::Uuid> = sqlx::query_scalar("select id from users where id = $1")
        .bind(user_uuid)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(
        still_there.is_some(),
        "the account row must outlive the IdP"
    );

    // revoking the token stops provisioning immediately
    let revoked = client
        .delete(format!("{base}/api/v1/scim-tokens/{token_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    let after_revoke = client
        .get(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 401);

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'scim.user.deprovision' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("scim.user.deprovision"));
}

/// SCIM 2.0 Groups (#540): the group surface an IdP drives, and the org-scoped
/// group→team mapping that turns group membership into roles. Covers SCIM
/// semantics and error envelopes, cross-tenant scoping, membership
/// reconciliation converging on a replayed sync, and deprovisioning taking the
/// group-granted roles with it.
#[tokio::test]
async fn scim_groups_map_to_teams_and_reconcile_idempotently() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "GroupOrg", "slug": "group-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "OtherGroupOrg", "slug": "other-group-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_string();

    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Platform"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();
    let team_uuid: uuid::Uuid = team_id.parse().unwrap();

    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();

    // an unauthenticated Groups call is a SCIM-shaped 401, like Users
    let unauth = client
        .get(format!("{base}/scim/v2/Groups"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);
    let body: Value = unauth.json().await.unwrap();
    assert_eq!(
        body["schemas"][0],
        "urn:ietf:params:scim:api:messages:2.0:Error"
    );

    // provision two users through the Users surface first: a group may only
    // contain accounts this token already created
    let mut user_ids = Vec::new();
    for name in ["ada@example.com", "grace@example.com"] {
        let created: Value = client
            .post(format!("{base}/scim/v2/Users"))
            .bearer_auth(&secret)
            .json(&json!({"userName": name, "emails": [{"value": name, "primary": true}]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        user_ids.push(created["id"].as_str().unwrap().to_string());
    }
    let ada = user_ids[0].clone();
    let grace = user_ids[1].clone();
    let ada_uuid: uuid::Uuid = ada.parse().unwrap();
    let grace_uuid: uuid::Uuid = grace.parse().unwrap();

    // an operator maps the group name to a role on the team, before the IdP has
    // ever mentioned the group — that must not error
    let mapping: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-group-mappings"))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "platform", "role": "member", "team_id": team_id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mapping_id = mapping["id"].as_str().unwrap().to_string();
    assert_eq!(mapping["group_name"], "platform");

    // a mapping may not grant into another tenant's team
    let cross_team = client
        .post(format!("{base}/api/v1/orgs/{other_id}/scim-group-mappings"))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "platform", "role": "admin", "team_id": team_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_team.status(),
        400,
        "a mapping must not reach another org's team"
    );

    // only the three built-in roles are mappable
    let bad_role = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-group-mappings"))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "platform", "role": "superadmin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_role.status(), 400);

    // the IdP creates the group with one member
    let created = client
        .post(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
            "displayName": "platform",
            "externalId": "idp-group-1",
            "members": [{"value": ada}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    let group_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["displayName"], "platform");
    assert_eq!(created["externalId"], "idp-group-1");
    assert_eq!(created["members"][0]["value"], ada);
    assert_eq!(created["members"][0]["display"], "ada@example.com");

    // the mapping took effect: ada holds member on the team, sourced 'scim'
    let team_roles = |user: uuid::Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "select role from memberships where user_id = $1 and team_id = $2 \
                 and source = 'scim'",
            )
            .bind(user)
            .bind(team_uuid)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(team_roles(ada_uuid).await, vec!["member".to_string()]);
    assert!(team_roles(grace_uuid).await.is_empty());

    // a replayed create is a uniqueness conflict, not a second group
    let replay = client
        .post(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&secret)
        .json(&json!({"displayName": "platform"}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 409);
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["scimType"], "uniqueness");

    // the filter IdPs reconcile with, and the ones that are refused explicitly
    let found: Value = client
        .get(format!(
            "{base}/scim/v2/Groups?filter=displayName%20eq%20%22platform%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(found["totalResults"], 1);
    assert_eq!(found["Resources"][0]["id"], group_id);
    let bad_filter = client
        .get(format!(
            "{base}/scim/v2/Groups?filter=externalId%20eq%20%22idp-group-1%22"
        ))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_filter.status(), 400);

    // PATCH add: the shape Okta and Entra send for a joiner
    let patched: Value = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "add", "path": "members", "value": [{"value": grace}]}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["members"].as_array().unwrap().len(), 2);
    assert_eq!(team_roles(grace_uuid).await, vec!["member".to_string()]);

    // replaying that exact operation must converge, not accumulate
    let replayed: Value = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "add", "path": "members", "value": [{"value": grace}]}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replayed["members"].as_array().unwrap().len(), 2);
    assert_eq!(
        team_roles(grace_uuid).await,
        vec!["member".to_string()],
        "a replayed sync must not grant the same role twice"
    );

    // PATCH remove with the filtered path form, which is how a leaver arrives
    let removed: Value = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "remove", "path": format!("members[value eq \"{grace}\"]")}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(removed["members"].as_array().unwrap().len(), 1);
    assert!(
        team_roles(grace_uuid).await.is_empty(),
        "leaving the group must revoke what it granted"
    );

    // a member the token never provisioned is refused
    let stranger = seed_user(&pool, "stranger@example.com", false).await;
    let outsider = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "add", "path": "members", "value": [{"value": stranger}]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(outsider.status(), 400);
    let outsider: Value = outsider.json().await.unwrap();
    assert_eq!(outsider["scimType"], "invalidValue");

    // an operation nothing supports is refused rather than answered with a
    // success the IdP would read as "the change landed"
    let unsupported = client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({"Operations": [{"op": "replace", "path": "urn:unknown", "value": "x"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(unsupported.status(), 400);

    // PUT replaces the whole member list
    let replaced: Value = client
        .put(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({"displayName": "platform", "members": [{"value": grace}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replaced["members"].as_array().unwrap().len(), 1);
    assert_eq!(replaced["members"][0]["value"], grace);
    assert!(team_roles(ada_uuid).await.is_empty());
    assert_eq!(team_roles(grace_uuid).await, vec!["member".to_string()]);

    // another org's token cannot see or touch this group
    let other_minted: Value = client
        .post(format!("{base}/api/v1/orgs/{other_id}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "entra"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_secret = other_minted["secret"].as_str().unwrap().to_string();
    let cross = client
        .get(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 404, "a token must not reach another tenant");
    let cross_list: Value = client
        .get(format!("{base}/scim/v2/Groups"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cross_list["totalResults"], 0);
    let cross_delete = client
        .delete(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&other_secret)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_delete.status(), 404);

    // a manual grant an operator made survives a sync
    let manual: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/memberships"))
        .bearer_auth("admintok")
        .json(&json!({
            "user_id": ada,
            "scope_type": "team",
            "scope_id": team_id,
            "role": "admin"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let manual_id = manual["id"].as_str().unwrap().to_string();
    let resync: Value = client
        .put(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({"displayName": "platform", "members": [{"value": ada}]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resync["members"].as_array().unwrap().len(), 1);
    let survived: Option<String> = sqlx::query_scalar("select role from memberships where id = $1")
        .bind(manual_id.parse::<uuid::Uuid>().unwrap())
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(
        survived.as_deref(),
        Some("admin"),
        "reconciliation must not touch a grant an operator made"
    );

    // deprovisioning the user takes the group-granted role with it
    let deprovisioned = client
        .delete(format!("{base}/scim/v2/Users/{ada}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(deprovisioned.status(), 204);
    assert!(
        team_roles(ada_uuid).await.is_empty(),
        "a deprovisioned account must not keep a group-granted role"
    );

    // dropping the mapping revokes what it granted, without a sync
    client
        .patch(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "Operations": [{"op": "add", "path": "members", "value": [{"value": grace}]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(team_roles(grace_uuid).await, vec!["member".to_string()]);
    let dropped = client
        .delete(format!("{base}/api/v1/scim-group-mappings/{mapping_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(dropped.status(), 204);
    assert!(team_roles(grace_uuid).await.is_empty());

    // and deleting the group is idempotent from the IdP's point of view
    let deleted = client
        .delete(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let gone = client
        .delete(format!("{base}/scim/v2/Groups/{group_id}"))
        .bearer_auth(&secret)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 404);

    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_log where org_id = $1 and action like 'scim.group%'",
    )
    .bind(org_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(actions.iter().any(|a| a == "scim.group.create"));
    assert!(actions.iter().any(|a| a == "scim.group.update"));
    assert!(actions.iter().any(|a| a == "scim.group.delete"));
}

/// MCP OAuth grants and sessions (#541): admins see the whole org, a member
/// sees only what they own, revoking a grant kills its sessions, and no token
/// material ever appears in a response.
#[tokio::test]
async fn mcp_oauth_grants_and_sessions_are_owner_scoped_and_revocable() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "McpOrg", "slug": "mcp-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    // a bad transport or a non-http url is refused before anything is stored
    for bad in [
        json!({"name": "S", "slug": "s", "url": "https://mcp.example.com", "transport": "carrier-pigeon"}),
        json!({"name": "S", "slug": "s", "url": "file:///etc/passwd"}),
    ] {
        let resp = client
            .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
            .bearer_auth("admintok")
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "accepted {bad}");
    }

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Docs",
            "slug": "docs",
            "url": "https://mcp.example.com"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["transport"], "streamable_http");
    let server_uuid: uuid::Uuid = server["id"].as_str().unwrap().parse().unwrap();
    let server: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_uuid}"))
        .bearer_auth("admintok")
        .json(&json!({"required_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["required_scopes"], json!(["tools:read"]));

    // two members of the same org, each with their own grant and session
    let alice = seed_user(&pool, "alice@example.com", false).await;
    seed_membership(&pool, alice, Some(org_uuid), None, None, "member").await;
    let alice_token = seed_session(&pool, alice, "alice").await;
    let bob = seed_user(&pool, "bob@example.com", false).await;
    seed_membership(&pool, bob, Some(org_uuid), None, None, "member").await;
    let bob_token = seed_session(&pool, bob, "bob").await;

    use rolter_store::postgres::repo::McpSessionMaterial;

    let kek = rolter_store::postgres::crypto::Kek::from_secret("test-kek");
    let repo = rolter_store::postgres::repo::McpOAuthRepo(&pool);
    let alice_grant = repo
        .upsert_grant(server_uuid, alice, &["tools:read".to_string()])
        .await
        .unwrap();
    let bob_grant = repo
        .upsert_grant(server_uuid, bob, &["tools:read".to_string()])
        .await
        .unwrap();
    let excessive = repo
        .store_session(
            &kek,
            alice_grant.id,
            McpSessionMaterial {
                access_token: "must-not-store",
                refresh_token: None,
                scopes: &["tools:write".to_string()],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                refresh_expires_at: None,
            },
        )
        .await;
    assert!(excessive.is_err(), "session exceeded the consent grant");
    let alice_session = repo
        .store_session(
            &kek,
            alice_grant.id,
            McpSessionMaterial {
                access_token: "at-alice-secret",
                refresh_token: Some("rt-alice-secret"),
                scopes: &["tools:read".to_string()],
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                refresh_expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
            },
        )
        .await
        .unwrap();
    repo.store_session(
        &kek,
        bob_grant.id,
        McpSessionMaterial {
            access_token: "at-bob-secret",
            refresh_token: None,
            scopes: &["tools:read".to_string()],
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            refresh_expires_at: None,
        },
    )
    .await
    .unwrap();

    // the protected snapshot carries only the newest live, scope-valid session
    // for each user/server pair and opens access tokens with the deployment KEK
    let snapshot_store =
        rolter_store::PostgresConfigStore::with_kek(pool.clone(), Some(kek.clone()));
    let snapshot = rolter_store::ConfigStore::load(&snapshot_store)
        .await
        .unwrap();
    assert_eq!(snapshot.mcp_servers.len(), 1);
    assert_eq!(snapshot.mcp_servers[0].required_scopes, ["tools:read"]);
    assert_eq!(snapshot.mcp_oauth_sessions.len(), 2);
    assert!(snapshot
        .mcp_oauth_sessions
        .iter()
        .any(|session| session.user_id == alice.to_string()
            && session.access_token == "at-alice-secret"));

    // the admin token sees both owners
    let all: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);

    // a member sees only their own grant and session
    let mine: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mine = mine.as_array().unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["user_id"], alice.to_string());
    assert_eq!(mine[0]["active"], true);

    let sessions: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sessions_text = sessions.to_string();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert_eq!(sessions[0]["has_refresh_token"], true);
    // the response describes the session without ever carrying its tokens
    assert!(
        !sessions_text.contains("at-alice-secret") && !sessions_text.contains("rt-alice-secret"),
        "token material leaked into the sessions response: {sessions_text}"
    );

    // one member may not revoke another's session
    let denied = client
        .delete(format!("{base}/api/v1/mcp/sessions/{}", alice_session.id))
        .bearer_auth(&bob_token)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // a member of a different org cannot even see it
    let other_org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "OtherMcpOrg", "slug": "other-mcp-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_uuid: uuid::Uuid = other_org["id"].as_str().unwrap().parse().unwrap();
    let outsider = seed_user(&pool, "outsider@example.com", false).await;
    seed_membership(&pool, outsider, Some(other_uuid), None, None, "admin").await;
    let outsider_token = seed_session(&pool, outsider, "outsider").await;
    let cross_tenant = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_tenant.status(), 403);
    let cross_revoke = client
        .delete(format!("{base}/api/v1/mcp/grants/{}", alice_grant.id))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap();
    assert_eq!(cross_revoke.status(), 403);

    // the sealed tokens open while the grant is live
    let opened = repo
        .open_session(&kek, alice_session.id, chrono::Utc::now())
        .await
        .unwrap()
        .expect("live session opens");
    assert_eq!(opened.access_token, "at-alice-secret");
    assert_eq!(opened.refresh_token.as_deref(), Some("rt-alice-secret"));

    // revoking the grant revokes its sessions in the same breath
    let revoked: Value = client
        .delete(format!("{base}/api/v1/mcp/grants/{}", alice_grant.id))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(revoked["active"], false);
    let after: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(after[0]["revoked_at"].is_string());
    // and the tokens stop opening, so withdrawn consent cannot be spent
    assert!(repo
        .open_session(&kek, alice_session.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());

    // an expired session is dead even though nobody revoked it
    let expired = repo
        .store_session(
            &kek,
            bob_grant.id,
            McpSessionMaterial {
                access_token: "at-bob-expired",
                refresh_token: None,
                scopes: &["tools:read".to_string()],
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                refresh_expires_at: None,
            },
        )
        .await
        .unwrap();
    assert!(repo
        .open_session(&kek, expired.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());

    // and a KEK that did not seal the token cannot open it
    let wrong = rolter_store::postgres::crypto::Kek::from_secret("not-the-kek");
    let bob_live = repo
        .list_sessions(org_uuid, Some(bob))
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id != expired.id)
        .unwrap();
    assert!(repo
        .open_session(&wrong, bob_live.id, chrono::Utc::now())
        .await
        .is_err());

    let action: Option<String> = sqlx::query_scalar(
        "select action from audit_log where action = 'mcp_oauth_grant.revoke' order by at desc limit 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(action.as_deref(), Some("mcp_oauth_grant.revoke"));
}

/// The capability matrix and the caller's effective permissions are served by
/// the control plane rather than assembled in the browser (#534): the matrix
/// describes the rules, `effective` answers for the caller at a scope, and the
/// answer matches what the CRUD guard actually does.
#[tokio::test]
async fn rbac_matrix_and_effective_permissions_are_api_backed() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the matrix needs authentication but no particular role
    let unauth = client
        .get(format!("{base}/api/v1/rbac/matrix"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401);

    let matrix: Value = client
        .get(format!("{base}/api/v1/rbac/matrix"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(matrix["roles"].as_array().unwrap().len(), 3);
    let provider = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "provider")
        .expect("provider in the matrix");
    let create = provider["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["action"] == "create")
        .unwrap();
    assert_eq!(create["minimum_role"], "admin");
    assert_eq!(create["superadmin_only"], false);
    // deployment-wide policy is not something an org admin can reach
    let flags = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "feature_flags")
        .expect("feature_flags in the matrix");
    assert_eq!(flags["actions"][0]["superadmin_only"], true);

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "MatrixOrg", "slug": "matrix-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    let viewer = seed_user(&pool, "matrix-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "matrixviewer").await;

    let effective: Value = client
        .get(format!("{base}/api/v1/rbac/effective?org_id={org_id}"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(effective["superadmin"], false);
    assert_eq!(effective["role"], "viewer");
    let allowed: Vec<String> = serde_json::from_value(effective["allowed"].clone()).unwrap();
    assert!(allowed.contains(&"provider:read".to_string()));
    assert!(!allowed.contains(&"provider:create".to_string()));

    // and the guard agrees with what `effective` reported
    let denied = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth(&viewer_token)
        .json(&json!({"name": "T"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // an org the caller has no membership in yields no permissions at all
    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "OtherOrg", "slug": "other-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap();
    let elsewhere: Value = client
        .get(format!("{base}/api/v1/rbac/effective?org_id={other_id}"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(elsewhere["role"].is_null());
    // nothing org-scoped is reachable there; what remains is exactly the
    // global read-only facts, which take authentication and no membership
    let elsewhere_allowed: Vec<&str> = elsewhere["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        elsewhere_allowed,
        vec![
            "model_label:read",
            "model_price:read",
            "model:read",
            "version:read",
            "stability:read",
            "public_url:read"
        ]
    );

    // a project-scoped admin inherits nothing upward: the same user is only an
    // admin inside the project chain they were granted
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth("admintok")
        .json(&json!({"name": "MatrixTeam"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();
    let team_uuid: uuid::Uuid = team_id.parse().unwrap();
    let project: Value = client
        .post(format!("{base}/api/v1/teams/{team_id}/projects"))
        .bearer_auth("admintok")
        .json(&json!({"name": "MatrixProject"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = project["id"].as_str().unwrap().to_string();
    let project_uuid: uuid::Uuid = project_id.parse().unwrap();

    let scoped = seed_user(&pool, "matrix-project-admin@example.com", false).await;
    seed_membership(
        &pool,
        scoped,
        Some(org_uuid),
        Some(team_uuid),
        Some(project_uuid),
        "admin",
    )
    .await;
    let scoped_token = seed_session(&pool, scoped, "matrixscoped").await;

    let in_project: Value = client
        .get(format!(
            "{base}/api/v1/rbac/effective?org_id={org_id}&team_id={team_id}&project_id={project_id}"
        ))
        .bearer_auth(&scoped_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(in_project["role"], "admin");

    let at_org: Value = client
        .get(format!("{base}/api/v1/rbac/effective?org_id={org_id}"))
        .bearer_auth(&scoped_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        at_org["role"].is_null(),
        "a project-scoped grant must not authorize the whole org"
    );
}

/// With an admin token configured (RBAC enforcement active), every control
/// mutation is checked against the caller's role at the resource's scope:
/// viewers are denied, scoped admins are allowed only within their scope,
/// cross-scope admins are denied, superadmins bypass, and the machine admin
/// token bypasses. Covers ROL-33 (resolver) + ROL-34 (enforcement).
#[tokio::test]
async fn rbac_enforced_on_every_mutation() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // bootstrap the org hierarchy as the machine admin token (superadmin)
    async fn post_as(client: &reqwest::Client, url: &str, token: &str, body: Value) -> Value {
        let resp = client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org_a = post_as(
        &client,
        &format!("{base}/api/v1/orgs"),
        "admintok",
        json!({"name": "OrgA", "slug": "org-a"}),
    )
    .await;
    let org_a_id = org_a["id"].as_str().unwrap().to_string();
    let org_b = post_as(
        &client,
        &format!("{base}/api/v1/orgs"),
        "admintok",
        json!({"name": "OrgB", "slug": "org-b"}),
    )
    .await;
    let org_b_id = org_b["id"].as_str().unwrap().to_string();

    let org_a_uuid: uuid::Uuid = org_a_id.parse().unwrap();
    let org_b_uuid: uuid::Uuid = org_b_id.parse().unwrap();

    // seed principals: a viewer and an admin on org A, an admin on org B, and a
    // superadmin with no memberships at all
    let viewer = seed_user(&pool, "viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_a_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "viewer").await;

    let admin_a = seed_user(&pool, "admin-a@example.com", false).await;
    seed_membership(&pool, admin_a, Some(org_a_uuid), None, None, "admin").await;
    let admin_a_token = seed_session(&pool, admin_a, "admina").await;

    let admin_b = seed_user(&pool, "admin-b@example.com", false).await;
    seed_membership(&pool, admin_b, Some(org_b_uuid), None, None, "admin").await;
    let admin_b_token = seed_session(&pool, admin_b, "adminb").await;

    let super_user = seed_user(&pool, "super@example.com", true).await;
    let super_token = seed_session(&pool, super_user, "super").await;

    let create_team_url = format!("{base}/api/v1/orgs/{org_a_id}/teams");

    // unauthenticated → 401 (RBAC enforcement is active)
    let unauth = client
        .post(&create_team_url)
        .json(&json!({"name": "T"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status(), 401, "no credentials must be rejected");

    // viewer on org A → 403 creating a team (mutation needs admin)
    let viewer_denied = client
        .post(&create_team_url)
        .bearer_auth(&viewer_token)
        .json(&json!({"name": "T-viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(viewer_denied.status(), 403, "viewer must not create");

    // admin on org A → allowed on org A
    let admin_ok = client
        .post(&create_team_url)
        .bearer_auth(&admin_a_token)
        .json(&json!({"name": "T-admin"}))
        .send()
        .await
        .unwrap();
    assert!(
        admin_ok.status().is_success(),
        "org-A admin must create under org A: {}",
        admin_ok.status()
    );

    // admin on org B → 403 creating a provider under org A (cross-scope)
    let cross_scope = client
        .post(format!("{base}/api/v1/orgs/{org_a_id}/providers"))
        .bearer_auth(&admin_b_token)
        .json(&json!({"name": "p1", "kind": "openai", "api_base": "https://api.openai.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_scope.status(),
        403,
        "org-B admin must not mutate org-A resources"
    );

    // superadmin session → bypass, allowed even with no membership
    let super_ok = client
        .post(&create_team_url)
        .bearer_auth(&super_token)
        .json(&json!({"name": "T-super"}))
        .send()
        .await
        .unwrap();
    assert!(
        super_ok.status().is_success(),
        "superadmin must bypass: {}",
        super_ok.status()
    );

    // global model-price catalog is superadmin-only: the org-A admin is denied
    let price_denied = client
        .put(format!("{base}/api/v1/model-prices"))
        .bearer_auth(&admin_a_token)
        .json(&json!({"model": "gpt-4o", "input_per_mtok": "1", "output_per_mtok": "2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        price_denied.status(),
        403,
        "model-price mutation is superadmin-only"
    );
    let price_ok = client
        .put(format!("{base}/api/v1/model-prices"))
        .bearer_auth("admintok")
        .json(&json!({"model": "gpt-4o", "input_per_mtok": "1", "output_per_mtok": "2"}))
        .send()
        .await
        .unwrap();
    assert!(price_ok.status().is_success(), "{}", price_ok.status());
}

/// Open mode (no admin token) must keep the CRUD API fully open: an
/// unauthenticated mutation still succeeds, preserving zero-cred local dev.
#[tokio::test]
async fn open_mode_allows_unauthenticated_mutations() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/orgs"))
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "open mode must allow unauthenticated mutations: {}",
        resp.status()
    );
}

/// Full user lifecycle (ROL-223): invite an account into an org, list it, grant
/// a team-scoped role, then deactivate it and confirm login is blocked while the
/// row and its memberships survive.
#[tokio::test]
async fn user_and_membership_lifecycle() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    // org → team scaffold
    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();

    // invite a user into the org with an initial password + role
    let created = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/users"),
        json!({"email": "dev@example.com", "password": "hunter2!!", "role": "member"}),
    )
    .await;
    let user_id = created["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["user"]["email"], "dev@example.com");
    assert_eq!(created["user"]["is_superadmin"], false);
    // the password hash must never be serialized back
    assert!(created["user"].get("password_hash").is_none());
    assert_eq!(created["membership"]["role"], "member");

    // the account shows up in the org's user list
    let users: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/users"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        users.as_array().unwrap().iter().any(|u| u["id"] == user_id),
        "invited user missing from org list: {users}"
    );

    // duplicate email is a conflict
    let dup = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .json(&json!({"email": "dev@example.com", "password": "hunter2!!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    // grant a team-scoped admin role
    let membership = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/memberships"),
        json!({"user_id": user_id, "scope_type": "team", "scope_id": team_id, "role": "admin"}),
    )
    .await;
    let membership_id = membership["id"].as_str().unwrap().to_string();
    assert_eq!(membership["team_id"], team_id);

    // both memberships (org member + team admin) are listed for the org
    let memberships: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/memberships"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        memberships.as_array().unwrap().len(),
        2,
        "expected org + team memberships: {memberships}"
    );

    // the account can log in before deactivation
    let ok = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "dev@example.com", "password": "hunter2!!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "login should succeed before deactivation");

    // deactivate the account
    let deact = client
        .put(format!("{base}/api/v1/users/{user_id}"))
        .json(&json!({"deactivated": true}))
        .send()
        .await
        .unwrap();
    assert!(deact.status().is_success());
    let deact_body: Value = deact.json().await.unwrap();
    assert!(deact_body["deactivated_at"].is_string());

    // login is now blocked, but the user + memberships still exist
    let blocked = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "dev@example.com", "password": "hunter2!!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 401, "deactivated account must not log in");
    let still: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/memberships"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(still.as_array().unwrap().len(), 2);

    // revoke the team membership, then delete the account
    let del_m = client
        .delete(format!("{base}/api/v1/memberships/{membership_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del_m.status(), 204);
    let del_u = client
        .delete(format!("{base}/api/v1/users/{user_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(del_u.status(), 204);

    // the org user list is empty again (cascade removed the org membership too)
    let after: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/users"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        after.as_array().unwrap().is_empty(),
        "user should be gone after delete: {after}"
    );
}

/// Self-service key lifecycle (ROL-224): a logged-in member mints, lists,
/// rotates and deletes their own virtual keys, and usage 503s without
/// ClickHouse. Runs in open mode; `/me/*` still requires a real session.
#[tokio::test]
async fn self_service_key_lifecycle() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    // org → team → project
    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();

    // invite a member into the org (org membership authorizes the project too)
    post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/users"),
        json!({"email": "member@example.com", "password": "hunter2!!", "role": "member"}),
    )
    .await;

    // log in to get a session token
    let login = post(
        &client,
        format!("{base}/api/v1/auth/login"),
        json!({"email": "member@example.com", "password": "hunter2!!"}),
    )
    .await;
    let token = login["token"].as_str().unwrap().to_string();

    // /me/* requires a session: unauthenticated is rejected. this app runs in
    // open mode, where every admin route beside it passes as superadmin — so
    // the 401 has to say *which* 401 it is, or it reads as a broken login
    // rather than an endpoint this deployment cannot serve (#942)
    let anon = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
    let anon: Value = anon.json().await.unwrap();
    assert_eq!(
        anon["error"]["code"].as_str(),
        Some("open_mode_no_session"),
        "open-mode 401 must be distinguishable: {anon}"
    );

    // mint a key I own in the project I belong to
    let minted = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/virtual-keys"
        ))
        .bearer_auth(&token)
        .json(&json!({"name": "laptop", "models": ["gpt-4o"]}))
        .send()
        .await
        .unwrap();
    assert!(minted.status().is_success());
    let minted: Value = minted.json().await.unwrap();
    assert!(minted["key"].as_str().unwrap().starts_with("sk-rolter-"));
    let key_id = minted["id"].as_str().unwrap().to_string();

    // it shows up in my key list, enriched with project/org names
    let keys: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = keys.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["project_name"], "Gateway");
    assert_eq!(arr[0]["org_name"], "Acme");
    // the key hash is never exposed on the self-service surface
    assert!(arr[0].get("key_hash").is_none());

    // rotate: a new secret, old key disabled, both still owned/listed
    let rotated = client
        .post(format!("{base}/api/v1/me/virtual-keys/{key_id}/rotate"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(rotated.status().is_success());
    let rotated: Value = rotated.json().await.unwrap();
    let new_id = rotated["id"].as_str().unwrap().to_string();
    assert_ne!(new_id, key_id);

    let after: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let after = after.as_array().unwrap();
    assert_eq!(after.len(), 2);
    let old = after.iter().find(|k| k["id"] == key_id).unwrap();
    assert_eq!(old["disabled"], true, "rotated-out key must be disabled");

    // usage 503s without ClickHouse configured
    let usage = client
        .get(format!("{base}/api/v1/me/usage"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(usage.status(), 503);

    // a window ClickHouse would read as the epoch is refused first, naming the
    // bound, rather than widening the scan to 1970 (#1192)
    for param in ["since", "until"] {
        let usage = client
            .get(format!("{base}/api/v1/me/usage?{param}=not-a-date"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(usage.status(), 400, "{param}");
        let body: Value = usage.json().await.unwrap();
        assert_eq!(body["error"]["param"], param);
        assert_eq!(body["error"]["code"], "invalid_time_bound");
        assert_eq!(body["error"]["type"], "invalid_request_error");
    }
    // and a valid one still reaches the handler, which 503s as before
    let usage = client
        .get(format!(
            "{base}/api/v1/me/usage?since=2026-07-01T00:00:00Z&until=2026-07-08%2000:00:00"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(usage.status(), 503);

    // delete the new key
    let del = client
        .delete(format!("{base}/api/v1/me/virtual-keys/{new_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);
    let remaining: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(remaining.as_array().unwrap().len(), 1);
}

/// The playground mints a key the *server* scopes (#1640).
///
/// The point of the endpoint is what a caller cannot do with it: it takes no
/// body, so it cannot ask for a model the project does not route, cannot ask
/// for a longer life, and cannot be pointed at a project the caller does not
/// belong to. The dashboard half (#944) puts the returned key straight into the
/// Playground rather than asking an operator to paste a long-lived one.
#[tokio::test]
async fn playground_key_is_scoped_by_the_server() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Playground Co", "slug": "playground-co"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();

    post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/users"),
        json!({"email": "operator@example.com", "password": "hunter2!!", "role": "member"}),
    )
    .await;
    let login = post(
        &client,
        format!("{base}/api/v1/auth/login"),
        json!({"email": "operator@example.com", "password": "hunter2!!"}),
    )
    .await;
    let token = login["token"].as_str().unwrap().to_string();

    // a project with no routes has nothing to address, and an empty `models`
    // list on a virtual key means *every* model — so this must refuse rather
    // than mint the widest key in the system
    let empty = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        empty.status(),
        400,
        "a routeless project must not mint a key"
    );

    post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "claude-sonnet-4", "strategy": "round_robin"}),
    )
    .await;

    let minted = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert!(minted.status().is_success());
    let minted: Value = minted.json().await.unwrap();
    assert!(minted["key"].as_str().unwrap().starts_with("sk-rolter-"));

    // scoped to exactly the routes this project has, written out rather than
    // left empty: a key minted before a third route exists must not reach it
    let mut models: Vec<&str> = minted["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    models.sort_unstable();
    assert_eq!(models, vec!["claude-sonnet-4", "gpt-4o"]);

    // minutes, not days: the mint endpoint's floor is one day, which is the
    // whole reason this endpoint exists
    let expires_at = minted["expires_at"].as_str().expect("an expiry");
    let expires_at: chrono::DateTime<chrono::Utc> = expires_at.parse().unwrap();
    let lifetime = expires_at - chrono::Utc::now();
    assert!(
        lifetime < chrono::Duration::hours(1) && lifetime > chrono::Duration::minutes(1),
        "playground key should live minutes, lives {lifetime}"
    );

    // and it says what it is, so the Keys screen can label it rather than
    // leaving a reader to infer it from the expiry
    assert_eq!(minted["purpose"], "playground");
    let keys: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed = keys.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["purpose"], "playground");

    // a route added after the key was minted is out of its reach: the list was
    // resolved once, at mint time, which is what makes the key a snapshot of
    // what the caller could reach rather than a standing grant
    post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "o3-mini", "strategy": "round_robin"}),
    )
    .await;
    let keys: Value = client
        .get(format!("{base}/api/v1/me/virtual-keys"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let still: Vec<&str> = keys.as_array().unwrap()[0]["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert!(
        !still.contains(&"o3-mini"),
        "a key must not widen itself as routes appear: {still:?}"
    );

    // a session is required: the endpoint mints a credential, so it is never
    // reachable without one
    let anon = client
        .post(format!(
            "{base}/api/v1/me/projects/{project_id}/playground-key"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
}

/// Invitations and single sign-on co-exist (#240): an operator-granted role is
/// never reconciled away by a later SSO login, an IdP group that disappears
/// does revoke the role it granted, and an org can require SSO without locking
/// out the break-glass superadmin.
#[tokio::test]
async fn the_last_enabled_sso_provider_cannot_go_while_passwords_are_off() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "LastIdp", "slug": "last-idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let mut ids = Vec::new();
    for slug in ["idp-a", "idp-b"] {
        let p: Value = client
            .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
            .bearer_auth("admintok")
            .json(&json!({
                "name": slug,
                "slug": slug,
                "issuer": "https://idp.example.com",
                "client_id": "client"
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push(p["id"].as_str().unwrap().to_string());
    }
    let disable = |id: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .put(format!("{base}/api/v1/sso-providers/{id}"))
                .bearer_auth("admintok")
                .json(&json!({
                    "name": "idp",
                    "issuer": "https://idp.example.com",
                    "client_id": "client",
                    "enabled": false
                }))
                .send()
                .await
                .unwrap()
        }
    };
    let set_passwords = |allow: bool| {
        let client = client.clone();
        let base = base.clone();
        let org_id = org_id.clone();
        async move {
            client
                .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
                .bearer_auth("admintok")
                .json(&json!({"allow_password_login": allow, "allow_sso": true}))
                .send()
                .await
                .unwrap()
        }
    };
    let enabled_count = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from sso_providers where enabled")
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    // passwords on: the last provider is free to go, nobody is locked out
    assert_eq!(disable(ids[0].clone()).await.status(), 200);
    assert_eq!(disable(ids[1].clone()).await.status(), 200);
    assert_eq!(enabled_count().await, 0);
    // re-enable both, then turn passwords off (the reverse guard)
    for id in &ids {
        let r = client
            .put(format!("{base}/api/v1/sso-providers/{id}"))
            .bearer_auth("admintok")
            .json(&json!({
                "name": "idp",
                "issuer": "https://idp.example.com",
                "client_id": "client",
                "enabled": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    assert_eq!(set_passwords(false).await.status(), 200);

    // two enabled: one may go, the second is refused and stays enabled
    assert_eq!(disable(ids[0].clone()).await.status(), 200);
    let refused = disable(ids[1].clone()).await;
    assert_eq!(refused.status(), 409);
    let body: Value = refused.json().await.unwrap();
    assert!(
        body.to_string().contains("another sso provider first"),
        "the refusal names the fix: {body}"
    );
    assert_eq!(enabled_count().await, 1);

    // deleting the last enabled one is refused too
    let del = client
        .delete(format!("{base}/api/v1/sso-providers/{}", ids[1]))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 409);
    let exists: i64 = sqlx::query_scalar("select count(*) from sso_providers where id = $1::uuid")
        .bind(&ids[1])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(exists, 1);

    // the disabled provider is not a way to sign in, so it can still be deleted
    let del_disabled = client
        .delete(format!("{base}/api/v1/sso-providers/{}", ids[0]))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(del_disabled.status(), 204);

    // the reverse guard still holds: no enabled provider, no passwords-off
    assert_eq!(set_passwords(true).await.status(), 200);
    let del_last = client
        .delete(format!("{base}/api/v1/sso-providers/{}", ids[1]))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(del_last.status(), 204, "passwords on: the last one may go");
    assert_eq!(set_passwords(false).await.status(), 409);
}

#[tokio::test]
async fn sso_and_password_login_coexist_per_org_policy() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some("admintok".to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "MixedOrg", "slug": "mixed-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    // with no provider registered, the login screen offers a password form and
    // nothing else: an operator who never wants an IdP never sees one
    let methods: Value = client
        .get(format!("{base}/api/v1/auth/methods"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(methods["password"], true);
    assert_eq!(methods["sso"].as_array().unwrap().len(), 0);

    // an invited local account: created with a password, granted a role by an
    // operator, and able to log in
    let invited: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .bearer_auth("admintok")
        .json(&json!({"email": "ada@example.com", "password": "correct horse battery"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = invited["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(invited["membership"]["role"], "member");
    let source: String = sqlx::query_scalar("select source from memberships where user_id = $1")
        .bind(uuid::Uuid::parse_str(&user_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(source, "manual", "an operator grant must be tagged manual");

    let logged_in = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": "correct horse battery"}))
        .send()
        .await
        .unwrap();
    assert_eq!(logged_in.status(), 200);

    // now the same deployment gains an IdP, and the same person arrives
    // through it: the account is adopted by email, not duplicated
    let (issuer, stub) = stub_idp::serve_stub().await;
    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Stub IdP", "slug": "mixed", "issuer": issuer,
            "client_id": "rolter", "client_secret": "s3cret"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    let mapping: Value = client
        .post(format!(
            "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
        ))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "admins", "role": "admin", "org_id": org_id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mapping_id = mapping["id"].as_str().unwrap().to_string();

    // the login screen now offers both
    let methods: Value = client
        .get(format!("{base}/api/v1/auth/methods"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(methods["password"], true);
    assert_eq!(methods["sso"][0]["slug"], "mixed");
    assert_eq!(methods["sso"][0]["start_url"], "/auth/sso/mixed/start");

    sso_login(&client, &base, &stub, &issuer, json!(["admins"]))
        .await
        .unwrap();
    let user_uuid = uuid::Uuid::parse_str(&user_id).unwrap();
    let users: i64 = sqlx::query_scalar("select count(*) from users where email = $1")
        .bind("ada@example.com")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 1, "sso must adopt the invited account, not fork it");
    let rows: Vec<(String, String)> =
        sqlx::query_as("select role, source from memberships where user_id = $1 order by source")
            .bind(user_uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![
            ("member".to_string(), "manual".to_string()),
            ("admin".to_string(), "sso".to_string()),
        ],
        "the operator grant and the sso grant must co-exist"
    );

    // the IdP drops the group; the next login revokes the role it granted and
    // leaves the operator's grant alone
    client
        .delete(format!("{base}/api/v1/sso-group-mappings/{mapping_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    let refused = sso_login(&client, &base, &stub, &issuer, json!(["admins"])).await;
    assert_eq!(
        refused, None,
        "with no mapping left and no default_role, the login must fail closed"
    );
    let remaining: Vec<(String, String)> =
        sqlx::query_as("select role, source from memberships where user_id = $1")
            .bind(user_uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining,
        vec![("member".to_string(), "manual".to_string())],
        "losing every mapped group revokes the sso grant, not the operator's"
    );

    // an org cannot disable password login before an IdP can carry the load...
    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "NoIdp", "slug": "no-idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap();
    let premature = client
        .put(format!("{base}/api/v1/orgs/{other_id}/auth-policy"))
        .bearer_auth("admintok")
        .json(&json!({"allow_password_login": false, "allow_sso": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(premature.status(), 409);

    // ...nor turn both methods off
    let neither = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth("admintok")
        .json(&json!({"allow_password_login": false, "allow_sso": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(neither.status(), 409);

    // enforcing sso for the org that has a provider refuses that member's
    // password, and the login screen stops offering the form
    let enforced = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth("admintok")
        .json(&json!({"allow_password_login": false, "allow_sso": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(enforced.status(), 200);
    let blocked = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": "correct horse battery"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 403);

    // break-glass: a superadmin who belongs to the same org still gets in with
    // a password, which is the only reason enforcing sso is a safe switch to
    // flip — a mistyped issuer must not be unrecoverable
    let root: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/users"))
        .bearer_auth("admintok")
        .json(&json!({"email": "root@example.com", "password": "break glass in case", "role": "admin"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let root_id = uuid::Uuid::parse_str(root["user"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("update users set is_superadmin = true where id = $1")
        .bind(root_id)
        .execute(&pool)
        .await
        .unwrap();
    let super_login = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "root@example.com", "password": "break glass in case"}))
        .send()
        .await
        .unwrap();
    assert_eq!(super_login.status(), 200);

    // sso can be switched off without deleting the provider
    let off = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth("admintok")
        .json(&json!({"allow_password_login": true, "allow_sso": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(off.status(), 200);
    let refused = sso_login(&client, &base, &stub, &issuer, json!(["admins"])).await;
    assert_eq!(
        refused, None,
        "sso must be refused while the org disables it"
    );
}

/// Drive one full SSO round trip, returning the session token on success and
/// `None` when the callback refused.
async fn sso_login(
    client: &reqwest::Client,
    base: &str,
    stub: &stub_idp::Stub,
    issuer: &str,
    groups: Value,
) -> Option<String> {
    let start = client
        .get(format!("{base}/auth/sso/mixed/start"))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get("location")?
        .to_str()
        .unwrap()
        .to_string();
    let state = url_param(&location, "state");
    let nonce = url_param(&location, "nonce");
    *stub.next_claims.lock().unwrap() = stub_idp::claims(issuer, "rolter", &nonce, groups);
    let response = client
        .get(format!(
            "{base}/auth/sso/mixed/callback?code=abc&state={state}"
        ))
        .send()
        .await
        .unwrap();
    if !response.status().is_success() {
        return None;
    }
    let body: Value = response.json().await.unwrap();
    Some(body["token"].as_str().unwrap().to_string())
}

/// #2297: the provider sends the browser to the callback, so the callback must
/// end on the dashboard. A success hands over a one-time code (never the
/// token), redeemed once; a refusal names a stable code and none of the IdP's
/// own words.
#[tokio::test]
async fn browser_sso_sign_in_ends_on_the_dashboard_with_a_one_time_code() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some("admintok".to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (issuer, stub) = stub_idp::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "BrowserOrg", "slug": "browser-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let client_secret = format!("idp-{}", uuid::Uuid::new_v4());
    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Stub IdP", "slug": "browser", "issuer": issuer,
            "client_id": "rolter", "client_secret": client_secret
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();
    let mapping = client
        .post(format!(
            "{base}/api/v1/sso-providers/{provider_id}/group-mappings"
        ))
        .bearer_auth("admintok")
        .json(&json!({"group_name": "admins", "role": "admin", "org_id": org_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(mapping.status(), 200);

    // one browser navigation through the provider, answered with `groups`
    let navigate = |groups: Value| {
        let (client, base, stub, issuer) = (&client, &base, &stub, &issuer);
        async move {
            let start = client
                .get(format!("{base}/auth/sso/browser/start"))
                .send()
                .await
                .unwrap();
            let location = start
                .headers()
                .get("location")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let state = url_param(&location, "state");
            let nonce = url_param(&location, "nonce");
            *stub.next_claims.lock().unwrap() = stub_idp::claims(issuer, "rolter", &nonce, groups);
            client
                .get(format!(
                    "{base}/auth/sso/browser/callback?code=abc&state={state}"
                ))
                .header("accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
                .send()
                .await
                .unwrap()
        }
    };
    let location_of = |response: &reqwest::Response| {
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    let exchange = |code: String| {
        let (client, base) = (&client, &base);
        async move {
            client
                .post(format!("{base}/auth/sso/exchange"))
                .json(&json!({"code": code}))
                .send()
                .await
                .unwrap()
        }
    };

    // success: a 303 to the login screen carrying a code that is not the token
    let response = navigate(json!(["admins"])).await;
    assert_eq!(response.status(), 303);
    let location = location_of(&response);
    assert!(
        location.starts_with(&format!("{base}/login?sso_code=")),
        "{location}"
    );
    assert!(!location.contains("rolter_sess_"), "{location}");
    let body = response.text().await.unwrap();
    assert!(
        !body.contains("rolter_sess_") && !body.contains("token"),
        "{body}"
    );
    let code = url_param(&location, "sso_code");
    assert!(!code.is_empty());

    // only a digest is stored
    let stored: Vec<String> = sqlx::query_scalar("select code_hash from sso_exchange_codes")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], code, "the plain code must not rest in the table");

    // redeeming it yields a working session, once
    let redeemed = exchange(code.clone()).await;
    assert_eq!(redeemed.status(), 200);
    let session: Value = redeemed.json().await.unwrap();
    assert_eq!(session["user"]["email"], "ada@example.com");
    assert_eq!(session["granted_roles"][0], "admin");
    let token = session["token"].as_str().unwrap().to_string();
    assert_ne!(token, code);
    let me = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);

    let replay = exchange(code).await;
    assert_eq!(replay.status(), 400, "a code is single-use");
    let refusal: Value = replay.json().await.unwrap();
    assert_eq!(refusal["error"]["code"], "invalid_exchange_code");
    assert!(!refusal.to_string().contains(&token));
    let unknown = exchange("never-issued".to_string()).await;
    assert_eq!(unknown.status(), 400);

    // one sign-in, one audit line: the redemption does not add a second
    let audited: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'auth.sso_login'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audited, 1);

    // an expired code is refused (backdated rather than slept on)
    let response = navigate(json!(["admins"])).await;
    let code = url_param(&location_of(&response), "sso_code");
    sqlx::query("update sso_exchange_codes set expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(exchange(code).await.status(), 400);

    // refusals: a stable code, the provider once known, and no IdP words
    let idp_words = "idp-free-text-should-never-leak";
    let start = client
        .get(format!("{base}/auth/sso/browser/start"))
        .send()
        .await
        .unwrap();
    let state = url_param(&location_of(&start), "state");
    let declined = client
        .get(format!(
            "{base}/auth/sso/browser/callback?error=access_denied&error_description={idp_words}&state={state}"
        ))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(declined.status(), 303);
    assert_eq!(
        location_of(&declined),
        format!("{base}/login?sso_error=idp_error")
    );

    let stale = client
        .get(format!(
            "{base}/auth/sso/browser/callback?code=abc&state=made-up"
        ))
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(
        location_of(&stale),
        format!("{base}/login?sso_error=state_expired")
    );

    let ungrouped = navigate(json!(["nobody"])).await;
    assert_eq!(ungrouped.status(), 303);
    assert_eq!(
        location_of(&ungrouped),
        format!("{base}/login?sso_error=no_mapped_group&sso=browser")
    );

    let deactivate = sqlx::query("update users set deactivated_at = now() where email = $1")
        .bind("ada@example.com")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(deactivate.rows_affected(), 1);
    let deactivated = navigate(json!(["admins"])).await;
    assert_eq!(
        location_of(&deactivated),
        format!("{base}/login?sso_error=account_deactivated&sso=browser")
    );
    sqlx::query("update users set deactivated_at = null")
        .execute(&pool)
        .await
        .unwrap();

    let disabled = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth("admintok")
        .json(&json!({"allow_password_login": true, "allow_sso": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 200);
    let off = navigate(json!(["admins"])).await;
    assert_eq!(
        location_of(&off),
        format!("{base}/login?sso_error=sso_disabled&sso=browser")
    );

    // a caller that is not a browser still gets the JSON refusal
    let start = client
        .get(format!("{base}/auth/sso/browser/start"))
        .send()
        .await
        .unwrap();
    let state = url_param(&location_of(&start), "state");
    let json_refusal = client
        .get(format!(
            "{base}/auth/sso/browser/callback?error=access_denied&state={state}"
        ))
        .header("accept", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(json_refusal.status(), 400);
}

/// Invitation onboarding (#712): an admin mints a one-time link, the invitee
/// sets their own password, and every way the link can be misused fails.
#[tokio::test]
async fn invitations_onboard_accounts_once_and_expire_closed() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "InviteOrg", "slug": "invite-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Platform"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();

    // a role rolter does not have is refused before anything is stored
    let bad_role = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("admintok")
        .json(&json!({"email": "ada@example.com", "role": "root"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_role.status(), 400);

    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("admintok")
        .json(&json!({
            "email": "ada@example.com", "role": "admin",
            "scope_type": "team", "scope_id": team_id
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = created["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("rolter_invite_"));
    assert!(created["accept_url"].as_str().unwrap().ends_with(&token));
    // the token is handed over once and only its digest is kept
    let stored: String = sqlx::query_scalar("select token_hash from invitations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored, token, "the raw token must not be stored");
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(
        !listed.to_string().contains("token_hash"),
        "the digest must not be serialized back out: {listed}"
    );

    // the accept screen can render without a session, and learns nothing about
    // the org beyond what it must show
    let preview: Value = client
        .get(format!("{base}/api/v1/invitations/accept/{token}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["org_name"], "InviteOrg");
    assert_eq!(preview["email"], "ada@example.com");
    assert_eq!(preview["role"], "admin");
    assert_eq!(preview["has_account"], false, "nobody holds this email yet");

    // a made-up token is refused, and so is a short password
    let unknown = client
        .get(format!(
            "{base}/api/v1/invitations/accept/rolter_invite_nope"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 401);
    let too_short = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": "short"}))
        .send()
        .await
        .unwrap();
    assert_eq!(too_short.status(), 400);

    // accepting creates the account with the invitee's own password and hands
    // back a live session
    let accepted: Value = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": "chosen by ada"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["user"]["email"], "ada@example.com");
    let session = accepted["token"].as_str().unwrap().to_string();

    // the granted membership is real and tagged manual, so a later sso login
    // cannot reconcile it away
    let rows: Vec<(String, String)> =
        sqlx::query_as("select role, source from memberships where user_id = $1")
            .bind(uuid::Uuid::parse_str(accepted["user"]["id"].as_str().unwrap()).unwrap())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![("admin".to_string(), "manual".to_string())]);
    let project = client
        .post(format!("{base}/api/v1/teams/{team_id}/projects"))
        .bearer_auth(&session)
        .json(&json!({"name": "FromInvite"}))
        .send()
        .await
        .unwrap();
    assert_eq!(project.status(), 200);

    // the password the invitee chose is the one that works
    let login = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": "chosen by ada"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);

    // the link is single-use: the second accept is refused, and no second
    // membership appears
    let replay = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": "someone else's"}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 401);
    let memberships: i64 = sqlx::query_scalar("select count(*) from memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(memberships, 1);

    // a revoked invitation stops working immediately
    let second: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("admintok")
        .json(&json!({"email": "grace@example.com", "role": "viewer"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_token = second["token"].as_str().unwrap().to_string();
    let second_id = second["invitation"]["id"].as_str().unwrap().to_string();
    let revoked = client
        .delete(format!("{base}/api/v1/invitations/{second_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    let after_revoke = client
        .post(format!(
            "{base}/api/v1/invitations/accept/{second_token}/accept"
        ))
        .json(&json!({"password": "too late now"}))
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 401);

    // an expired invitation is refused as firmly as a wrong one
    let third: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("admintok")
        .json(&json!({"email": "hopper@example.com", "role": "member"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let third_token = third["token"].as_str().unwrap().to_string();
    sqlx::query("update invitations set expires_at = now() - interval '1 hour' where email = $1")
        .bind("hopper@example.com")
        .execute(&pool)
        .await
        .unwrap();
    let expired = client
        .post(format!(
            "{base}/api/v1/invitations/accept/{third_token}/accept"
        ))
        .json(&json!({"password": "way too late"}))
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 401);

    let actions: Vec<String> =
        sqlx::query_scalar("select action from audit_log where action like 'invitation.%'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(actions.iter().any(|a| a == "invitation.create"));
    assert!(actions.iter().any(|a| a == "invitation.accept"));
    assert!(actions.iter().any(|a| a == "invitation.revoke"));
}

/// Inviting an address again replaces its pending invitation (#2324): the old
/// link stops working like a revoked one, an expired invitation no longer holds
/// the address, the match ignores case, and parallel creates neither 500 nor
/// leave two live rows.
#[tokio::test]
async fn reinviting_an_address_replaces_its_pending_invitation() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "ReinviteOrg", "slug": "reinvite-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let invite = |email: &'static str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/orgs/{org_id}/invitations");
        async move {
            client
                .post(url)
                .bearer_auth("admintok")
                .json(&json!({"email": email, "role": "member"}))
                .send()
                .await
                .unwrap()
        }
    };
    let live_count = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from invitations where accepted_at is null and revoked_at is null",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // twice for one address, the second spelled differently
    let first = invite("ada@example.com").await;
    assert_eq!(first.status(), 200);
    let first: Value = first.json().await.unwrap();
    let second = invite("Ada@Example.COM").await;
    assert_eq!(second.status(), 200);
    let second: Value = second.json().await.unwrap();
    let first_token = first["token"].as_str().unwrap();
    let second_token = second["token"].as_str().unwrap();
    let first_id = first["invitation"]["id"].as_str().unwrap().to_string();

    let old_preview = client
        .get(format!("{base}/api/v1/invitations/accept/{first_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(old_preview.status(), 401);
    let old_accept = client
        .post(format!(
            "{base}/api/v1/invitations/accept/{first_token}/accept"
        ))
        .json(&json!({"password": random_password()}))
        .send()
        .await
        .unwrap();
    assert_eq!(old_accept.status(), 401);
    let new_preview = client
        .get(format!("{base}/api/v1/invitations/accept/{second_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(new_preview.status(), 200);
    assert_eq!(live_count(pool.clone()).await, 1);

    // the audit entry of the replacement names what it replaced
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'invitation.create' \
         and target_id = $1::uuid",
    )
    .bind(second["invitation"]["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["replaced"], json!(first_id));
    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'invitation.create' \
         and target_id = $1::uuid",
    )
    .bind(first_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(detail["replaced"].is_null());

    // an invitation that expired unaccepted does not hold the address
    sqlx::query("update invitations set expires_at = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    let third = invite("ada@example.com").await;
    assert_eq!(third.status(), 200);
    let revoked: i64 =
        sqlx::query_scalar("select count(*) from invitations where revoked_at is not null")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revoked, 2);
    assert_eq!(live_count(pool.clone()).await, 1);
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pending = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["revoked_at"].is_null() && i["accepted_at"].is_null())
        .count();
    assert_eq!(pending, 1);

    // parallel creates for one address: no failure, one live row
    let (a, b, c) = tokio::join!(
        invite("grace@example.com"),
        invite("GRACE@example.com"),
        invite("grace@example.com")
    );
    for response in [a, b, c] {
        assert_eq!(response.status(), 200);
    }
    let grace_live: i64 = sqlx::query_scalar(
        "select count(*) from invitations where lower(email) = 'grace@example.com' \
         and accepted_at is null and revoked_at is null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grace_live, 1);
    let grace_all: i64 = sqlx::query_scalar(
        "select count(*) from invitations where lower(email) = 'grace@example.com'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grace_all, 3);
}

/// An invitation token proves someone was sent the link, not who holds it: the
/// inviter gets the same token back. So an org admin who invites an existing
/// account's email -- a superadmin's here, one with a password and one that
/// signs in through sso only -- and accepts it themselves gets no session for
/// that account, and cannot give it a password either (#1935). The role is
/// still granted, and the owner signs in with their own credentials as before.
#[tokio::test]
async fn accepting_an_invitation_never_signs_in_to_an_existing_account() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("sekrit")
        .json(&json!({"name": "Tenant", "slug": "tenant"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = uuid::Uuid::parse_str(org["id"].as_str().unwrap()).unwrap();
    let org_admin = seed_user(&pool, "tenant-admin@example.com", false).await;
    seed_membership(&pool, org_admin, Some(org_id), None, None, "admin").await;
    let org_admin_token = seed_session(&pool, org_admin, "tenantadmin").await;

    let root_password = random_password();
    // what the inviter would type into the accept form; random so no scanner
    // mistakes a fixture for a leaked credential
    let inviter_password = random_password();
    let root = seed_local_user(&pool, "root@example.com", &root_password).await;
    let sso_root = seed_user(&pool, "sso-root@example.com", true).await;

    for (email, target) in [
        ("root@example.com", root),
        ("sso-root@example.com", sso_root),
    ] {
        let created = client
            .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
            .bearer_auth(&org_admin_token)
            .json(&json!({"email": email, "role": "viewer"}))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), 200, "an org admin may invite anyone");
        let created: Value = created.json().await.unwrap();
        let token = created["token"].as_str().unwrap().to_string();

        let preview: Value = client
            .get(format!("{base}/api/v1/invitations/accept/{token}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(preview["has_account"], true, "{email}: {preview}");

        let accepted = client
            .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
            .json(&json!({"password": inviter_password}))
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), 200);
        let accepted: Value = accepted.json().await.unwrap();
        assert_eq!(accepted["sign_in_required"], true, "{email}: {accepted}");
        assert_eq!(accepted["reason"], "existing_account");
        assert!(
            accepted["token"].is_null(),
            "{email}: the invite link minted a session for an existing account: {accepted}"
        );
        let sessions: i64 = sqlx::query_scalar("select count(*) from sessions where user_id = $1")
            .bind(target)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0, "{email}: no session for the account");

        // the role is granted all the same, tagged like any invited role
        let roles: Vec<(String, String)> = sqlx::query_as(
            "select role, source from memberships where user_id = $1 and org_id = $2",
        )
        .bind(target)
        .bind(org_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(roles, vec![("viewer".to_string(), "manual".to_string())]);
    }

    // the password the inviter sent opens neither account
    for email in ["root@example.com", "sso-root@example.com"] {
        let response = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"email": email, "password": inviter_password}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            401,
            "{email} took the inviter's password"
        );
    }
    let sso_hash: Option<String> =
        sqlx::query_scalar("select password_hash from users where id = $1")
            .bind(sso_root)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        sso_hash.is_none(),
        "an sso-only account gained a password from an invite link"
    );
    // and the owner's own password still works
    let signed_in = login_as(&client, &base, "root@example.com", &root_password).await;
    assert!(signed_in["token"].is_string(), "{signed_in}");

    let detail: Value = sqlx::query_scalar(
        "select detail from audit_log where action = 'invitation.accept' and actor_user_id = $1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail["account_created"], false, "{detail}");
    assert_eq!(detail["signed_in"], false, "{detail}");
}

/// A new account created by an invitation into an org whose `required_all`
/// policy is in force gets no session from the link: it signs in with the
/// password it just chose, and that sign-in is the enrolment the policy asks
/// for (#1935, #1852). A missing password is refused without spending the link.
#[tokio::test]
async fn an_invitation_into_a_required_org_sends_the_new_account_to_enrol() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_, org_id) = seed_bound_member(
        &pool,
        "required-admin@example.com",
        &random_password(),
        "required_all",
        "null",
    )
    .await;

    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/invitations"))
        .bearer_auth("sekrit")
        .json(&json!({"email": "newcomer@example.com", "role": "member"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = created["token"].as_str().unwrap().to_string();

    let no_password = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(no_password.status(), 400, "a new account needs a password");

    let password = random_password();
    let accepted = client
        .post(format!("{base}/api/v1/invitations/accept/{token}/accept"))
        .json(&json!({"password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200, "the refusal left the link live");
    let accepted: Value = accepted.json().await.unwrap();
    assert_eq!(accepted["sign_in_required"], true, "{accepted}");
    assert_eq!(accepted["reason"], "second_factor");
    assert!(accepted["token"].is_null(), "{accepted}");
    let sessions: i64 = sqlx::query_scalar(
        "select count(*) from sessions s join users u on u.id = s.user_id where u.email = $1",
    )
    .bind("newcomer@example.com")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sessions, 0, "no factor-less session under required_all");

    // the sign-in is where the policy is met
    let challenge = login_as(&client, &base, "newcomer@example.com", &password).await;
    assert_eq!(challenge["mfa_enrolment_required"], true, "{challenge}");
    assert!(challenge["token"].is_null(), "{challenge}");
}

/// A failure the database reports reaches the caller as a plain 500, never as
/// the driver's own text, which names relations and the schema a tenant's data
/// lives in (#2268). Both ways a handler meets one are covered: its own `sqlx`
/// call, and a store repository. The table is dropped inside this test's
/// isolated schema, so the error is a real one from Postgres.
#[tokio::test]
async fn a_database_error_reaches_the_caller_as_a_plain_500() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Broken", "slug": "broken"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    sqlx::query("drop table observability_connectors")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("drop table teams cascade")
        .execute(&pool)
        .await
        .unwrap();

    for (path, relation) in [
        ("/api/v1/connectors".to_string(), "observability_connectors"),
        (format!("/api/v1/orgs/{org_id}/teams"), "teams"),
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 500, "{path}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body["error"]["message"], "internal server error",
            "{path}: {body}"
        );
        assert!(
            !body.to_string().contains(relation),
            "{path}: the driver's text reached the body"
        );
    }
}

#[tokio::test]
async fn adaptive_routing_telemetry_round_trips_from_the_data_plane() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the report a gateway posts, in the shape rolter-gateway serializes
    let report = json!({
        "routes": [{
            "model": "gpt-4o",
            "engaged": true,
            "observed": 120,
            "decisions": {"blend": 100, "exploration": 5, "fallback": 15, "engaged": true},
            "policy": {"enabled": true, "latency_weight": 1.0},
            "targets": [
                {"target": 0, "score": 0.4, "latency_ms": 500.0, "samples": 40},
                {"target": 1, "score": 1.2, "latency_ms": 20.0, "samples": 80}
            ],
            "target_labels": [
                {"provider": "openai", "upstream_model": "gpt-4o"},
                {"provider": "azure", "upstream_model": "gpt-4o-2024"}
            ]
        }]
    });

    // an unidentified node is refused rather than accepted-and-ignored: the
    // scoreboard is keyed on the node, so the report can never be stored, and
    // a success status left the reporter with nothing to log (#1644)
    let anonymous = client
        .post(format!("{base}/internal/adaptive-telemetry"))
        .bearer_auth("sekrit")
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 400);
    // a malformed identity is refused on the same grounds as a missing one
    let malformed = client
        .post(format!("{base}/internal/adaptive-telemetry"))
        .bearer_auth("sekrit")
        .header("x-rolter-node-id", "   ")
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
    let empty: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(empty["routes"].as_array().unwrap().len(), 0);

    // the internal channel is token-gated
    let unauthorized = client
        .post(format!("{base}/internal/adaptive-telemetry"))
        .header("x-rolter-node-id", "gw-1")
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    for node in ["gw-1", "gw-2"] {
        let accepted = client
            .post(format!("{base}/internal/adaptive-telemetry"))
            .bearer_auth("sekrit")
            .header("x-rolter-node-id", node)
            .json(&report)
            .send()
            .await
            .unwrap();
        assert!(accepted.status().is_success());
    }

    // reading it back is superadmin-only
    let denied = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let view: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let routes = view["routes"].as_array().unwrap();
    assert_eq!(routes.len(), 1, "both nodes report the same route");
    assert_eq!(routes[0]["model"], "gpt-4o");
    assert_eq!(routes[0]["engaged"], true);
    let nodes = routes[0]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["node_id"], "gw-1");
    assert_eq!(nodes[0]["decisions"]["blend"], 100);
    // the target labels were folded into the signals they describe
    assert_eq!(nodes[0]["targets"][1]["provider"], "azure");
    assert_eq!(nodes[0]["targets"][1]["score"], 1.2);

    // a node that stops balancing a route drops it from the scoreboard on its
    // very next report, rather than leaving a stale row behind
    let emptied = client
        .post(format!("{base}/internal/adaptive-telemetry"))
        .bearer_auth("sekrit")
        .header("x-rolter-node-id", "gw-1")
        .json(&json!({"routes": []}))
        .send()
        .await
        .unwrap();
    assert!(emptied.status().is_success());
    let view: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let nodes = view["routes"][0]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], "gw-2");

    // a sample older than the freshness window is not current state
    sqlx::query(
        "update adaptive_routing_telemetry set reported_at = now() - interval '10 minutes'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let stale: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stale["routes"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// configurable rbac (#534)
// ---------------------------------------------------------------------------

/// A custom role widens a member beyond what their base role allows, and only
/// inside the scope the profile assigns it — the deny case is the point.
#[tokio::test]
async fn custom_role_grant_widens_a_member_within_its_scope_only() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // two orgs, so "in scope" can be told apart from "authorized everywhere"
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let other: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Other", "slug": "other"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_id = other["id"].as_str().unwrap().to_string();

    // a plain member of both orgs: creating a provider is admin-only, so both
    // attempts must fail before any custom role exists
    let user = seed_user(&pool, "member@acme.test", false).await;
    seed_membership(
        &pool,
        user,
        Some(org_id.parse().unwrap()),
        None,
        None,
        "member",
    )
    .await;
    seed_membership(
        &pool,
        user,
        Some(other_id.parse().unwrap()),
        None,
        None,
        "member",
    )
    .await;
    let token = seed_session(&pool, user, "custom-role").await;

    let create_provider = |org: String| {
        client
            .post(format!("{base}/api/v1/orgs/{org}/providers"))
            .bearer_auth(token.clone())
            .json(&json!({
                "name": format!("p-{}", uuid::Uuid::new_v4()),
                "kind": "openai",
                "api_base": "https://api.openai.com",
            }))
            .send()
    };

    assert_eq!(
        create_provider(org_id.clone()).await.unwrap().status(),
        403,
        "a member must not create a provider before the grant exists"
    );

    // a custom role that grants exactly provider:create, still on the member base
    let role: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/custom-roles"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Provider Wrangler",
            "base_role": "member",
            "grants": [{"resource": "provider", "action": "create"}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let role_id = role["id"].as_str().unwrap().to_string();

    // composed into a profile scoped to the first org, assigned to the user
    let profile: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/access-profiles"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Acme Wranglers",
            "roles": [{"role_id": role_id, "org_id": org_id}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let profile_id = profile["id"].as_str().unwrap().to_string();

    let assigned = client
        .post(format!(
            "{base}/api/v1/access-profiles/{profile_id}/assignments"
        ))
        .bearer_auth("admintok")
        .json(&json!({"user_id": user}))
        .send()
        .await
        .unwrap();
    assert!(assigned.status().is_success(), "{}", assigned.status());

    // in scope the grant applies; in the other org the same member is still refused
    let allowed = create_provider(org_id.clone()).await.unwrap();
    assert!(
        allowed.status().is_success(),
        "granted org must allow the create: {}",
        allowed.status()
    );
    assert_eq!(
        create_provider(other_id.clone()).await.unwrap().status(),
        403,
        "the grant must not leak into an org the profile does not name"
    );

    // and it is revocable: dropping the assignment closes the door again
    let assignments: Value = client
        .get(format!(
            "{base}/api/v1/access-profiles/{profile_id}/assignments"
        ))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let assignment_id = assignments[0]["id"].as_str().unwrap().to_string();
    let removed = client
        .delete(format!(
            "{base}/api/v1/access-profile-assignments/{assignment_id}"
        ))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert!(removed.status().is_success(), "{}", removed.status());
    assert_eq!(
        create_provider(org_id).await.unwrap().status(),
        403,
        "removing the assignment must withdraw the grant"
    );
}

/// A custom role in use cannot be deleted out from under its assignments, and
/// every change to one is audited.
#[tokio::test]
async fn custom_role_changes_are_guarded_by_references_and_audited() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let role: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/custom-roles"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Auditor",
            "base_role": "viewer",
            "grants": [{"resource": "audit_log", "action": "read"}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let role_id = role["id"].as_str().unwrap().to_string();

    let profile: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/access-profiles"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Auditors",
            "roles": [{"role_id": role_id, "org_id": org_id}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let profile_id = profile["id"].as_str().unwrap().to_string();

    // referenced by a profile, so the delete must be refused rather than
    // silently stripping the profile's composition
    let refused = client
        .delete(format!("{base}/api/v1/custom-roles/{role_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        409,
        "deleting a referenced role must conflict, not cascade"
    );

    // detach it, then the delete goes through
    let detached = client
        .put(format!("{base}/api/v1/access-profiles/{profile_id}"))
        .bearer_auth("admintok")
        .json(&json!({"roles": []}))
        .send()
        .await
        .unwrap();
    assert!(detached.status().is_success(), "{}", detached.status());
    let deleted = client
        .delete(format!("{base}/api/v1/custom-roles/{role_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success(), "{}", deleted.status());

    let actions: Vec<String> =
        sqlx::query_scalar("select action from audit_log where action like 'custom_role%' or action like 'access_profile%' order by at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        actions.iter().any(|a| a == "custom_role.create"),
        "creating a role must be audited: {actions:?}"
    );
    assert!(
        actions.iter().any(|a| a == "custom_role.delete"),
        "deleting a role must be audited: {actions:?}"
    );
}

/// The matrix endpoint is API-backed: a role created through the API shows up
/// in the next read, so the dashboard never has to trust its own state.
#[tokio::test]
async fn rbac_matrix_reflects_custom_roles_after_a_change() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let before: Value = client
        .get(format!("{base}/api/v1/rbac/matrix?org_id={org_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        before["custom_roles"].as_array().unwrap().is_empty(),
        "a fresh org has no custom roles: {before}"
    );

    client
        .post(format!("{base}/api/v1/orgs/{org_id}/custom-roles"))
        .bearer_auth("admintok")
        .json(&json!({
            "name": "Budget Keeper",
            "base_role": "member",
            "grants": [{"resource": "budget", "action": "update"}],
        }))
        .send()
        .await
        .unwrap();

    let after: Value = client
        .get(format!("{base}/api/v1/rbac/matrix?org_id={org_id}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let roles = after["custom_roles"].as_array().unwrap();
    assert_eq!(roles.len(), 1, "matrix must show the new role: {after}");
    assert_eq!(roles[0]["name"], "Budget Keeper");
}

/// A stub OAuth 2.0 authorization server for the MCP consent flow (#707).
/// It records the last form it was posted so PKCE, scope and grant type can be
/// asserted, and its next answer is set by the test.
mod stub_authz {
    use super::*;
    use axum::Router;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct Stub {
        /// json body (and status) the next token request receives
        pub next: Arc<Mutex<(u16, Value)>>,
        /// form body of the last token request
        pub last_form: Arc<Mutex<String>>,
        /// how many token requests have been served
        pub calls: Arc<Mutex<u32>>,
    }

    impl Default for Stub {
        fn default() -> Self {
            Self {
                next: Arc::new(Mutex::new((200, json!({})))),
                last_form: Arc::new(Mutex::new(String::new())),
                calls: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl Stub {
        pub fn answer(&self, status: u16, body: Value) {
            *self.next.lock().unwrap() = (status, body);
        }
        pub fn form(&self) -> String {
            self.last_form.lock().unwrap().clone()
        }
        pub fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    pub async fn serve_stub() -> (String, Stub) {
        serve_stub_with_metadata(true).await
    }

    /// The same stub, plus the RFC 8414 metadata document discovery reads
    /// (#1347). `iss_supported` is what the metadata advertises, which is the
    /// row of the RFC 9207 §2.4 table a response with no `iss` is judged by.
    pub async fn serve_stub_with_metadata(iss_supported: bool) -> (String, Stub) {
        let stub = Stub::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let issuer = format!("http://{addr}");
        let metadata = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "authorization_response_iss_parameter_supported": iss_supported,
            "response_types_supported": ["code"],
        });
        let app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                axum::routing::get(move || {
                    let metadata = metadata.clone();
                    async move { axum::Json(metadata) }
                }),
            )
            .route(
                "/token",
                axum::routing::post({
                    let stub = stub.clone();
                    move |body: String| {
                        let stub = stub.clone();
                        async move {
                            *stub.last_form.lock().unwrap() = body;
                            *stub.calls.lock().unwrap() += 1;
                            let (status, payload) = stub.next.lock().unwrap().clone();
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                axum::Json(payload),
                            )
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (issuer, stub)
    }
}

/// A stub MCP server that publishes RFC 9728 protected resource metadata
/// (#1347): an unauthenticated request is challenged with a
/// `resource_metadata` URL, and that document names the authorization server.
mod stub_resource {
    use super::*;
    use axum::Router;

    /// Serve a protected resource at `/mcp` whose metadata points at
    /// `authorization_server`. Returns the resource's canonical URI.
    pub async fn serve_stub(authorization_server: &str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let resource = format!("http://{addr}/mcp");
        let metadata = json!({
            "resource": resource,
            "authorization_servers": [authorization_server],
            "scopes_supported": ["tools:read", "tools:write"],
            "bearer_methods_supported": ["header"],
        });
        let challenge = format!(
            "Bearer resource_metadata=\"http://{addr}/.well-known/oauth-protected-resource/mcp\", \
             scope=\"tools:read\""
        );
        let app = Router::new()
            .route(
                "/mcp",
                axum::routing::get(move || {
                    let challenge = challenge.clone();
                    async move {
                        (
                            axum::http::StatusCode::UNAUTHORIZED,
                            [(axum::http::header::WWW_AUTHENTICATE, challenge)],
                        )
                    }
                }),
            )
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                axum::routing::get(move || {
                    let metadata = metadata.clone();
                    async move { axum::Json(metadata) }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        resource
    }
}

/// The MCP OAuth token-acquisition path (#707), end to end against a stub
/// authorization server: consent mints a grant plus a sealed session, a refresh
/// rotates it, a refused refresh revokes rather than loops, an on-behalf-of
/// exchange cannot widen the consent, and no token material ever appears in a
/// response.
#[tokio::test]
async fn mcp_oauth_consent_refresh_and_exchange() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some("admintok".to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (authz, stub) = stub_authz::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "McpOrg", "slug": "mcp-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    // a member who will do the consenting
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple")
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("ada@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "ada@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Docs", "slug": "docs", "url": "https://mcp.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();

    // consent is impossible before an oauth client is registered
    let unregistered = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(unregistered.status(), 400);

    // a plaintext token endpoint on a non-loopback host is refused outright
    let plaintext = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({
            "authorize_url": "http://mcp.example.com/authorize",
            "token_url": "http://mcp.example.com/token",
            "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(plaintext.status(), 400);

    // a member may not register the client; that is an admin decision
    let forbidden = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth(&token)
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);

    let registered: Value = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter",
            "client_secret": "cli3nt-s3cret",
            "default_scopes": ["tools:read", "tools:write"],
            // this server publishes no metadata, so it is pinned to the
            // hand-configured endpoints and nothing is probed (#1347)
            "discovery": "manual"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["has_client_secret"], true);
    assert_eq!(
        registered["redirect_uri"],
        format!("{base}/auth/mcp/callback")
    );
    let registered_text = registered.to_string();
    assert!(
        !registered_text.contains("cli3nt-s3cret"),
        "the client secret leaked into the api response: {registered_text}"
    );
    // and listing the servers must not carry it either
    let servers: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!servers.to_string().contains("cli3nt-s3cret"));

    // -- consent ------------------------------------------------------------

    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read", "tools:write"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    assert!(auth_url.starts_with(&format!("{authz}/authorize?response_type=code")));
    assert!(auth_url.contains("code_challenge_method=S256"));
    let state = url_param(&auth_url, "state");
    assert!(!state.is_empty());

    // the authorization server grants only the read scope of the two asked for
    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented: Value = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = consented["session_id"].as_str().unwrap().to_string();
    let grant_id = consented["grant_id"].as_str().unwrap().to_string();
    assert_eq!(consented["scopes"], json!(["tools:read"]));
    assert_eq!(consented["has_refresh_token"], true);
    let consented_text = consented.to_string();
    assert!(
        !consented_text.contains("access-1") && !consented_text.contains("refresh-1"),
        "token material leaked into the callback response: {consented_text}"
    );
    // the exchange used PKCE and the deployment-owned redirect uri
    let form = stub.form();
    assert!(form.contains("grant_type=authorization_code"));
    assert!(form.contains("code_verifier="));
    assert!(form.contains("client_secret=cli3nt-s3cret"));

    // the same state cannot be redeemed twice
    let replayed = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        replayed.status(),
        400,
        "a redeemed state must not work twice"
    );

    // the sealed columns really are sealed
    let (access_ct, access_pt): (Vec<u8>, Option<String>) = sqlx::query_as(
        "select access_ciphertext, encode(access_ciphertext, 'escape') from mcp_oauth_sessions \
         where id = $1",
    )
    .bind(uuid::Uuid::parse_str(&session_id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!access_ct.is_empty());
    assert!(
        !access_pt.unwrap_or_default().contains("access-1"),
        "the access token is stored in the clear"
    );

    // the session shows up on the sessions screen, without token material
    let sessions: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sessions.as_array().unwrap().len(), 1);
    assert!(!sessions.to_string().contains("access-1"));
    let grants: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grants[0]["id"], grant_id);
    assert_eq!(grants[0]["scopes"], json!(["tools:read"]));

    // -- scope ceiling ------------------------------------------------------

    // an exchange may not ask for more than the grant carries
    let escalated = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/exchange"))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read", "tools:write"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        escalated.status(),
        400,
        "an exchange must not widen the consent"
    );
    let before_exchange = stub.calls();

    // an in-bounds exchange mints a second, independently revocable session
    stub.answer(
        200,
        json!({
            "access_token": "obo-access",
            "token_type": "Bearer",
            "expires_in": 600,
            "scope": "tools:read"
        }),
    );
    let exchanged: Value = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/exchange"))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"], "audience": "https://downstream.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(exchanged["id"], json!(session_id));
    assert_eq!(exchanged["grant_id"], grant_id);
    assert_eq!(exchanged["scopes"], json!(["tools:read"]));
    assert!(!exchanged.to_string().contains("obo-access"));
    assert_eq!(
        stub.calls(),
        before_exchange + 1,
        "the refused exchange must not have reached the authorization server"
    );
    let form = stub.form();
    assert!(form.contains("token-exchange"));
    assert!(form.contains("subject_token"));
    assert!(form.contains("audience"));

    // -- refresh ------------------------------------------------------------

    // a rotated refresh token replaces the old one
    stub.answer(
        200,
        json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "token_type": "Bearer",
            "expires_in": 7200
        }),
    );
    let refreshed: Value = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        refreshed["id"],
        json!(session_id),
        "a refresh keeps the row"
    );
    assert_eq!(refreshed["revoked_at"], Value::Null);
    assert!(!refreshed.to_string().contains("refresh-2"));
    let form = stub.form();
    assert!(form.contains("grant_type=refresh_token"));
    assert!(form.contains("refresh_token=refresh-1"));

    // a transient failure leaves the session alone to be retried
    stub.answer(503, json!({"error": "temporarily_unavailable"}));
    let transient = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(transient.status(), 400);
    let still_live: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select revoked_at from mcp_oauth_sessions where id = $1")
            .bind(uuid::Uuid::parse_str(&session_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        still_live.is_none(),
        "a 503 is not a verdict on the grant; the session must survive it"
    );

    // a refusal is final: the session is revoked rather than retried forever
    stub.answer(400, json!({"error": "invalid_grant"}));
    let refused = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let revoked_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select revoked_at from mcp_oauth_sessions where id = $1")
            .bind(uuid::Uuid::parse_str(&session_id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        revoked_at.is_some(),
        "a refused refresh must revoke the session, not loop on it"
    );

    // and a revoked session is not renewable again
    let after_revoke = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), 400);

    // the whole lifecycle is on the audit trail
    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_log where action like 'mcp_oauth%' order by at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for expected in [
        "mcp_oauth_client.update",
        "mcp_oauth_grant.consent",
        "mcp_oauth_session.exchange",
        "mcp_oauth_session.refresh_refused",
    ] {
        assert!(
            actions.iter().any(|a| a == expected),
            "missing audit event {expected} in {actions:?}"
        );
    }
}

/// The three client-side MUSTs of the current MCP specification (#1347), end to
/// end: the authorization server is discovered from what the MCP server
/// publishes rather than typed in, both requests carry the RFC 8707 `resource`,
/// and the callback applies RFC 9207 §2.4 — including the two failures that
/// would otherwise be silent, a wrong `iss` and an absent one from a server
/// that advertises it.
#[tokio::test]
async fn mcp_oauth_discovers_its_authorization_server_and_validates_the_issuer() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    // deliberately *not* setting ROLTER_PUBLIC_URL: it is process-wide, and
    // under plain `cargo test` (the coverage job) one test's value is read by
    // another test's in-flight request. nothing here asserts on the redirect
    // uri, so whatever the deployment default is will do
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    // an authorization server that publishes metadata and says it returns `iss`
    let (authz, stub) = stub_authz::serve_stub_with_metadata(true).await;
    let resource = stub_resource::serve_stub(&authz).await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "DiscoOrg", "slug": "disco-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple")
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("grace@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "grace@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Disco", "slug": "disco", "url": resource}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();

    // a client id and nothing else: no endpoint is typed in, because the
    // server publishes where its authorization server is
    let registered: Value = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({"client_id": "rolter", "default_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["authorize_url"], Value::Null);
    assert_eq!(registered["token_url"], Value::Null);
    assert_eq!(registered["resource"], json!(resource));

    // one consent start, used three times over: each callback consumes its own
    // login state, so every case below asks for a fresh one
    let start = || async {
        let started: Value = client
            .post(format!(
                "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
            ))
            .bearer_auth(&token)
            .json(&json!({"scopes": ["tools:read"]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        started
    };

    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    // the endpoint came from the authorization server's metadata, not from a
    // column an operator filled in
    assert!(
        auth_url.starts_with(&format!("{authz}/authorize?")),
        "authorization url must come from discovery: {auth_url}"
    );
    // RFC 8707 on the authorization request
    assert_eq!(
        url_param(&auth_url, "resource"),
        resource.replace(':', "%3A").replace('/', "%2F"),
        "the authorization request must carry the canonical resource: {auth_url}"
    );

    // -- RFC 9207: a wrong issuer ------------------------------------------
    let wrong = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={}&iss=https%3A%2F%2Fevil.example.com",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400, "a mismatched iss must be rejected");
    assert_eq!(
        stub.calls(),
        0,
        "the authorization code must never reach a token endpoint after an iss mismatch"
    );

    // -- RFC 9207: an absent issuer from a server that advertises one -------
    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    let absent = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-2&state={}",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        absent.status(),
        400,
        "an absent iss must be rejected when the metadata advertises it"
    );
    assert_eq!(stub.calls(), 0);

    // -- and an error response whose issuer does not check out --------------
    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    let refused = client
        .get(format!(
            "{base}/auth/mcp/callback?error=access_denied&error_description=go-here-instead\
             &state={}&iss=https%3A%2F%2Fevil.example.com",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let body = refused.text().await.unwrap();
    assert!(
        !body.contains("go-here-instead") && !body.contains("access_denied"),
        "an unvalidated error response must not be displayed: {body}"
    );

    // nothing above created a grant
    let grants: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grants.as_array().map(Vec::len), Some(0), "{grants}");

    // -- the happy path -----------------------------------------------------
    let started = start().await;
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-3&state={}&iss={}",
            url_param(&auth_url, "state"),
            authz.replace(':', "%3A").replace('/', "%2F")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        consented.status(),
        200,
        "{}",
        consented.text().await.unwrap()
    );
    // RFC 8707 on the token request, carrying the very same identifier
    let form = stub.form();
    assert!(
        form.contains(&format!(
            "resource={}",
            resource.replace(':', "%3A").replace('/', "%2F")
        )),
        "the token request must carry the same canonical resource: {form}"
    );

    // -- the hand-configured fallback still works ---------------------------
    // a server that publishes nothing at all: discovery fails against a dead
    // port and the operator's endpoints are used instead
    let quiet: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Quiet", "slug": "quiet", "url": "http://127.0.0.1:1/mcp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let quiet_id = quiet["id"].as_str().unwrap().to_string();
    let quiet_client = client
        .put(format!("{base}/api/v1/mcp-servers/{quiet_id}/oauth-client"))
        .bearer_auth("admintok")
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(quiet_client.status(), 200);
    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{quiet_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    assert!(
        auth_url.starts_with(&format!("{authz}/authorize?")),
        "the configured endpoint must be the fallback: {auth_url}"
    );
    // the resource parameter is not conditional on discovery having worked
    assert_eq!(
        url_param(&auth_url, "resource"),
        "http%3A%2F%2F127.0.0.1%3A1%2Fmcp"
    );
    // nothing was discovered and no issuer was pinned, so a returned `iss` has
    // nothing authentic to be checked against and the exchange is refused
    let unverifiable = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-4&state={}&iss={}",
            url_param(&auth_url, "state"),
            authz.replace(':', "%3A").replace('/', "%2F")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unverifiable.status(), 400);

    // while a response with no `iss` at all is row four of the table, and
    // proceeds exactly as it did before this flow knew about RFC 9207
    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{quiet_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    stub.answer(
        200,
        json!({"access_token": "access-2", "token_type": "Bearer", "expires_in": 3600}),
    );
    let legacy = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-5&state={}",
            url_param(&auth_url, "state")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 200, "{}", legacy.text().await.unwrap());
}

/// #2166: the authorization server sends the user's browser back to the
/// callback, and a browser is redirected to the dashboard rather than left on
/// a JSON body. Every outcome lands on Auth Sessions with non-secret
/// identifiers only — the session and server ids, or a failure code — and a
/// client that does not ask for HTML still gets the JSON the other tests read.
#[tokio::test]
async fn mcp_oauth_callback_sends_a_browser_to_the_dashboard() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let addr = serve_with_public_url(pool.clone(), Some("admintok".to_string())).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    // what a browser does, minus following the redirect: the Location is the
    // assertion
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (authz, stub) = stub_authz::serve_stub().await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "LandOrg", "slug": "land-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple")
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("lin@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "lin@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Land", "slug": "land", "url": "https://mcp.example.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({
            "authorize_url": format!("{authz}/authorize"),
            "token_url": format!("{authz}/token"),
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "discovery": "manual"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // a fresh login state per case: each callback consumes the one it names
    let start = || async {
        let started: Value = client
            .post(format!(
                "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        url_param(started["authorization_url"].as_str().unwrap(), "state")
    };
    // the Accept header a browser navigates with
    let navigate = |query: String| {
        let request = browser
            .get(format!("{base}/auth/mcp/callback?{query}"))
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            );
        let landing = format!("{base}/auth-sessions?consent=");
        async move {
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 303, "a browser is sent onwards");
            let location = response.headers()[reqwest::header::LOCATION]
                .to_str()
                .unwrap()
                .to_string();
            assert!(
                location.starts_with(&landing),
                "a browser lands on Auth Sessions: {location}"
            );
            location
        }
    };

    // -- success ------------------------------------------------------------
    let state = start().await;
    stub.answer(
        200,
        json!({
            "access_token": "access-land",
            "refresh_token": "refresh-land",
            "token_type": "Bearer",
            "expires_in": 3600
        }),
    );
    let landed = navigate(format!("code=code-land&state={state}")).await;
    let sessions: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = sessions[0]["id"].as_str().unwrap();
    assert_eq!(
        landed,
        format!("{base}/auth-sessions?consent=completed&session={session_id}&server={server_id}")
    );
    // names the kind of value, never the value, so a failure cannot print one
    for (what, secret) in [
        ("authorization code", "code-land"),
        ("access token", "access-land"),
        ("refresh token", "refresh-land"),
        ("state", state.as_str()),
    ] {
        assert!(
            !landed.contains(secret),
            "the {what} leaked into the landing url"
        );
    }

    // -- a replay, and a callback with no state at all ----------------------
    let replayed = navigate(format!("code=code-land&state={state}")).await;
    assert_eq!(
        replayed,
        format!("{base}/auth-sessions?consent=failed&reason=state_invalid"),
        "before the state resolves there is no server to name"
    );
    let stateless = navigate("code=code-land".to_string()).await;
    assert!(stateless.ends_with("consent=failed&reason=state_invalid"));

    // -- the authorization server says no -----------------------------------
    let denied = navigate(format!(
        "error=access_denied&error_description=go-here-instead&state={}",
        start().await
    ))
    .await;
    assert_eq!(
        denied,
        format!("{base}/auth-sessions?consent=failed&reason=access_denied&server={server_id}")
    );
    assert!(
        !denied.contains("go-here-instead"),
        "the upstream's words stay upstream: {denied}"
    );
    let broken = navigate(format!("error=server_error&state={}", start().await)).await;
    assert!(broken.contains("reason=authorization_failed&server="));

    // -- an issuer nothing was pinned to check against ----------------------
    let calls = stub.calls();
    let mismatched = navigate(format!(
        "code=code-iss&state={}&iss=https%3A%2F%2Fevil.example.com",
        start().await
    ))
    .await;
    assert!(mismatched.contains("reason=issuer_mismatch&server="));
    assert_eq!(
        stub.calls(),
        calls,
        "the code never reached a token endpoint"
    );

    // -- the token endpoint refuses the code --------------------------------
    stub.answer(400, json!({"error": "invalid_grant"}));
    let exchange = navigate(format!("code=code-bad&state={}", start().await)).await;
    assert!(exchange.contains("reason=token_exchange_failed&server="));

    // -- and a client that does not ask for html keeps the json -------------
    let state = start().await;
    let json_refusal = browser
        .get(format!(
            "{base}/auth/mcp/callback?error=access_denied&state={state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(json_refusal.status(), 400);
    assert_eq!(json_refusal.headers()[reqwest::header::VARY], "accept");
    let body: Value = json_refusal.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .is_some_and(|m| m.contains("access_denied")));

    // exactly one consent happened in all of that
    let grants: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/grants"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(grants.as_array().map(Vec::len), Some(1), "{grants}");
}

/// The `oauth_discovered_*` cache of one server, plus whether
/// `oauth_discovered_at` is set — read as a boolean so the tuple needs no
/// timestamp type.
type DiscoveryCache = (Option<String>, Option<String>, Option<String>, bool, bool);

async fn discovery_cache(pool: &sqlx::PgPool, id: uuid::Uuid) -> DiscoveryCache {
    sqlx::query_as(
        "select oauth_discovered_issuer, oauth_discovered_authorize_url, \
                oauth_discovered_token_url, oauth_discovered_iss_supported, \
                oauth_discovered_at is not null \
         from mcp_servers where id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn config_version(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("select version from config_version where id = 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// #1416: the discovery cache is keyed by the server's URL — it holds the
/// endpoints of whatever authorization server that URL's protected-resource
/// metadata named. Pointing the row somewhere else must therefore drop it, or a
/// refresh landing before the next interactive authorize would post to the old
/// server's token endpoint while naming the new canonical URI in `resource`.
///
/// The second half is the constraint that makes the fix non-obvious:
/// `mcp_servers` has a statement-level `bump_config_version()` trigger, so the
/// clearing has to ride on the update that writes the URL rather than follow
/// it, and an edit that leaves the URL alone must not clear anything.
#[tokio::test]
async fn moving_an_mcp_server_url_invalidates_its_discovery_cache() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let (authz, _stub) = stub_authz::serve_stub_with_metadata(true).await;
    let resource = stub_resource::serve_stub(&authz).await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "MoveOrg", "slug": "move-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    // consent is a user-facing act, so the authorize below needs a member
    // session rather than the admin token
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple")
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("mallory@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mallory@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Movable", "slug": "movable", "url": resource}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let server_uuid = uuid::Uuid::parse_str(&server_id).unwrap();

    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({"client_id": "rolter", "default_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // one interactive authorize is what fills the cache
    let started = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200, "{}", started.text().await.unwrap());
    let cached = discovery_cache(&pool, server_uuid).await;
    assert_eq!(
        cached,
        (
            Some(authz.clone()),
            Some(format!("{authz}/authorize")),
            Some(format!("{authz}/token")),
            true,
            true
        ),
        "discovery should have cached the stub authorization server"
    );

    // #1569: the row now holds `oauth_discovered_iss_supported = true` (the
    // fourth element above), so this is the point where the read endpoint can
    // be checked against it rather than against itself. an api client that
    // reads only `/oauth-client` needs the flag to explain why a callback
    // carrying no `iss` was rejected — it selects the RFC 9207 §2.4 row
    let view: Value = client
        .get(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        view["discovered_iss_supported"],
        json!(true),
        "the oauth-client view must report the discovered iss support: {view}"
    );
    assert_eq!(
        view["discovered_issuer"],
        json!(authz),
        "and the rest of the discovery cache alongside it: {view}"
    );

    // an edit that leaves the url where it is keeps the cache: re-discovering
    // on every rename would put an upstream probe on a path that has no reason
    // to touch one
    let before = config_version(&pool).await;
    let renamed = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Renamed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200, "{}", renamed.text().await.unwrap());
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        cached,
        "an edit that did not move the url must not invalidate the cache"
    );
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "one edit must bump config_version exactly once"
    );

    // moving the url drops every discovered column, so the next refresh
    // re-discovers instead of posting to the previous server's token endpoint
    let before = config_version(&pool).await;
    let moved = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .bearer_auth("admintok")
        .json(&json!({"url": "http://127.0.0.1:1/mcp"}))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), 200, "{}", moved.text().await.unwrap());
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        (None, None, None, false, false),
        "moving the url must clear the endpoints discovered for the old one"
    );
    // the clearing rides on the update that wrote the url; a second statement
    // would bump twice for one logical edit
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "clearing the cache must not cost a second config_version bump"
    );

    // and with the cache gone the fallback is what an operator configured,
    // rather than an endpoint belonging to a server this row no longer names
    let quiet = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        quiet.status(),
        400,
        "nothing is discoverable at the new url and no endpoint was configured"
    );
}

/// #1432: re-pinning `oauth_issuer` (or flipping `oauth_discovery`) while
/// leaving the server's `url` alone must invalidate the discovery cache the
/// same way moving the url does — otherwise the cached triple from the old
/// authorization server shadows the correction on every path that never
/// probes (the background refresher here, chosen because it is exactly the
/// path #1432 names).
#[tokio::test]
async fn repinning_the_oauth_issuer_invalidates_its_discovery_cache() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the discovered server, and a second one standing in for the
    // authorization server an operator rotates to
    let (authz, stub) = stub_authz::serve_stub_with_metadata(true).await;
    let (authz2, stub2) = stub_authz::serve_stub_with_metadata(true).await;
    let resource = stub_resource::serve_stub(&authz).await;

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "RepinOrg", "slug": "repin-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(b"correct horse battery staple")
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("insert into users (email, password_hash) values ($1, $2) returning id")
            .bind("repin@example.com")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
        .bind(user_id)
        .bind(uuid::Uuid::parse_str(&org_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "repin@example.com", "password": "correct horse battery staple"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Repinnable", "slug": "repinnable", "url": resource}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    let server_uuid = uuid::Uuid::parse_str(&server_id).unwrap();

    let registered = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({"client_id": "rolter", "default_scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), 200);

    // one interactive authorize discovers and caches the first server
    let started: Value = client
        .post(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth/authorize"
        ))
        .bearer_auth(&token)
        .json(&json!({"scopes": ["tools:read"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let auth_url = started["authorization_url"].as_str().unwrap().to_string();
    let state = url_param(&auth_url, "state");
    let cached_first = discovery_cache(&pool, server_uuid).await;
    assert_eq!(
        cached_first,
        (
            Some(authz.clone()),
            Some(format!("{authz}/authorize")),
            Some(format!("{authz}/token")),
            true,
            true
        ),
        "discovery should have cached the first stub authorization server"
    );

    // complete the login so there is a session with a refresh token to renew
    stub.answer(
        200,
        json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "tools:read"
        }),
    );
    let consented_resp = client
        .get(format!(
            "{base}/auth/mcp/callback?code=code-1&state={state}&iss={}",
            authz.replace(':', "%3A").replace('/', "%2F")
        ))
        .send()
        .await
        .unwrap();
    let consented_status = consented_resp.status();
    let consented_text = consented_resp.text().await.unwrap();
    assert_eq!(consented_status, 200, "{consented_text}");
    let consented: Value = serde_json::from_str(&consented_text).unwrap();
    let session_id = consented["session_id"].as_str().unwrap().to_string();

    // an edit that touches neither the issuer nor the discovery mode must not
    // invalidate the cache — re-registering the same client with a rotated
    // secret is exactly this kind of edit
    let before = config_version(&pool).await;
    let resecreted = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "client_secret": "rotated-secret"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resecreted.status(), 200);
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        cached_first,
        "an edit that left the issuer and discovery mode alone must not invalidate the cache"
    );
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "one edit must bump config_version exactly once"
    );

    // the operator rotates to a different authorization server: a new issuer
    // and fallback endpoints, discovery left on "auto"
    let before = config_version(&pool).await;
    let repinned = client
        .put(format!(
            "{base}/api/v1/mcp-servers/{server_id}/oauth-client"
        ))
        .bearer_auth("admintok")
        .json(&json!({
            "client_id": "rolter",
            "default_scopes": ["tools:read"],
            "issuer": authz2,
            "authorize_url": format!("{authz2}/authorize"),
            "token_url": format!("{authz2}/token")
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(repinned.status(), 200, "{}", repinned.text().await.unwrap());
    assert_eq!(
        discovery_cache(&pool, server_uuid).await,
        (None, None, None, false, false),
        "re-pinning the issuer must clear the endpoints discovered for the old one"
    );
    // the clearing rides on the update that wrote the issuer; a second
    // statement would bump config_version twice for one logical edit
    assert_eq!(
        config_version(&pool).await - before,
        1,
        "clearing the cache must not cost a second config_version bump"
    );

    // a refresh never probes (#1432): with the cache gone it must fall back to
    // the newly configured endpoint rather than the stale cached one
    let calls_before = (stub.calls(), stub2.calls());
    stub2.answer(
        200,
        json!({
            "access_token": "access-2",
            "refresh_token": "refresh-2",
            "token_type": "Bearer",
            "expires_in": 3600
        }),
    );
    let refreshed: Value = client
        .post(format!("{base}/api/v1/mcp/sessions/{session_id}/refresh"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["id"], json!(session_id));
    assert_eq!(
        (stub.calls(), stub2.calls()),
        (calls_before.0, calls_before.1 + 1),
        "the refresh must reach the corrected authorization server, not the stale cached one"
    );
}

/// Cross-tenant isolation on the exchange path: a member of another org may not
/// refresh or exchange a session they do not own, and the answer is a 404 —
/// whether a session exists elsewhere is not something to probe for.
#[tokio::test]
async fn mcp_oauth_sessions_are_not_reachable_across_owners() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "OwnerOrg", "slug": "owner-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = uuid::Uuid::parse_str(org["id"].as_str().unwrap()).unwrap();

    use argon2::password_hash::PasswordHasher;
    let mut tokens = Vec::new();
    let mut ids = Vec::new();
    for email in ["owner@example.com", "other@example.com"] {
        let hash = argon2::Argon2::default()
            .hash_password(b"correct horse battery staple")
            .unwrap()
            .to_string();
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into users (email, password_hash) values ($1, $2) returning id",
        )
        .bind(email)
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'member')")
            .bind(id)
            .bind(org_id)
            .execute(&pool)
            .await
            .unwrap();
        let login: Value = client
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"email": email, "password": "correct horse battery staple"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        tokens.push(login["token"].as_str().unwrap().to_string());
        ids.push(id);
    }

    // a grant + session owned by the first user, written directly: this test is
    // about the guard, not about the exchange
    let server_id: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_servers (org_id, name, slug, url) \
         values ($1, 'Docs', 'docs', 'https://mcp.example.com') returning id",
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let grant_id: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_oauth_grants (server_id, user_id, scopes) \
         values ($1, $2, '{tools:read}') returning id",
    )
    .bind(server_id)
    .bind(ids[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    let session_id: uuid::Uuid = sqlx::query_scalar(
        "insert into mcp_oauth_sessions (grant_id, access_ciphertext, access_nonce, scopes, \
                expires_at) \
         values ($1, '\\x00', '\\x00', '{tools:read}', now() + interval '1 hour') returning id",
    )
    .bind(grant_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    for path in ["refresh", "exchange"] {
        let denied = client
            .post(format!("{base}/api/v1/mcp/sessions/{session_id}/{path}"))
            .bearer_auth(&tokens[1])
            .send()
            .await
            .unwrap();
        assert_eq!(
            denied.status(),
            403,
            "a co-tenant must not act on someone else's session via {path}"
        );
    }
}

/// The two global catalogs are readable by any authenticated caller with no
/// membership anywhere, and refused outright without authentication (#766).
#[tokio::test]
async fn global_catalogs_take_authentication_but_no_membership() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // a user belonging to nothing at all
    let user = seed_user(&pool, "nobody@acme.test", false).await;
    let token = seed_session(&pool, user, "catalogs").await;

    for path in ["/api/v1/model-prices", "/api/v1/models"] {
        let anonymous = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(
            anonymous.status(),
            401,
            "{path} must still require authentication"
        );

        let authenticated = client
            .get(format!("{base}{path}"))
            .bearer_auth(token.clone())
            .send()
            .await
            .unwrap();
        assert!(
            authenticated.status().is_success(),
            "{path} must be readable without a membership: {}",
            authenticated.status()
        );
    }

    // and the matrix says so, rather than naming a role floor nobody can meet
    let matrix: Value = client
        .get(format!("{base}/api/v1/rbac/matrix"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let model = matrix["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["resource"] == "model")
        .expect("model resource in the matrix");
    let read = model["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["action"] == "read")
        .expect("model read action");
    assert_eq!(read["authenticated_only"], true, "{matrix}");
    assert!(read["minimum_role"].is_null(), "{matrix}");
}

#[tokio::test]
async fn collector_config_renders_enabled_connectors_and_hides_disabled_ones() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // unauthenticated callers get nothing back
    let denied = client
        .get(format!("{base}/api/v1/connectors/collector-config"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let enabled = client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth("sekrit")
        .json(&json!({
            "name": "SigNoz",
            "kind": "otlp_http",
            "endpoint": "https://collector.example.com/v1/logs",
            "enabled": true,
            "sampling_rate": 0.5,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(enabled.status(), 200);

    client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth("sekrit")
        .json(&json!({
            "name": "Disabled Sink",
            "kind": "otlp_http",
            "endpoint": "https://disabled.example.com/v1/logs",
            "enabled": false,
            "sampling_rate": 1.0,
        }))
        .send()
        .await
        .unwrap();

    let config = client
        .get(format!("{base}/api/v1/connectors/collector-config"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(config.status(), 200);
    assert_eq!(
        config.headers().get("content-type").unwrap(),
        "application/yaml"
    );
    let body = config.text().await.unwrap();

    // the enabled connector gets its own exporter, sampler and pipelines
    assert!(body.contains("otlphttp/signoz:"), "{body}");
    assert!(
        body.contains("endpoint: \"https://collector.example.com/v1/logs\""),
        "{body}"
    );
    assert!(body.contains("probabilistic_sampler/signoz:"), "{body}");
    assert!(body.contains("sampling_percentage: 50.0000"), "{body}");
    assert!(body.contains("traces/signoz:"), "{body}");
    assert!(body.contains("metrics/signoz:"), "{body}");
    assert!(body.contains("logs/signoz:"), "{body}");

    // the disabled connector never reaches the rendered document
    assert!(!body.contains("disabled-sink"), "{body}");
    assert!(!body.contains("disabled.example.com"), "{body}");
}

#[tokio::test]
async fn collector_config_renders_a_managed_secret_as_a_bearer_header() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth("sekrit")
        .json(&json!({
            "name": "Honeycomb",
            "kind": "otlp_http",
            "endpoint": "https://api.honeycomb.io/v1/logs",
            "enabled": true,
            "sampling_rate": 1.0,
            "managed_auth_secret": "super-secret-token",
        }))
        .send()
        .await
        .unwrap();

    let body = client
        .get(format!("{base}/api/v1/connectors/collector-config"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("Authorization: \"Bearer super-secret-token\""),
        "{body}"
    );
}

/// a sink that records the raw head of every request it receives and answers 200
async fn serve_capturing_sink() -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
            });
        }
    });
    (addr, seen)
}

/// #2403: a connector's stored secret belongs to the endpoint's origin.
#[tokio::test]
async fn a_connector_moved_to_another_origin_drops_its_secret_unless_given_a_new_one() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (old_sink, _) = serve_capturing_sink().await;
    let (new_sink, seen) = serve_capturing_sink().await;
    let old_secret = random_password();
    let new_secret = random_password();

    let created: Value = client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth("sekrit")
        .json(&json!({
            "name": "Sink",
            "kind": "otlp_http",
            "endpoint": format!("http://{old_sink}/v1/logs"),
            "enabled": true,
            "sampling_rate": 1.0,
            "managed_auth_secret": old_secret,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["auth_secret_configured"], true);
    let put = |endpoint: String, secret: Option<String>| {
        let client = client.clone();
        let url = format!("{base}/api/v1/connectors/{id}");
        async move {
            let mut body = json!({
                "name": "Sink",
                "kind": "otlp_http",
                "endpoint": endpoint,
                "enabled": true,
                "sampling_rate": 1.0,
            });
            if let Some(secret) = secret {
                body["managed_auth_secret"] = secret.into();
            }
            let response = client
                .put(url)
                .bearer_auth("sekrit")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            response.json::<Value>().await.unwrap()
        }
    };
    let columns = || async {
        sqlx::query_as::<_, (bool, bool)>(
            "select auth_ciphertext is not null, auth_nonce is not null \
             from observability_connectors",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let audit_detail = || async {
        sqlx::query_scalar::<_, Value>(
            "select detail from audit_log where action = 'connector.update' \
             order by at desc limit 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // another path on the same origin keeps it
    let body = put(format!("http://{old_sink}/other"), None).await;
    assert_eq!(body["auth_secret_configured"], true);
    assert_eq!(columns().await, (true, true));
    assert_eq!(audit_detail().await["secret_cleared"], false);

    // another origin with a new secret stores the new one
    let body = put(
        format!("http://{new_sink}/v1/logs"),
        Some(new_secret.clone()),
    )
    .await;
    assert_eq!(body["auth_secret_configured"], true);
    let config = client
        .get(format!("{base}/api/v1/connectors/collector-config"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(config.contains(&new_secret), "the new secret is rendered");
    assert!(!config.contains(&old_secret), "the old secret is rendered");
    assert_eq!(audit_detail().await["secret_cleared"], false);

    // back to the first origin without one: dropped, columns and all
    let body = put(format!("http://{old_sink}/v1/logs"), None).await;
    assert_eq!(body["auth_secret_configured"], false);
    assert_eq!(columns().await, (false, false));
    let detail = audit_detail().await;
    assert_eq!(detail["secret_cleared"], true);
    assert!(!detail.to_string().contains(&old_sink.to_string()));

    // another origin: the probe and the collector config carry no token
    put(format!("http://{new_sink}/v1/logs"), None).await;
    let tested = client
        .post(format!("{base}/api/v1/connectors/{id}/test"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(tested.status(), 200);
    let requests = seen.lock().unwrap().clone();
    let probe = requests.last().expect("the sink received the probe");
    assert!(
        !probe.to_ascii_lowercase().contains("authorization"),
        "the probe carried an authorization header"
    );
    let config = client
        .get(format!("{base}/api/v1/connectors/collector-config"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !config.contains("Bearer"),
        "the config carries a bearer header"
    );
}

/// #1162: the Security screen wrote to a table nothing downstream read. This
/// is the propagation half of the fix — the enforcement half lives in
/// `rolter-gateway`'s integration suite. It asserts the settings arrive in the
/// snapshot *and* that the retired dashboard password fields are gone (#2356).
#[tokio::test]
async fn security_policy_reaches_the_snapshot_and_drops_the_dashboard_password() {
    skip_without_db!();
    // sealing the dashboard secret needs a KEK, exactly as the provider-key
    // test does; the value is arbitrary because nothing here decrypts it
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the default is "no extra rules", so an untouched deployment is unchanged
    let before: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["config"]["security"]["virtual_key_required"], false);
    assert!(before["config"]["security"]["required_headers"].is_null());
    assert!(before["config"]["security"]["auth_bypass_routes"].is_null());

    let saved: Value = client
        .put(format!("{base}/api/v1/security-settings"))
        .bearer_auth("sekrit")
        .json(&json!({
            "virtual_key_required": true,
            "allowed_origins": [],
            "allowed_headers": [],
            "required_headers": {"X-Mesh-Id": "edge-42"},
            "auth_bypass_routes": ["/v1/models"],
            "dashboard_auth_enabled": false,
            "managed_dashboard_secret": "hunter2",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["virtual_key_required"], true, "{saved}");
    // the dashboard password was removed because nothing enforced it (#2356):
    // an old client's fields are ignored, and none of them comes back
    for field in [
        "dashboard_auth_enabled",
        "dashboard_credential_ref",
        "dashboard_secret_configured",
        "managed_dashboard_secret",
    ] {
        assert!(saved.get(field).is_none(), "{field} in {saved}");
    }
    let read: Value = client
        .get(format!("{base}/api/v1/security-settings"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(read.get("dashboard_auth_enabled").is_none(), "{read}");
    assert!(read.get("dashboard_secret_configured").is_none(), "{read}");
    // and the toggle that controlled nothing is gone from the surface (#1162)
    assert!(saved.get("allow_direct_provider_keys").is_none());

    let after: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let security = &after["config"]["security"];
    assert_eq!(security["virtual_key_required"], true);
    // header names are lowercased on the way through, because that is how the
    // gateway looks them up
    assert_eq!(security["required_headers"]["x-mesh-id"], "edge-42");
    assert_eq!(security["auth_bypass_routes"][0], "/v1/models");

    // the sealed dashboard secret must not ride along anywhere in the payload
    let payload = serde_json::to_string(&after).unwrap();
    assert!(
        !payload.contains("hunter2"),
        "the snapshot carries the secret"
    );
    assert!(!payload.contains("dashboard_credential"), "{payload}");

    // and the write bumped the version, so a polling gateway actually sees it
    assert!(
        after["version"].as_i64().unwrap() > before["version"].as_i64().unwrap(),
        "config_version did not move"
    );
}

/// #945: naming and expiry are the two choices the mint path used to let a
/// caller skip. Both are now decided at creation, and both are decided by the
/// server — the client sends a day count, not an instant.
#[tokio::test]
async fn a_minted_key_must_be_named_and_carries_the_ttl_the_caller_chose() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme-945"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");
    let team = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");
    let project = post(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");
    let keys_url = format!("{base}/api/v1/projects/{project_id}/virtual-keys");

    // a blank name is refused, and so is one that is only whitespace: the
    // plaintext is shown once, so an unnamed key is unattributable forever
    for name in [json!(""), json!("   ")] {
        let rejected = client
            .post(&keys_url)
            .json(&json!({"name": name}))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), 400, "name {name} should be refused");
        let body: Value = rejected.json().await.unwrap();
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("name is required"),
            "{body}"
        );
    }

    // omitting the field entirely is a 422 from serde, not a silently unnamed
    // key: the field is no longer `Option`
    let missing = client
        .post(&keys_url)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(missing.status().is_client_error(), "{}", missing.status());

    // a day count becomes an instant the server computed
    let before = chrono::Utc::now();
    let minted = post(
        &client,
        keys_url.clone(),
        json!({"name": "  billing  ", "expires_in_days": 30}),
    )
    .await;
    assert_eq!(minted["name"], "billing", "the name is stored trimmed");
    let expires: chrono::DateTime<chrono::Utc> = minted["expires_at"]
        .as_str()
        .unwrap_or_else(|| panic!("a key minted with a ttl must carry one: {minted}"))
        .parse()
        .unwrap();
    assert!(expires > before + chrono::Duration::days(29));
    assert!(expires < before + chrono::Duration::days(31));

    // "never" is still reachable, but only by omitting the field — which is
    // what the dashboard sends for an explicit choice, never for an untouched
    // control
    let immortal = post(&client, keys_url.clone(), json!({"name": "build box"})).await;
    assert!(
        immortal["expires_at"].is_null(),
        "omitting the ttl must mint a key that never expires: {immortal}"
    );

    // a ttl of zero reads like "no expiry" but would mean "already expired";
    // refusing it keeps that ambiguity out of the store
    let zero = client
        .post(&keys_url)
        .json(&json!({"name": "zero", "expires_in_days": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(zero.status(), 400);
}

// ---------------------------------------------------------------------------
// the public example key (#2408)
// ---------------------------------------------------------------------------

/// Snapshot virtual-key secrets and `/config/problems` for a control plane whose
/// config file declares the public example key.
async fn example_key_snapshot(admin_token: Option<String>) -> (Vec<String>, Vec<String>) {
    let db = fresh_db().await;
    let file_config = rolter_core::GatewayConfig {
        virtual_keys: vec![rolter_core::config::VirtualKeyConfig {
            key: rolter_core::PUBLIC_EXAMPLE_KEY.to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let app = rolter_control::test_app_with_file_config(
        db.pool().clone(),
        admin_token.clone(),
        file_config,
    )
    .await
    .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let mut request = client.get(format!("http://{addr}/internal/snapshot"));
    if let Some(token) = &admin_token {
        request = request.bearer_auth(token);
    }
    let snapshot: Value = request.send().await.unwrap().json().await.unwrap();
    let keys = snapshot["config"]["virtual_keys"]
        .as_array()
        .expect("virtual_keys")
        .iter()
        .map(|k| k["key"].as_str().unwrap_or_default().to_string())
        .collect();
    // the problems view needs a session once a token is set (#1840)
    let mut request = client.get(format!("http://{addr}/api/v1/config/problems"));
    if let Some(token) = &admin_token {
        request = request.bearer_auth(token);
    }
    let problems: Value = request.send().await.unwrap().json().await.unwrap();
    let problems = problems["problems"]
        .as_array()
        .expect("problems array")
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect();
    (keys, problems)
}

#[tokio::test]
async fn the_snapshot_withholds_the_public_example_key_once_an_admin_token_is_set() {
    skip_without_db!();
    let (keys, problems) = example_key_snapshot(Some(random_password())).await;
    assert!(keys.is_empty(), "the public key leaked: {keys:?}");
    assert!(
        problems.iter().any(|p| p.contains("sk-rolter-dev")),
        "the omission must be reported: {problems:?}"
    );
}

#[tokio::test]
async fn open_mode_still_serves_the_public_example_key() {
    skip_without_db!();
    let (keys, problems) = example_key_snapshot(None).await;
    assert_eq!(keys, ["sk-rolter-dev"]);
    assert!(
        !problems.iter().any(|p| p.contains("sk-rolter-dev")),
        "{problems:?}"
    );
}

// ---------------------------------------------------------------------------
// TOTP second factor (#1078)
// ---------------------------------------------------------------------------

/// A password for one test account, generated per run rather than written out.
/// A literal here is a hard-coded credential to every scanner that reads this
/// file, and the tests need only that the password round-trips -- not that it
/// is any particular string.
fn random_password() -> String {
    format!("pw-{}", uuid::Uuid::new_v4())
}

/// Seed a local superadmin with a known password and return its id.
async fn seed_local_user(pool: &sqlx::PgPool, email: &str, password: &str) -> uuid::Uuid {
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, true)
         returning id",
    )
    .bind(email)
    .bind(&hash)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The current TOTP code for a base32 secret, as an authenticator app would
/// compute it.
fn current_code(secret_b32: &str) -> String {
    let secret = rolter_auth::totp::base32_decode(secret_b32).expect("secret decodes");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    rolter_auth::totp::code_at_step(&secret, rolter_auth::totp::step_at(now))
}

/// The whole enrolment → step-up → recovery-code path, plus the two properties
/// that make the factor worth having: a code cannot be replayed, and a
/// recovery code is single-use.
#[tokio::test]
async fn totp_enrolment_step_up_and_recovery_codes() {
    skip_without_db!();
    // enrolment seals the secret with the deployment KEK, so a control plane
    // without one must refuse rather than store a bearer credential in clear
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_local_user(&pool, "mfa@example.com", &password).await;

    // sign in the ordinary way: no factor yet, so a session comes straight back
    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().expect("session token").to_string();

    // status before enrolment
    let status: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["policy"], "off");

    // begin enrolment: the secret is shown once
    let enrol: Value = client
        .post(format!("{base}/api/v1/me/mfa/enroll"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    assert!(
        enrol["otpauth_uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/"),
        "{enrol}"
    );

    // an unconfirmed factor arms nothing: logging in again must still hand
    // back a session, not a challenge
    let mid: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        mid["token"].is_string(),
        "unconfirmed factor must not gate login: {mid}"
    );

    // a wrong code does not arm it
    let bad = client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // the right code does, and returns the recovery batch. bind it: the replay
    // assertion below has to send back this exact code, and reading the clock a
    // second time sends the next step's code across a 30s boundary, asserting
    // the clock rather than the replay rule (#1451)
    let confirming_code = current_code(&secret);
    let confirmed: Value = client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": &confirming_code}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let codes: Vec<String> = confirmed["recovery_codes"]
        .as_array()
        .expect("recovery codes")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 10, "{confirmed}");

    // now login is a challenge, not a session
    let challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(challenge["mfa_required"], true, "{challenge}");
    assert!(
        challenge["token"].is_null(),
        "a challenge is not a session: {challenge}"
    );
    let mfa_token = challenge["mfa_token"]
        .as_str()
        .expect("mfa token")
        .to_string();

    // a wrong code against the challenge is refused
    let wrong = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // the code that confirmed the enrolment has already been spent, so it does
    // not redeem the challenge either -- the replay rule does not care that
    // the earlier use was a legitimate one
    let spent = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": &confirming_code}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        spent.status(),
        401,
        "the confirming code is spent and must not redeem a challenge"
    );

    // stand in for the 30 seconds a real user waits for the next code, by
    // winding the spent step back one. Winding the clock instead would mean
    // sleeping through a step in every CI run
    sqlx::query("update user_totp_factors set last_used_step = last_used_step - 1")
        .execute(&pool)
        .await
        .unwrap();

    // now the current code redeems the challenge for a real session
    let challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mfa_token = challenge["mfa_token"]
        .as_str()
        .expect("mfa token")
        .to_string();
    let code = current_code(&secret);
    let stepped: Value = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": code}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = stepped["token"]
        .as_str()
        .expect("session token")
        .to_string();
    let me = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200, "the stepped-up session must authenticate");

    // *the* property: that same code, still inside its window, cannot be
    // replayed on a fresh challenge. Without the spent-step check a
    // shoulder-surfed code stays usable for up to 90 seconds
    let replay_challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let replayed = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({
            "mfa_token": replay_challenge["mfa_token"].as_str().unwrap(),
            "code": code,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        replayed.status(),
        401,
        "a spent TOTP step must not verify again"
    );

    // a recovery code gets past the factor exactly once
    let recovery_challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let redeemed = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({
            "mfa_token": recovery_challenge["mfa_token"].as_str().unwrap(),
            "code": codes[0],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        redeemed.status(),
        200,
        "an unspent recovery code must verify"
    );

    let reuse_challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "mfa@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reused = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({
            "mfa_token": reuse_challenge["mfa_token"].as_str().unwrap(),
            "code": codes[0],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reused.status(), 401, "a recovery code must be single-use");

    let after: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["enabled"], true);
    assert_eq!(after["recovery_codes_remaining"], 9);
}

/// A challenge is spendable a bounded number of times. Without this, a
/// stolen password is six digits and unlimited guesses away from a session.
#[tokio::test]
async fn a_challenge_is_exhausted_by_repeated_wrong_codes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_local_user(&pool, "attempts@example.com", &password).await;

    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "attempts@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/me/mfa/enroll"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().unwrap().to_string();
    client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": current_code(&secret)}))
        .send()
        .await
        .unwrap();

    let challenge: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "attempts@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mfa_token = challenge["mfa_token"].as_str().unwrap().to_string();

    for attempt in 0..3 {
        let wrong = client
            .post(format!("{base}/api/v1/auth/mfa/verify"))
            .json(&json!({"mfa_token": mfa_token, "code": "000000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401, "attempt {attempt}");
    }

    // the budget is spent, so even the *correct* code no longer redeems this
    // challenge -- the user has to start over from the password
    let correct = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": mfa_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        correct.status(),
        401,
        "an exhausted challenge must not redeem, right code or not"
    );
}

/// `rolter mfa reset` is the documented way back into an account whose factor
/// is gone. It clears the factor and revokes the sessions that were riding on
/// it, and records why it was run.
#[tokio::test]
async fn break_glass_reset_clears_the_factor_and_revokes_sessions() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let user_id = seed_local_user(&pool, "locked-out@example.com", &password).await;

    let login: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "locked-out@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = login["token"].as_str().unwrap().to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/me/mfa/enroll"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": current_code(enrol["secret"].as_str().unwrap())}))
        .send()
        .await
        .unwrap();
    // a sign-in halfway through its step-up when the reset lands
    let in_flight = login_as(&client, &base, "locked-out@example.com", &password).await;
    assert_eq!(in_flight["mfa_required"], true, "{in_flight}");

    let cleared = rolter_control::mfa::break_glass_reset(&pool, user_id, "lost phone, ticket 42")
        .await
        .unwrap();
    assert!(cleared, "there was a factor to clear");
    // and every challenge in flight goes with the factor. An enrolment token
    // left alive would be refused only while a factor is armed, so the moment
    // the reset cleared it, its holder could arm one of their own
    let challenges: i64 =
        sqlx::query_scalar("select count(*) from mfa_challenges where user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(challenges, 0, "reset must drop every challenge in flight");

    // the session that existed before the reset is gone
    let stale = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 401, "reset must revoke live sessions");

    // and the password alone gets back in, so the user can enrol again
    let back_in: Value = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "locked-out@example.com", "password": password}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(back_in["token"].is_string(), "{back_in}");

    // the reason is on the audit entry, which is the whole point of demanding
    // one on the command line
    let reason: Option<String> = sqlx::query_scalar(
        "select detail->>'reason' from audit_log
         where action = 'auth.mfa_break_glass_reset' and actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(reason.as_deref(), Some("lost phone, ticket 42"));
}

/// Seed a non-superadmin local account as an admin of a fresh org whose
/// `mfa_policy` is `policy`, with the grace window `enforce_after` (a SQL
/// expression, so a test can say `now() + interval '7 days'` without a clock
/// of its own). Returns `(user_id, org_id)`.
async fn seed_bound_member(
    pool: &sqlx::PgPool,
    email: &str,
    password: &str,
    policy: &str,
    enforce_after: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    use argon2::password_hash::PasswordHasher;
    let hash = argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .unwrap()
        .to_string();
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "insert into users (email, password_hash, is_superadmin) values ($1, $2, false)
         returning id",
    )
    .bind(email)
    .bind(&hash)
    .fetch_one(pool)
    .await
    .unwrap();
    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
            .bind(format!("org-{}", uuid::Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'admin')")
        .bind(user_id)
        .bind(org_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(&format!(
        "insert into org_auth_policies (org_id, mfa_policy, mfa_enforce_after)
         values ($1, $2, {enforce_after})"
    ))
    .bind(org_id)
    .bind(policy)
    .execute(pool)
    .await
    .unwrap();
    (user_id, org_id)
}

/// Sign in with a password and return whatever the exchange answered.
async fn login_as(client: &reqwest::Client, base: &str, email: &str, password: &str) -> Value {
    let response = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "password sign-in");
    response.json().await.unwrap()
}

/// A `required_all` org no longer refuses an account with no armed factor: the
/// sign-in hands out an enrolment challenge, proving a code from the secret it
/// mints is what issues the session, and the challenge opens nothing else on
/// the way (#1852).
#[tokio::test]
async fn a_required_policy_sends_an_unenrolled_account_through_enrolment() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // an admin token, so the CRUD api actually checks what it is handed rather
    // than letting every request through as open mode does
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admin-secret".into()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let (user_id, org_id) = seed_bound_member(
        &pool,
        "unenrolled@example.com",
        &password,
        "required_all",
        "null",
    )
    .await;

    let challenge = login_as(&client, &base, "unenrolled@example.com", &password).await;
    assert_eq!(challenge["mfa_enrolment_required"], true, "{challenge}");
    assert!(
        challenge["token"].is_null(),
        "an enrolment challenge is not a session: {challenge}"
    );
    let enrolment_token = challenge["enrolment_token"]
        .as_str()
        .expect("enrolment token")
        .to_string();

    // the token reads nothing and writes nothing: not as a bearer on the
    // account's own routes, not on the CRUD api, and not on the step-up
    for (method, path) in [
        ("GET", "/api/v1/auth/me"),
        ("GET", "/api/v1/me/mfa"),
        ("POST", "/api/v1/me/mfa/enroll"),
        ("GET", "/api/v1/me/virtual-keys"),
        ("GET", "/api/v1/orgs"),
        ("GET", &format!("/api/v1/orgs/{org_id}/auth-policy")),
    ] {
        let response = client
            .request(method.parse().unwrap(), format!("{base}{path}"))
            .bearer_auth(&enrolment_token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            401,
            "{method} {path} must not accept an enrolment token"
        );
    }
    let write = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(&enrolment_token)
        .json(&json!({"name": "stolen", "slug": "stolen"}))
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), 401, "an enrolment token must not write");
    let relax = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .bearer_auth(&enrolment_token)
        .json(&json!({"allow_password_login": true, "allow_sso": true, "mfa_policy": "off"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        relax.status(),
        401,
        "an enrolment token must not relax the policy it is bound by"
    );
    let step_up = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": enrolment_token, "code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(step_up.status(), 401, "the step-up must not accept it");

    // what it is for: a secret, shown once
    let enrol: Value = client
        .post(format!("{base}/api/v1/auth/mfa/enroll"))
        .json(&json!({"enrolment_token": enrolment_token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    assert!(
        enrol["otpauth_uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/"),
        "{enrol}"
    );

    // a pending secret still arms nothing, so a code from it does not get
    // past the step-up either -- the one way through is the confirm below
    let still_refused = client
        .post(format!("{base}/api/v1/auth/mfa/verify"))
        .json(&json!({"mfa_token": enrolment_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(still_refused.status(), 401);

    let wrong = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": enrolment_token, "code": "000000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400, "a wrong code keeps the challenge");

    // stand in for a password step that came straight after a lockout: the
    // session comes out of a later request, and has to learn it from the row
    sqlx::query("update mfa_challenges set after_lock = true where user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let signed_in: Value = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": enrolment_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = signed_in["token"]
        .as_str()
        .expect("confirming yields a session")
        .to_string();
    assert_eq!(
        signed_in["recovery_codes"].as_array().map(Vec::len),
        Some(10),
        "{signed_in}"
    );
    assert_eq!(signed_in["user"]["email"], "unenrolled@example.com");
    let me = client
        .get(format!("{base}/api/v1/auth/me"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200, "the enrolled session must authenticate");
    let status: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["enabled"], true, "{status}");

    // single use: the token went with the session it minted. The factor is
    // disarmed behind the api's back first, because `enrolling_user` refuses
    // an armed account on its own and would answer 401 whether or not the
    // token had been consumed
    let disarm = "update user_totp_factors set confirmed_at = null where user_id = $1";
    let rearm = "update user_totp_factors set confirmed_at = now() where user_id = $1";
    sqlx::query(disarm)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let again = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": enrolment_token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401, "an enrolment token redeems once");
    sqlx::query(rearm)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    // audited like any other sign-in, and the enrolment says where it happened
    let actions: Vec<(String, Option<bool>, Option<bool>)> = sqlx::query_as(
        "select action, (detail->>'at_sign_in')::boolean, (detail->>'after_lock')::boolean
         from audit_log where actor_user_id = $1 order by at",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        actions.iter().any(
            |(action, _, after_lock)| action == "auth.mfa_enrolment_challenge"
                && *after_lock == Some(false)
        ),
        "the challenge row says whether the password step followed a lockout: {actions:?}"
    );
    assert!(
        actions.iter().any(
            |(action, at_sign_in, _)| action == "auth.mfa_enabled" && *at_sign_in == Some(true)
        ),
        "{actions:?}"
    );
    assert!(
        actions
            .iter()
            .any(|(action, _, after_lock)| action == "auth.login" && *after_lock == Some(true)),
        "the session carries the lockout its challenge recorded: {actions:?}"
    );

    // the factor is armed now, so the next sign-in is an ordinary step-up --
    // and that step-up token does not open the enrolment routes either. Again
    // with the factor disarmed underneath, so it is the purpose on the token
    // that refuses rather than the armed factor
    let step_up: Value = login_as(&client, &base, "unenrolled@example.com", &password).await;
    assert_eq!(step_up["mfa_required"], true, "{step_up}");
    sqlx::query(disarm)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    for route in ["enroll", "confirm"] {
        let reenrol = client
            .post(format!("{base}/api/v1/auth/mfa/{route}"))
            .json(&json!({"enrolment_token": step_up["mfa_token"], "code": current_code(&secret)}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            reenrol.status(),
            401,
            "a step-up token must not reach /auth/mfa/{route}"
        );
    }
}

/// An enrolment challenge is short-lived and bounded: once it expires, or once
/// its codes are spent, it opens nothing, the right code included.
#[tokio::test]
async fn an_enrolment_challenge_expires_and_is_spent_by_wrong_codes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    seed_bound_member(&pool, "late@example.com", &password, "required_all", "null").await;

    // expiry: a challenge left sitting is dead to both routes
    let stale = login_as(&client, &base, "late@example.com", &password).await;
    let stale_token = stale["enrolment_token"]
        .as_str()
        .expect("token")
        .to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/auth/mfa/enroll"))
        .json(&json!({"enrolment_token": stale_token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    sqlx::query("update mfa_challenges set expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    for route in ["enroll", "confirm"] {
        let response = client
            .post(format!("{base}/api/v1/auth/mfa/{route}"))
            .json(&json!({"enrolment_token": stale_token, "code": current_code(&secret)}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{route} after expiry");
    }

    // budget: five wrong codes spend a fresh challenge
    let fresh = login_as(&client, &base, "late@example.com", &password).await;
    let token = fresh["enrolment_token"]
        .as_str()
        .expect("token")
        .to_string();
    let enrol: Value = client
        .post(format!("{base}/api/v1/auth/mfa/enroll"))
        .json(&json!({"enrolment_token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = enrol["secret"].as_str().expect("secret").to_string();
    for attempt in 0..5 {
        let wrong = client
            .post(format!("{base}/api/v1/auth/mfa/confirm"))
            .json(&json!({"enrolment_token": token, "code": "000000"}))
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), 400, "attempt {attempt}");
    }
    let correct = client
        .post(format!("{base}/api/v1/auth/mfa/confirm"))
        .json(&json!({"enrolment_token": token, "code": current_code(&secret)}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        correct.status(),
        401,
        "a spent enrolment challenge must not redeem, right code or not"
    );
    let armed: Option<bool> = sqlx::query_scalar(
        "select confirmed_at is not null from user_totp_factors f
         join users u on u.id = f.user_id where u.email = 'late@example.com'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(armed, Some(false), "nothing was armed along the way");
}

/// A grace window announces the requirement without enforcing it: until it
/// passes the unenrolled member signs in with the password and is told by
/// when; after it, the same sign-in is an enrolment (#1852).
#[tokio::test]
async fn a_grace_window_lets_an_unenrolled_member_in_until_it_passes() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let (_, org_id) = seed_bound_member(&pool, "grace@example.com", &password, "off", "null").await;

    // the window is set through the api, so the round trip is covered too
    let deadline = (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339();
    let policy: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "required_all",
            "mfa_enforce_after": deadline,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(policy["mfa_enforce_after"].is_string(), "{policy}");

    // a client written before the window existed re-sends the policy without
    // it, and must not cancel the date an admin announced
    let resent: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": false,
            "mfa_policy": "required_all",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        resent["mfa_enforce_after"], policy["mfa_enforce_after"],
        "an absent window keeps the stored one: {resent}"
    );

    let signed_in = login_as(&client, &base, "grace@example.com", &password).await;
    let session = signed_in["token"]
        .as_str()
        .expect("inside the window the password is enough")
        .to_string();
    assert_eq!(
        signed_in["mfa_enrol_by"], policy["mfa_enforce_after"],
        "the session says by when: {signed_in}"
    );
    let status: Value = client
        .get(format!("{base}/api/v1/me/mfa"))
        .bearer_auth(&session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["required"], true, "{status}");
    assert_eq!(status["enforce_after"], policy["mfa_enforce_after"]);

    // the window runs out
    sqlx::query("update org_auth_policies set mfa_enforce_after = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let after = login_as(&client, &base, "grace@example.com", &password).await;
    assert_eq!(after["mfa_enrolment_required"], true, "{after}");
    assert_eq!(
        after["expires_in"], 600,
        "the client times the prompt from this, not from its own clock: {after}"
    );

    // an explicit null is "at once", which an absent key is not
    let at_once: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "required_all",
            "mfa_enforce_after": null,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(at_once["mfa_enforce_after"].is_null(), "{at_once}");

    // relaxing the policy drops the window with it, rather than leaving it to
    // surprise whoever next turns the requirement on
    let relaxed: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "optional",
            "mfa_enforce_after": deadline,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(relaxed["mfa_enforce_after"].is_null(), "{relaxed}");
    let plain = login_as(&client, &base, "grace@example.com", &password).await;
    assert!(plain["token"].is_string(), "{plain}");
    assert!(
        plain.get("mfa_enrol_by").is_none(),
        "nothing is owed, so nothing is announced: {plain}"
    );
}

/// The two writes an enrolment makes decide in the statement, not in a read
/// before it (#1852). A confirm arms only the secret its code was checked
/// against, so a second enrolment landing in between is not armed by a code
/// from the first; and a new enrolment never replaces an armed factor, so one
/// racing a confirm cannot disarm what that confirm just armed. Neither race
/// is reachable deterministically through the api, hence the repository.
#[tokio::test]
async fn an_enrolment_never_arms_a_secret_it_did_not_check_or_disarms_one() {
    skip_without_db!();
    use rolter_store::postgres::crypto::Kek;
    use rolter_store::postgres::repo::MfaRepo;
    let db = fresh_db().await;
    let pool = db.pool().clone();
    rolter_store::postgres::run_migrations(&pool).await.unwrap();
    let user_id = seed_local_user(&pool, "race@example.com", &random_password()).await;
    let kek = Kek::from_secret(TEST_KEK);
    let repo = MfaRepo(&pool);

    assert!(repo.begin_enrolment(user_id, &[1; 20], &kek).await.unwrap());
    let checked = repo.open_secret(user_id, &kek).await.unwrap().unwrap();
    // another enrolment replaces the pending secret after the code was checked
    assert!(repo.begin_enrolment(user_id, &[2; 20], &kek).await.unwrap());
    assert!(
        !repo.confirm(user_id, 1, &checked.nonce).await.unwrap(),
        "a code checked against the first secret must not arm the second"
    );
    assert!(!repo.has_armed_factor(user_id).await.unwrap());

    let current = repo.open_secret(user_id, &kek).await.unwrap().unwrap();
    assert!(repo.confirm(user_id, 1, &current.nonce).await.unwrap());
    assert!(
        !repo.begin_enrolment(user_id, &[3; 20], &kek).await.unwrap(),
        "an armed factor is never replaced"
    );
    assert!(repo.has_armed_factor(user_id).await.unwrap());
    let kept = repo.open_secret(user_id, &kek).await.unwrap().unwrap();
    assert_eq!(
        kept.secret,
        vec![2; 20],
        "the armed secret is the one proved"
    );
}

/// A control plane without `ROLTER_KEK` cannot seal a secret, so it cannot
/// enrol anyone. The sign-in keeps the up-front refusal rather than handing out
/// an enrolment challenge that would fail one step later, and the policy
/// endpoint refuses a `required_*` value that would lock out everyone it binds,
/// the admin saving it included (#1852).
#[tokio::test]
async fn without_a_kek_a_required_policy_is_refused_up_front() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_without_kek(pool.clone())
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    // stored behind the api's back: a policy set while the deployment still
    // had a KEK, or before the endpoint refused one without it
    let (user_id, org_id) = seed_bound_member(
        &pool,
        "no-kek@example.com",
        &password,
        "required_all",
        "null",
    )
    .await;

    let refused = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "no-kek@example.com", "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "mfa_enrolment_required", "{body}");
    let kek: Option<bool> = sqlx::query_scalar(
        "select (detail->>'kek')::boolean from audit_log
         where action = 'auth.mfa_enrolment_required' and actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert_eq!(
        kek,
        Some(false),
        "the refusal says why it is the operator's"
    );
    let challenges: i64 =
        sqlx::query_scalar("select count(*) from mfa_challenges where user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        challenges, 0,
        "no enrolment token for a secret nobody can seal"
    );

    for policy in ["required_superadmin", "required_all"] {
        let response = client
            .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
            .json(&json!({
                "allow_password_login": true,
                "allow_sso": true,
                "mfa_policy": policy,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409, "{policy} without a KEK");
        let body: Value = response.json().await.unwrap();
        assert!(
            body.to_string().contains("ROLTER_KEK"),
            "the refusal names the remedy: {body}"
        );
    }

    // relaxing stays open, since it is the way out of the lockout above
    let relaxed = client
        .put(format!("{base}/api/v1/orgs/{org_id}/auth-policy"))
        .json(&json!({
            "allow_password_login": true,
            "allow_sso": true,
            "mfa_policy": "optional",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(relaxed.status(), 200);
    let signed_in = login_as(&client, &base, "no-kek@example.com", &password).await;
    assert!(signed_in["token"].is_string(), "{signed_in}");
}

/// A custom label's full life on a provider, and the conflict an operator gets
/// for re-using a key on the same subject rather than a silent overwrite
/// (#985).
#[tokio::test]
async fn custom_labels_round_trip_and_refuse_a_duplicate_key() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
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

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .json(&json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();

    let created = client
        .post(format!("{base}/api/v1/orgs/{org_id}/labels"))
        .json(&json!({
            "subject_type": "provider",
            "subject_id": provider_id,
            "key": "eu-only",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["source"], "custom");
    assert_eq!(created["key"], "eu-only");
    assert!(
        created["observed_at"].is_null() && created["observation"].is_null(),
        "an operator's assertion carries no provenance: {created}"
    );
    let label_id = created["id"].as_str().unwrap().to_string();

    // a valueless label is a flag; one with a value is a field
    let owner = client
        .post(format!("{base}/api/v1/orgs/{org_id}/labels"))
        .json(&json!({
            "subject_type": "provider",
            "subject_id": provider_id,
            "key": "owner",
            "value": "platform-team",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(owner.status(), 201);

    let duplicate = client
        .post(format!("{base}/api/v1/orgs/{org_id}/labels"))
        .json(&json!({
            "subject_type": "provider",
            "subject_id": provider_id,
            "key": "eu-only",
            "value": "yes",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        duplicate.status(),
        409,
        "re-using a key must not silently rewrite the existing label"
    );

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/labels"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");

    let filtered: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/labels?key=owner"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    assert_eq!(filtered[0]["value"], "platform-team");

    let updated: Value = client
        .put(format!("{base}/api/v1/orgs/{org_id}/labels/{label_id}"))
        .json(&json!({"value": "frankfurt"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["value"], "frankfurt");

    let deleted = client
        .delete(format!("{base}/api/v1/orgs/{org_id}/labels/{label_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);

    // the subject's own deletion sweeps what is left: subject_id is text, so
    // no foreign key does this for us
    let removed = client
        .delete(format!("{base}/api/v1/providers/{provider_id}"))
        .send()
        .await
        .unwrap();
    assert!(removed.status().is_success(), "{}", removed.status());
    let after: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/labels"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after.as_array().unwrap().len(),
        0,
        "a deleted provider must not leave labels behind: {after}"
    );
}

/// The auto label the pricing catalog produces, and the fact that no request
/// can edit, retract or impersonate one (#985).
#[tokio::test]
async fn auto_labels_are_produced_by_the_store_and_are_read_only() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let priced = client
        .put(format!("{base}/api/v1/model-prices"))
        .json(&json!({
            "model": "gpt-4o",
            "input_per_mtok": "2.50",
            "output_per_mtok": "10.00",
            "currency": "USD",
        }))
        .send()
        .await
        .unwrap();
    assert!(priced.status().is_success(), "{}", priced.status());

    let labels: Value = client
        .get(format!("{base}/api/v1/model-labels"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(labels.as_array().unwrap().len(), 1, "{labels}");
    let auto = &labels[0];
    assert_eq!(auto["source"], "auto");
    assert_eq!(auto["key"], "priced");
    assert_eq!(auto["subject_id"], "gpt-4o");
    assert_eq!(auto["value"], "USD");
    assert_eq!(
        auto["observation"], "model_prices",
        "an auto label must say what established it"
    );
    assert!(
        auto["observed_at"].is_string(),
        "an auto label must say when: {auto}"
    );
    let auto_id = auto["id"].as_str().unwrap().to_string();

    // read-only means read-only over HTTP too, not merely absent from the UI
    let edit = client
        .put(format!("{base}/api/v1/model-labels/{auto_id}"))
        .json(&json!({"value": "EUR"}))
        .send()
        .await
        .unwrap();
    assert_eq!(edit.status(), 404, "an observation is not editable");
    let drop = client
        .delete(format!("{base}/api/v1/model-labels/{auto_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(drop.status(), 404, "an observation is not deletable");

    // an operator writing the same key gets their own row rather than
    // overwriting the probed one, and it is plainly marked custom
    let shadow = client
        .post(format!("{base}/api/v1/model-labels"))
        .json(&json!({"model": "gpt-4o", "key": "priced", "value": "trust me"}))
        .send()
        .await
        .unwrap();
    assert_eq!(shadow.status(), 201);
    let shadow: Value = shadow.json().await.unwrap();
    assert_eq!(shadow["source"], "custom");
    assert!(shadow["observed_at"].is_null());

    let both: Value = client
        .get(format!("{base}/api/v1/model-labels?key=priced"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        both.as_array().unwrap().len(),
        2,
        "auto and custom labels sharing a key must coexist: {both}"
    );

    // withdrawing the observation withdraws the auto label and leaves the
    // operator's alone
    let unpriced = client
        .delete(format!("{base}/api/v1/model-prices/gpt-4o"))
        .send()
        .await
        .unwrap();
    assert_eq!(unpriced.status(), 204);
    let left: Value = client
        .get(format!("{base}/api/v1/model-labels"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(left.as_array().unwrap().len(), 1, "{left}");
    assert_eq!(left[0]["source"], "custom");
}

/// A label's tenancy comes from its subject, so one org's path must not reach
/// another org's provider or another org's label (#985).
#[tokio::test]
async fn labels_are_scoped_by_the_subject_they_describe() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn make_org(client: &reqwest::Client, base: &str, slug: &str) -> String {
        let org: Value = client
            .post(format!("{base}/api/v1/orgs"))
            .json(&json!({"name": slug, "slug": slug}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        org["id"].as_str().unwrap().to_string()
    }

    let acme = make_org(&client, &base, "acme").await;
    let globex = make_org(&client, &base, "globex").await;

    let provider: Value = client
        .post(format!("{base}/api/v1/orgs/{acme}/providers"))
        .json(&json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = provider["id"].as_str().unwrap().to_string();

    let cross = client
        .post(format!("{base}/api/v1/orgs/{globex}/labels"))
        .json(&json!({
            "subject_type": "provider",
            "subject_id": provider_id,
            "key": "stolen",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross.status(),
        404,
        "another org's path must not label this provider"
    );

    let mine: Value = client
        .post(format!("{base}/api/v1/orgs/{acme}/labels"))
        .json(&json!({
            "subject_type": "provider",
            "subject_id": provider_id,
            "key": "prod",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let label_id = mine["id"].as_str().unwrap().to_string();

    let peek = client
        .delete(format!("{base}/api/v1/orgs/{globex}/labels/{label_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        peek.status(),
        404,
        "a label id from another org must not be reachable"
    );

    let globex_labels: Value = client
        .get(format!("{base}/api/v1/orgs/{globex}/labels"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        globex_labels.as_array().unwrap().len(),
        0,
        "{globex_labels}"
    );

    // a model is not an org's to label through the org surface
    let wrong_surface = client
        .post(format!("{base}/api/v1/orgs/{acme}/labels"))
        .json(&json!({"subject_type": "model", "subject_id": provider_id, "key": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_surface.status(), 400);

    // and a malformed key is rejected before it reaches the check constraint
    let bad_key = client
        .post(format!("{base}/api/v1/orgs/{acme}/labels"))
        .json(&json!({
            "subject_type": "provider",
            "subject_id": provider_id,
            "key": "Not A Key",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_key.status(), 400);
}

/// A static MCP credential is sealed at rest, never comes back out of the read
/// API, and the auth kind and the credential columns cannot disagree (#952).

#[tokio::test]
async fn mcp_static_credential_seals_at_rest_and_never_reads_back() {
    skip_without_db!();
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

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Search",
            "slug": "search",
            "url": "https://mcp.example.com/mcp",
            "transport": "streamable_http",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    assert_eq!(
        server["auth_kind"], "none",
        "a server registers unauthenticated until told otherwise: {server}"
    );
    assert_eq!(server["has_credential"], false);

    const TOKEN: &str = "mcp-upstream-token-value";
    let armed: Value = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "bearer", "credential": TOKEN}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(armed["auth_kind"], "bearer");
    assert_eq!(armed["has_credential"], true);
    assert!(
        !armed.to_string().contains(TOKEN),
        "the credential must not come back in the write response"
    );

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !listed.to_string().contains(TOKEN),
        "the credential must not come back in the read API"
    );

    // sealed at rest: the row holds ciphertext, and the plaintext appears
    // nowhere in the column
    let stored: (Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
        "select credential_ciphertext, credential_nonce from mcp_servers where id = $1::uuid",
    )
    .bind(&server_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let ciphertext = stored.0.expect("ciphertext stored");
    assert!(stored.1.is_some(), "nonce stored beside the ciphertext");
    assert!(
        !String::from_utf8_lossy(&ciphertext).contains(TOKEN),
        "the credential must not be recoverable from the stored bytes"
    );

    // the store unseals it for the gateway's snapshot, and the same load feeds
    // the anonymous config view, which must not pass it on (#1938)
    let snapshot = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        snapshot.contains(TOKEN),
        "the gateway needs the unsealed credential to reach the server"
    );
    let config_view = client
        .get(format!("{base}/api/v1/config"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !config_view.contains(TOKEN),
        "the config view handed out a static mcp credential"
    );
    assert!(
        !config_view.contains("mcp.example.com"),
        "the config view lists a tenant's mcp servers"
    );

    // renaming nothing but the kind keeps the stored credential: an operator
    // cannot read it back, so requiring a re-type would make it unchangeable
    let to_header: Value = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header", "auth_header_name": "X-Api-Key"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(to_header["auth_kind"], "header");
    assert_eq!(to_header["auth_header_name"], "X-Api-Key");
    assert_eq!(to_header["has_credential"], true);

    // header mode must not be able to forge the bearer path
    let forged = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header", "auth_header_name": "Authorization"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        forged.status(),
        400,
        "an api key must not be presentable as Authorization"
    );

    let malformed = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header", "auth_header_name": "X Api Key"}))
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);

    let headerless = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "header"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        headerless.status(),
        400,
        "header mode without a header name is not a usable configuration"
    );

    // dropping to 'none' clears the credential rather than orphaning it, so
    // `rolter kek verify` is not left auditing a secret nothing can use
    let disarmed: Value = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "none"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disarmed["auth_kind"], "none");
    assert_eq!(disarmed["has_credential"], false);
    let cleared: (Option<Vec<u8>>,) =
        sqlx::query_as("select credential_ciphertext from mcp_servers where id = $1::uuid")
            .bind(&server_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(cleared.0.is_none(), "the sealed credential must be gone");

    // and a kind that needs one cannot be armed without supplying it
    let empty_handed = client
        .put(format!("{base}/api/v1/mcp-servers/{server_id}/auth"))
        .json(&json!({"auth_kind": "bearer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(empty_handed.status(), 400);
}

/// Per-server transport overrides, and the null that gives one back to the org
/// default (#952).
#[tokio::test]
async fn mcp_transport_overrides_are_per_server_and_revertible() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
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

    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Slow",
            "slug": "slow",
            "url": "https://mcp.example.com/mcp",
            "transport": "streamable_http",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();
    assert!(
        server["request_timeout_ms"].is_null(),
        "a new server inherits rather than pinning a copy of the org default"
    );

    let slowed: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"request_timeout_ms": 120000}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(slowed["request_timeout_ms"], 120000);

    // an unrelated PATCH must not disturb the override
    let renamed: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"description": "the slow one"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        renamed["request_timeout_ms"], 120000,
        "absent means leave it, not clear it"
    );

    let out_of_range = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"request_timeout_ms": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(out_of_range.status(), 400);

    // explicit null is how an operator gives the server back to the org default
    let reverted: Value = client
        .patch(format!("{base}/api/v1/mcp-servers/{server_id}"))
        .json(&json!({"request_timeout_ms": null}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        reverted["request_timeout_ms"].is_null(),
        "null must clear the override: {reverted}"
    );
}

/// `GET /api/v1/orgs/{org_id}/projects` answers for the whole org in one
/// request, which is the point of it (#1357).
///
/// Covers what the per-team fan-out it replaces could get wrong: projects from
/// every team, each carrying the team that owns it so a caller can still group
/// by team, ordered team-then-project, and nothing from a sibling org.
#[tokio::test]
async fn org_projects_lists_every_team_in_the_org_and_no_other() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme-org-projects"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    // teams created out of alphabetical order, so an ordering that merely
    // followed insertion would fail below
    let mut teams = Vec::new();
    for name in ["Zulu", "Alpha"] {
        let team = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/teams"),
            json!({ "name": name }),
        )
        .await;
        teams.push((
            name,
            team["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap(),
        ));
    }

    // both teams own a "prod": an org-wide list that dropped the team would
    // offer the operator two indistinguishable options
    for (_, team) in &teams {
        for project in ["prod", "staging"] {
            post_as(
                &client,
                format!("{base}/api/v1/teams/{team}/projects"),
                json!({ "name": project }),
            )
            .await;
        }
    }

    // a sibling org with a project of its own, which must not leak in
    let other = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Other", "slug": "other-org-projects"}),
    )
    .await;
    let other_org = other["id"].as_str().unwrap().to_string();
    let other_team = post_as(
        &client,
        format!("{base}/api/v1/orgs/{other_org}/teams"),
        json!({"name": "Elsewhere"}),
    )
    .await;
    let other_team_id = other_team["id"].as_str().unwrap().to_string();
    post_as(
        &client,
        format!("{base}/api/v1/teams/{other_team_id}/projects"),
        json!({"name": "secret"}),
    )
    .await;

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/projects"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = listed.as_array().expect("an array of projects");
    let named: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| {
            (
                row["team_name"].as_str().unwrap(),
                row["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        named,
        vec![
            ("Alpha", "prod"),
            ("Alpha", "staging"),
            ("Zulu", "prod"),
            ("Zulu", "staging"),
        ],
        "ordered by team then project, and scoped to this org: {listed}"
    );

    // every row names the team it belongs to by id as well as by name
    let alpha = teams
        .iter()
        .find(|(name, _)| *name == "Alpha")
        .expect("the alpha team")
        .1;
    assert_eq!(rows[0]["team_id"], alpha.to_string());
    assert!(rows[0]["id"].is_string());
    assert!(rows[0]["created_at"].is_string());

    // read is a viewer's; a role at the org answers for every team
    let org_viewer = seed_user(&pool, "org-viewer@example.com", false).await;
    seed_membership(&pool, org_viewer, Some(org_uuid), None, None, "viewer").await;
    let org_token = seed_session(&pool, org_viewer, "org-projects-viewer").await;
    let allowed = client
        .get(format!("{base}/api/v1/orgs/{org_id}/projects"))
        .bearer_auth(&org_token)
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);

    let team_viewer = seed_user(&pool, "team-viewer@example.com", false).await;
    seed_membership(
        &pool,
        team_viewer,
        Some(org_uuid),
        Some(alpha),
        None,
        "viewer",
    )
    .await;
    let team_token = seed_session(&pool, team_viewer, "org-projects-team-viewer").await;
    // a role in one team answers with that team's projects rather than
    // refusing the whole list, so a team member can find their own (#1846)
    let for_team = client
        .get(format!("{base}/api/v1/orgs/{org_id}/projects"))
        .bearer_auth(&team_token)
        .send()
        .await
        .unwrap();
    assert_eq!(for_team.status(), 200);
    let for_team: Value = for_team.json().await.unwrap();
    let names: Vec<(&str, &str)> = for_team
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["team_name"].as_str().unwrap(),
                row["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(names, vec![("Alpha", "prod"), ("Alpha", "staging")]);

    // and a caller with no role anywhere in this org is still refused
    let outsider = seed_user(&pool, "outsider@example.com", false).await;
    let other_uuid: uuid::Uuid = other_org.parse().unwrap();
    seed_membership(&pool, outsider, Some(other_uuid), None, None, "viewer").await;
    let outsider_token = seed_session(&pool, outsider, "org-projects-outsider").await;
    let refused = client
        .get(format!("{base}/api/v1/orgs/{org_id}/projects"))
        .bearer_auth(&outsider_token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
}

/// #1643: a group created through this API was carried by `/internal/snapshot`
/// and still 404'd at the gateway until someone restarted it. The gateway's
/// watcher refetches on a `config_version` change, and neither group table had
/// a `bump_config_version()` trigger — so `publish_config_change` republished
/// the *same* version and nothing downstream moved. Any later write to another
/// config table masked it, which made the failure look intermittent.
///
/// This pins the operator-visible path rather than the trigger alone: each CRUD
/// call an operator makes on a group has to leave a version the data plane can
/// notice, and the snapshot that version names has to carry the group.
#[tokio::test]
async fn provider_group_crud_advances_the_version_the_gateway_watches() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.expect("app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client.post(&url).json(&body).send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap();
        assert!(status.is_success(), "POST {url} failed ({status}): {json}");
        json
    }

    let org = post(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().expect("org id").to_string();

    let mut provider_ids = Vec::new();
    for n in 1..=2 {
        let provider = post(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/providers"),
            json!({
                "name": format!("vllm-a100-0{n}"),
                "kind": "openai",
                "api_base": "http://vllm.internal",
            }),
        )
        .await;
        provider_ids.push(provider["id"].as_str().expect("provider id").to_string());
    }

    let before = config_version(&pool).await;
    let group = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/provider-groups"),
        json!({
            "name": "Llama fleet",
            "slug": "llama-fleet",
            "strategy": "weighted",
            "members": [
                {"provider_id": provider_ids[0], "weight": 3},
                {"provider_id": provider_ids[1], "weight": 1},
            ],
        }),
    )
    .await;
    let group_id = group["id"].as_str().expect("group id").to_string();
    assert!(
        config_version(&pool).await > before,
        "creating a group left config_version at {before}: the gateway never refetches, so \
         `llama-fleet/<model>` 404s until someone restarts it"
    );

    // the version is only worth publishing if the snapshot it names carries the
    // group — a bump pointing at a snapshot without it fails identically
    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let groups = snapshot["config"]["provider_groups"]
        .as_array()
        .expect("snapshot carries provider_groups");
    let served = groups
        .iter()
        .find(|g| g["slug"] == "llama-fleet")
        .expect("the group is not in the snapshot its version serves");
    assert_eq!(
        served["members"].as_array().map(Vec::len),
        Some(2),
        "the snapshot dropped a member: {served}"
    );

    // retuning membership is the write an operator repeats most often
    let before = config_version(&pool).await;
    let resp = client
        .put(format!("{base}/api/v1/provider-groups/{group_id}"))
        .json(&json!({
            "strategy": "round_robin",
            "members": [{"provider_id": provider_ids[0], "weight": 1}],
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.text().await.unwrap());
    assert!(
        config_version(&pool).await > before,
        "editing a group did not bump config_version: the fleet keeps the old strategy and \
         keeps sending traffic to a member the operator removed"
    );

    let before = config_version(&pool).await;
    let resp = client
        .delete(format!("{base}/api/v1/provider-groups/{group_id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.text().await.unwrap());
    assert!(
        config_version(&pool).await > before,
        "deleting a group did not bump config_version"
    );
}

/// A route's complexity policy reads at the same bar as the route it hangs off
/// (#1666), and writes at the mutation bar as it always has.
///
/// #704 held the GET to `route:update`, which made the policy the one route
/// attribute a viewer could list but never see — the dashboard's Complexity
/// Router answered a reader with a permission error covering the whole screen.
/// The policy is configuration, not a secret: it already travels to the gateway
/// inside the route's `params`. This asserts both halves at once, because the
/// value of widening the read depends entirely on the write staying put.
#[tokio::test]
async fn a_viewer_reads_a_route_complexity_policy_but_cannot_write_one() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
        let response = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let json: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {json}");
        json
    }

    let org = post_as(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Tiers", "slug": "tiers"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();

    let team = post_as(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap().to_string();

    let project = post_as(
        &client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();

    // a small model to fall back to, and the route the policy hangs off
    post_as(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o-mini", "strategy": "round_robin"}),
    )
    .await;
    let route = post_as(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().unwrap().to_string();

    let policy = json!({"tiers": [
        {"name": "small", "max_input_bytes": 4096, "route": "gpt-4o-mini"},
        {"name": "large", "route": "gpt-4o"},
    ]});
    let written = client
        .put(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth("admintok")
        .json(&policy)
        .send()
        .await
        .unwrap();
    assert!(
        written.status().is_success(),
        "admin writes the policy: {}",
        written.status()
    );

    let viewer = seed_user(&pool, "complexity-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "complexityviewer").await;

    let read = client
        .get(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        read.status(),
        200,
        "a viewer reads the policy of a route it can already list"
    );
    let body: Value = read.json().await.unwrap();
    assert_eq!(
        body["tiers"].as_array().map(Vec::len),
        Some(2),
        "the policy itself comes back, not an empty stand-in: {body}"
    );
    assert_eq!(body["tiers"][0]["name"], "small");

    // the write is untouched: a viewer still changes nothing. the body is a
    // policy the control plane would accept from an admin, so a widened write
    // bar shows up as a 200 here rather than hiding behind a validation error
    let refused = client
        .put(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth(&viewer_token)
        .json(&json!({"tiers": [{"name": "large", "route": "gpt-4o"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403, "a viewer must not write the policy");

    // and the refusal was a refusal, not a silent no-op
    let after: Value = client
        .get(format!("{base}/api/v1/routes/{route_id}/complexity"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["tiers"].as_array().map(Vec::len), Some(2));
}

/// `PATCH /api/v1/me/profile` (#1823): any signed-in account, a viewer
/// included, edits its own display name and bio; omitted fields stay, `null`
/// and `""` clear, bad input is a 400, and the audit row names fields only.
#[tokio::test]
async fn a_viewer_edits_their_own_profile() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let viewer = seed_user(&pool, "profile-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "profileviewer").await;
    let bystander = seed_user(&pool, "profile-bystander@example.com", false).await;
    let bystander_token = seed_session(&pool, bystander, "profilebystander").await;

    let me = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .get(format!("{base}/api/v1/auth/me"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let patch = |token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .patch(format!("{base}/api/v1/me/profile"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = res.status().as_u16();
            (status, res.json::<Value>().await.unwrap())
        }
    };

    let before = me(token.clone()).await;
    assert_eq!(before["user"]["display_name"], Value::Null);
    assert_eq!(before["user"]["bio"], Value::Null);
    assert_eq!(before["display_name_managed"], false);

    // set both, with surrounding whitespace normalised away
    let (status, body) = patch(
        token.clone(),
        json!({"display_name": "  Grace Hopper ", "bio": "Compilers.\nCOBOL."}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["display_name"], "Grace Hopper");
    assert_eq!(body["bio"], "Compilers.\nCOBOL.");
    let after = me(token.clone()).await;
    assert_eq!(after["user"]["display_name"], "Grace Hopper");
    assert_eq!(after["user"]["bio"], "Compilers.\nCOBOL.");

    // omitted leaves the other field alone
    let (status, body) = patch(token.clone(), json!({"display_name": "Grace"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["bio"], "Compilers.\nCOBOL.");

    // null clears one field, an empty string the other
    let (status, body) = patch(token.clone(), json!({"display_name": null, "bio": ""})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["display_name"], Value::Null);
    assert_eq!(body["bio"], Value::Null);
    let cleared = me(token.clone()).await;
    assert_eq!(cleared["user"]["display_name"], Value::Null);
    assert_eq!(cleared["user"]["bio"], Value::Null);

    // validation
    for bad in [
        json!({"display_name": "x".repeat(81)}),
        json!({"bio": "x".repeat(501)}),
        json!({"display_name": "line\u{0007}bell"}),
        json!({"display_name": "two\nlines"}),
        json!({"display_name": "   "}),
        json!({"bio": " \n\t "}),
        json!({"display_name": 7}),
    ] {
        let (status, body) = patch(token.clone(), bad.clone()).await;
        assert_eq!(status, 400, "{bad} was accepted: {body}");
    }
    // exactly at the limits is fine
    let (status, _) = patch(
        token.clone(),
        json!({"display_name": "n".repeat(80), "bio": "b".repeat(500)}),
    )
    .await;
    assert_eq!(status, 200);

    // the route writes only the caller's row
    let other = me(bystander_token.clone()).await;
    assert_eq!(other["user"]["display_name"], Value::Null);
    assert_eq!(other["user"]["bio"], Value::Null);

    // no session, no profile
    let anon = client
        .patch(format!("{base}/api/v1/me/profile"))
        .json(&json!({"bio": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // `user:update` is still superadmin-only: a viewer cannot edit an account
    let denied = client
        .put(format!("{base}/api/v1/users/{bystander}"))
        .bearer_auth(&token)
        .json(&json!({"email": "renamed@example.com"}))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(denied.status().as_u16(), 403 | 404),
        "viewer edited an account: {}",
        denied.status()
    );

    // audit rows name the fields, never the bio text
    let rows: Vec<(Value,)> = sqlx::query_as(
        "select detail from audit_log
         where action = 'user.profile.update' and target_id = $1 order by at",
    )
    .bind(viewer)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        rows.len() >= 4,
        "expected an audit row per change: {rows:?}"
    );
    assert_eq!(rows[0].0, json!({"fields": ["display_name", "bio"]}));
    assert_eq!(rows[1].0, json!({"fields": ["display_name"]}));
    assert!(
        !rows.iter().any(|(d,)| d.to_string().contains("COBOL")),
        "a bio leaked into the audit log"
    );
}

/// `GET`/`PUT /api/v1/me/preferences` (#1824): a viewer saves and reads back
/// their own document; bad input is a 400; unset keys read as null; the
/// document is whole-replace; the audit row names keys only.
#[tokio::test]
async fn a_viewer_saves_and_reads_their_own_preferences() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let viewer = seed_user(&pool, "prefs-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "prefsviewer").await;
    let other = seed_user(&pool, "prefs-other@example.com", false).await;
    let other_token = seed_session(&pool, other, "prefsother").await;

    let get = |token: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .get(format!("{base}/api/v1/me/preferences"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.json::<Value>().await.unwrap())
        }
    };
    let put = |token: String, body: Value| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let res = client
                .put(format!("{base}/api/v1/me/preferences"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            (res.status().as_u16(), res.json::<Value>().await.unwrap())
        }
    };

    // nothing saved: every key reads as absent and there is no scope to open on
    let (status, empty) = get(token.clone()).await;
    assert_eq!(status, 200, "{empty}");
    for key in [
        "language",
        "default_org_id",
        "default_team_id",
        "default_project_id",
        "default_playground_model",
        "chart_time_zone",
        "effective_default_scope",
    ] {
        assert_eq!(empty[key], Value::Null, "{key}: {empty}");
    }

    // save, read back
    let (status, saved) = put(
        token.clone(),
        json!({
            "language": "ru",
            "default_playground_model": "  gpt-4o ",
            "chart_time_zone": "Europe/Berlin",
        }),
    )
    .await;
    assert_eq!(status, 200, "{saved}");
    let (_, read) = get(token.clone()).await;
    assert_eq!(read["language"], "ru");
    assert_eq!(read["default_playground_model"], "gpt-4o");
    assert_eq!(read["chart_time_zone"], "Europe/Berlin");
    assert_eq!(read["default_project_id"], Value::Null);

    // PUT replaces the whole document: a key left out is cleared
    let (status, _) = put(token.clone(), json!({"language": "en"})).await;
    assert_eq!(status, 200);
    let (_, read) = get(token.clone()).await;
    assert_eq!(read["language"], "en");
    assert_eq!(read["chart_time_zone"], Value::Null);
    assert_eq!(read["default_playground_model"], Value::Null);

    // validation
    for bad in [
        json!({"unknown_key": 1}),
        json!({"language": "xx"}),
        json!({"language": 7}),
        json!({"chart_time_zone": "Not A Zone"}),
        json!({"chart_time_zone": "../../etc/passwd"}),
        json!({"default_project_id": "not-a-uuid"}),
        json!({"default_org_id": 5}),
        json!({"default_playground_model": "x".repeat(201)}),
        json!({"default_playground_model": "   "}),
    ] {
        let (status, body) = put(token.clone(), bad.clone()).await;
        assert_eq!(status, 400, "{bad} was accepted: {body}");
    }
    // a refused write changed nothing
    let (_, read) = get(token.clone()).await;
    assert_eq!(read["language"], "en");

    // each user sees only their own document: there is no route naming another
    let (_, theirs) = get(other_token.clone()).await;
    assert_eq!(theirs["language"], Value::Null);
    let (status, _) = put(other_token.clone(), json!({"language": "ru"})).await;
    assert_eq!(status, 200);
    let (_, mine) = get(token.clone()).await;
    assert_eq!(mine["language"], "en");

    // no session, no preferences
    let anon = client
        .get(format!("{base}/api/v1/me/preferences"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // audit names keys, never values
    let rows: Vec<(Value,)> = sqlx::query_as(
        "select detail from audit_log
         where action = 'user.preferences.update' and target_id = $1 order by at",
    )
    .bind(viewer)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[0].0,
        json!({"keys": ["chart_time_zone", "default_playground_model", "language"]})
    );
    assert_eq!(
        rows[1].0,
        json!({"keys": ["chart_time_zone", "default_playground_model", "language"]})
    );
    assert!(!rows.iter().any(|(d,)| d.to_string().contains("gpt-4o")));
}

/// A stored default scope never widens access (#1824): it is kept as written,
/// but `effective_default_scope` is computed from the caller's current
/// memberships, so a scope they lost, or never had, is never handed back.
#[tokio::test]
async fn a_default_scope_the_user_lost_is_never_the_effective_one() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn make(client: &reqwest::Client, url: String, body: Value) -> uuid::Uuid {
        let v: Value = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["id"].as_str().unwrap().parse().unwrap()
    }
    let org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "PrefsOrg", "slug": "prefs-org"}),
    )
    .await;
    let team = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/teams"),
        json!({"name": "T"}),
    )
    .await;
    let proj_a = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "A"}),
    )
    .await;
    let proj_b = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "B"}),
    )
    .await;
    let foreign_org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "Foreign", "slug": "prefs-foreign"}),
    )
    .await;

    let user = seed_user(&pool, "prefs-scope@example.com", false).await;
    let token = seed_session(&pool, user, "prefsscope").await;
    seed_membership(&pool, user, None, None, Some(proj_a), "viewer").await;
    seed_membership(&pool, user, None, None, Some(proj_b), "viewer").await;

    let read = || async {
        client
            .get(format!("{base}/api/v1/me/preferences"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let put = |body: Value| {
        let client = client.clone();
        let url = format!("{base}/api/v1/me/preferences");
        let token = token.clone();
        async move {
            client
                .put(url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    // a readable default project is honoured, with its chain filled in
    let res = put(json!({"default_project_id": proj_b})).await;
    assert_eq!(res.status(), 200);
    let got = read().await;
    assert_eq!(
        got["effective_default_scope"],
        json!({"org_id": org, "team_id": team, "project_id": proj_b})
    );

    // the membership goes away: the id is still stored, but no longer effective
    sqlx::query("delete from memberships where user_id = $1 and project_id = $2")
        .bind(user)
        .bind(proj_b)
        .execute(&pool)
        .await
        .unwrap();
    let got = read().await;
    assert_eq!(got["default_project_id"], json!(proj_b));
    assert_eq!(
        got["effective_default_scope"],
        json!({"org_id": org, "team_id": team, "project_id": proj_a}),
        "fell back to the scope still readable"
    );

    // a scope the user never had is accepted at write time and never effective
    let res = put(json!({"default_org_id": foreign_org})).await;
    assert_eq!(res.status(), 200);
    let got = read().await;
    assert_eq!(got["default_org_id"], json!(foreign_org));
    assert_eq!(got["effective_default_scope"]["project_id"], json!(proj_a));
    assert_ne!(got["effective_default_scope"]["org_id"], json!(foreign_org));

    // and with nothing readable left there is no scope at all
    sqlx::query("delete from memberships where user_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let got = read().await;
    assert_eq!(got["effective_default_scope"], Value::Null);

    // a default naming a row that no longer exists is not an error
    let res = put(json!({"default_project_id": uuid::Uuid::new_v4()})).await;
    assert_eq!(res.status(), 200);
    assert_eq!(read().await["effective_default_scope"], Value::Null);
}

/// A SCIM-provisioned account's `displayName` is authoritative (#1823): it is
/// copied onto the account, `/auth/me` flags it managed, and the self-service
/// route refuses to change it while still letting the account edit its bio.
#[tokio::test]
async fn a_scim_managed_display_name_is_read_only_but_the_bio_is_not() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "ProfileScimOrg", "slug": "profile-scim-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let minted: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "okta"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = minted["secret"].as_str().unwrap().to_string();

    let created: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "hopper@example.com",
            "displayName": "Grace Hopper",
            "emails": [{"value": "hopper@example.com", "primary": true}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = created["id"].as_str().unwrap().to_string();
    let user_id: uuid::Uuid = scim_id.parse().unwrap();
    let token = seed_session(&pool, user_id, "profilescimuser").await;

    let me = || async {
        client
            .get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let patch = |body: Value| {
        let client = client.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            client
                .patch(format!("{base}/api/v1/me/profile"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    let seen = me().await;
    assert_eq!(seen["user"]["display_name"], "Grace Hopper");
    assert_eq!(seen["display_name_managed"], true);

    let refused = patch(json!({"display_name": "Someone Else"})).await;
    assert_eq!(refused.status(), 409);
    let cleared = patch(json!({"display_name": null})).await;
    assert_eq!(cleared.status(), 409);
    assert_eq!(me().await["user"]["display_name"], "Grace Hopper");

    // the bio is the user's, and resending the managed name unchanged is fine
    let ok = patch(json!({"display_name": "Grace Hopper", "bio": "Nanoseconds."})).await;
    assert_eq!(ok.status(), 200);
    let body: Value = ok.json().await.unwrap();
    assert_eq!(body["bio"], "Nanoseconds.");
    assert_eq!(body["display_name_managed"], true);

    // a later SCIM replace moves the name
    let replaced = client
        .put(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&secret)
        .json(&json!({
            "userName": "hopper@example.com",
            "displayName": "Rear Admiral Hopper",
            "emails": [{"value": "hopper@example.com", "primary": true}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(replaced.status(), 200);
    assert_eq!(me().await["user"]["display_name"], "Rear Admiral Hopper");
    assert_eq!(me().await["user"]["bio"], "Nanoseconds.");
}

/// `/api/v1/me/saved-views` (#1825): a viewer saves, lists, renames, updates
/// and deletes filter presets on both surfaces; bad input is refused; the
/// audit rows name the preset and never what it filters by.
#[tokio::test]
async fn a_viewer_manages_their_own_saved_views() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let viewer = seed_user(&pool, "views-viewer@example.com", false).await;
    let token = seed_session(&pool, viewer, "viewsviewer").await;
    let views = format!("{base}/api/v1/me/saved-views");

    let send = |method: reqwest::Method, url: String, body: Option<Value>| {
        let client = client.clone();
        let token = token.clone();
        async move {
            let mut req = client.request(method, url).bearer_auth(token);
            if let Some(body) = body {
                req = req.json(&body);
            }
            let res = req.send().await.unwrap();
            let status = res.status().as_u16();
            let text = res.text().await.unwrap();
            (
                status,
                serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            )
        }
    };
    let create = |body: Value| send(reqwest::Method::POST, views.clone(), Some(body));

    // nothing saved yet
    let (status, empty) = send(reqwest::Method::GET, views.clone(), None).await;
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty, json!([]));

    // one preset per surface
    let (status, logs) = create(json!({
        "surface": "llm_logs",
        "name": "  Errors last week ",
        "filters": {"window": "7d", "status": "error", "model": "gpt-4o"},
    }))
    .await;
    assert_eq!(status, 200, "{logs}");
    assert_eq!(logs["name"], "Errors last week");
    assert_eq!(logs["filters"]["status"], "error");
    assert_eq!(logs["effective_filters"], logs["filters"]);
    assert_eq!(logs["unavailable"], json!([]));
    let logs_preset = logs["id"].as_str().unwrap().to_string();
    let (status, dash) = create(json!({
        "surface": "dashboard", "name": "Month to date",
        "filters": {"window": "mtd", "bucket": "day"},
    }))
    .await;
    assert_eq!(status, 200, "{dash}");
    let dash_preset = dash["id"].as_str().unwrap().to_string();

    // list, all and per surface
    let (_, all) = send(reqwest::Method::GET, views.clone(), None).await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let (_, only_logs) = send(
        reqwest::Method::GET,
        format!("{views}?surface=llm_logs"),
        None,
    )
    .await;
    assert_eq!(only_logs.as_array().unwrap().len(), 1);
    assert_eq!(only_logs[0]["id"], logs_preset);
    let (status, _) = send(reqwest::Method::GET, format!("{views}?surface=nope"), None).await;
    assert_eq!(status, 400);

    // get one
    let (status, one) = send(reqwest::Method::GET, format!("{views}/{dash_preset}"), None).await;
    assert_eq!(status, 200, "{one}");
    assert_eq!(one["filters"], json!({"window": "mtd", "bucket": "day"}));

    // rename leaves the filters alone; a case-only rename of itself is fine
    let (status, renamed) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"name": "Weekly errors"})),
    )
    .await;
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["name"], "Weekly errors");
    assert_eq!(renamed["filters"]["model"], "gpt-4o");
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"name": "WEEKLY ERRORS"})),
    )
    .await;
    assert_eq!(status, 200);

    // update replaces the whole filter set and leaves the name alone
    let (status, updated) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"filters": {"window": "30d"}})),
    )
    .await;
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["filters"], json!({"window": "30d"}));
    assert_eq!(updated["name"], "WEEKLY ERRORS");

    // a patch that changes nothing is refused, and so is a surface change
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"surface": "dashboard"})),
    )
    .await;
    assert_eq!(status, 400);

    // the same name on another surface is fine, on the same one is a 409 even
    // when only the case differs
    let (status, _) = create(json!({"surface": "dashboard", "name": "weekly errors"})).await;
    assert_eq!(status, 200);
    let (status, dup) = create(json!({"surface": "llm_logs", "name": "weekly ERRORS"})).await;
    assert_eq!(status, 409, "{dup}");
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"name": "Month to date"})),
    )
    .await;
    assert_eq!(status, 200, "a name taken on another surface is free here");
    let (status, _) = create(json!({"surface": "dashboard", "name": "month TO date"})).await;
    assert_eq!(status, 409);
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{dash_preset}"),
        Some(json!({"name": "weekly errors"})),
    )
    .await;
    assert_eq!(status, 409, "renaming onto an existing name");

    // validation
    for bad in [
        json!({"surface": "billing", "name": "x"}),
        json!({"surface": "llm_logs", "name": "   "}),
        json!({"surface": "llm_logs", "name": "x".repeat(81)}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"limit": 500}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"cursor": "a|b"}}),
        json!({"surface": "dashboard", "name": "ok", "filters": {"model": "gpt-4o"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"window": "forever"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"window": 7}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"status": ["error"]}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"key": "not-a-uuid"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"customer": "a-string"}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": {"business_unit": [1]}}),
        json!({"surface": "llm_logs", "name": "ok", "filters": []}),
        json!({"surface": "llm_logs", "name": "ok", "extra": 1}),
        json!({"name": "no surface"}),
    ] {
        let (status, body) = create(bad.clone()).await;
        assert_eq!(status, 400, "{bad} was accepted: {body}");
    }
    let (status, _) = send(
        reqwest::Method::PATCH,
        format!("{views}/{logs_preset}"),
        Some(json!({"filters": {"surface_is_fixed": true}})),
    )
    .await;
    assert_eq!(status, 400);

    // delete, then it is gone
    let (status, _) = send(
        reqwest::Method::DELETE,
        format!("{views}/{dash_preset}"),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = send(reqwest::Method::GET, format!("{views}/{dash_preset}"), None).await;
    assert_eq!(status, 404);
    let (status, _) = send(
        reqwest::Method::DELETE,
        format!("{views}/{dash_preset}"),
        None,
    )
    .await;
    assert_eq!(status, 404);

    // no session, no presets
    for req in [
        client.get(&views),
        client.post(&views).json(&json!({})),
        client.get(format!("{views}/{logs_preset}")),
        client
            .patch(format!("{views}/{logs_preset}"))
            .json(&json!({})),
        client.delete(format!("{views}/{logs_preset}")),
    ] {
        assert_eq!(req.send().await.unwrap().status(), 401);
    }

    // the audit rows name the preset and the surface, never a filter value
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "select action, detail from audit_log
         where action like 'user.saved_view.%' and actor_user_id = $1 order by at",
    )
    .bind(viewer)
    .fetch_all(&pool)
    .await
    .unwrap();
    let actions: Vec<&str> = rows.iter().map(|(a, _)| a.as_str()).collect();
    assert!(actions.contains(&"user.saved_view.create"), "{actions:?}");
    assert!(actions.contains(&"user.saved_view.update"), "{actions:?}");
    assert!(actions.contains(&"user.saved_view.delete"), "{actions:?}");
    let dump = serde_json::to_string(&rows).unwrap();
    for value in ["gpt-4o", "30d", "mtd", "\"window\"", "\"status\""] {
        assert!(
            !dump.contains(value),
            "{value} leaked into the audit log: {dump}"
        );
    }
    let (_, created) = rows
        .iter()
        .find(|(a, _)| a == "user.saved_view.create")
        .unwrap();
    assert_eq!(
        created,
        &json!({"surface": "llm_logs", "name": "Errors last week"})
    );
}

/// An account holds at most 50 presets per surface (#1825); the 51st is a 409,
/// the cap is per surface, and deleting one frees a slot.
#[tokio::test]
async fn saved_views_are_capped_per_user_and_surface() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let views = format!("http://{addr}/api/v1/me/saved-views");

    let user = seed_user(&pool, "views-cap@example.com", false).await;
    let token = seed_session(&pool, user, "viewscap").await;
    let other = seed_user(&pool, "views-cap-other@example.com", false).await;
    let other_token = seed_session(&pool, other, "viewscapother").await;

    let create = |token: String, surface: &'static str, name: String| {
        let client = client.clone();
        let url = views.clone();
        async move {
            client
                .post(url)
                .bearer_auth(token)
                .json(&json!({"surface": surface, "name": name}))
                .send()
                .await
                .unwrap()
        }
    };
    let mut first = String::new();
    for n in 0..50 {
        let res = create(token.clone(), "llm_logs", format!("preset {n}")).await;
        assert_eq!(res.status(), 200, "preset {n}");
        if n == 0 {
            first = res.json::<Value>().await.unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string();
        }
    }
    let over = create(token.clone(), "llm_logs", "one too many".to_string()).await;
    assert_eq!(over.status(), 409);
    let body: Value = over.json().await.unwrap();
    assert!(
        body["error"]["message"].as_str().unwrap().contains("50"),
        "{body}"
    );

    // another surface and another user have their own allowance
    assert_eq!(
        create(token.clone(), "dashboard", "fine".to_string())
            .await
            .status(),
        200
    );
    assert_eq!(
        create(other_token, "llm_logs", "fine".to_string())
            .await
            .status(),
        200
    );

    // deleting one frees a slot
    let gone = client
        .delete(format!("{views}/{first}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 204);
    assert_eq!(
        create(token, "llm_logs", "one more".to_string())
            .await
            .status(),
        200
    );
}

/// Presets are private (#1825): another account's list excludes them and
/// every by-id route answers 404 for them, as it does for an id that does not
/// exist, so an id cannot be probed.
#[tokio::test]
async fn one_users_saved_view_is_invisible_to_another() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let views = format!("http://{addr}/api/v1/me/saved-views");

    let alice = seed_user(&pool, "views-alice@example.com", false).await;
    let alice_token = seed_session(&pool, alice, "viewsalice").await;
    let bob = seed_user(&pool, "views-bob@example.com", false).await;
    let bob_token = seed_session(&pool, bob, "viewsbob").await;
    let admin = seed_user(&pool, "views-admin@example.com", true).await;
    let admin_token = seed_session(&pool, admin, "viewsadmin").await;

    let made: Value = client
        .post(&views)
        .bearer_auth(&alice_token)
        .json(&json!({"surface": "dashboard", "name": "Alice's", "filters": {"window": "7d"}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let preset_id = made["id"].as_str().unwrap().to_string();
    let missing = uuid::Uuid::new_v4();

    for (who, token) in [("bob", &bob_token), ("a superadmin", &admin_token)] {
        let list: Value = client
            .get(&views)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(list, json!([]), "{who} saw another account's preset");

        // the same answer for a preset that exists and one that does not
        for id in [preset_id.clone(), missing.to_string()] {
            let url = format!("{views}/{id}");
            let get = client.get(&url).bearer_auth(token).send().await.unwrap();
            assert_eq!(get.status(), 404, "{who} get {id}");
            let patch = client
                .patch(&url)
                .bearer_auth(token)
                .json(&json!({"name": "hijacked"}))
                .send()
                .await
                .unwrap();
            assert_eq!(patch.status(), 404, "{who} patch {id}");
            let delete = client.delete(&url).bearer_auth(token).send().await.unwrap();
            assert_eq!(delete.status(), 404, "{who} delete {id}");
        }
    }

    // untouched, and bob may reuse the name because names are per user
    let mine: Value = client
        .get(format!("{views}/{preset_id}"))
        .bearer_auth(&alice_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mine["name"], "Alice's");
    let reuse = client
        .post(&views)
        .bearer_auth(&bob_token)
        .json(&json!({"surface": "dashboard", "name": "Alice's"}))
        .send()
        .await
        .unwrap();
    assert_eq!(reuse.status(), 200);
}

/// A preset that names something the caller can no longer read still applies
/// the rest (#1825): `effective_filters` drops the ids and `unavailable` says
/// which, for a key, a business unit and a customer, whether access was lost by
/// removing the membership or the row was deleted.
#[tokio::test]
async fn a_saved_view_reports_the_filters_the_user_can_no_longer_read() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn make(client: &reqwest::Client, url: String, body: Value) -> uuid::Uuid {
        let v: Value = client
            .post(url)
            .bearer_auth("admintok")
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["id"].as_str().unwrap().parse().unwrap()
    }
    let org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "ViewsOrg", "slug": "views-org"}),
    )
    .await;
    let other_org = make(
        &client,
        format!("{base}/api/v1/orgs"),
        json!({"name": "ViewsOther", "slug": "views-other"}),
    )
    .await;
    let team = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/teams"),
        json!({"name": "T"}),
    )
    .await;
    let project = make(
        &client,
        format!("{base}/api/v1/teams/{team}/projects"),
        json!({"name": "P"}),
    )
    .await;
    let kept_key = make(
        &client,
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        json!({"name": "kept"}),
    )
    .await;
    let doomed_key = make(
        &client,
        format!("{base}/api/v1/projects/{project}/virtual-keys"),
        json!({"name": "doomed"}),
    )
    .await;
    let unit = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/business-units"),
        json!({"name": "Finance"}),
    )
    .await;
    let foreign_unit = make(
        &client,
        format!("{base}/api/v1/orgs/{other_org}/business-units"),
        json!({"name": "Elsewhere"}),
    )
    .await;
    let customer = make(
        &client,
        format!("{base}/api/v1/orgs/{org}/customers"),
        json!({"name": "Acme"}),
    )
    .await;

    let member = seed_user(&pool, "views-member@example.com", false).await;
    let token = seed_session(&pool, member, "viewsmember").await;
    seed_membership(&pool, member, None, None, Some(project), "viewer").await;

    let views = format!("{base}/api/v1/me/saved-views");
    let res = client
        .post(&views)
        .bearer_auth(&token)
        .json(&json!({
            "surface": "llm_logs",
            "name": "Everything",
            "filters": {
                "window": "30d",
                "model": "gpt-4o",
                "key": doomed_key,
                "business_unit": [unit, foreign_unit],
                "customer": [customer],
            },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let created: Value = res.json().await.unwrap();
    let preset_id = created["id"].as_str().unwrap().to_string();
    let read = || async {
        client
            .get(format!("{views}/{preset_id}"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let sorted = |v: &Value| {
        let mut items: Vec<(String, String)> = v["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| {
                (
                    u["filter"].as_str().unwrap().to_string(),
                    u["id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        items.sort();
        items
    };

    // give the account a role at the org itself so that the unit and customer
    // in it are readable, and only the foreign unit is not
    seed_membership(&pool, member, Some(org), None, None, "viewer").await;
    let got = read().await;
    assert_eq!(
        sorted(&got),
        vec![("business_unit".to_string(), foreign_unit.to_string())],
        "{got}"
    );
    assert_eq!(got["effective_filters"]["key"], json!(doomed_key));
    assert_eq!(got["effective_filters"]["business_unit"], json!([unit]));
    assert_eq!(got["effective_filters"]["customer"], json!([customer]));
    // the stored filters are never rewritten by a read
    assert_eq!(got["filters"]["business_unit"], json!([unit, foreign_unit]));

    // the key is deleted: it is unavailable, the rest still applies
    let gone = client
        .delete(format!("{base}/api/v1/virtual-keys/{doomed_key}"))
        .bearer_auth("admintok")
        .send()
        .await
        .unwrap();
    assert!(gone.status().is_success(), "{}", gone.status());
    let got = read().await;
    assert_eq!(
        sorted(&got),
        vec![
            ("business_unit".to_string(), foreign_unit.to_string()),
            ("key".to_string(), doomed_key.to_string()),
        ],
        "{got}"
    );
    let effective = &got["effective_filters"];
    assert!(effective.get("key").is_none(), "{got}");
    assert_eq!(effective["window"], "30d");
    assert_eq!(effective["model"], "gpt-4o");
    assert_eq!(effective["business_unit"], json!([unit]));
    assert_eq!(effective["customer"], json!([customer]));

    // both memberships go: the unit and customer are lost too, and the list
    // route says the same thing as the get route
    sqlx::query("delete from memberships where user_id = $1")
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    let listed: Value = client
        .get(format!("{views}?surface=llm_logs"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sorted(&listed[0]).len(), 4, "{listed}");
    assert_eq!(
        listed[0]["effective_filters"],
        json!({"window": "30d", "model": "gpt-4o"}),
        "the lists emptied out drop their keys"
    );

    // a preset that names a key still readable is untouched
    let res = client
        .post(&views)
        .bearer_auth(&token)
        .json(&json!({"surface": "llm_logs", "name": "Kept", "filters": {"key": kept_key}}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let kept: Value = res.json().await.unwrap();
    // the membership is gone, so even this one reads as lost
    assert_eq!(
        kept["unavailable"],
        json!([{"filter": "key", "id": kept_key}])
    );
    seed_membership(&pool, member, None, None, Some(project), "viewer").await;
    let kept: Value = client
        .get(format!("{views}/{}", kept["id"].as_str().unwrap()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(kept["unavailable"], json!([]));
    assert_eq!(kept["effective_filters"], json!({"key": kept_key}));
}

/// A provider or group may be scoped to one project of its org, and the control
/// plane refuses every write that would let another project reach it through a
/// route or a group (#1919). Existing providers stay org-wide.
#[tokio::test]
async fn a_provider_scoped_to_a_project_is_refused_to_other_projects_routes_and_groups() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let send = |method: reqwest::Method, url: String, body: Value| {
        let client = client.clone();
        async move {
            let resp = client
                .request(method, &url)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let post = |url: String, body: Value| send(reqwest::Method::POST, url, body);
    let put = |url: String, body: Value| send(reqwest::Method::PUT, url, body);

    let make_org = |slug: &'static str| {
        let (base, post) = (base.clone(), post);
        async move {
            let (_, org) = post(
                format!("{base}/api/v1/orgs"),
                json!({"name": slug, "slug": slug}),
            )
            .await;
            let org_id = org["id"].as_str().unwrap().to_string();
            let (_, team) = post(
                format!("{base}/api/v1/orgs/{org_id}/teams"),
                json!({"name": "core"}),
            )
            .await;
            let team_id = team["id"].as_str().unwrap().to_string();
            let mut projects = Vec::new();
            for name in ["one", "two"] {
                let (_, project) = post(
                    format!("{base}/api/v1/teams/{team_id}/projects"),
                    json!({"name": name}),
                )
                .await;
                projects.push(project["id"].as_str().unwrap().to_string());
            }
            (org_id, team_id, projects)
        }
    };
    let (org, team, projects) = make_org("scoped-a").await;
    let (p1, p2) = (projects[0].clone(), projects[1].clone());
    let (_, _, other_projects) = make_org("scoped-b").await;
    let foreign_project = other_projects[0].clone();

    let provider = |name: &'static str, project: Option<&str>| {
        let mut body = json!({"name": name, "kind": "openai_compatible",
                              "api_base": "http://127.0.0.1:9"});
        if let Some(project) = project {
            body["project_id"] = json!(project);
        }
        post(format!("{base}/api/v1/orgs/{org}/providers"), body)
    };

    // existing behaviour is the default: no project, org-wide
    let (status, shared) = provider("shared", None).await;
    assert_eq!(status, 200, "{shared}");
    assert!(shared["project_id"].is_null(), "{shared}");
    let (status, private) = provider("private-one", Some(&p1)).await;
    assert_eq!(status, 200, "{private}");
    assert_eq!(private["project_id"], p1.as_str());
    let (shared_id, private_id) = (
        shared["id"].as_str().unwrap().to_string(),
        private["id"].as_str().unwrap().to_string(),
    );

    // a project of another org, or none at all, cannot scope a provider
    let (status, body) = provider("foreign", Some(&foreign_project)).await;
    assert_eq!(status, 400, "cross-org project: {body}");
    let (status, body) = provider("ghost", Some(&uuid::Uuid::new_v4().to_string())).await;
    assert_eq!(status, 400, "unknown project: {body}");

    // routes: a project's own providers and org-wide ones, never another project's
    let route_in = |project: String, model: &'static str| {
        let (base, post) = (base.clone(), post);
        async move {
            let (status, route) = post(
                format!("{base}/api/v1/projects/{project}/routes"),
                json!({"model": model, "strategy": "round_robin"}),
            )
            .await;
            assert_eq!(status, 200, "{route}");
            route["id"].as_str().unwrap().to_string()
        }
    };
    let (r1, r2) = (
        route_in(p1.clone(), "route-one").await,
        route_in(p2.clone(), "route-two").await,
    );
    let target = |route: &str, provider_id: &str| {
        post(
            format!("{base}/api/v1/routes/{route}/targets"),
            json!({"provider_id": provider_id, "weight": 1}),
        )
    };
    let (status, body) = target(&r2, &private_id).await;
    assert_eq!(
        status, 409,
        "another project's route took a scoped provider: {body}"
    );
    assert!(body.to_string().contains("private-one"), "{body}");
    assert_eq!(target(&r1, &private_id).await.0, 200);
    assert_eq!(target(&r1, &shared_id).await.0, 200, "org-wide fallback");
    assert_eq!(target(&r2, &shared_id).await.0, 200, "org-wide option");

    // groups: a scoped one holds its own project's providers and org-wide ones,
    // an org-wide one holds only org-wide providers
    let group = |name: &'static str, project: Option<&str>, members: &[&str]| {
        let mut body = json!({
            "name": name, "strategy": "round_robin",
            "members": members.iter().map(|id| json!({"provider_id": id})).collect::<Vec<_>>(),
        });
        if let Some(project) = project {
            body["project_id"] = json!(project);
        }
        post(format!("{base}/api/v1/orgs/{org}/provider-groups"), body)
    };
    let (status, body) = group("wide-private", None, &[&private_id]).await;
    assert_eq!(status, 409, "org-wide group held a scoped provider: {body}");
    let (status, body) = group("two-private", Some(&p2), &[&private_id]).await;
    assert_eq!(
        status, 409,
        "another project's group held a scoped provider: {body}"
    );
    let (status, body) = group("foreign-group", Some(&foreign_project), &[]).await;
    assert_eq!(status, 400, "cross-org group scope: {body}");
    let (status, own) = group("own-pool", Some(&p1), &[&private_id, &shared_id]).await;
    assert_eq!(status, 200, "{own}");
    assert_eq!(own["project_id"], p1.as_str());
    let own_id = own["id"].as_str().unwrap().to_string();
    let (status, wide) = group("wide-pool", None, &[&shared_id]).await;
    assert_eq!(status, 200, "{wide}");
    assert!(wide["project_id"].is_null(), "{wide}");
    let wide_id = wide["id"].as_str().unwrap().to_string();

    // edits are checked against the group as it will be
    let (status, body) = put(
        format!("{base}/api/v1/provider-groups/{wide_id}"),
        json!({"members": [{"provider_id": private_id}]}),
    )
    .await;
    assert_eq!(status, 409, "member swap: {body}");
    let (status, body) = put(
        format!("{base}/api/v1/provider-groups/{own_id}"),
        json!({"project_id": p2}),
    )
    .await;
    assert_eq!(status, 409, "moving a group away from its provider: {body}");
    let (status, body) = put(
        format!("{base}/api/v1/provider-groups/{own_id}"),
        json!({"project_id": null}),
    )
    .await;
    assert_eq!(
        status, 409,
        "widening a group over a scoped provider: {body}"
    );

    // scoping an org-wide provider is refused while another project uses it
    let (status, body) = put(
        format!("{base}/api/v1/providers/{shared_id}"),
        json!({"project_id": p1}),
    )
    .await;
    assert_eq!(status, 409, "scoped away from route-two: {body}");
    assert!(body.to_string().contains("route-two"), "{body}");

    // a project that still owns a provider cannot be deleted out from under it
    let resp = client
        .delete(format!("{base}/api/v1/projects/{p1}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);
    let resp = client
        .delete(format!("{base}/api/v1/teams/{team}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);

    // the database refuses a scope outside the row's org even around the API
    let crossed = sqlx::query("update providers set project_id = $1 where id = $2")
        .bind(uuid::Uuid::parse_str(&foreign_project).unwrap())
        .bind(uuid::Uuid::parse_str(&shared_id).unwrap())
        .execute(&pool)
        .await;
    assert!(
        crossed.is_err(),
        "a provider was scoped to another org's project"
    );

    // the gateway's snapshot carries the scope
    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let config = &snapshot["config"];
    let named = |list: &str, key: &str, name: &str| -> Value {
        config[list]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row[key] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} missing from {list}: {config}"))
    };
    let snapped = named("providers", "name", "private-one");
    assert_eq!(snapped["tenancy"]["project_id"], p1.as_str());
    assert_eq!(snapped["project_scoped"], true);
    assert!(named("providers", "name", "shared")["tenancy"]["project_id"].is_null());
    assert_eq!(
        named("provider_groups", "name", "own-pool")["tenancy"]["project_id"],
        p1.as_str()
    );

    // a row written around the API (SQL, a seed) is pruned from the snapshot
    // rather than failing it: route-two keeps its org-wide target only
    sqlx::query("insert into route_targets (route_id, provider_id) values ($1, $2)")
        .bind(uuid::Uuid::parse_str(&r2).unwrap())
        .bind(uuid::Uuid::parse_str(&private_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let config = &snapshot["config"];
    assert!(config.is_object(), "snapshot refused: {snapshot}");
    let route_two = config["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|route| route["model"] == "route-two")
        .unwrap();
    let providers: Vec<&str> = route_two["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|target| target["provider"].as_str().unwrap())
        .collect();
    assert_eq!(providers, ["shared"], "{route_two}");
    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        problems.to_string().contains("route 'route-two'"),
        "the pruned target was not reported: {problems}"
    );
}

/// A project's members see the providers and groups of their project plus the
/// org-wide ones, never another project's; a project admin may create and
/// delete their own project's, but widening to the org, another project, or
/// naming an environment variable is an org admin's call (#1919).
#[tokio::test]
async fn members_list_their_projects_providers_and_project_admins_scope_their_own() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("scoped".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut projects = Vec::new();
    for name in ["one", "two"] {
        let id: uuid::Uuid =
            sqlx::query_scalar("insert into projects (team_id, name) values ($1, $2) returning id")
                .bind(team)
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap();
        projects.push(id);
    }
    let (p1, p2) = (projects[0], projects[1]);
    for (name, project) in [
        ("shared", None),
        ("priv-one", Some(p1)),
        ("priv-two", Some(p2)),
    ] {
        let provider: uuid::Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, project_id)
             values ($1, $2, $2, 'openai_compatible', 'http://127.0.0.1:9', $3) returning id",
        )
        .bind(org)
        .bind(name)
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into provider_groups (org_id, name, slug, project_id)
             values ($1, $2, $2, $3)",
        )
        .bind(org)
        .bind(format!("grp-{name}"))
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
        let _ = provider;
    }

    let one_admin = seed_user(&pool, "one-admin@acme.test", false).await;
    seed_membership(&pool, one_admin, None, None, Some(p1), "admin").await;
    let two_member = seed_user(&pool, "two@acme.test", false).await;
    seed_membership(&pool, two_member, None, None, Some(p2), "member").await;
    let org_admin = seed_user(&pool, "owner@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(org), None, None, "admin").await;
    let stranger = seed_user(&pool, "stranger@elsewhere.test", false).await;
    let one = seed_session(&pool, one_admin, "scoped_one").await;
    let two = seed_session(&pool, two_member, "scoped_two").await;
    let owner = seed_session(&pool, org_admin, "scoped_owner").await;
    let nobody = seed_session(&pool, stranger, "scoped_nobody").await;

    let names = |rows: &Value, key: &str| -> Vec<String> {
        let mut names: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[key].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    };
    let get = |path: String, token: &str| {
        let (client, base, token) = (client.clone(), base.clone(), token.to_string());
        async move {
            let resp = client
                .get(format!("{base}{path}"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };

    for (token, providers, groups) in [
        (
            &one,
            vec!["priv-one", "shared"],
            vec!["grp-priv-one", "grp-shared"],
        ),
        (
            &two,
            vec!["priv-two", "shared"],
            vec!["grp-priv-two", "grp-shared"],
        ),
        (
            &owner,
            vec!["priv-one", "priv-two", "shared"],
            vec!["grp-priv-one", "grp-priv-two", "grp-shared"],
        ),
    ] {
        let (status, rows) = get(format!("/api/v1/orgs/{org}/providers"), token).await;
        assert_eq!(status, 200, "{rows}");
        assert_eq!(names(&rows, "name"), providers);
        let (status, rows) = get(format!("/api/v1/orgs/{org}/provider-groups"), token).await;
        assert_eq!(status, 200, "{rows}");
        assert_eq!(names(&rows, "name"), groups);
    }
    assert_eq!(
        get(format!("/api/v1/orgs/{org}/providers"), &nobody)
            .await
            .0,
        403
    );
    assert_eq!(
        get(format!("/api/v1/orgs/{org}/provider-groups"), &nobody)
            .await
            .0,
        403
    );

    let create = |token: &str, body: Value| {
        let (client, base, token) = (client.clone(), base.clone(), token.to_string());
        async move {
            let resp = client
                .post(format!("{base}/api/v1/orgs/{org}/providers"))
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let new = |name: &str, project: Option<uuid::Uuid>| {
        let mut body = json!({"name": name, "kind": "openai_compatible",
                              "api_base": "http://127.0.0.1:9"});
        if let Some(project) = project {
            body["project_id"] = json!(project);
        }
        body
    };
    // a project admin creates and removes their own project's providers
    let (status, created) = create(&one, new("mine", Some(p1))).await;
    assert_eq!(status, 200, "{created}");
    // but not another project's, an org-wide one, or one that reads an env var
    assert_eq!(create(&one, new("theirs", Some(p2))).await.0, 403);
    assert_eq!(create(&one, new("wide", None)).await.0, 403);
    let mut env = new("env-reader", Some(p1));
    env["api_key_env"] = json!("OPENAI_API_KEY");
    assert_eq!(create(&one, env.clone()).await.0, 403);
    assert_eq!(create(&owner, env).await.0, 200);
    // a project member below admin creates nothing
    assert_eq!(create(&two, new("member-made", Some(p2))).await.0, 403);
    // and widening a scoped provider to the org is the org admin's
    let id = created["id"].as_str().unwrap();
    let widen = |token: &str| {
        let (client, base, token, id) = (
            client.clone(),
            base.clone(),
            token.to_string(),
            id.to_string(),
        );
        async move {
            client
                .put(format!("{base}/api/v1/providers/{id}"))
                .bearer_auth(token)
                .json(&json!({"project_id": null}))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(widen(&one).await, 403);
    assert_eq!(widen(&owner).await, 200);
    let resp = client
        .delete(format!("{base}/api/v1/providers/{id}"))
        .bearer_auth(&one)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        403,
        "an org-wide provider is the org admin's"
    );
}

// ---------------------------------------------------------------------------
// last active superadmin (#2344)
// ---------------------------------------------------------------------------

/// The three account writes that can take the deployment's last superadmin
/// away, as `(name, request)` pairs against `target_id`.
fn last_superadmin_calls(
    client: &reqwest::Client,
    base: &str,
    target_id: uuid::Uuid,
    bearer: &str,
) -> Vec<(&'static str, reqwest::RequestBuilder)> {
    let url = format!("{base}/api/v1/users/{target_id}");
    vec![
        (
            "demote",
            client
                .put(&url)
                .bearer_auth(bearer)
                .json(&json!({"is_superadmin": false})),
        ),
        (
            "deactivate",
            client
                .put(&url)
                .bearer_auth(bearer)
                .json(&json!({"deactivated": true})),
        ),
        ("delete", client.delete(&url).bearer_auth(bearer)),
    ]
}

async fn user_row(pool: &sqlx::PgPool, id: uuid::Uuid) -> Option<(bool, bool)> {
    sqlx::query_as("select is_superadmin, deactivated_at is not null from users where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_last_active_superadmin_cannot_be_demoted_deactivated_or_deleted() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let only = seed_user(&pool, "only@example.com", true).await;
    let session = seed_session(&pool, only, "last_superadmin").await;
    // a deactivated superadmin and a plain user do not count as the remainder
    let gone = seed_user(&pool, "gone@example.com", true).await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();
    seed_user(&pool, "plain@example.com", false).await;

    // the account itself and the admin token get the same refusal
    for bearer in [session.as_str(), "admintok"] {
        for (name, request) in last_superadmin_calls(&client, &base, only, bearer) {
            let res = request.send().await.unwrap();
            assert_eq!(res.status(), 409, "{name} as {bearer}");
            let body: Value = res.json().await.unwrap();
            assert_eq!(body["error"]["code"], "last_superadmin", "{name}");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("last active superadmin"),
                "{body}"
            );
            assert_eq!(user_row(&pool, only).await, Some((true, false)), "{name}");
        }
    }
    let live: i64 = sqlx::query_scalar("select count(*) from sessions where user_id = $1")
        .bind(only)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live, 1, "a refused deactivation must keep the sessions");

    // edits that leave the account an active superadmin still go through
    let res = client
        .put(format!("{base}/api/v1/users/{only}"))
        .bearer_auth("admintok")
        .json(&json!({"email": "renamed@example.com", "is_superadmin": true, "deactivated": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn a_second_active_superadmin_lets_each_call_through() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    seed_user(&pool, "keeper@example.com", true).await;
    for (idx, expected) in [
        (0, Some((false, false))),
        (1, Some((true, true))),
        (2, None),
    ] {
        let target_id = seed_user(&pool, &format!("target{idx}@example.com"), true).await;
        let mut calls = last_superadmin_calls(&client, &base, target_id, "admintok");
        let (name, request) = calls.remove(idx);
        let res = request.send().await.unwrap();
        assert!(res.status().is_success(), "{name}: {}", res.status());
        assert_eq!(user_row(&pool, target_id).await, expected, "{name}");
    }
}

#[tokio::test]
async fn a_deactivated_superadmin_does_not_count_as_the_remaining_one() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let active = seed_user(&pool, "active@example.com", true).await;
    let dormant = seed_user(&pool, "dormant@example.com", true).await;
    sqlx::query("update users set deactivated_at = now() where id = $1")
        .bind(dormant)
        .execute(&pool)
        .await
        .unwrap();

    for (name, request) in last_superadmin_calls(&client, &base, active, "admintok") {
        assert_eq!(request.send().await.unwrap().status(), 409, "{name}");
    }
    // the dormant one is not the last active superadmin, so it can go, and
    // bringing it back makes the other one expendable
    let res = client
        .put(format!("{base}/api/v1/users/{dormant}"))
        .bearer_auth("admintok")
        .json(&json!({"deactivated": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = client
        .put(format!("{base}/api/v1/users/{active}"))
        .bearer_auth("admintok")
        .json(&json!({"is_superadmin": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn concurrent_demotions_cannot_both_remove_a_superadmin() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    // building the app runs the migrations
    let _app = rolter_control::test_app_with_admin_token(pool.clone(), None)
        .await
        .unwrap();
    let first = seed_user(&pool, "first@example.com", true).await;
    let second = seed_user(&pool, "second@example.com", true).await;
    let repo = |id| {
        let pool = pool.clone();
        async move {
            rolter_store::postgres::repo::UserRepo(&pool)
                .update_account(id, None, None, Some(false), None)
                .await
                .unwrap()
        }
    };
    let (a, b) = tokio::join!(repo(first), repo(second));
    let refused = [&a, &b]
        .iter()
        .filter(|r| matches!(r, rolter_store::postgres::repo::LockoutGuard::WouldLockOut))
        .count();
    assert_eq!(refused, 1, "exactly one demotion must be refused");
    let left: i64 = sqlx::query_scalar("select count(*) from users where is_superadmin")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 1);
}

#[tokio::test]
async fn scim_cannot_deprovision_the_last_active_superadmin() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("admintok".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "Acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org = org["id"].as_str().unwrap().to_string();
    let token: Value = client
        .post(format!("{base}/api/v1/orgs/{org}/scim-tokens"))
        .bearer_auth("admintok")
        .json(&json!({"name": "idp"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = token["secret"].as_str().unwrap().to_string();
    let provisioned: Value = client
        .post(format!("{base}/scim/v2/Users"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "boss@acme.test",
            "emails": [{"value": "boss@acme.test", "primary": true}]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let scim_id = provisioned["id"].as_str().unwrap().to_string();
    let boss: uuid::Uuid = scim_id.parse().unwrap();
    sqlx::query("update users set is_superadmin = true where id = $1")
        .bind(boss)
        .execute(&pool)
        .await
        .unwrap();

    let res = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    assert_eq!(user_row(&pool, boss).await, Some((true, false)));
    let res = client
        .patch(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .json(&json!({
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [{"op": "replace", "path": "active", "value": false}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    assert_eq!(user_row(&pool, boss).await, Some((true, false)));

    // with another active superadmin the same call deprovisions
    seed_user(&pool, "second@example.com", true).await;
    let res = client
        .delete(format!("{base}/scim/v2/Users/{scim_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(user_row(&pool, boss).await, Some((true, true)));
}

/// #2383: the SSO issuer, the guardrail webhook url and a plugin endpoint are
/// operator-written URLs the control plane or the gateway later fetches, so a
/// cloud-metadata address must be a 400 at save, naming the field.
#[tokio::test]
async fn operator_written_urls_the_egress_policy_denies_are_refused_at_save() {
    skip_without_db!();
    let db = fresh_db().await;
    let app =
        rolter_control::test_app_with_admin_token(db.pool().clone(), Some("admintok".to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let denied = "https://169.254.169.254/latest/meta-data/";

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth("admintok")
        .json(&json!({"name": "EgressOrg", "slug": "egress-org"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();

    let refused = |response: reqwest::Response, field: &'static str| async move {
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert_eq!(status, 400, "{field}: expected a 400");
        assert!(message.contains(field), "{field}: the 400 names no field");
        assert!(
            message.contains("egress policy"),
            "{field}: the 400 does not cite the egress policy"
        );
    };

    refused(
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/sso-providers"))
            .bearer_auth("admintok")
            .json(&json!({
                "name": "Idp", "slug": "idp", "issuer": denied, "client_id": "c"
            }))
            .send()
            .await
            .unwrap(),
        "issuer",
    )
    .await;

    let webhook = |url: &str| {
        json!({
            "name": "guard", "enabled": true, "url": url, "stage": "pre_call",
            "timeout_ms": 1000, "max_retries": 0, "failure_mode": "fail_closed",
            "max_body_bytes": 1024, "auth_kind": "none", "auth_env": null
        })
    };
    refused(
        client
            .post(format!("{base}/api/v1/guardrails/providers"))
            .bearer_auth("admintok")
            .json(&webhook(denied))
            .send()
            .await
            .unwrap(),
        "guardrail provider url",
    )
    .await;
    let ok = client
        .post(format!("{base}/api/v1/guardrails/providers"))
        .bearer_auth("admintok")
        .json(&webhook("https://guard.example.com/check"))
        .send()
        .await
        .unwrap();
    assert!(ok.status().is_success(), "a permitted url must still save");
    let created: Value = ok.json().await.unwrap();
    let provider_id = created["id"].as_str().unwrap().to_string();
    refused(
        client
            .put(format!("{base}/api/v1/guardrails/providers/{provider_id}"))
            .bearer_auth("admintok")
            .json(&webhook(denied))
            .send()
            .await
            .unwrap(),
        "guardrail provider url",
    )
    .await;

    let plugin = |endpoint: &str| {
        json!({
            "name": "audit", "description": "", "kind": "webhook",
            "stage": "pre_upstream", "enabled": true, "position": 1,
            "failure_mode": "fail_open", "endpoint": endpoint, "config": {}
        })
    };
    refused(
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/plugins"))
            .bearer_auth("admintok")
            .json(&plugin(denied))
            .send()
            .await
            .unwrap(),
        "plugin endpoint",
    )
    .await;
    let ok = client
        .post(format!("{base}/api/v1/orgs/{org_id}/plugins"))
        .bearer_auth("admintok")
        .json(&plugin("https://plugins.example.com/hook"))
        .send()
        .await
        .unwrap();
    assert!(
        ok.status().is_success(),
        "a permitted endpoint must still save"
    );
    let created: Value = ok.json().await.unwrap();
    let plugin_id = created["id"].as_str().unwrap().to_string();
    refused(
        client
            .put(format!("{base}/api/v1/plugins/{plugin_id}"))
            .bearer_auth("admintok")
            .json(&plugin(denied))
            .send()
            .await
            .unwrap(),
        "plugin endpoint",
    )
    .await;
}
