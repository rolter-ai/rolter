//! Weighted route variants stored in the database (#391): the CRUD on the
//! route, its validation, and the edit reaching `/internal/snapshot`.

use crate::support::*;
use serde_json::{json, Value};

/// POST json, assert 2xx, return the parsed body
async fn post(client: &reqwest::Client, url: String, body: Value) -> Value {
    let resp = client.post(&url).json(&body).send().await.unwrap();
    let status = resp.status();
    let json: Value = resp.json().await.unwrap();
    assert!(status.is_success(), "POST {url} failed ({status}): {json}");
    json
}

/// PUT json, return the status and the parsed body
async fn put(client: &reqwest::Client, url: String, body: Value) -> (u16, Value) {
    let resp = client.put(&url).json(&body).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

async fn snapshot_route(client: &reqwest::Client, base: &str, model: &str) -> Value {
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    snap["config"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .find(|r| r["model"] == model)
        .cloned()
        .unwrap_or_else(|| panic!("route '{model}' missing from the snapshot: {snap}"))
}

/// The org, project and two providers every test here starts from: returns
/// `(org_id, project_id, [provider ids])`.
async fn tenant(client: &reqwest::Client, base: &str, slug: &str) -> (String, String, [String; 2]) {
    let org = post(
        client,
        format!("{base}/api/v1/orgs"),
        json!({"name": slug, "slug": slug}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = post(
        client,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        json!({"name": "Platform"}),
    )
    .await;
    let team_id = team["id"].as_str().unwrap();
    let project = post(
        client,
        format!("{base}/api/v1/teams/{team_id}/projects"),
        json!({"name": "Gateway"}),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let mut providers = Vec::new();
    for name in ["primary", "secondary"] {
        let provider = post(
            client,
            format!("{base}/api/v1/orgs/{org_id}/providers"),
            json!({
                "name": format!("{slug}-{name}"),
                "kind": "openai",
                "api_base": "https://api.openai.com"
            }),
        )
        .await;
        providers.push(provider["id"].as_str().unwrap().to_string());
    }
    let providers: [String; 2] = providers.try_into().unwrap();
    (org_id, project_id, providers)
}

/// A variant edit is a config change like any other: it bumps the version in
/// the write transaction and the next snapshot carries the new split, while a
/// save that changes nothing bumps nothing. Clearing the variants puts the
/// route back on its own targets.
#[tokio::test]
async fn a_variant_edit_reaches_the_snapshot() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_, project_id, [primary, secondary]) = tenant(&client, &base, "acme").await;

    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "split", "strategy": "round_robin"}),
    )
    .await;
    let route_id = route["id"].as_str().unwrap();
    post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": primary}),
    )
    .await;

    let split = |canary_weight: i64| {
        json!({"variants": [
            {"name": "stable", "weight": 9, "targets": [
                {"provider_id": primary, "upstream_model": "gpt-4o"}
            ]},
            {"name": "canary", "weight": canary_weight, "params": {"temperature": 0.7},
             "targets": [
                {"provider_id": secondary, "weight": 2},
                {"provider_id": primary, "upstream_model": ""}
            ]}
        ]})
    };

    let before = config_version(db.pool()).await;
    let (status, stored) = put(
        &client,
        format!("{base}/api/v1/routes/{route_id}/variants"),
        split(1),
    )
    .await;
    assert_eq!(status, 200, "{stored}");
    assert_eq!(stored[1]["name"], "canary");
    assert_eq!(stored[1]["position"], 1);
    assert_eq!(stored[1]["targets"][0]["position"], 0);
    assert_eq!(
        stored[1]["targets"][1]["upstream_model"],
        Value::Null,
        "a blank upstream model is stored as none"
    );
    let after_create = config_version(db.pool()).await;
    assert!(after_create > before, "the write did not bump the version");

    let snapped = snapshot_route(&client, &base, "split").await;
    let variants = snapped["variants"].as_array().expect("variants");
    assert_eq!(variants.len(), 2, "{snapped}");
    assert_eq!(variants[0]["name"], "stable");
    assert_eq!(variants[0]["weight"], 9);
    assert_eq!(variants[0]["targets"][0]["provider"], "acme-primary");
    assert_eq!(variants[0]["targets"][0]["model"], "gpt-4o");
    assert_eq!(variants[1]["params"]["temperature"], 0.7);
    assert_eq!(variants[1]["targets"][0]["provider"], "acme-secondary");
    assert_eq!(variants[1]["targets"][0]["weight"], 2);
    assert_eq!(variants[1]["targets"][1]["provider"], "acme-primary");

    // the edit: the canary's share goes up, and the snapshot follows
    let (status, _) = put(
        &client,
        format!("{base}/api/v1/routes/{route_id}/variants"),
        split(3),
    )
    .await;
    assert_eq!(status, 200);
    let after_edit = config_version(db.pool()).await;
    assert!(
        after_edit > after_create,
        "the edit did not bump the version"
    );
    let snapped = snapshot_route(&client, &base, "split").await;
    assert_eq!(snapped["variants"][1]["weight"], 3, "{snapped}");

    // the same set again writes nothing
    let (status, _) = put(
        &client,
        format!("{base}/api/v1/routes/{route_id}/variants"),
        split(3),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(config_version(db.pool()).await, after_edit);

    // the list embeds them on request, and only then
    let listed: Value = client
        .get(format!(
            "{base}/api/v1/projects/{project_id}/routes?include=variants"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["variants"].as_array().map(Vec::len), Some(2));
    let plain: Value = client
        .get(format!("{base}/api/v1/projects/{project_id}/routes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(plain[0].get("variants").is_none(), "{plain}");
    let read: Value = client
        .get(format!("{base}/api/v1/routes/{route_id}/variants"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read, listed[0]["variants"]);

    // a provider a variant still sends traffic to cannot be deleted
    let resp = client
        .delete(format!("{base}/api/v1/providers/{secondary}"))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("route 'split'"),
        "{body}"
    );

    // cleared, the route is back on its own target
    let (status, cleared) = put(
        &client,
        format!("{base}/api/v1/routes/{route_id}/variants"),
        json!({"variants": []}),
    )
    .await;
    assert_eq!((status, cleared), (200, json!([])));
    let snapped = snapshot_route(&client, &base, "split").await;
    assert_eq!(snapped["variants"], json!([]), "{snapped}");
    assert_eq!(snapped["targets"][0]["provider"], "acme-primary");
}

/// The write refuses what the gateway could not route: a negative weight, a
/// set whose weights are all zero, a variant with no target, and a target on
/// a provider that is not the route's org's. Each refusal names its field, and
/// none of them writes anything.
#[tokio::test]
async fn a_variant_set_the_gateway_cannot_route_is_refused() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_, project_id, [primary, _]) = tenant(&client, &base, "acme").await;
    let (_, _, [foreign, _]) = tenant(&client, &base, "globex").await;
    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "split"}),
    )
    .await;
    let url = format!(
        "{base}/api/v1/routes/{}/variants",
        route["id"].as_str().unwrap()
    );
    let before = config_version(db.pool()).await;

    let one = |name: &str, weight: i64| json!({"name": name, "weight": weight, "targets": [{"provider_id": primary}]});
    for (body, field) in [
        (json!({"variants": [one("a", -1)]}), "variants[0].weight"),
        (json!({"variants": [one("a", 0), one("b", 0)]}), "variants"),
        (
            json!({"variants": [{"name": "a", "targets": []}]}),
            "variants[0].targets",
        ),
        (
            json!({"variants": [{"name": "a", "targets": [
                {"provider_id": primary, "weight": -2}
            ]}]}),
            "variants[0].targets[0].weight",
        ),
        (
            json!({"variants": [one("a", 1), one("a", 1)]}),
            "variants[1].name",
        ),
    ] {
        let (status, refused) = put(&client, url.clone(), body.clone()).await;
        assert_eq!(status, 400, "{body}: {refused}");
        assert_eq!(refused["error"]["code"], "invalid_field", "{refused}");
        assert_eq!(refused["error"]["field"], field, "{refused}");
    }

    // another org's provider reads as one that does not exist
    let (status, refused) = put(
        &client,
        url.clone(),
        json!({"variants": [{"name": "a", "targets": [{"provider_id": foreign}]}]}),
    )
    .await;
    assert_eq!(status, 404, "{refused}");

    assert_eq!(
        config_version(db.pool()).await,
        before,
        "a refused write bumped the version"
    );
    let read: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(read, json!([]));
}
