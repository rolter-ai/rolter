//! Budgets, rate limits and the model pricing catalog.

use crate::support::*;
use serde_json::{json, Value};

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

/// #2876: a price row carries the rate for tokens written to the prompt cache.
/// It reaches the snapshot, a body that does not name it (an older client, the
/// dashboard until its half lands) leaves it alone, and `null` clears it.
#[tokio::test]
async fn a_model_price_carries_a_cache_write_rate_an_older_client_cannot_reset() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // a row with no rate is priced at the input rate, as every row was
    let (status, bare) = put_model_price(
        &client,
        &base,
        json!({"model": "claude", "input_per_mtok": "3", "output_per_mtok": "15"}),
    )
    .await;
    assert!(status.is_success(), "{bare}");
    assert!(bare["cache_write_per_mtok"].is_null(), "{bare}");
    let price = snapshot_price(&client, &base, "claude").await;
    assert!(price["cache_write_per_mtok"].is_null(), "{price}");

    let (status, set) = put_model_price(
        &client,
        &base,
        json!({
            "model": "claude", "input_per_mtok": "3", "output_per_mtok": "15",
            "cached_input_per_mtok": "0.3", "cache_write_per_mtok": "3.75",
        }),
    )
    .await;
    assert!(status.is_success(), "{set}");
    assert_eq!(set["cache_write_per_mtok"], "3.750000");
    let price = snapshot_price(&client, &base, "claude").await;
    assert_eq!(price["cache_write_per_mtok"], 3.75, "{price}");

    // an edit from a client that has never heard of the field
    let (status, kept) = put_model_price(
        &client,
        &base,
        json!({"model": "claude", "input_per_mtok": "4", "output_per_mtok": "20"}),
    )
    .await;
    assert!(status.is_success(), "{kept}");
    assert_eq!(kept["input_per_mtok"], "4.000000", "the rest is replaced");
    assert!(kept["cached_input_per_mtok"].is_null(), "{kept}");
    assert_eq!(kept["cache_write_per_mtok"], "3.750000", "{kept}");
    let listed: Value = client
        .get(format!("{base}/api/v1/model-prices"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["cache_write_per_mtok"], "3.750000", "{listed}");

    // a rate that is not a price is refused and changes nothing
    for bad in ["-1", "NaN", "inf", "abc", ""] {
        let (status, body) = put_model_price(
            &client,
            &base,
            json!({
                "model": "claude", "input_per_mtok": "4", "output_per_mtok": "20",
                "cache_write_per_mtok": bad,
            }),
        )
        .await;
        assert_eq!(status, 400, "{bad:?} should be refused: {body}");
        assert_eq!(body["error"]["field"], "cache_write_per_mtok", "{body}");
    }
    let price = snapshot_price(&client, &base, "claude").await;
    assert_eq!(price["cache_write_per_mtok"], 3.75, "{price}");

    // clearing is its own, explicit request
    let (status, cleared) = put_model_price(
        &client,
        &base,
        json!({
            "model": "claude", "input_per_mtok": "4", "output_per_mtok": "20",
            "cache_write_per_mtok": null,
        }),
    )
    .await;
    assert!(status.is_success(), "{cleared}");
    assert!(cleared["cache_write_per_mtok"].is_null(), "{cleared}");
    let price = snapshot_price(&client, &base, "claude").await;
    assert!(price["cache_write_per_mtok"].is_null(), "{price}");
}

/// #2889: every rate on a price is a finite number of at least zero. A
/// negative one made `cost` negative, so a request credited its budget, and
/// `NaN` parses as a float, is a valid `numeric` and could not be read back, so
/// the snapshot priced the model at zero.
#[tokio::test]
async fn a_model_price_rate_must_be_a_finite_non_negative_number() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let (status, good) = put_model_price(
        &client,
        &base,
        json!({
            "model": "claude", "input_per_mtok": "3", "output_per_mtok": "15",
            "cached_input_per_mtok": "0.3", "cache_write_per_mtok": "3.75",
            "cache_write_1h_per_mtok": "6",
        }),
    )
    .await;
    assert!(status.is_success(), "{good}");

    for field in [
        "input_per_mtok",
        "output_per_mtok",
        "cached_input_per_mtok",
        "cache_write_per_mtok",
        "cache_write_1h_per_mtok",
    ] {
        for bad in [
            "-1",
            "-0.000001",
            "NaN",
            "nan",
            "inf",
            "-inf",
            "Infinity",
            "1e999",
            // `numeric(12, 6)` cannot hold a million, and the refusal used to
            // be the database's, which reached the client as a 500 (#2898)
            "999999.9999995",
            "1000000",
            "1e20",
            "abc",
            "",
        ] {
            let mut body = json!({
                "model": "claude", "input_per_mtok": "4", "output_per_mtok": "20",
                "cached_input_per_mtok": "0.4", "cache_write_per_mtok": "5",
                "cache_write_1h_per_mtok": "7",
            });
            body[field] = json!(bad);
            let (status, error) = put_model_price(&client, &base, body).await;
            assert_eq!(status, 400, "{field} = {bad:?} should be refused: {error}");
            assert_eq!(error["error"]["field"], field, "{error}");
        }
    }

    // none of the refusals touched the row
    let price = snapshot_price(&client, &base, "claude").await;
    assert_eq!(price["input_per_mtok"], 3.0, "{price}");
    assert_eq!(price["output_per_mtok"], 15.0, "{price}");
    assert_eq!(price["cached_input_per_mtok"], 0.3, "{price}");
    assert_eq!(price["cache_write_per_mtok"], 3.75, "{price}");
    assert_eq!(price["cache_write_1h_per_mtok"], 6.0, "{price}");

    // zero is a rate: a self-hosted model costs nothing to call
    let (status, free) = put_model_price(
        &client,
        &base,
        json!({
            "model": "local", "input_per_mtok": "0", "output_per_mtok": "0.000000",
            "cached_input_per_mtok": "0",
        }),
    )
    .await;
    assert!(status.is_success(), "{free}");
    assert_eq!(free["input_per_mtok"], "0.000000", "{free}");

    // and the largest the columns hold is still a rate
    let (status, top) = put_model_price(
        &client,
        &base,
        json!({
            "model": "dear", "input_per_mtok": "999999.999999", "output_per_mtok": "999999.9999994",
            "cached_input_per_mtok": "999999.999999", "cache_write_per_mtok": "999999.999999",
            "cache_write_1h_per_mtok": "999999.999999",
        }),
    )
    .await;
    assert!(status.is_success(), "{top}");
    assert_eq!(top["cache_write_1h_per_mtok"], "999999.999999", "{top}");
    assert_eq!(top["output_per_mtok"], "999999.999999", "{top}");
}

/// #2891: the 1 hour cache-write rate follows the rule the 5 minute one does,
/// and the two are independent. A body that does not name it leaves it alone, a
/// bad value is refused with the field named, and `null` clears it.
#[tokio::test]
async fn a_model_price_carries_a_one_hour_cache_write_rate_an_older_client_cannot_reset() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let (status, bare) = put_model_price(
        &client,
        &base,
        json!({"model": "claude", "input_per_mtok": "3", "output_per_mtok": "15"}),
    )
    .await;
    assert!(status.is_success(), "{bare}");
    assert!(bare["cache_write_1h_per_mtok"].is_null(), "{bare}");
    let price = snapshot_price(&client, &base, "claude").await;
    assert!(price["cache_write_1h_per_mtok"].is_null(), "{price}");

    let (status, set) = put_model_price(
        &client,
        &base,
        json!({
            "model": "claude", "input_per_mtok": "3", "output_per_mtok": "15",
            "cache_write_per_mtok": "3.75", "cache_write_1h_per_mtok": "6",
        }),
    )
    .await;
    assert!(status.is_success(), "{set}");
    assert_eq!(set["cache_write_1h_per_mtok"], "6.000000", "{set}");
    let price = snapshot_price(&client, &base, "claude").await;
    assert_eq!(price["cache_write_1h_per_mtok"], 6.0, "{price}");
    assert_eq!(price["cache_write_per_mtok"], 3.75, "{price}");

    // a client that has never heard of either field replaces the rest of the
    // row and resets neither rate
    let (status, kept) = put_model_price(
        &client,
        &base,
        json!({"model": "claude", "input_per_mtok": "4", "output_per_mtok": "20"}),
    )
    .await;
    assert!(status.is_success(), "{kept}");
    assert_eq!(kept["input_per_mtok"], "4.000000", "the rest is replaced");
    assert_eq!(kept["cache_write_per_mtok"], "3.750000", "{kept}");
    assert_eq!(kept["cache_write_1h_per_mtok"], "6.000000", "{kept}");

    // one that knows the 5 minute rate only does not reset the 1 hour one
    let (status, five_minute) = put_model_price(
        &client,
        &base,
        json!({
            "model": "claude", "input_per_mtok": "4", "output_per_mtok": "20",
            "cache_write_per_mtok": "5",
        }),
    )
    .await;
    assert!(status.is_success(), "{five_minute}");
    assert_eq!(five_minute["cache_write_per_mtok"], "5.000000");
    assert_eq!(five_minute["cache_write_1h_per_mtok"], "6.000000");

    // a rate that is not a price is refused and changes nothing; the whole
    // list, the largest value `numeric(12, 6)` cannot hold included, is
    // `a_model_price_rate_must_be_a_finite_non_negative_number`'s
    for bad in ["-1", "NaN", "inf", "1000000", "abc", ""] {
        let (status, body) = put_model_price(
            &client,
            &base,
            json!({
                "model": "claude", "input_per_mtok": "4", "output_per_mtok": "20",
                "cache_write_1h_per_mtok": bad,
            }),
        )
        .await;
        assert_eq!(status, 400, "{bad:?} should be refused: {body}");
        assert_eq!(body["error"]["field"], "cache_write_1h_per_mtok", "{body}");
    }
    let price = snapshot_price(&client, &base, "claude").await;
    assert_eq!(price["cache_write_1h_per_mtok"], 6.0, "{price}");

    // clearing is its own, explicit request, and only for the rate named
    let (status, cleared) = put_model_price(
        &client,
        &base,
        json!({
            "model": "claude", "input_per_mtok": "4", "output_per_mtok": "20",
            "cache_write_1h_per_mtok": null,
        }),
    )
    .await;
    assert!(status.is_success(), "{cleared}");
    assert!(cleared["cache_write_1h_per_mtok"].is_null(), "{cleared}");
    assert_eq!(cleared["cache_write_per_mtok"], "5.000000", "{cleared}");
    let price = snapshot_price(&client, &base, "claude").await;
    assert!(price["cache_write_1h_per_mtok"].is_null(), "{price}");
}

/// #2937: the three unit rates follow the rule the cache-write rates do. A body
/// that does not name them leaves them alone (the dashboard until its half
/// landed, and every older client, sends exactly that), a bad value is refused
/// with the field named, `null` clears one, and they reach the snapshot.
#[tokio::test]
async fn a_model_price_carries_unit_rates_an_older_client_cannot_reset() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let (status, set) = put_model_price(
        &client,
        &base,
        json!({
            "model": "media", "input_per_mtok": "0", "output_per_mtok": "0",
            "image_per_unit": "0.04", "audio_input_per_minute": "0.006",
            "speech_per_mchar": "15",
        }),
    )
    .await;
    assert!(status.is_success(), "{set}");
    assert_eq!(set["image_per_unit"], "0.040000", "{set}");
    let price = snapshot_price(&client, &base, "media").await;
    assert_eq!(price["image_per_unit"], 0.04, "{price}");
    assert_eq!(price["audio_input_per_minute"], 0.006, "{price}");
    assert_eq!(price["speech_per_mchar"], 15.0, "{price}");

    // a client that never heard of the fields replaces the rest of the row
    let (status, kept) = put_model_price(
        &client,
        &base,
        json!({"model": "media", "input_per_mtok": "1", "output_per_mtok": "2"}),
    )
    .await;
    assert!(status.is_success(), "{kept}");
    assert_eq!(kept["input_per_mtok"], "1.000000", "the rest is replaced");
    assert_eq!(kept["image_per_unit"], "0.040000", "{kept}");
    assert_eq!(kept["audio_input_per_minute"], "0.006000", "{kept}");
    assert_eq!(kept["speech_per_mchar"], "15.000000", "{kept}");

    for field in [
        "image_per_unit",
        "audio_input_per_minute",
        "speech_per_mchar",
    ] {
        for bad in ["-1", "NaN", "inf", "1000000", "abc"] {
            let (status, body) = put_model_price(
                &client,
                &base,
                json!({
                    "model": "media", "input_per_mtok": "1", "output_per_mtok": "2",
                    field: bad,
                }),
            )
            .await;
            assert_eq!(status, 400, "{field} {bad:?} should be refused: {body}");
            assert_eq!(body["error"]["field"], field, "{body}");
        }
    }

    // clearing one is its own request and leaves the others
    let (status, cleared) = put_model_price(
        &client,
        &base,
        json!({
            "model": "media", "input_per_mtok": "1", "output_per_mtok": "2",
            "image_per_unit": null,
        }),
    )
    .await;
    assert!(status.is_success(), "{cleared}");
    assert!(cleared["image_per_unit"].is_null(), "{cleared}");
    assert_eq!(cleared["speech_per_mchar"], "15.000000", "{cleared}");
    let price = snapshot_price(&client, &base, "media").await;
    assert!(price["image_per_unit"].is_null(), "{price}");
}

/// #1882: the three audio token rates follow the same rule. A body that does
/// not name them leaves them alone, a bad value is refused with the field
/// named, `null` clears one, and they reach the snapshot.
#[tokio::test]
async fn a_model_price_carries_audio_rates_an_older_client_cannot_reset() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let (status, set) = put_model_price(
        &client,
        &base,
        json!({
            "model": "voice", "input_per_mtok": "4", "output_per_mtok": "16",
            "audio_input_per_mtok": "32", "audio_output_per_mtok": "64",
            "cached_audio_input_per_mtok": "0.4",
        }),
    )
    .await;
    assert!(status.is_success(), "{set}");
    assert_eq!(set["audio_input_per_mtok"], "32.000000", "{set}");
    let price = snapshot_price(&client, &base, "voice").await;
    assert_eq!(price["audio_input_per_mtok"], 32.0, "{price}");
    assert_eq!(price["audio_output_per_mtok"], 64.0, "{price}");
    assert_eq!(price["cached_audio_input_per_mtok"], 0.4, "{price}");

    // a client that never heard of the fields replaces the rest of the row
    let (status, kept) = put_model_price(
        &client,
        &base,
        json!({"model": "voice", "input_per_mtok": "5", "output_per_mtok": "20"}),
    )
    .await;
    assert!(status.is_success(), "{kept}");
    assert_eq!(kept["input_per_mtok"], "5.000000", "the rest is replaced");
    assert_eq!(kept["audio_input_per_mtok"], "32.000000", "{kept}");
    assert_eq!(kept["audio_output_per_mtok"], "64.000000", "{kept}");
    assert_eq!(kept["cached_audio_input_per_mtok"], "0.400000", "{kept}");

    for field in [
        "audio_input_per_mtok",
        "audio_output_per_mtok",
        "cached_audio_input_per_mtok",
    ] {
        for bad in ["-1", "NaN", "inf", "1000000", "abc"] {
            let (status, body) = put_model_price(
                &client,
                &base,
                json!({
                    "model": "voice", "input_per_mtok": "1", "output_per_mtok": "2",
                    field: bad,
                }),
            )
            .await;
            assert_eq!(status, 400, "{field} {bad:?} should be refused: {body}");
            assert_eq!(body["error"]["field"], field, "{body}");
        }
    }

    let (status, cleared) = put_model_price(
        &client,
        &base,
        json!({
            "model": "voice", "input_per_mtok": "5", "output_per_mtok": "20",
            "audio_output_per_mtok": null,
        }),
    )
    .await;
    assert!(status.is_success(), "{cleared}");
    assert!(cleared["audio_output_per_mtok"].is_null(), "{cleared}");
    assert_eq!(cleared["audio_input_per_mtok"], "32.000000", "{cleared}");
    let price = snapshot_price(&client, &base, "voice").await;
    assert!(price["audio_output_per_mtok"].is_null(), "{price}");
}

/// The spend read reports the counter the gateway keeps, and says so when it
/// cannot (#2091). A control plane without Redis must answer `not_configured`
/// with no figure at all, since zero would read as an untouched budget.
#[tokio::test]
async fn budget_spend_reads_the_gateway_counter_and_never_invents_zero() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let redis_url = std::env::var("ROLTER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty());
    let app = match &redis_url {
        Some(url) => rolter_control::test_app_with_redis(pool.clone(), url)
            .await
            .expect("build app"),
        None => rolter_control::test_app(pool.clone())
            .await
            .expect("build app"),
    };
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Spender", "slug": "spender"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id").to_string();
    let budget: Value = client
        .post(format!("{base}/api/v1/budgets"))
        .json(
            &json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "100", "period": "30d"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = format!("{base}/api/v1/budgets/spend?scope_type=org&scope_id={org_id}");

    let Some(redis_url) = redis_url else {
        let report: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        assert_eq!(report["status"], "not_configured");
        assert_eq!(report["budgets"][0]["id"], budget["id"]);
        assert!(report["budgets"][0]["spent_usd"].is_null());
        assert!(report["budgets"][0]["resets_at"].is_string());
        return;
    };

    // nothing recorded yet is a real zero
    let report: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(report["status"], "ok");
    assert_eq!(report["budgets"][0]["spent_usd"], "0.0000");

    let key = format!(
        "rolter:budget:org:{org_id}:{}",
        chrono::Utc::now().format("%Y%m")
    );
    let redis = redis::Client::open(redis_url.as_str()).expect("redis url");
    let mut conn = redis
        .get_multiplexed_async_connection()
        .await
        .expect("redis");
    let _: f64 = redis::cmd("INCRBYFLOAT")
        .arg(&key)
        .arg(12.5)
        .query_async(&mut conn)
        .await
        .expect("record spend");
    let report: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    let _: i64 = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .expect("clean up");
    assert_eq!(report["budgets"][0]["spent_usd"], "12.5000");
}

/// #3128: the batch read returns the counter the per-scope read would, for
/// each budget of the org, in one request.
#[tokio::test]
async fn the_org_wide_spend_read_returns_every_counter() {
    skip_without_db!();
    let Some(redis_url) = std::env::var("ROLTER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let db = fresh_db().await;
    let app = rolter_control::test_app_with_redis(db.pool().clone(), &redis_url)
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Batch", "slug": format!("batch-{}", uuid::Uuid::new_v4().simple())}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let team: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
        .json(&json!({"name": "T"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let team_id = team["id"].as_str().unwrap().to_string();
    let mut ids = Vec::new();
    for (scope, id) in [("org", &org_id), ("team", &team_id)] {
        let budget: Value = client
            .post(format!("{base}/api/v1/budgets"))
            .json(&json!({"scope_type": scope, "scope_id": id, "limit_usd": "100", "period": "monthly"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push((
            scope,
            id.clone(),
            budget["id"].as_str().unwrap().to_string(),
        ));
    }
    let redis = redis::Client::open(redis_url.as_str()).unwrap();
    let mut conn = redis.get_multiplexed_async_connection().await.unwrap();
    let month = chrono::Utc::now().format("%Y%m");
    let mut keys = Vec::new();
    for (n, (scope, id, _)) in ids.iter().enumerate() {
        let key = format!("rolter:budget:{scope}:{id}:{month}");
        let _: f64 = redis::cmd("INCRBYFLOAT")
            .arg(&key)
            .arg(10.0 * (n as f64 + 1.0))
            .query_async(&mut conn)
            .await
            .unwrap();
        keys.push(key);
    }
    let report: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/budgets/spend"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for key in &keys {
        let _: i64 = redis::cmd("DEL")
            .arg(key)
            .query_async(&mut conn)
            .await
            .unwrap();
    }
    assert_eq!(report["status"], "ok", "{report}");
    for (n, (_, _, budget_id)) in ids.iter().enumerate() {
        let row = report["budgets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["id"] == budget_id.as_str())
            .unwrap_or_else(|| panic!("budget {budget_id} missing: {report}"));
        assert_eq!(row["spent_usd"], format!("{:.4}", 10.0 * (n as f64 + 1.0)));
    }
}

/// #1171: a key is minted with its cap. The cap is a budget and a rate limit
/// scoped to the key, written in the mint's own transaction, so it reaches the
/// snapshot with the key; a cap the gateway would misread is refused before a
/// key exists; rotation carries the caps over; and deleting the key takes them
/// with it rather than leaving rows for a key that is gone.
#[tokio::test]
async fn a_key_is_minted_with_its_cap_and_keeps_it_through_rotation() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("mint".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
        let resp = client
            .post(&url)
            .bearer_auth("mint")
            .json(&body)
            .send()
            .await
            .unwrap();
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
    let project_id = project["id"].as_str().expect("project id").to_string();
    let mint_url = format!("{base}/api/v1/projects/{project_id}/virtual-keys");

    // a cap the gateway would misread is refused, and no key is left behind
    for bad in [
        json!({"name": "k", "budget": {"limit_usd": "-5"}}),
        json!({"name": "k", "budget": {"limit_usd": "50", "period": "7d"}}),
        json!({"name": "k", "rate_limit": {}}),
        json!({"name": "k", "rate_limit": {"rpm": 0}}),
    ] {
        let resp = client
            .post(&mint_url)
            .bearer_auth("mint")
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{bad}");
    }
    let none: i64 = sqlx::query_scalar("select count(*) from virtual_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(none, 0, "a refused mint must not leave a key");

    let minted = post(
        &client,
        mint_url.clone(),
        json!({
            "name": "contractor",
            "budget": {"limit_usd": "50", "period": "monthly"},
            "rate_limit": {"rpm": 30, "tpm": 20000},
        }),
    )
    .await;
    let key_id = minted["id"].as_str().expect("key id").to_string();

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth("mint")
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
        .find(|b| b["scope"] == "key" && b["id"] == key_id.as_str())
        .unwrap_or_else(|| panic!("the key's budget is missing from the snapshot: {snap}"));
    assert_eq!(budget["limit_usd"], 50.0);
    assert_eq!(budget["period"], "monthly");
    let limit = snap["config"]["rate_limits"]
        .as_array()
        .expect("limits")
        .iter()
        .find(|l| l["scope"] == "key" && l["id"] == key_id.as_str())
        .unwrap_or_else(|| panic!("the key's rate limit is missing from the snapshot: {snap}"));
    assert_eq!(
        (limit["rpm"].clone(), limit["tpm"].clone()),
        (json!(30), json!(20000))
    );

    // editable afterwards through the budgets API, in place
    let listed: Value = client
        .get(format!(
            "{base}/api/v1/budgets?scope_type=virtual_key&scope_id={key_id}"
        ))
        .bearer_auth("mint")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // an uncapped mint stays uncapped
    let plain = post(&client, mint_url, json!({"name": "plain"})).await;
    let plain_id = plain["id"].as_str().expect("key id");
    let capped_rows: i64 =
        sqlx::query_scalar("select count(*) from budgets where scope_id = $1::uuid")
            .bind(plain_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(capped_rows, 0);

    // deleting the key takes its caps along
    let deleted = client
        .delete(format!("{base}/api/v1/virtual-keys/{key_id}"))
        .bearer_auth("mint")
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success(), "{}", deleted.status());
    let orphans: i64 = sqlx::query_scalar(
        "select (select count(*) from budgets where scope_id = $1::uuid)
              + (select count(*) from rate_limits where scope_id = $1::uuid)",
    )
    .bind(&key_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orphans, 0, "a deleted key's caps must go with it");
}

/// #1830: a budget and a rate limit scope to a person, counted across every key
/// they minted. A person belongs to no org, so the cap is authorized through
/// the place it is narrowed to: an org admin manages caps narrowed to their
/// org, and only a superadmin may set one that counts the person everywhere.
/// The cap reaches the snapshot with its narrowing, and the person reads their
/// own caps, and nobody else's, from `/me/budgets`.
#[tokio::test]
async fn a_budget_scopes_to_a_person_and_is_managed_through_its_org() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().into()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let acme: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let other: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Other', 'other') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let actors = seed_actors(&pool, acme).await;
    let engineer = seed_user(&pool, "engineer@example.com", false).await;
    let engineer_bearer = seed_session(&pool, engineer, "eng_budget").await;
    let colleague = seed_user(&pool, "colleague@example.com", false).await;
    let colleague_bearer = seed_session(&pool, colleague, "col_budget").await;

    let post = |bearer: String, path: &'static str, body: Value| {
        let client = client.clone();
        let url = format!("{base}{path}");
        async move {
            client
                .post(url)
                .bearer_auth(bearer)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };

    // the org's admin narrows a cap on the engineer to the org
    let created = post(
        actors.manager_bearer.clone(),
        "/api/v1/budgets",
        json!({"scope_type": "user", "scope_id": engineer, "within_org_id": acme,
               "limit_usd": "50", "period": "monthly"}),
    )
    .await;
    assert_eq!(created.status(), 200, "{}", created.text().await.unwrap());
    let limit = post(
        actors.manager_bearer.clone(),
        "/api/v1/rate-limits",
        json!({"scope_type": "user", "scope_id": engineer, "within_org_id": acme, "rpm": 20}),
    )
    .await;
    assert_eq!(limit.status(), 200);

    // ... but not to another org, and not to everywhere
    for (within, label) in [
        (json!({"within_org_id": other}), "another org"),
        (json!({}), "everywhere"),
    ] {
        let mut body = json!({"scope_type": "user", "scope_id": engineer,
                              "limit_usd": "50", "period": "monthly"});
        body.as_object_mut()
            .unwrap()
            .extend(within.as_object().unwrap().clone());
        let refused = post(actors.manager_bearer.clone(), "/api/v1/budgets", body).await;
        assert_eq!(refused.status(), 403, "{label}");
    }
    // a superadmin may set the one that counts the person everywhere
    let everywhere = post(
        actors.operator_bearer.clone(),
        "/api/v1/budgets",
        json!({"scope_type": "user", "scope_id": colleague,
               "limit_usd": "10", "period": "daily"}),
    )
    .await;
    assert_eq!(everywhere.status(), 200);

    // a narrowing means something only on a user, and names one place
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(acme)
            .fetch_one(&pool)
            .await
            .unwrap();
    for body in [
        json!({"scope_type": "team", "scope_id": team, "within_org_id": acme,
               "limit_usd": "5"}),
        json!({"scope_type": "user", "scope_id": engineer, "within_org_id": acme,
               "within_project_id": uuid::Uuid::new_v4(), "limit_usd": "5"}),
    ] {
        let bad = post(actors.operator_bearer.clone(), "/api/v1/budgets", body).await;
        assert_eq!(bad.status(), 400);
    }
    // an unknown person is a 404, not a cap on nobody
    let ghost = post(
        actors.operator_bearer.clone(),
        "/api/v1/budgets",
        json!({"scope_type": "user", "scope_id": uuid::Uuid::new_v4(), "limit_usd": "5"}),
    )
    .await;
    assert_eq!(ghost.status(), 404);

    // the org-wide list carries the narrowed row for the org's admin
    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{acme}/budgets"))
        .bearer_auth(&actors.manager_bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["scope_type"] == "user" && b["within_org_id"] == acme.to_string()));
    // and not the one that counts the colleague everywhere
    assert!(!listed
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["scope_id"] == colleague.to_string()));

    // the snapshot carries both, with their narrowing
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let engineer_id = engineer.to_string();
    let budget = snap["config"]["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["scope"] == "user" && b["id"] == engineer_id.as_str())
        .unwrap_or_else(|| panic!("the user budget is missing from the snapshot: {snap}"));
    assert_eq!(budget["within"], json!({"org": acme.to_string()}));
    let rate = snap["config"]["rate_limits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["scope"] == "user" && l["id"] == engineer_id.as_str())
        .unwrap_or_else(|| panic!("the user rate limit is missing from the snapshot: {snap}"));
    assert_eq!(
        (rate["rpm"].clone(), rate["within"].clone()),
        (json!(20), json!({"org": acme.to_string()}))
    );

    // a person reads their own caps from /me/budgets, and only their own
    let mine = |bearer: &str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/me/budgets");
        let bearer = bearer.to_string();
        async move {
            let resp = client.get(url).bearer_auth(bearer).send().await.unwrap();
            assert_eq!(resp.status(), 200);
            resp.json::<Value>().await.unwrap()
        }
    };
    let own = mine(&engineer_bearer).await;
    assert_eq!(own["budgets"].as_array().unwrap().len(), 1, "{own}");
    assert_eq!(own["budgets"][0]["limit_usd"], "50.0000");
    assert_eq!(own["budgets"][0]["within_org_id"], acme.to_string());
    // no Redis behind the control plane in this test: unknown, never zero
    assert_eq!(own["status"], "not_configured");
    assert!(own["budgets"][0]["spent_usd"].is_null());
    assert_eq!(own["rate_limits"][0]["rpm"], 20);
    let theirs = mine(&colleague_bearer).await;
    assert_eq!(theirs["budgets"].as_array().unwrap().len(), 1);
    assert_eq!(theirs["budgets"][0]["limit_usd"], "10.0000");
    assert_eq!(theirs["rate_limits"].as_array().unwrap().len(), 0);

    // deleting the person takes their caps with them
    let deleted = client
        .delete(format!("{base}/api/v1/users/{engineer}"))
        .bearer_auth(&actors.operator_bearer)
        .send()
        .await
        .unwrap();
    assert!(deleted.status().is_success(), "{}", deleted.status());
    let left: i64 = sqlx::query_scalar(
        "select (select count(*) from budgets where scope_id = $1)
              + (select count(*) from rate_limits where scope_id = $1)",
    )
    .bind(engineer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, 0);
}

/// #3128: the org-wide list names a key's budget, and one batch read carries
/// the counter of every budget the caller may read, with the same `status`
/// rules as the per-scope read.
#[tokio::test]
async fn the_org_wide_reads_name_keys_and_carry_every_counter() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("org".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let post = |path: String, body: Value| {
        let client = client.clone();
        let url = format!("{base}{path}");
        async move {
            let resp = client
                .post(url)
                .bearer_auth("org")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert!(resp.status().is_success(), "{}", resp.status());
            resp.json::<Value>().await.unwrap()
        }
    };
    let org = post(
        "/api/v1/orgs".into(),
        json!({"name": "Acme", "slug": "acme"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        format!("/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let project = post(
        format!("/api/v1/teams/{}/projects", team["id"].as_str().unwrap()),
        json!({"name": "Gateway"}),
    )
    .await;
    let key = post(
        format!(
            "/api/v1/projects/{}/virtual-keys",
            project["id"].as_str().unwrap()
        ),
        json!({"name": "contractor", "budget": {"limit_usd": "50"}, "rate_limit": {"rpm": 5}}),
    )
    .await;
    post(
        "/api/v1/budgets".into(),
        json!({"scope_type": "org", "scope_id": org_id, "limit_usd": "900"}),
    )
    .await;

    let get = |path: String| {
        let client = client.clone();
        let url = format!("{base}{path}");
        async move {
            client
                .get(url)
                .bearer_auth("org")
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let listed = get(format!("/api/v1/orgs/{org_id}/budgets")).await;
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let keyed = rows
        .iter()
        .find(|b| b["scope_type"] == "virtual_key")
        .unwrap();
    assert_eq!(keyed["scope_name"], "contractor");
    assert_eq!(keyed["scope_id"], key["id"]);
    let orged = rows.iter().find(|b| b["scope_type"] == "org").unwrap();
    assert!(orged["scope_name"].is_null());
    let limits = get(format!("/api/v1/orgs/{org_id}/rate-limits")).await;
    assert_eq!(limits[0]["scope_name"], "contractor");

    // no Redis behind this control plane: every budget is listed, none is
    // reported as zero
    let spend = get(format!("/api/v1/orgs/{org_id}/budgets/spend")).await;
    assert_eq!(spend["status"], "not_configured");
    let ids: Vec<&Value> = spend["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| &b["id"])
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(spend["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|b| b["spent_usd"].is_null()));
}
