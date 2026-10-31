//! What `/internal/snapshot` serves, and what it withholds.

use crate::support::*;
use serde_json::{json, Value};

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
                "limits": {"output_tokens": 2048, "timeout_secs": 30, "retries": 0},
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
    assert_eq!(route["advanced"]["limits"]["timeout_secs"], 30);
    assert_eq!(
        route["advanced"]["limits"]["retries"], 0,
        "zero turns retries off, so it must survive as a value"
    );
    assert_eq!(route["advanced"]["headers"]["x-model-region"], "eu");
}

/// #2924: `base_url`, `insecure_tls`, `pricing` and `limits.context_window`
/// were stored and read by nothing, and are gone (`rpm`, `tpm` and
/// `concurrency` came back with a reader in #2936). A client that still sends them
/// (an older dashboard, a script) is not refused: the keys are dropped before
/// the blob is stored, so the routes API never reads one back as if it were
/// policy, and what is left of the block is kept whole. A row written before
/// they were retired still loads into the snapshot, with the keys ignored.
#[tokio::test]
async fn retired_route_advanced_keys_are_accepted_and_dropped() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
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
        json!({"model": "claude", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");
    post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": provider_id, "weight": 1}),
    )
    .await;
    let put = |advanced: Value| {
        client
            .put(format!("{base}/api/v1/routes/{route_id}/advanced"))
            .json(&json!({ "advanced": advanced }))
            .send()
    };

    // nothing in a retired key is validated: it is applied to nothing, so
    // refusing a negative price would only break an older dashboard's save
    let saved = put(json!({
        "base_url": "ftp://not-even-a-url",
        "insecure_tls": true,
        "pricing": {"cache_write_per_mtok": -1.0, "image_per_unit": -1.0},
        "limits": {"context_window": 3, "timeout_secs": 30, "retries": 0},
        "headers": {"x-model-region": "eu"}
    }))
    .await
    .unwrap();
    let status = saved.status();
    let saved: Value = saved.json().await.unwrap();
    assert!(status.is_success(), "{saved}");
    let advanced = &saved["advanced"];
    for key in ["base_url", "insecure_tls", "pricing"] {
        assert!(advanced.get(key).is_none(), "{key} was stored: {saved}");
    }
    assert!(
        advanced["limits"].get("context_window").is_none(),
        "limits.context_window was stored: {saved}"
    );
    assert_eq!(advanced["limits"]["timeout_secs"], 30, "{saved}");
    assert_eq!(advanced["limits"]["retries"], 0, "{saved}");
    assert_eq!(advanced["headers"]["x-model-region"], "eu", "{saved}");

    // what the gateway applies is still validated
    for (what, advanced) in [
        (
            "a retry budget past the ceiling",
            json!({"limits": {"retries": 11}}),
        ),
        ("a zero timeout", json!({"limits": {"timeout_secs": 0}})),
        (
            "a credential header",
            json!({"headers": {"Authorization": "Bearer other"}}),
        ),
        (
            "a header value on two lines",
            json!({"headers": {"x-model-region": "a\nb"}}),
        ),
    ] {
        let refused = put(advanced).await.unwrap();
        assert_eq!(refused.status(), 400, "{what} should be refused");
    }

    // a row written before the keys were retired still loads, whole
    sqlx::query("update routes set advanced = $1 where id = $2")
        .bind(json!({
            "base_url": "https://models.example/v1",
            "insecure_tls": true,
            "pricing": {"cache_write_per_mtok": 3.75, "image_per_unit": 0.04},
            "limits": {"context_window": 8192, "output_tokens": 2048, "timeout_secs": 45},
            "visibility": {"minimum_role": "member"}
        }))
        .bind(uuid::Uuid::parse_str(route_id).unwrap())
        .execute(db.pool())
        .await
        .unwrap();
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let route = snap["config"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .find(|r| r["model"] == "claude")
        .unwrap_or_else(|| panic!("route missing from the snapshot: {snap}"));
    assert_eq!(route["advanced"]["limits"]["output_tokens"], 2048);
    assert_eq!(route["advanced"]["limits"]["timeout_secs"], 45);
    assert_eq!(route["advanced"]["visibility"]["minimum_role"], "member");
    assert!(route["advanced"].get("base_url").is_none(), "{route}");
    assert!(route["advanced"].get("insecure_tls").is_none(), "{route}");
}

/// #2936: a route's `rpm`, `tpm` and `concurrency` are stored, validated,
/// shipped in the snapshot and read back by the routes API. A write that
/// omits them (an older dashboard) clears them with the rest of the block, as
/// every other part of `advanced` does, and a cap of zero, which would refuse
/// every request, is refused on write and pruned from a row written around the
/// API rather than 500-ing `/internal/snapshot` for every tenant.
#[tokio::test]
async fn route_throughput_caps_round_trip_and_a_zero_one_is_pruned() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
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
        json!({"model": "claude", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");
    post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": provider_id, "weight": 1}),
    )
    .await;
    let put = |advanced: Value| {
        client
            .put(format!("{base}/api/v1/routes/{route_id}/advanced"))
            .json(&json!({ "advanced": advanced }))
            .send()
    };
    let snapshot_limits = || async {
        let snap: Value = client
            .get(format!("{base}/internal/snapshot"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let route = snap["config"]["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .find(|r| r["model"] == "claude")
            .unwrap_or_else(|| panic!("the route must be kept: {snap}"))
            .clone();
        route["advanced"]["limits"].clone()
    };

    let saved = put(json!({"limits": {"rpm": 60, "tpm": 90000, "concurrency": 4}}))
        .await
        .unwrap();
    assert!(saved.status().is_success(), "{}", saved.status());
    let saved: Value = saved.json().await.unwrap();
    assert_eq!(saved["advanced"]["limits"]["rpm"], 60, "{saved}");
    assert_eq!(saved["advanced"]["limits"]["tpm"], 90000, "{saved}");
    assert_eq!(saved["advanced"]["limits"]["concurrency"], 4, "{saved}");
    let limits = snapshot_limits().await;
    assert_eq!(
        (
            limits["rpm"].clone(),
            limits["tpm"].clone(),
            limits["concurrency"].clone()
        ),
        (json!(60), json!(90000), json!(4)),
        "the gateway must be handed the caps"
    );

    for (what, advanced) in [
        ("a zero rpm", json!({"limits": {"rpm": 0}})),
        ("a zero tpm", json!({"limits": {"tpm": 0}})),
        ("a zero concurrency", json!({"limits": {"concurrency": 0}})),
        ("a negative rpm", json!({"limits": {"rpm": -1}})),
        (
            "rpm past the ceiling",
            json!({"limits": {"rpm": 10_000_001}}),
        ),
    ] {
        let refused = put(advanced).await.unwrap();
        assert!(
            refused.status().is_client_error(),
            "{what} should be refused, got {}",
            refused.status()
        );
    }
    // a refused write changes nothing
    assert_eq!(snapshot_limits().await["rpm"], 60);

    // a row written around the API with a zero cap serves without that cap
    sqlx::query("update routes set advanced = $1 where id = $2")
        .bind(json!({"limits": {"rpm": 0, "tpm": 500, "concurrency": 2}}))
        .bind(uuid::Uuid::parse_str(route_id).unwrap())
        .execute(db.pool())
        .await
        .unwrap();
    let limits = snapshot_limits().await;
    assert!(limits.get("rpm").is_none(), "{limits}");
    assert_eq!(limits["tpm"], 500);
    assert_eq!(limits["concurrency"], 2);
    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let problems = problems["problems"].to_string();
    assert!(
        problems.contains("route 'claude'") && problems.contains("limits.rpm must be between"),
        "{problems}"
    );

    // an older client that sends a block without them clears them
    let cleared = put(json!({"limits": {"timeout_secs": 30}})).await.unwrap();
    let cleared: Value = cleared.json().await.unwrap();
    assert!(
        cleared["advanced"]["limits"].get("rpm").is_none(),
        "{cleared}"
    );
}

/// A header a route cannot send (a credential written by SQL around the API's
/// check) must not 500 `/internal/snapshot` for every tenant: the snapshot
/// drops that header, keeps the route, and reports it (#2924).
#[tokio::test]
async fn a_route_header_that_cannot_be_sent_is_dropped_from_the_snapshot() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
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
        json!({"model": "claude", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");
    post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": provider_id, "weight": 1}),
    )
    .await;

    sqlx::query("update routes set advanced = $1 where id = $2")
        .bind(json!({
            "headers": {"x-model-region": "eu", "Authorization": "Bearer sk-route"},
            "locked_headers": ["x-model-region", "Authorization", "not a name"]
        }))
        .bind(uuid::Uuid::parse_str(route_id).unwrap())
        .execute(db.pool())
        .await
        .unwrap();

    let response = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let snap: Value = response.json().await.unwrap();
    let route = snap["config"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .find(|r| r["model"] == "claude")
        .unwrap_or_else(|| panic!("the route must be kept: {snap}"));
    assert_eq!(route["advanced"]["headers"]["x-model-region"], "eu");
    assert!(route["advanced"]["headers"].get("Authorization").is_none());
    assert_eq!(
        route["advanced"]["locked_headers"],
        json!(["x-model-region", "Authorization"]),
        "only a name the wire cannot carry is dropped from the locks"
    );

    // and the operator is told, not left to wonder why the header is not sent
    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let problems = problems["problems"].to_string();
    assert!(
        problems.contains("route 'claude'")
            && problems.contains("header 'Authorization' cannot be set"),
        "{problems}"
    );
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

/// #2938: a route whose stored `advanced` settings do not parse was served on
/// the type's default, which is public and unrestricted. It is now left out of
/// the snapshot, the routes beside it keep serving, and
/// `GET /api/v1/config/problems` names it. The API parses every write, so the
/// row is written with SQL, as a seed or an older type version could leave it.
#[tokio::test]
async fn a_route_with_unreadable_advanced_settings_is_not_served_open() {
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

    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team_id: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project_id: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'api') returning id")
            .bind(team_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let provider_id: uuid::Uuid = sqlx::query_scalar(
        "insert into providers (org_id, name, slug, kind, api_base, api_key_env)
         values ($1, 'edge', 'edge', 'openai', 'https://example.com', 'EDGE_KEY') returning id",
    )
    .bind(org_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    for (model, advanced) in [
        ("sound", r#"{"visibility": {"project_only": true}}"#),
        (
            "restricted",
            r#"{"visibility": {"allowed_team_ids": "t1"}}"#,
        ),
    ] {
        let route_id: uuid::Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy, advanced)
             values ($1, $2, 'round_robin', $3::jsonb) returning id",
        )
        .bind(project_id)
        .bind(model)
        .bind(advanced)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into route_targets (route_id, provider_id, weight) values ($1, $2, 1)")
            .bind(route_id)
            .bind(provider_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    let snapshot: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let models: Vec<&str> = snapshot["config"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .filter_map(|route| route["model"].as_str())
        .collect();
    assert_eq!(models, ["sound"], "{snapshot}");

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
    assert!(line.contains("'restricted'"), "{line}");
    assert!(line.contains("advanced"), "{line}");
}

/// #2945: a stored provider whose own definition is invalid is left out of the
/// snapshot, and so is every reference to it. A route that named it beside a
/// healthy provider used to keep the dangling target, and `validate_snapshot`
/// refused the whole snapshot with a 500 for every tenant. Now the route serves
/// on the healthy target, a route with nothing else is dropped, a key that
/// allowed only the omitted provider is denied rather than widened to every
/// provider, and `GET /api/v1/config/problems` names each prune. The API
/// refuses a malformed `api_base`, so the row is written with SQL, as an older
/// build or a seed could leave it.
#[tokio::test]
async fn a_route_naming_an_omitted_provider_beside_a_healthy_one_still_serves() {
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

    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team_id: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project_id: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'api') returning id")
            .bind(team_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut provider_ids = std::collections::HashMap::new();
    for (name, api_base) in [
        ("healthy", "https://api.openai.com"),
        ("broken", "not-a-url"),
    ] {
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, api_key_env)
             values ($1, $2, $2, 'openai', $3, 'EDGE_KEY') returning id",
        )
        .bind(org_id)
        .bind(name)
        .bind(api_base)
        .fetch_one(&pool)
        .await
        .unwrap();
        provider_ids.insert(name, id);
    }
    for (model, providers) in [
        ("shared", ["healthy", "broken"].as_slice()),
        ("solo", ["broken"].as_slice()),
    ] {
        let route_id: uuid::Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy) values ($1, $2, 'round_robin')
             returning id",
        )
        .bind(project_id)
        .bind(model)
        .fetch_one(&pool)
        .await
        .unwrap();
        for provider in providers {
            sqlx::query(
                "insert into route_targets (route_id, provider_id, weight) values ($1, $2, 1)",
            )
            .bind(route_id)
            .bind(provider_ids[provider])
            .execute(&pool)
            .await
            .unwrap();
        }
    }
    let mut key_ids = std::collections::HashMap::new();
    for (name, providers) in [
        ("mixed", vec!["broken", "healthy"]),
        ("broken-only", vec!["broken"]),
        ("open", vec![]),
    ] {
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name, providers)
             values ($1, $2, 'sk-t', $3, $4) returning id",
        )
        .bind(project_id)
        .bind(format!("hash-{}", uuid::Uuid::new_v4()))
        .bind(name)
        .bind(providers)
        .fetch_one(&pool)
        .await
        .unwrap();
        key_ids.insert(name, id.to_string());
    }

    let response = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "one provider's defect must not refuse the snapshot"
    );
    let snapshot: Value = response.json().await.unwrap();
    let config = &snapshot["config"];

    let names = |items: &Value, field: &str| -> Vec<String> {
        items
            .as_array()
            .unwrap_or_else(|| panic!("{field}: {snapshot}"))
            .iter()
            .map(|item| item[field].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(
        names(&config["providers"], "name"),
        ["healthy"],
        "{snapshot}"
    );
    assert_eq!(names(&config["routes"], "model"), ["shared"], "{snapshot}");
    assert_eq!(
        names(&config["routes"][0]["targets"], "provider"),
        ["healthy"],
        "the route serves on its healthy target: {snapshot}"
    );

    // the key that allowed only the omitted provider is denied, not widened to
    // every provider by an emptied allow-list
    let keys = config["db_virtual_keys"].as_array().expect("keys");
    let providers_of = |name: &str| -> Option<Vec<&str>> {
        keys.iter().find(|k| k["id"] == key_ids[name]).map(|k| {
            k["providers"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect()
        })
    };
    assert_eq!(providers_of("mixed"), Some(vec!["healthy"]), "{snapshot}");
    assert_eq!(providers_of("broken-only"), None, "{snapshot}");
    assert_eq!(providers_of("open"), Some(vec![]), "{snapshot}");

    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lines: Vec<&str> = problems["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for needle in [
        "provider 'broken' omitted from the snapshot".to_string(),
        "route 'shared' target 'broken' omitted".to_string(),
        "route 'solo' omitted".to_string(),
        format!("virtual key '{}' omitted", key_ids["broken-only"]),
        format!(
            "virtual key '{}' allow-list entries 'broken' omitted",
            key_ids["mixed"]
        ),
    ] {
        assert!(
            lines.iter().any(|line| line.contains(&needle)),
            "{needle}: {lines:?}"
        );
    }
}

/// #2943: an unreadable `required_headers` used to load as no rule, and the
/// ingress gate the operator configured stopped being enforced without a word.
/// No snapshot is published while it cannot be read, so every gateway keeps the
/// one it has, gate included; `GET /api/v1/config/problems` still answers and
/// says why; and the setting can be repaired through the API that owns it,
/// after which the next snapshot carries the rule again.
#[tokio::test]
async fn an_unreadable_required_headers_gate_withholds_the_snapshot_until_repaired() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let gate_value = format!("edge-{}", uuid::Uuid::new_v4());

    let put_gate = |headers: Value| {
        client
            .put(format!("{base}/api/v1/security-settings"))
            .bearer_auth(admin_token())
            .json(&json!({
                "allowed_origins": [],
                "allowed_headers": [],
                "required_headers": headers,
                "auth_bypass_routes": [],
            }))
            .send()
    };
    let snapshot = || async {
        client
            .get(format!("{base}/internal/snapshot"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap()
    };
    let problems = || async {
        let response = client
            .get(format!("{base}/api/v1/config/problems"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json::<Value>().await.unwrap())
    };

    // the rule is saved through the api and reaches the snapshot
    let saved = put_gate(json!({"X-Mesh-Id": gate_value})).await.unwrap();
    assert_eq!(saved.status(), 200);
    let served: Value = snapshot().await.json().await.unwrap();
    assert_eq!(
        served["config"]["security"]["required_headers"]["x-mesh-id"], gate_value,
        "{served}"
    );
    let version = served["version"].as_i64().unwrap();

    // then a value the api would have refused is written around it
    sqlx::query(
        "alter table security_settings
         drop constraint security_settings_required_headers_shape",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "update security_settings
         set required_headers = jsonb_build_object('x-mesh-id', $1::text, 'x-tenant', 7)",
    )
    .bind(&gate_value)
    .execute(&pool)
    .await
    .unwrap();

    let refused = client
        .get(format!("{base}/internal/snapshot"))
        .query(&[("version", version.to_string())])
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        500,
        "a snapshot without the gate must not be published"
    );
    let body = refused.text().await.unwrap();
    assert!(!body.contains(&gate_value), "{body}");

    // the operator is told why, and the value is not repeated
    let (status, listed) = problems().await;
    assert_eq!(status, 200, "{listed}");
    let lines = listed["problems"].as_array().expect("problems");
    assert!(
        lines.iter().any(|line| {
            let line = line.as_str().unwrap();
            line.contains("required_headers") && line.contains("'x-tenant'")
        }),
        "{listed}"
    );
    assert!(!listed.to_string().contains(&gate_value), "{listed}");

    // and the setting can still be read and repaired where it lives
    let raw: Value = client
        .get(format!("{base}/api/v1/security-settings"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(raw["required_headers"]["x-tenant"], 7, "{raw}");
    let repaired = put_gate(json!({"X-Mesh-Id": gate_value})).await.unwrap();
    assert_eq!(repaired.status(), 200);

    let served: Value = snapshot().await.json().await.unwrap();
    assert_eq!(
        served["config"]["security"]["required_headers"]["x-mesh-id"], gate_value,
        "{served}"
    );
    assert!(served["version"].as_i64().unwrap() > version, "{served}");
    let (status, listed) = problems().await;
    assert_eq!(status, 200, "{listed}");
    assert!(
        listed["problems"].as_array().unwrap().is_empty(),
        "{listed}"
    );
}

/// #2943: a provider whose `egress_proxies` holds something that is not a proxy
/// URL used to load with an empty pool and reach its upstream directly. It is
/// left out of the snapshot instead, along with every target that named it, so
/// the snapshot is still served and the providers beside it keep working.
#[tokio::test]
async fn a_provider_with_an_unreadable_proxy_pool_is_left_out_of_the_snapshot() {
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

    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team_id: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project_id: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'api') returning id")
            .bind(team_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("alter table providers drop constraint providers_egress_proxies_strings")
        .execute(&pool)
        .await
        .unwrap();
    let mut provider_ids = std::collections::HashMap::new();
    for (name, egress_proxies) in [
        ("pooled", r#"["http://proxy.example.com:3128"]"#),
        ("unreadable", r#"["http://proxy.example.com:3128", 7]"#),
    ] {
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, api_key_env,
                                    egress_proxies)
             values ($1, $2, $2, 'openai_compatible', 'https://llm.example.com/v1',
                     'UPSTREAM_KEY', $3::jsonb)
             returning id",
        )
        .bind(org_id)
        .bind(name)
        .bind(egress_proxies)
        .fetch_one(&pool)
        .await
        .unwrap();
        provider_ids.insert(name, id);
    }
    for (model, targets) in [
        ("shared", vec!["pooled", "unreadable"]),
        ("only-unreadable", vec!["unreadable"]),
    ] {
        let route_id: uuid::Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy)
             values ($1, $2, 'round_robin') returning id",
        )
        .bind(project_id)
        .bind(model)
        .fetch_one(&pool)
        .await
        .unwrap();
        for name in targets {
            sqlx::query(
                "insert into route_targets (route_id, provider_id, weight) values ($1, $2, 1)",
            )
            .bind(route_id)
            .bind(provider_ids[name])
            .execute(&pool)
            .await
            .unwrap();
        }
    }

    let response = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "one unreadable pool must not withhold the snapshot from every tenant"
    );
    let snapshot: Value = response.json().await.unwrap();
    let names = |key: &str, field: &str| -> Vec<String> {
        snapshot["config"][key]
            .as_array()
            .expect(key)
            .iter()
            .filter_map(|item| item[field].as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(names("providers", "name"), ["pooled"], "{snapshot}");
    assert_eq!(names("routes", "model"), ["shared"], "{snapshot}");
    let targets: Vec<&str> = snapshot["config"]["routes"][0]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|target| target["provider"].as_str())
        .collect();
    assert_eq!(targets, ["pooled"], "{snapshot}");

    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lines: Vec<&str> = problems["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("provider 'unreadable'") && line.contains("egress_proxies")),
        "{problems}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("route 'only-unreadable' omitted")),
        "{problems}"
    );
}

/// #2950: a bootstrap-file route may name a stored provider, and when the
/// loader leaves that provider out the file's reference used to dangle, which
/// refused the snapshot for every tenant.
#[tokio::test]
async fn a_file_route_naming_a_left_out_stored_provider_keeps_the_snapshot_served() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    rolter_store::postgres::run_migrations(&pool)
        .await
        .expect("migrate");
    let org_id: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("alter table providers drop constraint providers_egress_proxies_strings")
        .execute(&pool)
        .await
        .unwrap();
    // the healthy provider is the file's own: a deployment-wide route never
    // reaches an org's stored provider (#1857), so only the left-out one is
    // stored here
    sqlx::query(
        "insert into providers (org_id, name, slug, kind, api_base, api_key_env,
                                egress_proxies)
         values ($1, 'unreadable', 'unreadable', 'openai_compatible',
                 'https://llm.example.com/v1', 'UPSTREAM_KEY', $2::jsonb)",
    )
    .bind(org_id)
    .bind(r#"["http://proxy.example.com:3128", 7]"#)
    .execute(&pool)
    .await
    .unwrap();
    let file_config = rolter_core::GatewayConfig::from_toml_str(
        r#"
        [[providers]]
        name = "pooled"
        kind = "openai_compatible"
        api_base = "https://llm.example.com/v1"

        [[routes]]
        model = "from-file"
        [[routes.targets]]
        provider = "pooled"
        [[routes.targets]]
        provider = "unreadable"

        [[routes]]
        model = "only-unreadable"
        [[routes.targets]]
        provider = "unreadable"
        "#,
    )
    .expect("file config");
    let app = rolter_control::test_app_with_file_config(pool.clone(), None, file_config)
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("http://{addr}/internal/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "a file route naming a left-out provider must not withhold the snapshot"
    );
    let snapshot: Value = response.json().await.unwrap();
    let routes = snapshot["config"]["routes"].as_array().expect("routes");
    assert_eq!(routes.len(), 1, "{snapshot}");
    assert_eq!(routes[0]["model"], "from-file", "{snapshot}");
    let targets: Vec<&str> = routes[0]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["provider"].as_str())
        .collect();
    assert_eq!(targets, ["pooled"], "{snapshot}");
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

/// #1162: the Security screen wrote to a table nothing downstream read. This
/// is the propagation half of the fix — the enforcement half lives in
/// `rolter-gateway`'s integration suite. It asserts the settings arrive in the
/// snapshot *and* that the retired dashboard password fields are gone (#2356).
#[tokio::test]
async fn security_policy_reaches_the_snapshot_and_drops_the_dashboard_password() {
    skip_without_db!();
    let dashboard_secret = random_password();
    // sealing the dashboard secret needs a KEK, exactly as the provider-key
    // test does; the value is arbitrary because nothing here decrypts it
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the default is "no extra rules", so an untouched deployment is unchanged
    let before: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(before["config"]["security"]["virtual_key_required"].is_null());
    assert!(before["config"]["security"]["required_headers"].is_null());
    assert!(before["config"]["security"]["auth_bypass_routes"].is_null());

    let saved: Value = client
        .put(format!("{base}/api/v1/security-settings"))
        .bearer_auth(admin_token())
        .json(&json!({
            "virtual_key_required": true,
            "allowed_origins": [],
            "allowed_headers": [],
            "required_headers": {"X-Mesh-Id": "edge-42"},
            "auth_bypass_routes": ["/v1/models"],
            "dashboard_auth_enabled": false,
            "managed_dashboard_secret": dashboard_secret,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // an old client still sends the retired virtual-key switch (#2357): the
    // save goes through, and the field neither comes back nor reaches a gateway
    assert!(saved["auth_bypass_routes"].is_array(), "{saved}");
    assert!(saved.get("virtual_key_required").is_none(), "{saved}");
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
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(read.get("dashboard_auth_enabled").is_none(), "{read}");
    assert!(read.get("dashboard_secret_configured").is_none(), "{read}");
    assert!(read.get("virtual_key_required").is_none(), "{read}");
    // and the toggle that controlled nothing is gone from the surface (#1162)
    assert!(saved.get("allow_direct_provider_keys").is_none());

    let after: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let security = &after["config"]["security"];
    assert!(security.get("virtual_key_required").is_none(), "{security}");
    // header names are lowercased on the way through, because that is how the
    // gateway looks them up
    assert_eq!(security["required_headers"]["x-mesh-id"], "edge-42");
    assert_eq!(security["auth_bypass_routes"][0], "/v1/models");

    // the sealed dashboard secret must not ride along anywhere in the payload
    let payload = serde_json::to_string(&after).unwrap();
    assert!(
        !payload.contains(&dashboard_secret),
        "the snapshot carries the secret"
    );
    assert!(!payload.contains("dashboard_credential"), "{payload}");

    // and the write bumped the version, so a polling gateway actually sees it
    assert!(
        after["version"].as_i64().unwrap() > before["version"].as_i64().unwrap(),
        "config_version did not move"
    );
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
// a control plane started with a bootstrap file and a database (#2922)
// ---------------------------------------------------------------------------

/// `rolter control --config rolter.toml --database-url ...` (which is what
/// `easy-up` runs) used to snapshot the file's Client Settings, Model Settings
/// and Security rows and never the dashboard's, so a save was stored and then
/// ignored by every gateway.
#[tokio::test]
async fn a_control_plane_with_a_bootstrap_file_snapshots_what_the_dashboard_saved() {
    skip_without_db!();
    let db = fresh_db().await;
    let mut file_config = rolter_core::GatewayConfig::default();
    file_config.client.request_id_header = "x-from-file".to_string();
    file_config.model_defaults.enabled = true;
    file_config.model_defaults.temperature = Some(0.9);
    file_config
        .security
        .required_headers
        .insert("x-mesh-id".to_string(), "from-file".to_string());
    // #2931: a runtime policy, and the log sink the store has no column for
    file_config.retry.max_retries = 7;
    file_config.logging.clickhouse_url = Some("http://ch.example:8123".to_string());
    let app = rolter_control::test_app_with_file_config(
        db.pool().clone(),
        Some(admin_token().to_string()),
        file_config,
    )
    .await
    .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let snapshot = || async {
        let body: Value = client
            .get(format!("{base}/internal/snapshot"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        body["config"].clone()
    };

    // nothing saved yet: the file still speaks, so adopting a database does not
    // quietly drop the policy it declared
    let before = snapshot().await;
    assert_eq!(before["client"]["request_id_header"], "x-from-file");
    assert_eq!(before["model_defaults"]["temperature"], 0.9);
    assert_eq!(
        before["security"]["required_headers"]["x-mesh-id"],
        "from-file"
    );
    assert_eq!(before["retry"]["max_retries"], 7);

    for (path, body) in [
        (
            "client-settings",
            json!({
                "forwarded_headers": [], "injected_headers": {"X-Team": "platform"},
                "request_id_header": "x-from-dashboard"
            }),
        ),
        (
            "model-defaults",
            json!({"enabled": true, "default_temperature": 0.2}),
        ),
        (
            "security-settings",
            json!({
                "allowed_origins": [], "allowed_headers": [],
                "required_headers": {"X-Waf": "from-dashboard"},
                "auth_bypass_routes": ["/v1/models"]
            }),
        ),
        (
            "runtime-policy",
            json!({
                "retry_max_retries": 1, "retry_base_ms": 100, "retry_max_ms": 2000,
                "timeout_connect_s": 10, "timeout_request_s": 60, "queue_enabled": true,
                "queue_capacity": 256, "queue_workers": 8, "queue_backpressure": "error",
                "queue_block_ms": 1000
            }),
        ),
    ] {
        let saved = client
            .put(format!("{base}/api/v1/{path}"))
            .bearer_auth(admin_token())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(saved.status().is_success(), "{path}: {}", saved.status());
    }

    let after = snapshot().await;
    assert_eq!(after["client"]["request_id_header"], "x-from-dashboard");
    assert_eq!(after["client"]["injected_headers"]["x-team"], "platform");
    assert_eq!(after["model_defaults"]["temperature"], 0.2);
    assert_eq!(
        after["security"]["required_headers"]["x-waf"],
        "from-dashboard"
    );
    assert!(
        after["security"]["required_headers"]["x-mesh-id"].is_null(),
        "the file's header came back beside the dashboard's: {after}"
    );
    assert_eq!(after["security"]["auth_bypass_routes"][0], "/v1/models");
    assert_eq!(after["retry"]["max_retries"], 1);
    // the sink lives only in the file and has to survive the store's half
    assert_eq!(after["logging"]["clickhouse_url"], "http://ch.example:8123");
}

/// #2889: a price already stored with a rate that cannot be charged does not
/// 500 `/internal/snapshot` for every tenant, and does not price its model at
/// zero either. It is left out with a problem line, so the model is unpriced,
/// and saving a good rate puts it back.
#[tokio::test]
async fn a_stored_price_with_an_unchargeable_rate_is_omitted_from_the_snapshot() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let pool = db.pool().clone();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // the database as an older control plane left it: the API let both through
    // and nothing stopped the row going in. 0084 adds its constraints `not
    // valid` on such a database, which is what is recreated here
    let checks = [
        "model_prices_input_per_mtok_rate",
        "model_prices_output_per_mtok_rate",
        "model_prices_cached_input_per_mtok_rate",
        "model_prices_cache_write_per_mtok_rate",
    ];
    for name in checks {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "alter table model_prices drop constraint {name}"
        )))
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "insert into model_prices (model, input_per_mtok, output_per_mtok, cached_input_per_mtok)
         values ('gpt-4o', 2.5, 10, 1.25),
                ('negative-output', 3, -10, null),
                ('nan-input', 'NaN', 10, null)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (name, column) in [
        (checks[0], "input_per_mtok"),
        (checks[1], "output_per_mtok"),
        (checks[2], "cached_input_per_mtok"),
        (checks[3], "cache_write_per_mtok"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "alter table model_prices add constraint {name}
             check ({column} >= 0 and {column} <> 'NaN'::numeric) not valid"
        )))
        .execute(&pool)
        .await
        .unwrap();
    }

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let models: Vec<&str> = snap["config"]["model_prices"]
        .as_array()
        .expect("model_prices")
        .iter()
        .filter_map(|price| price["model"].as_str())
        .collect();
    assert_eq!(models, ["gpt-4o"], "{snap}");
    let lines = snap["problems"]
        .as_array()
        .expect("problems in the snapshot");
    assert_eq!(lines.len(), 1, "{snap}");
    let line = lines[0].as_str().unwrap();
    assert!(
        line.contains("'negative-output'") && line.contains("output_per_mtok"),
        "{line}"
    );

    // the dashboard's list names both, the NaN one included
    let problems: Value = client
        .get(format!("{base}/api/v1/config/problems"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let lines: Vec<&str> = problems["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(lines.len(), 2, "{problems}");
    assert!(
        lines.iter().any(|l| l.contains("'negative-output'")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("'nan-input'")), "{lines:?}");

    // correcting a rate through the API restores the model
    let (status, fixed) = put_model_price(
        &client,
        &base,
        json!({"model": "nan-input", "input_per_mtok": "3", "output_per_mtok": "10"}),
    )
    .await;
    assert!(status.is_success(), "{fixed}");
    let price = snapshot_price(&client, &base, "nan-input").await;
    assert_eq!(price["input_per_mtok"], 3.0, "{price}");
}
