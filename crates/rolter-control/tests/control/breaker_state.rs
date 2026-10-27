//! The fleet view of circuit-breaker state (#2254): gateways push it, any
//! signed-in caller reads the part that belongs to models they can see.

use crate::support::*;
use serde_json::{json, Value};

async fn post_as(client: &reqwest::Client, url: String, body: Value) -> Value {
    let response = client
        .post(url)
        .bearer_auth(admin_token())
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let json: Value = response.json().await.unwrap();
    assert!(status.is_success(), "{status}: {json}");
    json
}

async fn get_as(client: &reqwest::Client, url: String, token: &str) -> (u16, Value) {
    let response = client.get(url).bearer_auth(token).send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn report(client: &reqwest::Client, base: &str, node: &str, targets: Value) -> u16 {
    client
        .post(format!("{base}/internal/breaker-state"))
        .bearer_auth(admin_token())
        .header("x-rolter-node-id", node)
        .json(&json!({ "targets": targets }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

fn open_target(model: &str, state: &str) -> Value {
    json!({
        "model": model, "target_index": 0, "provider": "edge",
        "upstream_model": "gpt-x", "state": state,
        "consecutive_failures": 5, "retry_after_secs": 20
    })
}

#[tokio::test]
async fn gateways_report_breakers_and_callers_read_the_fleet_within_their_scope() {
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

    // two orgs, each with a route of its own
    let mut orgs = Vec::new();
    for (slug, model) in [("brk-a", "model-a"), ("brk-b", "model-b")] {
        let org = post_as(
            &client,
            format!("{base}/api/v1/orgs"),
            json!({"name": slug, "slug": slug}),
        )
        .await;
        let org_id = org["id"].as_str().unwrap().to_string();
        let team = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/teams"),
            json!({"name": "core"}),
        )
        .await;
        let team_id = team["id"].as_str().unwrap();
        let project = post_as(
            &client,
            format!("{base}/api/v1/teams/{team_id}/projects"),
            json!({"name": "app"}),
        )
        .await;
        let project_id = project["id"].as_str().unwrap();
        let provider = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/providers"),
            json!({"name": format!("edge-{slug}"), "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
        .await;
        let route = post_as(
            &client,
            format!("{base}/api/v1/projects/{project_id}/routes"),
            json!({"model": model}),
        )
        .await;
        post_as(
            &client,
            format!(
                "{base}/api/v1/routes/{}/targets",
                route["id"].as_str().unwrap()
            ),
            json!({"provider_id": provider["id"], "upstream_model": "gpt-x"}),
        )
        .await;
        orgs.push(org_id);
    }

    // an unidentified node and an unauthenticated one are both refused
    let anonymous = client
        .post(format!("{base}/internal/breaker-state"))
        .bearer_auth(admin_token())
        .json(&json!({"targets": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 400);
    let unauthorized = client
        .post(format!("{base}/internal/breaker-state"))
        .header("x-rolter-node-id", "gw-1")
        .json(&json!({"targets": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    // the view needs a signed-in caller
    let denied = client
        .get(format!("{base}/api/v1/breaker-state"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    // gw-1 has model-a open and half-open elsewhere; gw-2 has it open too; an
    // unknown state is dropped instead of failing the report
    let mut gw1 = vec![
        open_target("model-a", "open"),
        open_target("model-b", "open"),
    ];
    gw1.push(open_target("model-a", "exploded"));
    assert_eq!(report(&client, &base, "gw-1", json!(gw1)).await, 204);
    assert_eq!(
        report(
            &client,
            &base,
            "gw-2",
            json!([open_target("model-a", "half_open")])
        )
        .await,
        204
    );

    // a superadmin sees every model, with the nodes behind each target
    let (status, view) = get_as(
        &client,
        format!("{base}/api/v1/breaker-state"),
        admin_token(),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    let targets = view["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2, "{view}");
    assert_eq!(targets[0]["model"], "model-a");
    assert_eq!(targets[0]["state"], "open");
    assert_eq!(targets[0]["nodes_open"], 1);
    assert_eq!(targets[0]["nodes_half_open"], 1);
    assert_eq!(targets[0]["provider"], "edge");
    assert_eq!(targets[0]["nodes"].as_array().unwrap().len(), 2);

    // a member of org A sees model-a only, and no node ids
    let org_a: uuid::Uuid = orgs[0].parse().unwrap();
    let member = seed_user(&pool, "brk-member@example.com", false).await;
    seed_membership(&pool, member, Some(org_a), None, None, "viewer").await;
    let token = seed_session(&pool, member, "brkmember").await;
    let (status, scoped) = get_as(&client, format!("{base}/api/v1/breaker-state"), &token).await;
    assert_eq!(status, 200, "{scoped}");
    let targets = scoped["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{scoped}");
    assert_eq!(targets[0]["model"], "model-a");
    assert_eq!(targets[0]["nodes_open"], 1);
    assert_eq!(
        targets[0]["nodes"],
        json!([]),
        "node ids are not for members"
    );

    // a node's next report replaces its last: the breaker closing drops it
    assert_eq!(report(&client, &base, "gw-1", json!([])).await, 204);
    let (_, view) = get_as(
        &client,
        format!("{base}/api/v1/breaker-state"),
        admin_token(),
    )
    .await;
    let targets = view["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{view}");
    assert_eq!(targets[0]["state"], "half_open");

    // a report older than the freshness window is not current state
    sqlx::query("update breaker_state set reported_at = now() - interval '10 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    let (_, stale) = get_as(
        &client,
        format!("{base}/api/v1/breaker-state"),
        admin_token(),
    )
    .await;
    assert_eq!(stale["targets"], json!([]));
}
