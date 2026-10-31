//! Tenancy, provider, route, group, skill and prompt CRUD through the control-plane API.

use crate::support::*;
use serde_json::{json, Value};

/// Providers and provider groups answer `slug/model` from one namespace per
/// org at the gateway, so a slug either kind of an org holds is refused to the
/// other kind of that org too, on create and on rename, while another org may
/// reuse it (#1857). A route name is refused where it would sit on another
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

    let (status, own) = provider(&org_b, "edge-b", "edge-b").await;
    assert_eq!(status, 200, "{own}");
    let (status, own_group) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "pool-b", "slug": "pool-b", "strategy": "round_robin",
               "members": [{"provider_id": own["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{own_group}");

    // within one org, across kinds: a provider may not take its org's group
    // slug, nor a group its org's provider slug
    let (status, body) = provider(&org_b, "pool-b-provider", "pool-b").await;
    assert_eq!(status, 409, "provider on a group slug: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "edge-b group", "slug": "edge-b", "strategy": "round_robin",
               "members": [{"provider_id": own["id"]}]}),
    )
    .await;
    assert_eq!(status, 409, "group on a provider slug: {body}");

    // the same on rename
    let (status, body) = send(
        reqwest::Method::PUT,
        format!("{base}/api/v1/providers/{}", own["id"].as_str().unwrap()),
        json!({"slug": "pool-b", "allow_slug_change": true}),
    )
    .await;
    assert_eq!(status, 409, "provider renamed onto a group slug: {body}");
    let (status, body) = send(
        reqwest::Method::PUT,
        format!(
            "{base}/api/v1/provider-groups/{}",
            own_group["id"].as_str().unwrap()
        ),
        json!({"slug": "edge-b", "allow_slug_change": true}),
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

    // another org may reuse either slug, for either kind (#1857). this comes
    // last: an `edge/…` route of org a would then sit on org b's address too
    let (status, body) = provider(&org_b, "pool-provider", "pool").await;
    assert_eq!(status, 200, "another org's group slug: {body}");
    let (status, body) = post(
        format!("{base}/api/v1/orgs/{org_b}/provider-groups"),
        json!({"name": "edge", "slug": "edge", "strategy": "round_robin",
               "members": [{"provider_id": own["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "another org's provider slug: {body}");

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

/// Two creates of one route name, sent at once to two projects of one org,
/// both pass the API's check before either writes. The database's
/// `routes_org_model_unique` (#3459) lets exactly one through, and the other
/// reads the same `409` a sequential create gets, so the snapshot never holds
/// a duplicate it would refuse. Another org keeps its namesake.
#[tokio::test]
async fn concurrent_route_creates_in_one_org_end_in_one_route_and_a_409() {
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
    let id = |v: &Value| v["id"].as_str().unwrap().to_string();
    let mut projects = Vec::new();
    for (org, team_projects) in [("race-a", 2), ("race-b", 1)] {
        let (_, org) = post(
            format!("{base}/api/v1/orgs"),
            json!({"name": org, "slug": org}),
        )
        .await;
        let (_, team) = post(
            format!("{base}/api/v1/orgs/{}/teams", id(&org)),
            json!({"name": "core"}),
        )
        .await;
        for n in 0..team_projects {
            let (_, project) = post(
                format!("{base}/api/v1/teams/{}/projects", id(&team)),
                json!({"name": format!("app-{n}")}),
            )
            .await;
            projects.push(id(&project));
        }
    }
    let create = |project: String| {
        post(
            format!("{base}/api/v1/projects/{project}/routes"),
            json!({"model": "gpt-4o", "strategy": "round_robin"}),
        )
    };
    for _ in 0..5 {
        let ((s1, b1), (s2, b2)) =
            tokio::join!(create(projects[0].clone()), create(projects[1].clone()));
        let mut statuses = [s1, s2];
        statuses.sort();
        assert_eq!(statuses, [200, 409], "{b1} / {b2}");
        let refused = if s1 == 409 { &b1 } else { &b2 };
        assert_eq!(refused["error"]["code"], "name_taken", "{refused}");
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("in this organization"),
            "{refused}"
        );
        // clear the winner so the next round races again
        let winner = if s1 == 200 { &b1 } else { &b2 };
        let resp = client
            .delete(format!("{base}/api/v1/routes/{}", id(winner)))
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{}", resp.status());
    }
    let (s1, _) = create(projects[0].clone()).await;
    let (s2, other_org) = create(projects[2].clone()).await;
    assert_eq!((s1, s2), (200, 200), "another org's namesake: {other_org}");
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
    // a readonly group or provider slug meets the config-owned/namespace guard and answers 409
    for slug in ["edge-cluster", "shared-pool"] {
        let (status, body) = post(
            format!("{base}/api/v1/orgs/{org}/provider-groups"),
            json!({"name": format!("group-{slug}"), "slug": slug, "strategy": "round_robin",
                   "members": [{"provider_id": own["id"]}]}),
        )
        .await;
        assert_eq!(status, 409, "group on bootstrap slug {slug}: {body}");
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

/// An org's provider, group and route that a bootstrap file later repeats as
/// global rows are still the org's own (#3516): its admin edits and deletes
/// them, and the model list names the org's route as stored and only the
/// file's as config.
#[tokio::test]
async fn an_org_row_a_bootstrap_row_shares_a_name_with_stays_the_orgs() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let token = admin_token();
    let client = reqwest::Client::new();
    let send =
        |base: String, bearer: String, method: reqwest::Method, path: String, body: Value| {
            let client = client.clone();
            async move {
                let resp = client
                    .request(method, format!("{base}{path}"))
                    .bearer_auth(bearer)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                (status, resp.json::<Value>().await.unwrap_or(Value::Null))
            }
        };

    // the org's rows exist first, created while no file names them
    let before = serve(
        rolter_control::test_app_with_admin_token(pool.clone(), Some(token.to_string()))
            .await
            .unwrap(),
    )
    .await;
    let before = format!("http://{before}");
    let admin =
        |method, path: String, body| send(before.clone(), token.to_string(), method, path, body);
    let (_, org) = admin(
        reqwest::Method::POST,
        "/api/v1/orgs".into(),
        json!({"name": "tenant", "slug": "tenant"}),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let (_, team) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/teams"),
        json!({"name": "core"}),
    )
    .await;
    let (_, project) = admin(
        reqwest::Method::POST,
        format!("/api/v1/teams/{}/projects", team["id"].as_str().unwrap()),
        json!({"name": "app"}),
    )
    .await;
    let (status, provider) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/providers"),
        json!({"name": "openai", "slug": "openai", "kind": "openai_compatible",
               "api_base": "http://127.0.0.1:9"}),
    )
    .await;
    assert_eq!(status, 200, "{provider}");
    let (status, group) = admin(
        reqwest::Method::POST,
        format!("/api/v1/orgs/{org_id}/provider-groups"),
        json!({"name": "pool", "slug": "pool", "strategy": "round_robin",
               "members": [{"provider_id": provider["id"]}]}),
    )
    .await;
    assert_eq!(status, 200, "{group}");
    let (status, route) = admin(
        reqwest::Method::POST,
        format!(
            "/api/v1/projects/{}/routes",
            project["id"].as_str().unwrap()
        ),
        json!({"model": "gpt-4o", "strategy": "round_robin"}),
    )
    .await;
    assert_eq!(status, 200, "{route}");

    // then the control plane starts with a file holding the same names globally
    let mut file = rolter_core::GatewayConfig::default();
    file.providers.push(rolter_core::ProviderConfig {
        name: "openai".to_string(),
        slug: Some("openai".to_string()),
        kind: rolter_core::ProviderKind::OpenaiCompatible,
        api_base: "http://127.0.0.1:8".to_string(),
        ..Default::default()
    });
    file.provider_groups.push(rolter_core::ProviderGroupConfig {
        name: "pool".to_string(),
        slug: Some("pool".to_string()),
        strategy: rolter_core::BalancingStrategy::RoundRobin,
        members: vec![rolter_core::GroupMember {
            provider: "openai".to_string(),
            model: None,
            weight: 1,
        }],
        tenancy: None,
        ..Default::default()
    });
    file.routes.push(
        serde_json::from_value(json!({
            "model": "gpt-4o",
            "targets": [{"provider": "openai", "model": "gpt-4o"}]
        }))
        .unwrap(),
    );
    let after = serve(
        rolter_control::test_app_with_file_config(pool.clone(), Some(token.to_string()), file)
            .await
            .unwrap(),
    )
    .await;
    let after = format!("http://{after}");
    let org_uuid: uuid::Uuid = org_id.parse().unwrap();
    let actors = seed_actors(&pool, org_uuid).await;
    let manager = |method, path: String, body| {
        send(
            after.clone(),
            actors.manager_bearer.clone(),
            method,
            path,
            body,
        )
    };

    let (status, body) = manager(
        reqwest::Method::PUT,
        format!("/api/v1/providers/{}", provider["id"].as_str().unwrap()),
        json!({"api_base": "http://127.0.0.1:10"}),
    )
    .await;
    assert_eq!(status, 200, "the org's own provider: {body}");
    let (status, body) = manager(
        reqwest::Method::PUT,
        format!("/api/v1/provider-groups/{}", group["id"].as_str().unwrap()),
        json!({"name": "renamed pool"}),
    )
    .await;
    assert_eq!(status, 200, "the org's own group: {body}");

    let (status, models) = send(
        after.clone(),
        token.to_string(),
        reqwest::Method::GET,
        "/api/v1/models".into(),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "{models}");
    let sources: Vec<(Option<&str>, &str)> = models
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["model"] == "gpt-4o")
        .map(|m| (m["org_id"].as_str(), m["source"].as_str().unwrap()))
        .collect();
    assert_eq!(sources.len(), 2, "{models}");
    assert!(sources.contains(&(Some(org_id.as_str()), "db")), "{models}");
    assert!(sources.contains(&(None, "config")), "{models}");

    let (status, body) = manager(
        reqwest::Method::DELETE,
        format!("/api/v1/provider-groups/{}", group["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "the org's own group: {status} {body}"
    );
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

/// #2581: the business-unit and customer listings count the live virtual keys
/// attributed to each row, so a zero-spend card can tell "no key" from "no
/// traffic". A disabled or expired key is not live, and a row nothing points at
/// still comes back with a zero rather than dropping out of the join.
#[tokio::test]
async fn attribution_listings_count_live_virtual_keys() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let pool = db.pool().clone();
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
    async fn get(client: &reqwest::Client, url: String) -> Value {
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(resp.status(), 200, "GET {url}");
        resp.json().await.unwrap()
    }
    fn count_for(rows: &Value, id: &str) -> i64 {
        rows.as_array()
            .expect("a listing is an array")
            .iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("{id} listed"))["live_key_count"]
            .as_i64()
            .expect("live_key_count is an integer")
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

    let unit = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
        json!({"name": "Payments"}),
    )
    .await;
    let unit_id = unit["id"].as_str().expect("unit id");
    let idle_unit = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
        json!({"name": "Research"}),
    )
    .await;
    let idle_unit_id = idle_unit["id"].as_str().expect("idle unit id");
    let customer = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/customers"),
        json!({"name": "Acme EU", "business_unit_id": unit_id}),
    )
    .await;
    let customer_id = customer["id"].as_str().expect("customer id");
    let idle_customer = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/customers"),
        json!({"name": "Acme US"}),
    )
    .await;
    let idle_customer_id = idle_customer["id"].as_str().expect("idle customer id");

    // four keys on the same unit and customer: two live, one disabled, one
    // expired. only the two live ones count
    let mut key_ids = Vec::new();
    for name in ["live-a", "live-b", "disabled", "expired"] {
        let key = post(
            &client,
            format!("{base}/api/v1/projects/{project_id}/virtual-keys"),
            json!({"name": name}),
        )
        .await;
        let key_id = key["id"].as_str().expect("key id").to_string();
        let attributed = client
            .put(format!("{base}/api/v1/virtual-keys/{key_id}/attribution"))
            .json(&json!({"business_unit_id": unit_id, "customer_id": customer_id}))
            .send()
            .await
            .unwrap();
        assert_eq!(attributed.status(), 200);
        key_ids.push(key_id);
    }
    let disabled = client
        .put(format!("{base}/api/v1/virtual-keys/{}", key_ids[2]))
        .json(&json!({"disabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 200);
    // no endpoint backdates an expiry, so the store is told directly
    sqlx::query("update virtual_keys set expires_at = now() - interval '1 day' where id = $1")
        .bind(uuid::Uuid::parse_str(&key_ids[3]).expect("key uuid"))
        .execute(&pool)
        .await
        .expect("expire key");

    let units = get(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
    )
    .await;
    assert_eq!(count_for(&units, unit_id), 2);
    assert_eq!(count_for(&units, idle_unit_id), 0);
    // the row's own columns are still there beside the count
    assert_eq!(units[0]["name"], "Payments");
    assert_eq!(units[0]["org_id"], org_id);

    let customers = get(&client, format!("{base}/api/v1/orgs/{org_id}/customers")).await;
    assert_eq!(count_for(&customers, customer_id), 2);
    assert_eq!(count_for(&customers, idle_customer_id), 0);
    let listed_customer = customers
        .as_array()
        .expect("customers")
        .iter()
        .find(|row| row["id"] == customer_id)
        .expect("customer listed");
    assert_eq!(listed_customer["business_unit_id"], unit_id);

    // re-enabling a key brings it back into the count
    let enabled = client
        .put(format!("{base}/api/v1/virtual-keys/{}", key_ids[2]))
        .json(&json!({"disabled": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(enabled.status(), 200);
    let units = get(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/business-units"),
    )
    .await;
    assert_eq!(count_for(&units, unit_id), 3);
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

/// A provider's upstream catalogue comes back as the ids a route target can
/// name as its upstream model (#2810): sorted and without duplicates. An
/// upstream that does not answer is an empty list rather than an error, since
/// the list only ever suggests, and a provider that does not exist is a 404.
/// The credential goes out the way the provider's own API reads it, as the test
/// sends it: an Anthropic provider's key is `x-api-key`, so its listing works.
#[tokio::test]
async fn a_provider_lists_the_models_its_upstream_serves() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);

    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // an upstream whose catalogue is out of order and repeats an id
    let upstream = serve(axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|| async {
            axum::Json(json!({"data": [
                {"id": "llama-3.1-8b"},
                {"id": "gemma-2-9b"},
                {"id": "llama-3.1-8b"},
            ]}))
        }),
    ))
    .await;
    // one that lists only for the header Anthropic's API reads its key from
    let anthropic = serve(axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|headers: axum::http::HeaderMap| async move {
            if headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some("sk-ant-test") {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(json!({"data": [{"id": "claude-sonnet"}, {"id": "claude-haiku"}]})),
                )
            } else {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"error": "missing x-api-key"})),
                )
            }
        }),
    ))
    .await;
    // an address nothing is listening on
    let silent = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };

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
    let mut provider_ids = Vec::new();
    for (name, kind, api_base, key) in [
        (
            "serving",
            "openai_compatible",
            format!("http://{upstream}"),
            None,
        ),
        (
            "silent",
            "openai_compatible",
            format!("http://{silent}"),
            None,
        ),
        (
            "claude",
            "anthropic",
            format!("http://{anthropic}"),
            Some("sk-ant-test"),
        ),
    ] {
        let mut body = json!({"name": name, "kind": kind, "api_base": api_base});
        if let Some(key) = key {
            body["api_key"] = json!(key);
        }
        let provider: Value = client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        provider_ids.push(provider["id"].as_str().expect("provider id").to_string());
    }

    for (provider, expected) in [
        (&provider_ids[0], json!(["gemma-2-9b", "llama-3.1-8b"])),
        // answered nothing: the same empty list a catalogue-less upstream gives
        (&provider_ids[1], json!([])),
        (&provider_ids[2], json!(["claude-haiku", "claude-sonnet"])),
    ] {
        let listed = client
            .get(format!("{base}/api/v1/providers/{provider}/models"))
            .send()
            .await
            .unwrap();
        let status = listed.status();
        let listed: Value = listed.json().await.unwrap();
        assert_eq!(status, 200, "{listed}");
        assert_eq!(listed, json!({ "models": expected }));
    }

    let missing = client
        .get(format!(
            "{base}/api/v1/providers/00000000-0000-0000-0000-000000000000/models"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

/// The listing spends the provider's credential, so it takes the permission the
/// connection test takes, `provider:update` in the provider's scope (#2810): a
/// viewer who may see a provider is refused, an org admin and the admin of the
/// project a provider is scoped to are allowed, and a provider the caller
/// cannot see at all is a 404.
#[tokio::test]
async fn listing_a_providers_models_takes_the_permission_the_test_takes() {
    skip_without_db!();

    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("scoped".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let upstream = serve(axum::Router::new().route(
        "/v1/models",
        axum::routing::get(|| async { axum::Json(json!({"data": [{"id": "gemma-2-9b"}]})) }),
    ))
    .await;

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
    let mut provider_ids = std::collections::HashMap::new();
    for (name, project) in [("shared", None), ("priv-one", Some(p1))] {
        let id: uuid::Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, slug, kind, api_base, project_id)
             values ($1, $2, $2, 'openai_compatible', $3, $4) returning id",
        )
        .bind(org)
        .bind(name)
        .bind(format!("http://{upstream}"))
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
        provider_ids.insert(name, id);
    }

    let org_admin = seed_user(&pool, "owner@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(org), None, None, "admin").await;
    let org_viewer = seed_user(&pool, "viewer@acme.test", false).await;
    seed_membership(&pool, org_viewer, Some(org), None, None, "viewer").await;
    let one_admin = seed_user(&pool, "one-admin@acme.test", false).await;
    seed_membership(&pool, one_admin, None, None, Some(p1), "admin").await;
    let two_member = seed_user(&pool, "two@acme.test", false).await;
    seed_membership(&pool, two_member, None, None, Some(p2), "member").await;
    let owner = seed_session(&pool, org_admin, "models_owner").await;
    let viewer = seed_session(&pool, org_viewer, "models_viewer").await;
    let one = seed_session(&pool, one_admin, "models_one").await;
    let two = seed_session(&pool, two_member, "models_two").await;

    let listed = |provider: &str, token: &str| {
        let url = format!("{base}/api/v1/providers/{}/models", provider_ids[provider]);
        let (client, token) = (client.clone(), token.to_string());
        async move {
            let resp = client.get(url).bearer_auth(token).send().await.unwrap();
            let status = resp.status().as_u16();
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };

    // allowed: whoever may update the provider where it lives
    for (token, provider) in [(&owner, "shared"), (&owner, "priv-one"), (&one, "priv-one")] {
        let (status, body) = listed(provider, token).await;
        assert_eq!(status, 200, "{provider}: {body}");
        assert_eq!(body, json!({"models": ["gemma-2-9b"]}), "{provider}");
    }
    // refused: it can see the provider, and may not spend its credential. the
    // admin of one project does not own an org-wide provider
    for (token, provider) in [(&viewer, "shared"), (&viewer, "priv-one"), (&one, "shared")] {
        let (status, body) = listed(provider, token).await;
        assert_eq!(status, 403, "{provider}: {body}");
        assert!(body.get("models").is_none(), "{provider}: {body}");
    }
    // not visible at all: a role in another project of the org sees no sign of
    // a provider scoped to this one
    let (status, body) = listed("priv-one", &two).await;
    assert_eq!(status, 404, "{body}");
}

/// The provider DTO says whether a sealed key is stored, on the list, the
/// create response and the update response, and never carries the key itself.
#[tokio::test]
async fn provider_dto_reports_whether_a_sealed_key_is_stored() {
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
    let org_id = org["id"].as_str().expect("org id");
    let sealed_secret = random_password();

    let mut created = Vec::new();
    for (name, extra) in [
        ("sealed", json!({"api_key": sealed_secret})),
        ("env-only", json!({"api_key_env": "SOME_UPSTREAM_KEY"})),
        ("bare", json!({})),
    ] {
        let mut body =
            json!({"name": name, "kind": "openai", "api_base": "https://api.openai.com"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let resp = client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{}", resp.status());
        let text = resp.text().await.unwrap();
        assert!(!text.contains(&sealed_secret), "create leaked the key");
        created.push(serde_json::from_str::<Value>(&text).unwrap());
    }
    assert_eq!(created[0]["has_stored_key"], true);
    assert_eq!(created[1]["has_stored_key"], false);
    assert_eq!(created[2]["has_stored_key"], false);

    let listed = client
        .get(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for leak in [sealed_secret.as_str(), "ciphertext", "nonce"] {
        assert!(!listed.contains(leak), "list leaked {leak}");
    }
    let listed: Vec<Value> = serde_json::from_str(&listed).unwrap();
    let flag = |name: &str| {
        listed
            .iter()
            .find(|p| p["name"] == name)
            .map(|p| p["has_stored_key"].clone())
    };
    assert_eq!(flag("sealed"), Some(json!(true)));
    assert_eq!(flag("env-only"), Some(json!(false)));
    assert_eq!(flag("bare"), Some(json!(false)));

    // an update that omits `api_key` keeps the stored key and still reports it
    let sealed_id = created[0]["id"].as_str().unwrap();
    let kept: Value = client
        .put(format!("{base}/api/v1/providers/{sealed_id}"))
        .json(&json!({"api_base": "https://eu.api.openai.com"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(kept["has_stored_key"], true);

    // sealing a key on a bare provider flips it, and clearing flips it back
    let bare_id = created[2]["id"].as_str().unwrap();
    let sealed: Value = client
        .put(format!("{base}/api/v1/providers/{bare_id}"))
        .json(&json!({"api_key": random_password()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sealed["has_stored_key"], true);
    let cleared: Value = client
        .put(format!("{base}/api/v1/providers/{bare_id}"))
        .json(&json!({"api_key": ""}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["has_stored_key"], false);
}

/// #2806: "Test connection" presents each kind's key the way its API reads it.
///
/// The stub is the Anthropic API in miniature: it lists models only for a
/// request carrying the key in `x-api-key` and a version header, and answers
/// 401 to a bearer token. The probe used to send the bearer form, so every
/// Anthropic provider reported a valid key as rejected on its first test.
#[tokio::test]
async fn provider_test_sends_the_anthropic_key_as_x_api_key() {
    skip_without_db!();
    std::env::set_var("ROLTER_KEK", TEST_KEK);

    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let sealed_secret = random_password();
    let expected = sealed_secret.clone();
    let stub = axum::Router::new().route(
        "/v1/models",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let expected = expected.clone();
            async move {
                let keyed = headers.get("x-api-key").and_then(|v| v.to_str().ok())
                    == Some(expected.as_str());
                if keyed && headers.contains_key("anthropic-version") {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(json!({"data": [{"id": "claude-test"}]})),
                    )
                } else {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(json!({"error": "invalid x-api-key"})),
                    )
                }
            }
        }),
    );
    let upstream = serve(stub).await;

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
    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .json(&json!({
            "name": "claude",
            "kind": "anthropic",
            "api_base": format!("http://{upstream}"),
            "api_key": sealed_secret,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = created["id"].as_str().expect("provider id");

    let tested: Value = client
        .post(format!("{base}/api/v1/providers/{provider_id}/test"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tested["reachable"], true, "{tested}");
    assert_eq!(tested["status"], 200, "{tested}");
    assert_eq!(tested["credential"], "stored", "{tested}");
    assert_eq!(tested["models_found"], 1, "{tested}");
    assert!(!tested.to_string().contains(&sealed_secret), "leaked key");
}

/// #2008: the provider test, given a model name, says listed, not listed, or
/// cannot tell (a liveness-only kind), and still refuses a secret env name.
#[tokio::test]
async fn provider_test_reports_whether_a_model_is_listed() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let catalogue = serve(axum::Router::new().fallback(|| async {
        axum::Json(json!({"data": [{"id": "gpt-4o"}, {"id": "gpt-4o-mini"}]}))
    }))
    .await;
    let health = serve(axum::Router::new().fallback(|| async { "OK" })).await;

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
    let mut ids = Vec::new();
    for (name, kind, upstream) in [("oa", "openai", catalogue), ("embed", "tei", health)] {
        let created: Value = client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .json(&json!({
                "name": name,
                "kind": kind,
                "api_base": format!("http://{upstream}"),
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push(created["id"].as_str().expect("provider id").to_string());
    }

    let test = |id: String, query: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/api/v1/providers/{id}/test{query}"))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let listed = test(ids[0].clone(), "?model=gpt-4o").await;
    assert_eq!(listed["reachable"], true, "{listed}");
    assert_eq!(listed["model_listed"], true, "{listed}");
    let missing = test(ids[0].clone(), "?model=gpt-9").await;
    assert_eq!(missing["reachable"], true, "{missing}");
    assert_eq!(missing["model_listed"], false, "{missing}");
    let unasked = test(ids[0].clone(), "").await;
    assert!(unasked["model_listed"].is_null(), "{unasked}");
    let liveness = test(ids[1].clone(), "?model=gpt-4o").await;
    assert_eq!(liveness["reachable"], true, "{liveness}");
    assert!(liveness["model_listed"].is_null(), "{liveness}");
}

/// #966: an import creates catalogue prices stamped with their source, refreshes
/// them, never replaces an operator's own price, and an edit turns an imported
/// price into an override.
#[tokio::test]
async fn catalogue_import_never_replaces_an_operator_price() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let listing: Value = client
        .get(format!(
            "{base}/api/v1/model-prices/catalogue?q=gpt-4o&limit=3"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listing["source"], "litellm", "{listing}");
    assert_eq!(listing["currency"], "USD", "{listing}");
    assert!(listing["snapshot"].is_string() && listing["licence"].is_string());
    assert!(listing["entries"].as_array().unwrap().len() <= 3);

    // the operator prices gpt-4o by hand before any import
    let put = |model: &str, input: &str| {
        let client = client.clone();
        let url = format!("{base}/api/v1/model-prices");
        let body = json!({"model": model, "input_per_mtok": input, "output_per_mtok": "9", "currency": "USD"});
        async move { client.put(url).json(&body).send().await.unwrap() }
    };
    assert!(put("gpt-4o", "7").await.status().is_success());

    let import = |models: Value| {
        let client = client.clone();
        let url = format!("{base}/api/v1/model-prices/import");
        async move {
            client
                .post(url)
                .json(&json!({"models": models}))
                .send()
                .await
                .unwrap()
        }
    };
    let report: Value = import(json!(["gpt-4o", "gpt-4o-mini", "no-such-model-anywhere"]))
        .await
        .json()
        .await
        .unwrap();
    let outcome = |report: &Value, model: &str| {
        report["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["model"] == model)
            .map(|r| r["outcome"].as_str().unwrap().to_string())
    };
    assert_eq!(
        outcome(&report, "gpt-4o").as_deref(),
        Some("kept_override"),
        "{report}"
    );
    assert_eq!(
        outcome(&report, "gpt-4o-mini").as_deref(),
        Some("created"),
        "{report}"
    );
    assert_eq!(
        outcome(&report, "no-such-model-anywhere").as_deref(),
        Some("not_in_catalogue"),
        "{report}"
    );

    let prices = || {
        let client = client.clone();
        let url = format!("{base}/api/v1/model-prices");
        async move {
            client
                .get(url)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let row = |prices: &Value, model: &str| {
        prices
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["model"] == model)
            .cloned()
            .unwrap()
    };
    let listed = prices().await;
    let mine = row(&listed, "gpt-4o");
    assert_eq!(mine["input_per_mtok"], "7.000000", "{mine}");
    assert!(
        mine["source"].is_null(),
        "an operator price has no source: {mine}"
    );
    let imported = row(&listed, "gpt-4o-mini");
    assert_eq!(imported["source"], "litellm", "{imported}");
    assert_eq!(imported["currency"], "USD", "{imported}");
    assert!(imported["source_imported_at"].is_string(), "{imported}");

    // a second import refreshes the catalogue row and still leaves the override
    let again: Value = import(json!(["gpt-4o", "gpt-4o-mini"]))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        outcome(&again, "gpt-4o-mini").as_deref(),
        Some("updated"),
        "{again}"
    );
    assert_eq!(
        outcome(&again, "gpt-4o").as_deref(),
        Some("kept_override"),
        "{again}"
    );

    // editing an imported price makes it the operator's own
    assert!(put("gpt-4o-mini", "1").await.status().is_success());
    let edited = row(&prices().await, "gpt-4o-mini");
    assert!(edited["source"].is_null(), "{edited}");
    let kept: Value = import(json!(["gpt-4o-mini"])).await.json().await.unwrap();
    assert_eq!(
        outcome(&kept, "gpt-4o-mini").as_deref(),
        Some("kept_override"),
        "{kept}"
    );

    assert_eq!(
        import(json!([])).await.status(),
        400,
        "an empty import is refused"
    );
}

/// #984: the cache probe records a reporting provider, a non-reporting one and
/// leaves a provider that never answered unprobed; the listing carries only
/// the verdicts that exist.
#[tokio::test]
async fn cache_probe_records_a_verdict_per_provider_and_lists_only_probed_ones() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let reporting = serve(axum::Router::new().fallback(|| async {
        axum::Json(json!({"usage": {"prompt_tokens": 8, "completion_tokens": 1,
            "prompt_tokens_details": {"cached_tokens": 0}}}))
    }))
    .await;
    let silent = serve(axum::Router::new().fallback(|| async {
        axum::Json(json!({"usage": {"prompt_tokens": 8, "completion_tokens": 1}}))
    }))
    .await;
    let broken = serve(
        axum::Router::new()
            .fallback(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "down") }),
    )
    .await;

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
    let mut ids = Vec::new();
    for (name, upstream) in [
        ("reports", reporting),
        ("silent", silent),
        ("broken", broken),
    ] {
        let created: Value = client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .json(&json!({
                "name": name,
                "kind": "openai_compatible",
                "api_base": format!("http://{upstream}"),
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        ids.push(created["id"].as_str().expect("provider id").to_string());
    }
    let probe = |id: String| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/api/v1/providers/{id}/probe-cache?model=m"))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let listing = || {
        let client = client.clone();
        let base = base.clone();
        let org_id = org_id.to_string();
        async move {
            client
                .get(format!("{base}/api/v1/orgs/{org_id}/provider-cache-probes"))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    assert_eq!(listing().await, json!([]), "nothing is probed yet");

    let yes = probe(ids[0].clone()).await;
    assert_eq!(yes["recorded"], true, "{yes}");
    assert_eq!(yes["verdict"]["reports"], true, "{yes}");
    assert_eq!(
        yes["verdict"]["spelling"], "/prompt_tokens_details/cached_tokens",
        "{yes}"
    );
    let no = probe(ids[1].clone()).await;
    assert_eq!(no["recorded"], true, "{no}");
    assert_eq!(no["verdict"]["reports"], false, "{no}");
    assert!(no["verdict"]["spelling"].is_null(), "{no}");
    let unknown = probe(ids[2].clone()).await;
    assert_eq!(unknown["recorded"], false, "{unknown}");
    assert_eq!(unknown["status"], 500, "{unknown}");

    let listed = listing().await;
    let listed = listed.as_array().expect("a list");
    assert_eq!(listed.len(), 2, "the failed probe left no verdict");
    assert!(listed.iter().all(|row| row["probed_at"].is_string()));
    assert!(listed.iter().all(|row| row["provider_id"] != json!(ids[2])));

    let refused = client
        .post(format!("{base}/api/v1/providers/{}/probe-cache", ids[0]))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400, "the model is required");
}

/// #3085: a provider cannot name a control-plane secret as its credential
/// variable, on save or on test, and a row that already does is never read.
#[tokio::test]
async fn provider_api_key_env_cannot_name_a_control_plane_secret() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    let upstream = serve(axum::Router::new().fallback(move || {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            axum::Json(json!({"data": []}))
        }
    }))
    .await;

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
    let providers = format!("{base}/api/v1/orgs/{org_id}/providers");
    let draft = |name: &str, env: &str| {
        json!({"name": name, "kind": "openai", "api_base": format!("http://{upstream}"),
               "api_key_env": env})
    };

    // refused on create
    let resp = client
        .post(&providers)
        .json(&draft("leaky", "ROLTER_KEK"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "api_key_env", "{body}");

    // an ordinary variable is accepted, and an edit to a secret is refused
    let created: Value = client
        .post(&providers)
        .json(&draft("fine", "ROLTER_TEST_3085_PROVIDER_KEY"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // the prefix rule refuses it too: no ROLTER_ name is a provider key
    assert_eq!(created["error"]["field"], "api_key_env", "{created}");
    let created: Value = client
        .post(&providers)
        .json(&draft("fine", "TEST_3085_PROVIDER_KEY"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = created["id"].as_str().expect("provider id").to_string();
    let resp = client
        .put(format!("{base}/api/v1/providers/{provider_id}"))
        .json(&json!({"api_key_env": "rolter_database_url"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // an edit that omits api_key_env keeps the stored name
    let resp = client
        .put(format!("{base}/api/v1/providers/{provider_id}"))
        .json(&json!({"api_base": format!("http://{upstream}")}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());
    let view: Value = resp.json().await.unwrap();
    assert_eq!(view["api_key_env"], "TEST_3085_PROVIDER_KEY", "{view}");

    // a row that already names a secret (an import, or saved before the rule)
    // is refused at test and at listing time, and nothing is sent
    sqlx::query("update providers set api_key_env='ROLTER_KEK' where id=$1::uuid")
        .bind(&provider_id)
        .execute(db.pool())
        .await
        .unwrap();
    std::env::set_var("ROLTER_KEK", TEST_KEK);
    for (method, path) in [("post", "test"), ("get", "models")] {
        let url = format!("{base}/api/v1/providers/{provider_id}/{path}");
        let resp = if method == "post" {
            client.post(url).send().await.unwrap()
        } else {
            client.get(url).send().await.unwrap()
        };
        assert_eq!(resp.status(), 400, "{path}");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["error"]["code"], "invalid_field", "{path}: {body}");
        assert_eq!(body["error"]["field"], "api_key_env", "{path}: {body}");
        assert!(!body.to_string().contains(TEST_KEK), "{path}: leaked");
    }
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the upstream must not be contacted"
    );
}

#[tokio::test]
async fn skill_access_policy_filters_list_history_and_resolution() {
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
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

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
        .bearer_auth(admin_token())
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

/// #2438: deleting a provider a route still targets answered a 500 carrying
/// nothing but the store's foreign-key failure. It must refuse with a 409 that
/// names the route, and leave the provider in place.
#[tokio::test]
async fn deleting_a_provider_a_route_targets_is_a_conflict_naming_the_route() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("app");
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
        json!({"model": "gpt-4o"}),
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");
    let target = post(
        &client,
        format!("{base}/api/v1/routes/{route_id}/targets"),
        json!({"provider_id": provider_id, "weight": 1}),
    )
    .await;

    let resp = client
        .delete(format!("{base}/api/v1/providers/{provider_id}"))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 409, "{body}");
    let message = body["error"]["message"].as_str().expect("error message");
    assert!(
        message.contains("route 'gpt-4o'"),
        "the refusal does not name the route: {message}"
    );

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        listed
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == provider_id)),
        "the refused delete removed the provider: {listed}"
    );

    // once nothing references it the same delete goes through
    let target_id = target["id"].as_str().expect("target id");
    let resp = client
        .delete(format!("{base}/api/v1/route-targets/{target_id}"))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.text().await.unwrap());
    let resp = client
        .delete(format!("{base}/api/v1/providers/{provider_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "{}", resp.text().await.unwrap());
}

/// #2438: the same refusal for a provider a provider group still holds as a
/// member, naming the group.
#[tokio::test]
async fn deleting_a_provider_a_group_holds_is_a_conflict_naming_the_group() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("app");
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
    let provider = post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/providers"),
        json!({"name": "vllm-a100-01", "kind": "openai", "api_base": "http://vllm.internal"}),
    )
    .await;
    let provider_id = provider["id"].as_str().expect("provider id");
    post(
        &client,
        format!("{base}/api/v1/orgs/{org_id}/provider-groups"),
        json!({
            "name": "Llama fleet",
            "slug": "llama-fleet",
            "members": [{"provider_id": provider_id, "weight": 1}],
        }),
    )
    .await;

    let resp = client
        .delete(format!("{base}/api/v1/providers/{provider_id}"))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, 409, "{body}");
    let message = body["error"]["message"].as_str().expect("error message");
    assert!(
        message.contains("provider group 'llama-fleet'"),
        "the refusal does not name the group: {message}"
    );

    let listed: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/providers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        listed
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == provider_id)),
        "the refused delete removed the provider: {listed}"
    );
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
    assert_eq!(body["error"]["code"], "scope_mismatch", "{body}");
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
    assert_eq!(body["error"]["code"], "invalid_field", "{body}");
    assert_eq!(body["error"]["field"], "project_id", "{body}");
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
    assert_eq!(body["error"]["code"], "scope_mismatch", "{body}");

    // a project that still owns a provider cannot be deleted out from under it
    let resp = client
        .delete(format!("{base}/api/v1/projects/{p1}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "referenced", "{body}");
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

/// #2236: a database provider stores `kv_events` and `lmcache`, serves them in
/// the snapshot, keeps them through an update that omits them, clears them on
/// `null`, and refuses a value the file config would refuse.
#[tokio::test]
async fn a_provider_stores_and_serves_cache_telemetry_settings() {
    skip_without_db!();
    let (app, db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Telemetry", "slug": "telemetry"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let providers_url = format!("{base}/api/v1/orgs/{org_id}/providers");
    let body = |extra: Value| {
        let mut base = json!({
            "name": "vllm", "kind": "openai_compatible", "api_base": "https://llm.example.com"
        });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    };

    // refused: wrong scheme, zero interval, and an endpoint the egress policy denies
    for bad in [
        json!({"kv_events": {"endpoint": "http://vllm-0:5557"}}),
        json!({"kv_events": {"endpoint": "tcp://vllm-0:5557", "stale_secs": 0}}),
        json!({"lmcache": {"endpoint": "tcp://lmcache:9000"}}),
        json!({"lmcache": {"endpoint": "http://lmcache:9000", "refresh_secs": 0}}),
        json!({"lmcache": {"endpoint": "http://169.254.169.254/latest"}}),
    ] {
        let response = client
            .post(&providers_url)
            .json(&body(bad.clone()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{bad}");
    }

    let created: Value = client
        .post(&providers_url)
        .json(&body(json!({
            "kv_events": {"endpoint": "tcp://vllm-0:5557", "topic": "kv"},
            "lmcache": {"endpoint": "http://lmcache:9000"}
        })))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    assert_eq!(created["kv_events"]["endpoint"], "tcp://vllm-0:5557");
    assert_eq!(created["kv_events"]["max_blocks"], 1_000_000);
    assert_eq!(created["lmcache"]["refresh_secs"], 2);

    let served = || async {
        let snap: Value = client
            .get(format!("{base}/internal/snapshot"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        snap["config"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "vllm")
            .cloned()
            .expect("provider in snapshot")
    };
    let snapped = served().await;
    assert_eq!(snapped["kv_events"]["topic"], "kv");
    assert_eq!(snapped["lmcache"]["endpoint"], "http://lmcache:9000");

    let put = |patch: Value| {
        let url = format!("{base}/api/v1/providers/{provider_id}");
        let client = client.clone();
        async move { client.put(url).json(&patch).send().await.unwrap() }
    };

    // an older client that sends neither field must not reset them
    let kept: Value = put(json!({"api_base": "https://llm2.example.com"}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(kept["kv_events"]["endpoint"], "tcp://vllm-0:5557");
    assert_eq!(kept["lmcache"]["endpoint"], "http://lmcache:9000");
    assert_eq!(served().await["kv_events"]["endpoint"], "tcp://vllm-0:5557");

    // null clears one and leaves the other
    let cleared: Value = put(json!({"kv_events": null})).await.json().await.unwrap();
    assert!(cleared["kv_events"].is_null(), "{cleared}");
    assert_eq!(cleared["lmcache"]["endpoint"], "http://lmcache:9000");
    assert!(served().await["kv_events"].is_null());

    // an invalid replacement is refused and changes nothing
    let refused =
        put(json!({"lmcache": {"endpoint": "http://lmcache:9000", "stale_secs": 0}})).await;
    assert_eq!(refused.status(), 400);
    assert_eq!(served().await["lmcache"]["refresh_secs"], 2);

    // a stored value the loader cannot read costs the provider its setting,
    // not the snapshot
    sqlx::query("update providers set kv_events = '{\"topic\": 7}'::jsonb")
        .execute(db.pool())
        .await
        .unwrap();
    let degraded = served().await;
    assert!(degraded["kv_events"].is_null(), "{degraded}");
    assert_eq!(degraded["lmcache"]["endpoint"], "http://lmcache:9000");
}

/// #2235: `PUT /api/v1/routes/{id}` changes a route's strategy and its targets
/// and weights, not only `enabled`. Every field is optional and an omitted one
/// keeps its value, so a client that sends only `enabled` resets nothing. A
/// target list is the route's whole list, matched to the stored targets by
/// provider and upstream model, so an edited weight keeps the target's id. Each
/// entry is held to the checks adding one target applies, and a refused update
/// leaves the route and `config_version` as they were.
#[tokio::test]
async fn a_routes_strategy_targets_and_weights_change_in_one_update() {
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
    let created = |(status, body): (u16, Value)| {
        assert_eq!(status, 200, "{body}");
        body["id"].as_str().expect("id").to_string()
    };

    let org = created(
        post(
            format!("{base}/api/v1/orgs"),
            json!({"name": "Routes", "slug": "routes"}),
        )
        .await,
    );
    let team = created(
        post(
            format!("{base}/api/v1/orgs/{org}/teams"),
            json!({"name": "core"}),
        )
        .await,
    );
    let project = created(
        post(
            format!("{base}/api/v1/teams/{team}/projects"),
            json!({"name": "app"}),
        )
        .await,
    );
    let other_project = created(
        post(
            format!("{base}/api/v1/teams/{team}/projects"),
            json!({"name": "batch"}),
        )
        .await,
    );
    let provider = |org: String, name: &'static str, project: Option<&str>| {
        let mut body = json!({"name": name, "kind": "openai_compatible",
                              "api_base": "http://127.0.0.1:9"});
        if let Some(project) = project {
            body["project_id"] = json!(project);
        }
        post(format!("{base}/api/v1/orgs/{org}/providers"), body)
    };
    let primary = created(provider(org.clone(), "primary", None).await);
    let secondary = created(provider(org.clone(), "secondary", None).await);
    let elsewhere = created(provider(org.clone(), "elsewhere", Some(&other_project)).await);
    let other_org = created(
        post(
            format!("{base}/api/v1/orgs"),
            json!({"name": "Other", "slug": "other"}),
        )
        .await,
    );
    let foreign = created(provider(other_org, "foreign", None).await);

    let route = created(
        post(
            format!("{base}/api/v1/projects/{project}/routes"),
            json!({"model": "chat", "strategy": "round_robin"}),
        )
        .await,
    );
    let route_url = format!("{base}/api/v1/routes/{route}");
    let first = created(
        post(
            format!("{route_url}/targets"),
            json!({"provider_id": primary, "weight": 1}),
        )
        .await,
    );

    let targets = || {
        let client = client.clone();
        let url = format!("{route_url}/targets");
        async move {
            let rows: Value = client.get(&url).send().await.unwrap().json().await.unwrap();
            rows.as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    (
                        t["id"].as_str().unwrap().to_string(),
                        t["provider_id"].as_str().unwrap().to_string(),
                        t["upstream_model"].as_str().map(str::to_string),
                        t["weight"].as_i64().unwrap(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };
    let last_audit = |action: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Value>(
                "select detail from audit_log where action = $1 order by at desc limit 1",
            )
            .bind(action)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    // an older client sends only `enabled`: the strategy and the targets stay
    let before = targets().await;
    let (status, row) = put(route_url.clone(), json!({"enabled": false})).await;
    assert_eq!(status, 200, "{row}");
    assert_eq!(row["enabled"], false);
    assert_eq!(row["strategy"], "round_robin");
    assert_eq!(targets().await, before);
    assert_eq!(last_audit("route.set_enabled").await["enabled"], false);
    let (status, row) = put(route_url.clone(), json!({"enabled": true})).await;
    assert_eq!(status, 200, "{row}");

    // strategy and targets in one call: the primary target keeps its id and
    // takes the new weight, and the secondary one is added
    let version = config_version(&pool).await;
    let (status, row) = put(
        route_url.clone(),
        json!({
            "strategy": "weighted",
            "targets": [
                {"provider_id": primary, "weight": 3},
                {"provider_id": secondary, "upstream_model": "gpt-4o-mini", "weight": 1}
            ]
        }),
    )
    .await;
    assert_eq!(status, 200, "{row}");
    assert_eq!(row["strategy"], "weighted");
    assert_eq!(row["enabled"], true, "an omitted enabled flag is kept");
    assert!(
        config_version(&pool).await > version,
        "the edit bumps the version"
    );
    let now = targets().await;
    assert_eq!(now.len(), 2, "{now:?}");
    assert_eq!(now[0], (first.clone(), primary.clone(), None, 3));
    assert_eq!(
        (&now[1].1, now[1].2.as_deref(), now[1].3),
        (&secondary, Some("gpt-4o-mini"), 1)
    );
    let secondary_target = now[1].0.clone();
    let audit = last_audit("route.update").await;
    assert_eq!(audit["strategy"], "weighted");
    assert_eq!(audit["previous_strategy"], "round_robin");
    assert_eq!(audit["targets"]["reweighted"][0]["id"], first.as_str());
    assert_eq!(audit["targets"]["reweighted"][0]["previous_weight"], 1);
    assert_eq!(
        audit["targets"]["added"][0]["provider_id"],
        secondary.as_str()
    );

    // the gateway's snapshot carries the new strategy and both weights
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let served = snap["config"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "chat")
        .expect("route in snapshot")
        .clone();
    assert_eq!(served["strategy"], "weighted");
    let mut weights: Vec<(String, u64)> = served["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["provider"].as_str().unwrap().to_string(),
                t["weight"].as_u64().unwrap(),
            )
        })
        .collect();
    weights.sort();
    assert_eq!(
        weights,
        vec![("primary".to_string(), 3), ("secondary".to_string(), 1)]
    );

    // the strategy alone leaves the targets alone
    let before = targets().await;
    let (status, row) = put(route_url.clone(), json!({"strategy": "cache_aware"})).await;
    assert_eq!(status, 200, "{row}");
    assert_eq!(row["strategy"], "cache_aware");
    assert_eq!(targets().await, before);

    // the targets alone leave the strategy alone, and a target the list no
    // longer names is removed
    let (status, row) = put(
        route_url.clone(),
        json!({"targets": [
            {"provider_id": secondary, "upstream_model": "gpt-4o-mini", "weight": 2}
        ]}),
    )
    .await;
    assert_eq!(status, 200, "{row}");
    assert_eq!(row["strategy"], "cache_aware");
    assert_eq!(
        targets().await,
        vec![(
            secondary_target.clone(),
            secondary.clone(),
            Some("gpt-4o-mini".to_string()),
            2
        )]
    );
    assert_eq!(
        last_audit("route.update").await["targets"]["removed"][0]["id"],
        first.as_str()
    );

    // every refusal leaves the route, its targets and the version untouched
    let before = targets().await;
    let version = config_version(&pool).await;
    let ok_target = json!({"provider_id": primary, "weight": 1});
    let unknown = uuid::Uuid::new_v4().to_string();
    for (case, body, status, field) in [
        (
            "an unknown strategy",
            json!({"strategy": "fastest_ever", "targets": [ok_target]}),
            400,
            None,
        ),
        (
            "an empty target list",
            json!({"strategy": "random", "targets": []}),
            400,
            Some("targets"),
        ),
        (
            "a zero weight",
            json!({"targets": [ok_target, {"provider_id": secondary, "weight": 0}]}),
            400,
            Some("targets[1].weight"),
        ),
        (
            "a negative weight",
            json!({"targets": [{"provider_id": secondary, "weight": -2}]}),
            400,
            Some("targets[0].weight"),
        ),
        (
            "an unknown provider",
            json!({"strategy": "random", "targets": [ok_target, {"provider_id": unknown}]}),
            404,
            None,
        ),
        (
            "another org's provider",
            json!({"targets": [{"provider_id": foreign}]}),
            404,
            None,
        ),
        (
            "another project's provider",
            json!({"targets": [ok_target, {"provider_id": elsewhere}]}),
            409,
            None,
        ),
        (
            "a field the endpoint does not take",
            json!({"model": "renamed"}),
            400,
            None,
        ),
    ] {
        let (got, body) = put(route_url.clone(), body).await;
        assert_eq!(got, status, "{case}: {body}");
        if let Some(field) = field {
            assert_eq!(body["error"]["code"], "invalid_field", "{case}: {body}");
            assert_eq!(body["error"]["field"], field, "{case}: {body}");
        }
    }
    assert_eq!(targets().await, before);
    assert_eq!(config_version(&pool).await, version);
    let (_, rows) = send(
        reqwest::Method::GET,
        format!("{base}/api/v1/projects/{project}/routes"),
        Value::Null,
    )
    .await;
    assert_eq!(rows[0]["strategy"], "cache_aware");

    // a blank upstream model is stored as none, as the target endpoint does
    let (status, row) = put(
        route_url.clone(),
        json!({"targets": [{"provider_id": primary, "upstream_model": "  ", "weight": 1}]}),
    )
    .await;
    assert_eq!(status, 200, "{row}");
    let now = targets().await;
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].2, None);

    // an empty body changes nothing
    let version = config_version(&pool).await;
    let (status, row) = put(route_url.clone(), json!({})).await;
    assert_eq!(status, 200, "{row}");
    assert_eq!(row["strategy"], "cache_aware");
    assert_eq!(config_version(&pool).await, version);

    let (status, _) = put(
        format!("{base}/api/v1/routes/{}", uuid::Uuid::new_v4()),
        json!({"strategy": "random"}),
    )
    .await;
    assert_eq!(status, 404);
}

/// #2210, #3031, #3046: the routes list embeds each route's targets and
/// complexity policy on request, so a screen of N routes is one read. The
/// embedded fields obey the list's own scope: another organization's member is
/// refused, a plain list keeps its old body, and `/api/v1/models` carries each
/// entry's targets and the organization that owns it.
#[tokio::test]
async fn the_routes_list_embeds_targets_and_complexity_within_the_callers_scope() {
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

    let mut orgs = Vec::new();
    for slug in ["embed-a", "embed-b"] {
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
        let team_id = team["id"].as_str().unwrap().to_string();
        let project = post_as(
            &client,
            format!("{base}/api/v1/teams/{team_id}/projects"),
            json!({"name": "app"}),
        )
        .await;
        orgs.push((org_id, project["id"].as_str().unwrap().to_string()));
    }
    let (org_a, project_a) = orgs[0].clone();
    let (org_b, _) = orgs[1].clone();

    let mut provider_ids = Vec::new();
    for name in ["edge-one", "edge-two"] {
        let provider = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_a}/providers"),
            json!({"name": name, "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
        .await;
        provider_ids.push(provider["id"].as_str().unwrap().to_string());
    }
    let busy = post_as(
        &client,
        format!("{base}/api/v1/projects/{project_a}/routes"),
        json!({"model": "busy", "strategy": "weighted"}),
    )
    .await;
    let busy_id = busy["id"].as_str().unwrap().to_string();
    let idle = post_as(
        &client,
        format!("{base}/api/v1/projects/{project_a}/routes"),
        json!({"model": "idle", "strategy": "round_robin"}),
    )
    .await;
    let idle_id = idle["id"].as_str().unwrap().to_string();
    for (provider, weight) in provider_ids.iter().zip([3, 1]) {
        post_as(
            &client,
            format!("{base}/api/v1/routes/{busy_id}/targets"),
            json!({"provider_id": provider, "upstream_model": "gpt-x", "weight": weight}),
        )
        .await;
    }
    let policy = json!({"tiers": [
        {"name": "small", "max_input_bytes": 4096, "route": "idle"},
        {"name": "large", "route": "busy"},
    ]});
    let written = client
        .put(format!("{base}/api/v1/routes/{busy_id}/complexity"))
        .bearer_auth(admin_token())
        .json(&policy)
        .send()
        .await
        .unwrap();
    assert!(written.status().is_success(), "{}", written.status());

    // a plain list is the body it always was: no embedded keys at all
    let list_url = format!("{base}/api/v1/projects/{project_a}/routes");
    let (status, plain) = get_as(&client, list_url.clone(), admin_token()).await;
    assert_eq!(status, 200, "{plain}");
    for route in plain.as_array().unwrap() {
        assert!(route.get("targets").is_none() && route.get("complexity").is_none());
    }

    // one request answers what N per-route reads did
    let (status, embedded) = get_as(
        &client,
        format!("{list_url}?include=targets,complexity"),
        admin_token(),
    )
    .await;
    assert_eq!(status, 200, "{embedded}");
    let embedded = embedded.as_array().unwrap();
    assert_eq!(embedded.len(), 2);
    for route in embedded {
        let id = route["id"].as_str().unwrap();
        let (_, per_route_targets) = get_as(
            &client,
            format!("{base}/api/v1/routes/{id}/targets"),
            admin_token(),
        )
        .await;
        assert_eq!(route["targets"], per_route_targets, "{}", route["model"]);
        let (_, per_route_policy) = get_as(
            &client,
            format!("{base}/api/v1/routes/{id}/complexity"),
            admin_token(),
        )
        .await;
        assert_eq!(route["complexity"], per_route_policy, "{}", route["model"]);
    }
    let busy_row = embedded
        .iter()
        .find(|r| r["id"] == busy_id.as_str())
        .unwrap();
    assert_eq!(busy_row["targets"].as_array().map(Vec::len), Some(2));
    assert_eq!(busy_row["complexity"]["tiers"][1]["name"], "large");
    let idle_row = embedded
        .iter()
        .find(|r| r["id"] == idle_id.as_str())
        .unwrap();
    assert_eq!(
        idle_row["targets"],
        json!([]),
        "no targets is an empty list"
    );
    assert_eq!(idle_row["complexity"], json!({"tiers": []}));

    // each extra is asked for on its own, and an unknown one is refused
    let (_, only_targets) = get_as(
        &client,
        format!("{list_url}?include=targets"),
        admin_token(),
    )
    .await;
    assert!(
        only_targets[0].get("complexity").is_none(),
        "{only_targets}"
    );
    let (status, refused) = get_as(
        &client,
        format!("{list_url}?include=secrets"),
        admin_token(),
    )
    .await;
    assert_eq!(status, 400, "{refused}");

    // the embedded fields are no wider than the list: a member of the owning
    // org reads them, a member of another org is refused
    let org_a_uuid: uuid::Uuid = org_a.parse().unwrap();
    let org_b_uuid: uuid::Uuid = org_b.parse().unwrap();
    let insider = seed_user(&pool, "embed-insider@example.com", false).await;
    seed_membership(&pool, insider, Some(org_a_uuid), None, None, "viewer").await;
    let insider_token = seed_session(&pool, insider, "embedinsider").await;
    let outsider = seed_user(&pool, "embed-outsider@example.com", false).await;
    seed_membership(&pool, outsider, Some(org_b_uuid), None, None, "admin").await;
    let outsider_token = seed_session(&pool, outsider, "embedoutsider").await;
    let embed_url = format!("{list_url}?include=targets,complexity");
    let (status, seen) = get_as(&client, embed_url.clone(), &insider_token).await;
    assert_eq!(status, 200, "{seen}");
    assert_eq!(seen.as_array().map(Vec::len), Some(2));
    let (status, seen) = get_as(&client, embed_url, &outsider_token).await;
    assert_eq!(status, 403, "{seen}");

    // the model list names each entry's targets and its organization
    let (status, models) = get_as(&client, format!("{base}/api/v1/models"), admin_token()).await;
    assert_eq!(status, 200, "{models}");
    let busy_model = models
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["model"] == "busy")
        .unwrap();
    assert_eq!(busy_model["targets"], 2);
    assert_eq!(busy_model["org_id"], org_a.as_str());
    assert_eq!(busy_model["project_id"], project_a.as_str());
    let providers: Vec<&str> = busy_model["route_targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["provider"].as_str().unwrap())
        .collect();
    assert_eq!(providers.len(), 2);
    assert!(providers.contains(&"edge-one") && providers.contains(&"edge-two"));
    assert_eq!(busy_model["route_targets"][0]["model"], "gpt-x");
}

/// #1133: a stored provider can opt out of its kind's host pin as a file one
/// can, the opt-out reaches the snapshot, and a custom base without it is
/// refused on write instead of saving a row the snapshot then leaves out.
#[tokio::test]
async fn a_stored_provider_can_opt_out_of_the_host_pin() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .json(&json!({"name": "Pinned", "slug": "pinned"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().unwrap().to_string();
    let providers_url = format!("{base}/api/v1/orgs/{org_id}/providers");
    let draft = |extra: Value| {
        let mut body = json!({
            "name": "openrouter-edge", "kind": "openrouter",
            "api_base": "https://openrouter-edge.example.com/api/v1",
            "api_key_env": "OPENROUTER_API_KEY"
        });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        body
    };

    let refused = client
        .post(&providers_url)
        .json(&draft(json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["field"], "api_base", "{body}");

    let created: Value = client
        .post(&providers_url)
        .json(&draft(json!({"allow_custom_api_base": true})))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let provider_id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    assert_eq!(created["allow_custom_api_base"], true, "{created}");

    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let served = snap["config"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "openrouter-edge")
        .cloned()
        .unwrap_or_else(|| panic!("provider in snapshot: {snap}"));
    assert_eq!(served["allow_custom_api_base"], true);

    // dropping the opt-out while the base is still custom is refused, and an
    // edit that leaves the pin's inputs alone is not held to it
    let url = format!("{base}/api/v1/providers/{provider_id}");
    let dropped = client
        .put(&url)
        .json(&json!({"allow_custom_api_base": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(dropped.status(), 400);
    let pinned: Value = client
        .put(&url)
        .json(&json!({
            "api_base": "https://openrouter.ai/api/v1",
            "allow_custom_api_base": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pinned["allow_custom_api_base"], false, "{pinned}");
}

/// #1584: a route target takes `takes_over`, the failure classes it is a
/// fallback for. The classes are validated, stored, changed in place by a
/// route update and served in the snapshot; and the API never leaves a route
/// with only restricted targets, whether the last one is added alone, sent as
/// the whole list or left behind by a delete.
#[tokio::test]
async fn a_target_takes_over_failure_classes_and_a_route_is_never_left_only_restricted() {
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
            (status, resp.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let post = |url: String, body: Value| send(reqwest::Method::POST, url, body);
    let put = |url: String, body: Value| send(reqwest::Method::PUT, url, body);
    let created = |(status, body): (u16, Value)| {
        assert_eq!(status, 200, "{body}");
        body["id"].as_str().expect("id").to_string()
    };

    let org = created(
        post(
            format!("{base}/api/v1/orgs"),
            json!({"name": "Fallback", "slug": "fallback"}),
        )
        .await,
    );
    let team = created(
        post(
            format!("{base}/api/v1/orgs/{org}/teams"),
            json!({"name": "core"}),
        )
        .await,
    );
    let project = created(
        post(
            format!("{base}/api/v1/teams/{team}/projects"),
            json!({"name": "app"}),
        )
        .await,
    );
    let provider = |name: &'static str| {
        post(
            format!("{base}/api/v1/orgs/{org}/providers"),
            json!({"name": name, "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
    };
    let cheap = created(provider("cheap").await);
    let premium = created(provider("premium").await);
    let route = created(
        post(
            format!("{base}/api/v1/projects/{project}/routes"),
            json!({"model": "guarded", "strategy": "round_robin"}),
        )
        .await,
    );
    let route_url = format!("{base}/api/v1/routes/{route}");

    // a restricted target cannot be a route's first
    let (status, body) = post(
        format!("{route_url}/targets"),
        json!({"provider_id": premium, "takes_over": ["refusal"]}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["field"], "takes_over", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("all_targets_restricted"),
        "{body}"
    );

    let ordinary = created(
        post(
            format!("{route_url}/targets"),
            json!({"provider_id": cheap}),
        )
        .await,
    );

    // an unknown class and a repeated one are refused by field
    for bad in [json!(["rate_limited"]), json!(["timeout", "timeout"])] {
        let (status, body) = post(
            format!("{route_url}/targets"),
            json!({"provider_id": premium, "takes_over": bad}),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["error"]["field"], "takes_over", "{body}");
    }

    let restricted = created(
        post(
            format!("{route_url}/targets"),
            json!({"provider_id": premium, "takes_over": ["refusal:content_policy_violation"]}),
        )
        .await,
    );
    let listed: Value = client
        .get(format!("{route_url}/targets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let by_id = |id: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_id(&ordinary)["takes_over"], json!([]));
    assert_eq!(
        by_id(&restricted)["takes_over"],
        json!(["refusal:content_policy_violation"])
    );

    // the snapshot the gateway polls carries the classes
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let served = snap["config"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["model"] == "guarded")
        .cloned()
        .expect("the route is served");
    let premium_target = served["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provider"] == "premium")
        .cloned()
        .unwrap();
    assert_eq!(
        premium_target["takes_over"],
        json!(["refusal:content_policy_violation"])
    );

    // a whole list of restricted targets is refused, and changes nothing
    let (status, body) = put(
        route_url.clone(),
        json!({"targets": [
            {"provider_id": cheap, "takes_over": ["throttled"]},
            {"provider_id": premium, "takes_over": ["refusal"]},
        ]}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["field"], "targets", "{body}");

    // a changed set is applied in place: the target keeps its id
    let (status, body) = put(
        route_url.clone(),
        json!({"targets": [
            {"provider_id": cheap},
            {"provider_id": premium, "takes_over": ["refusal", "server_error"]},
        ]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let listed: Value = client
        .get(format!("{route_url}/targets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let premium_row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["provider_id"] == premium)
        .cloned()
        .unwrap();
    assert_eq!(premium_row["id"], restricted);
    assert_eq!(
        premium_row["takes_over"],
        json!(["refusal", "server_error"])
    );

    // removing the last ordinary target would strand the route
    let (status, body) = send(
        reqwest::Method::DELETE,
        format!("{base}/api/v1/route-targets/{ordinary}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 409, "{body}");
    // the restricted one goes freely
    let (status, _) = send(
        reqwest::Method::DELETE,
        format!("{base}/api/v1/route-targets/{restricted}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 204);
}
