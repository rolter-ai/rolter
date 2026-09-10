//! End-to-end control-plane integration tests against a real Postgres.
//!
//! Gated on the `postgres` feature and the `ROLTER_TEST_DATABASE_URL` env var:
//! when the var is unset (local runs without a database) the tests self-skip.
//! CI provides a Postgres service and the var, so they run there.
#![cfg(feature = "postgres")]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{json, Value};

fn database_url() -> Option<String> {
    std::env::var("ROLTER_TEST_DATABASE_URL").ok()
}

/// Monotonic sequence for per-test schema names, keeping concurrently-running
/// tests off a shared schema (plain `cargo test` — e.g. the coverage job —
/// runs them as threads in one process, so a shared `public` races on DDL).
static SCHEMA_SEQ: AtomicU32 = AtomicU32::new(0);

/// Isolated schema name unique to this process and call, safe to interpolate
/// (only ascii digits and underscores).
fn unique_schema() -> String {
    let n = SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("test_{}_{}", std::process::id(), n)
}

/// Return `url` with the connection pinned to `schema` via `search_path`, so
/// migrations and queries land in the isolated schema rather than `public`.
fn with_search_path(url: &str, schema: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    // percent-encode the space and `=` inside the libpq options string
    format!("{url}{sep}options=-c%20search_path%3D{schema}")
}

/// Create a fresh isolated schema and return a pool pinned to it. Tests that
/// only need a router use [`fresh_app`]; those that also query the store
/// directly build the router from this pool themselves.
async fn fresh_pool() -> sqlx::PgPool {
    let url = database_url().expect("ROLTER_TEST_DATABASE_URL checked by caller");
    let schema = unique_schema();

    // (re)create the isolated schema over a default-search_path connection
    let admin = rolter_store::postgres::connect(&url)
        .await
        .expect("connect");
    sqlx::query(&format!("drop schema if exists {schema} cascade"))
        .execute(&admin)
        .await
        .expect("reset schema");
    sqlx::query(&format!("create schema {schema}"))
        .execute(&admin)
        .await
        .expect("recreate schema");
    admin.close().await;

    // app pool scoped to the isolated schema so migrations run there
    rolter_store::postgres::connect(&with_search_path(&url, &schema))
        .await
        .expect("connect scoped")
}

/// Isolated schema + control-plane app router (migrations applied by `test_app`).
async fn fresh_app() -> axum::Router {
    let pool = fresh_pool().await;
    rolter_control::test_app(pool).await.expect("build app")
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
        if database_url().is_none() {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
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
    let pool = fresh_pool().await;
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
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;

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
    let addr = serve(fresh_app().await).await;
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

#[tokio::test]
async fn org_slug_is_validated() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;
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
    let pool = fresh_pool().await;
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

#[tokio::test]
async fn virtual_key_cost_attribution_round_trip() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
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

#[tokio::test]
async fn prompt_template_crud_publish_and_scope_round_trip() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
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
        json!({"variables": [], "decorators": []}),
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
    let addr = serve(fresh_app().await).await;
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

    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

/// `GET /api/v1/config/export` hands back the deployment as an importable
/// `rolter.toml` (#1082). The route is superadmin-only, the body is a TOML
/// document rather than JSON, and — the part worth an end-to-end test — the
/// sealed credential the same store decrypts into `/internal/snapshot` does not
/// appear in it.
#[tokio::test]
async fn config_export_serves_importable_toml_without_credentials() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);

    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

/// `GET /api/v1/stability` is the dashboard's one source for the nav's
/// experimental markers (#1385): the least-privileged signed-in caller reads it
/// — a viewer sees the nav too — an anonymous one does not, and every row it
/// returns is an exception, because absence is what "stable" means.
#[tokio::test]
async fn stability_endpoint_lists_only_experimental_subsystems() {
    skip_without_db!();
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

    // turning one on would persist a policy the gateway silently ignores
    let rejected = client
        .put(format!("{base}/api/v1/feature-flags"))
        .bearer_auth("sekrit")
        .json(&json!({
            "response_cache": true,
            "cache_aware_routing": false,
            "circuit_breaker": true,
            "active_health_checks": true,
            "complexity_routing": true,
            "guardrails": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);

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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

/// #1233: a provider is editable in place. Before this, rotating a client
/// secret or taking a provider out of service meant deleting it and
/// registering it again, which dropped every group mapping hanging off it and
/// changed the provider id in the audit trail.
#[tokio::test]
async fn sso_provider_updates_in_place_and_keeps_its_slug_and_mappings() {
    skip_without_db!();
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
            "stability:read"
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
    let pool = fresh_pool().await;
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
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;
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

/// Invitations and single sign-on co-exist (#240): an operator-granted role is
/// never reconciled away by a later SSO login, an IdP group that disappears
/// does revoke the role it granted, and an org can require SSO without locking
/// out the break-glass superadmin.
#[tokio::test]
async fn sso_and_password_login_coexist_per_org_policy() {
    skip_without_db!();
    let pool = fresh_pool().await;
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

/// Invitation onboarding (#712): an admin mints a one-time link, the invitee
/// sets their own password, and every way the link can be misused fails.
#[tokio::test]
async fn invitations_onboard_accounts_once_and_expire_closed() {
    skip_without_db!();
    let pool = fresh_pool().await;
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

#[tokio::test]
async fn adaptive_routing_telemetry_round_trips_from_the_data_plane() {
    skip_without_db!();
    let pool = fresh_pool().await;
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

    // an unidentified node is accepted and ignored, exactly like an anonymous
    // snapshot poll: it never lands in the scoreboard
    let anonymous = client
        .post(format!("{base}/internal/adaptive-telemetry"))
        .bearer_auth("sekrit")
        .json(&report)
        .send()
        .await
        .unwrap();
    assert!(anonymous.status().is_success());
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

/// Cross-tenant isolation on the exchange path: a member of another org may not
/// refresh or exchange a session they do not own, and the answer is a 404 —
/// whether a session exists elsewhere is not something to probe for.
#[tokio::test]
async fn mcp_oauth_sessions_are_not_reachable_across_owners() {
    skip_without_db!();
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

    let pool = fresh_pool().await;
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

/// #1162: the Security screen wrote to a table nothing downstream read. This
/// is the propagation half of the fix — the enforcement half lives in
/// `rolter-gateway`'s integration suite. It asserts the settings arrive in the
/// snapshot *and* that the dashboard credential material does not.
#[tokio::test]
async fn security_policy_reaches_the_snapshot_without_the_dashboard_secret() {
    skip_without_db!();
    // sealing the dashboard secret needs a KEK, exactly as the provider-key
    // test does; the value is arbitrary because nothing here decrypts it
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let pool = fresh_pool().await;
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
    // the write-only column is reported as configured, never returned
    assert_eq!(saved["dashboard_secret_configured"], true);
    assert!(saved.get("managed_dashboard_secret").is_none());
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
    let addr = serve(fresh_app().await).await;
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
    let pool = fresh_pool().await;
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

    // the right code does, and returns the recovery batch
    let confirmed: Value = client
        .post(format!("{base}/api/v1/me/mfa/confirm"))
        .bearer_auth(&token)
        .json(&json!({"code": current_code(&secret)}))
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
        .json(&json!({"mfa_token": mfa_token, "code": current_code(&secret)}))
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
    let pool = fresh_pool().await;
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
    let pool = fresh_pool().await;
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

    let cleared = rolter_control::mfa::break_glass_reset(&pool, user_id, "lost phone, ticket 42")
        .await
        .unwrap();
    assert!(cleared, "there was a factor to clear");

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

/// An org policy of `required_all` refuses a session to an account with no
/// armed factor, rather than letting it in unprotected.
#[tokio::test]
async fn a_required_policy_refuses_an_unenrolled_account() {
    skip_without_db!();
    let pool = fresh_pool().await;
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let password = random_password();
    let user_id = seed_local_user(&pool, "unenrolled@example.com", &password).await;

    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("insert into memberships (user_id, org_id, role) values ($1, $2, 'admin')")
        .bind(user_id)
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into org_auth_policies (org_id, mfa_policy) values ($1, 'required_all')")
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();

    let refused = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"email": "unenrolled@example.com", "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "mfa_enrolment_required", "{body}");
}

/// A custom label's full life on a provider, and the conflict an operator gets
/// for re-using a key on the same subject rather than a silent overwrite
/// (#985).
#[tokio::test]
async fn custom_labels_round_trip_and_refuse_a_duplicate_key() {
    skip_without_db!();
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;
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
    let addr = serve(fresh_app().await).await;
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

    let pool = fresh_pool().await;
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
    let addr = serve(fresh_app().await).await;
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
