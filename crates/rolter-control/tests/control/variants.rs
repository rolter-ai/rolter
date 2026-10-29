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

/// The write refuses what the gateway could not route as written: a negative
/// weight, a variant with no target, and a target on
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

/// A variant weight of 0 is a fallback-only variant, and a set of only zeroes
/// is stored too: the gateway samples it uniformly (#3341, #3427).
#[tokio::test]
async fn a_variant_weight_of_zero_is_stored_and_reaches_the_snapshot() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_, project_id, [primary, _]) = tenant(&client, &base, "acme").await;
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
    let one = |name: &str, weight: i64| json!({"name": name, "weight": weight, "targets": [{"provider_id": primary}]});

    for weights in [[3, 0], [0, 0]] {
        let before = config_version(db.pool()).await;
        let (status, stored) = put(
            &client,
            url.clone(),
            json!({"variants": [one("a", weights[0]), one("b", weights[1])]}),
        )
        .await;
        assert_eq!(status, 200, "{weights:?}: {stored}");
        assert_eq!(stored[1]["weight"], weights[1], "{stored}");
        assert!(config_version(db.pool()).await > before);
        let snapped = snapshot_route(&client, &base, "split").await;
        assert_eq!(snapped["variants"][0]["weight"], weights[0], "{snapped}");
        assert_eq!(snapped["variants"][1]["weight"], weights[1], "{snapped}");
    }
}

/// Variants are part of the route, so they take its permissions: a viewer of
/// the route's org reads them and cannot write them, an admin of another
/// project of the same org can do neither, and an access profile that hides a
/// route hides its variants from `?include=variants` with it.
#[tokio::test]
async fn variants_take_the_route_s_permissions() {
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

    let send = |method: reqwest::Method, url: String, token: String, body: Option<Value>| {
        let client = client.clone();
        async move {
            let mut request = client.request(method, url).bearer_auth(token);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            (
                status,
                response.json::<Value>().await.unwrap_or(Value::Null),
            )
        }
    };
    let admin = || admin_token().to_string();
    let ok = |(status, body): (u16, Value)| {
        assert!((200..300).contains(&status), "{status}: {body}");
        body
    };

    let org = ok(send(
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs"),
        admin(),
        Some(json!({"name": "Acme", "slug": "acme"})),
    )
    .await);
    let org_id = org["id"].as_str().unwrap().to_string();
    let team = ok(send(
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs/{org_id}/teams"),
        admin(),
        Some(json!({"name": "Platform"})),
    )
    .await);
    let team_id = team["id"].as_str().unwrap().to_string();
    let mut projects = Vec::new();
    for name in ["gateway", "elsewhere"] {
        let project = ok(send(
            reqwest::Method::POST,
            format!("{base}/api/v1/teams/{team_id}/projects"),
            admin(),
            Some(json!({"name": name})),
        )
        .await);
        projects.push(project["id"].as_str().unwrap().to_string());
    }
    let provider = ok(send(
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs/{org_id}/providers"),
        admin(),
        Some(json!({"name": "primary", "kind": "openai", "api_base": "https://api.openai.com"})),
    )
    .await);
    let provider_id = provider["id"].as_str().unwrap().to_string();
    let split = json!({"variants": [
        {"name": "stable", "weight": 3, "targets": [{"provider_id": provider_id}]}
    ]});
    let mut route_ids = Vec::new();
    for model in ["hidden", "shown"] {
        let route = ok(send(
            reqwest::Method::POST,
            format!("{base}/api/v1/projects/{}/routes", projects[0]),
            admin(),
            Some(json!({"model": model})),
        )
        .await);
        let id = route["id"].as_str().unwrap().to_string();
        ok(send(
            reqwest::Method::PUT,
            format!("{base}/api/v1/routes/{id}/variants"),
            admin(),
            Some(split.clone()),
        )
        .await);
        route_ids.push(id);
    }
    let url = format!("{base}/api/v1/routes/{}/variants", route_ids[0]);

    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let viewer = seed_user(&pool, "variants-viewer@example.com", false).await;
    seed_membership(&pool, viewer, Some(org_uuid), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "variantsviewer").await;
    let (status, read) = send(
        reqwest::Method::GET,
        url.clone(),
        viewer_token.clone(),
        None,
    )
    .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read[0]["name"], "stable");
    let (status, body) = send(
        reqwest::Method::PUT,
        url.clone(),
        viewer_token,
        Some(json!({"variants": []})),
    )
    .await;
    assert_eq!(status, 403, "{body}");

    let elsewhere: uuid::Uuid = projects[1].parse().unwrap();
    let neighbour = seed_user(&pool, "variants-neighbour@example.com", false).await;
    seed_membership(&pool, neighbour, None, None, Some(elsewhere), "admin").await;
    let neighbour_token = seed_session(&pool, neighbour, "variantsneighbour").await;
    let (status, body) = send(
        reqwest::Method::GET,
        url.clone(),
        neighbour_token.clone(),
        None,
    )
    .await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = send(
        reqwest::Method::PUT,
        url.clone(),
        neighbour_token,
        Some(json!({"variants": []})),
    )
    .await;
    assert_eq!(status, 403, "{body}");

    // a profile that denies one route: the list leaves it out, and its
    // variants with it, while the other route's variants are still embedded
    let profiled = seed_user(&pool, "variants-profiled@example.com", false).await;
    seed_membership(&pool, profiled, Some(org_uuid), None, None, "viewer").await;
    let profiled_token = seed_session(&pool, profiled, "variantsprofiled").await;
    let profile = ok(send(
        reqwest::Method::POST,
        format!("{base}/api/v1/orgs/{org_id}/access-profiles"),
        admin(),
        Some(json!({"name": "no-hidden", "slug": "no-hidden"})),
    )
    .await);
    let profile_id = profile["id"].as_str().unwrap().to_string();
    ok(send(
        reqwest::Method::PUT,
        format!("{base}/api/v1/access-profiles/{profile_id}/policy"),
        admin(),
        Some(json!({"denied_routes": ["hidden"]})),
    )
    .await);
    ok(send(
        reqwest::Method::POST,
        format!("{base}/api/v1/access-profiles/{profile_id}/assignments"),
        admin(),
        Some(json!({"user_id": profiled})),
    )
    .await);
    let (status, listed) = send(
        reqwest::Method::GET,
        format!(
            "{base}/api/v1/projects/{}/routes?include=variants",
            projects[0]
        ),
        profiled_token,
        None,
    )
    .await;
    assert_eq!(status, 200, "{listed}");
    let models: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["model"].as_str().unwrap())
        .collect();
    assert_eq!(models, ["shown"], "{listed}");
    assert_eq!(listed[0]["variants"][0]["name"], "stable");
    assert!(
        !listed.to_string().contains(&route_ids[0]),
        "the hidden route's variants leaked: {listed}"
    );
}

/// #3373: a variant target's `takes_over` is validated like a route target's,
/// stored, returned and carried to the snapshot; the all-restricted rule counts
/// the targets the gateway dials (the variants' when there are any, the pool
/// otherwise, #3487), on the variants write and on the route's target writes.
#[tokio::test]
async fn a_variant_target_takes_over_failure_classes() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_, project_id, [primary, secondary]) = tenant(&client, &base, "acme").await;
    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "split"}),
    )
    .await;
    let route_id = route["id"].as_str().unwrap();
    let url = format!("{base}/api/v1/routes/{route_id}/variants");

    let before = config_version(db.pool()).await;
    for (body, field) in [
        (
            json!({"variants": [{"name": "a", "targets": [
                {"provider_id": primary},
                {"provider_id": secondary, "takes_over": ["bogus"]}
            ]}]}),
            "variants[0].targets[1].takes_over",
        ),
        (
            json!({"variants": [{"name": "a", "targets": [
                {"provider_id": primary},
                {"provider_id": secondary, "takes_over": ["timeout", "timeout"]}
            ]}]}),
            "variants[0].targets[1].takes_over",
        ),
        // the route has no targets of its own, so a set of restricted ones
        // could never dial anything
        (
            json!({"variants": [
                {"name": "a", "targets": [{"provider_id": secondary, "takes_over": ["throttled"]}]},
                {"name": "b", "targets": [{"provider_id": primary, "takes_over": ["refusal"]}]}
            ]}),
            "variants",
        ),
    ] {
        let (status, refused) = put(&client, url.clone(), body.clone()).await;
        assert_eq!(status, 400, "{body}: {refused}");
        assert_eq!(refused["error"]["field"], field, "{refused}");
        assert!(
            field != "variants"
                || refused["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("all_targets_restricted"),
            "{refused}"
        );
    }
    assert_eq!(config_version(db.pool()).await, before);

    let (status, stored) = put(
        &client,
        url.clone(),
        json!({"variants": [{"name": "a", "targets": [
            {"provider_id": primary},
            {"provider_id": secondary, "takes_over": ["throttled", "refusal:violence"]}
        ]}]}),
    )
    .await;
    assert_eq!(status, 200, "{stored}");
    assert_eq!(stored[0]["targets"][0]["takes_over"], json!([]));
    assert_eq!(
        stored[0]["targets"][1]["takes_over"],
        json!(["throttled", "refusal:violence"])
    );
    let snapped = snapshot_route(&client, &base, "split").await;
    assert_eq!(
        snapped["variants"][0]["targets"][1]["takes_over"],
        json!(["throttled", "refusal:violence"]),
        "{snapped}"
    );

    // the variant's ordinary target lets the route's own pool be restricted
    let (status, body) = put(
        &client,
        format!("{base}/api/v1/routes/{route_id}"),
        json!({"targets": [{"provider_id": secondary, "takes_over": ["timeout"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    // and with it, clearing the variants would strand the route
    let (status, refused) = put(&client, url.clone(), json!({"variants": []})).await;
    assert_eq!(status, 400, "{refused}");
    assert_eq!(refused["error"]["field"], "variants", "{refused}");
}

/// #3487: every routes-API write applies the gateway's dialled-targets rule. A
/// route with variants dials only the variants' targets, so an ordinary pool
/// no longer lets a set of fallback-only variants through; a route without
/// variants dials its pool. Each write path that could strand a route is
/// refused naming `all_targets_restricted`.
#[tokio::test]
async fn the_routes_api_counts_only_the_targets_the_gateway_dials() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_, project_id, [primary, secondary]) = tenant(&client, &base, "dialled").await;
    let route = post(
        &client,
        format!("{base}/api/v1/projects/{project_id}/routes"),
        json!({"model": "pooled"}),
    )
    .await;
    let route_id = route["id"].as_str().unwrap();
    let route_url = format!("{base}/api/v1/routes/{route_id}");
    let variants_url = format!("{route_url}/variants");
    let pooled = post(
        &client,
        format!("{route_url}/targets"),
        json!({"provider_id": primary}),
    )
    .await;
    let pooled_id = pooled["id"].as_str().unwrap();
    let refused_restricted = |(status, body): (u16, Value), code: u16| {
        assert_eq!(status, code, "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("all_targets_restricted"),
            "{body}"
        );
    };

    // PUT variants: fallback-only variants over an ordinary pool would leave
    // the gateway nothing to dial, since it never consults the pool then
    let before = config_version(db.pool()).await;
    let fallbacks_only = json!({"variants": [
        {"name": "a", "targets": [{"provider_id": secondary, "takes_over": ["throttled"]}]},
        {"name": "b", "targets": [{"provider_id": primary, "takes_over": ["timeout"]}]}
    ]});
    refused_restricted(
        put(&client, variants_url.clone(), fallbacks_only).await,
        400,
    );
    assert_eq!(config_version(db.pool()).await, before);

    // a set with an ordinary target is accepted, and removing that last
    // ordinary variant target is refused in turn
    let (status, body) = put(
        &client,
        variants_url.clone(),
        json!({"variants": [{"name": "a", "targets": [
            {"provider_id": secondary},
            {"provider_id": primary, "takes_over": ["throttled"]}
        ]}]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let before = config_version(db.pool()).await;
    refused_restricted(
        put(
            &client,
            variants_url.clone(),
            json!({"variants": [{"name": "a", "targets": [
                {"provider_id": primary, "takes_over": ["throttled"]}
            ]}]}),
        )
        .await,
        400,
    );
    assert_eq!(config_version(db.pool()).await, before);

    // with ordinary variant targets the pool is not dialled, so the pool's
    // writes may leave it only fallbacks
    let (status, body) = put(
        &client,
        route_url.clone(),
        json!({"targets": [{"provider_id": primary, "takes_over": ["timeout"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = put(
        &client,
        route_url.clone(),
        json!({"targets": [{"provider_id": primary}]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // a route stranded before the rule reached the API (its variants hold
    // only fallbacks) refuses every pool write that leaves it so: the pool
    // is not dialled while the variants exist
    sqlx::query(
        "update route_variant_targets set takes_over = '{throttled}' \
         where variant_id in (select id from route_variants where route_id = $1::uuid)",
    )
    .bind(route_id)
    .execute(db.pool())
    .await
    .unwrap();
    // every refusal below writes nothing
    let before = config_version(db.pool()).await;
    // POST a target
    let resp = client
        .post(format!("{route_url}/targets"))
        .json(&json!({"provider_id": secondary, "takes_over": ["refusal"]}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    refused_restricted((status, resp.json().await.unwrap()), 400);
    assert_eq!(config_version(db.pool()).await, before);
    // PUT the target list
    refused_restricted(
        put(
            &client,
            route_url.clone(),
            json!({"targets": [{"provider_id": primary}]}),
        )
        .await,
        400,
    );
    assert_eq!(config_version(db.pool()).await, before);
    // DELETE a target
    let resp = client
        .delete(format!("{base}/api/v1/route-targets/{pooled_id}"))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    refused_restricted((status, resp.json().await.unwrap()), 409);
    assert_eq!(config_version(db.pool()).await, before);

    // clearing the variants hands the route back to its ordinary pool
    let (status, body) = put(&client, variants_url.clone(), json!({"variants": []})).await;
    assert_eq!(status, 200, "{body}");
    snapshot_route(&client, &base, "pooled").await;
}
