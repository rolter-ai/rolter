//! Deployment settings, connectors, labels and the read-only operator endpoints.

use crate::support::*;
use serde_json::{json, Value};
use std::net::SocketAddr;

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
    let app = rolter_control::test_app_with_admin_token(pool, Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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

    // the options narrow the document, and a slug nobody has is a 400 (#3440)
    let scoped = client
        .get(format!(
            "{base}/api/v1/config/export?org=acme&without_person_caps=true"
        ))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert!(scoped.status().is_success(), "{}", scoped.status());
    let scoped = scoped.text().await.unwrap();
    assert!(scoped.contains("name = \"openai\""), "{scoped}");
    let unknown = client
        .get(format!("{base}/api/v1/config/export?org=nobody"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 400);
}

/// Two organizations that share a provider name cannot be exported as one
/// file (#3440). The refusal carries a stable code, so the dashboard can tell
/// the person to pick an organization in its own words (#3480), and
/// `?org=<slug>` exports each.
#[tokio::test]
async fn an_unscoped_export_of_organizations_sharing_names_answers_a_code() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool, Some(admin_token().to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    for slug in ["acme", "globex"] {
        let org: Value = client
            .post(format!("{base}/api/v1/orgs"))
            .bearer_auth(admin_token())
            .json(&json!({"name": slug, "slug": slug}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let org_id = org["id"].as_str().expect("org id");
        let created = client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .bearer_auth(admin_token())
            .json(&json!({
                "name": "openai",
                "kind": "openai",
                "api_base": "https://api.openai.com",
            }))
            .send()
            .await
            .unwrap();
        assert!(created.status().is_success(), "{}", created.status());
    }

    let refused = client
        .get(format!("{base}/api/v1/config/export"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "config_export_needs_org", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("provider 'openai'") && m.contains("--org")),
        "{body}"
    );

    for slug in ["acme", "globex"] {
        let one = client
            .get(format!("{base}/api/v1/config/export?org={slug}"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap();
        assert!(one.status().is_success(), "{slug}: {}", one.status());
    }
    // a bad slug is still a plain 400 with no code to branch on
    let unknown = client
        .get(format!("{base}/api/v1/config/export?org=nobody"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 400);
    let body: Value = unknown.json().await.unwrap();
    assert!(body["error"]["code"].is_null(), "{body}");
}

/// An org's provider that the bootstrap file repeats as a global row (#3516):
/// the unscoped export answers `config_export_needs_org`, as for two orgs
/// sharing a name, and `?org=` writes the org's own row.
#[tokio::test]
async fn an_unscoped_export_of_an_org_shadowing_a_bootstrap_row_answers_a_code() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let mut file = rolter_core::GatewayConfig::default();
    file.providers.push(rolter_core::ProviderConfig {
        name: "openai".to_string(),
        kind: rolter_core::ProviderKind::Openai,
        api_base: "https://file.example.com".to_string(),
        api_key_env: Some("OPENAI_API_KEY".to_string()),
        ..Default::default()
    });
    let app = rolter_control::test_app_with_file_config(
        pool.clone(),
        Some(admin_token().to_string()),
        file,
    )
    .await
    .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
        .json(&json!({"name": "acme", "slug": "acme"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org_id = org["id"].as_str().expect("org id");
    // written around the API, as the write-time guard refuses the name
    sqlx::query(
        "insert into providers (org_id, name, slug, kind, api_base) \
         values ($1::uuid, 'openai', 'openai', 'openai', 'https://org.example.com')",
    )
    .bind(org_id)
    .execute(&pool)
    .await
    .unwrap();

    let refused = client
        .get(format!("{base}/api/v1/config/export"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "config_export_needs_org", "{body}");

    let one = client
        .get(format!("{base}/api/v1/config/export?org=acme"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert!(one.status().is_success(), "{}", one.status());
    let document = one.text().await.unwrap();
    assert!(document.contains("https://org.example.com"), "{document}");
    assert!(!document.contains("https://file.example.com"), "{document}");
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
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
    let addr = serve_with_public_url(pool.clone(), Some(admin_token().to_string())).await;
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let unset = serve(app).await;
    let body: Value = client
        .get(format!("http://{unset}/api/v1/public-url"))
        .bearer_auth(admin_token())
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

/// `GET /api/v1/stability` is the dashboard's one source for the nav's
/// experimental markers (#1385): the least-privileged signed-in caller reads it
/// — a viewer sees the nav too — an anonymous one does not, and every row it
/// returns is an exception, because absence is what "stable" means.
#[tokio::test]
async fn stability_endpoint_lists_only_experimental_subsystems() {
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
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(as_admin.status(), 200);
}

/// Persisted feature flags are superadmin-only, survive reads, and write an
/// audit entry for each update.
#[tokio::test]
async fn feature_flags_are_superadmin_only_and_audited() {
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

    let denied = client
        .get(format!("{base}/api/v1/feature-flags"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/feature-flags"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // an anonymous poll stays out of the inventory: single-node deployments
    // need no cluster setup
    let anonymous = client
        .get(format!("{base}/internal/snapshot"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert!(anonymous.status().is_success());
    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
        .header("x-rolter-node-id", "gw-1")
        .header("x-rolter-node-role", "gateway")
        .header("x-rolter-node-build", "0.0.10")
        .send()
        .await
        .unwrap();
    assert!(identified.status().is_success());

    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
        .json(&json!({"draining": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);

    // with a second live gateway the drain is safe, and the draining node
    // learns about it on its next poll
    let second = client
        .get(format!("{base}/internal/snapshot?version=0"))
        .bearer_auth(admin_token())
        .header("x-rolter-node-id", "gw-2")
        .header("x-rolter-node-role", "gateway")
        .send()
        .await
        .unwrap();
    assert!(second.status().is_success());

    let drained: Value = client
        .put(format!("{base}/api/v1/cluster/nodes/gw-1/drain"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(forgotten_second.status(), 204);

    // a decommissioned node can be forgotten, and the action is audited
    let forgotten = client
        .delete(format!("{base}/api/v1/cluster/nodes/gw-1"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap();
    assert_eq!(forgotten.status(), 204);
    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
            .await
            .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // this deployment has no redis and no cache-publishing provider
    let view: Value = client
        .get(format!("{base}/api/v1/feature-flags"))
        .bearer_auth(admin_token())
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
                .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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

/// A control plane started with `--config` whose `[logging]` samples, with
/// nobody having saved Logs Settings, has gateways logging at the file's rate,
/// so the sampling read reports that rate rather than the seeded row's 100 %
/// (#3171). Once an admin saves the policy the stored rate wins, as it does in
/// the snapshot.
#[tokio::test]
async fn the_sampling_read_reports_the_file_rate_an_untouched_row_serves() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let mut file = rolter_core::GatewayConfig::default();
    file.logging.sample_rate = 0.25;
    let app = rolter_control::test_app_with_file_config(
        pool.clone(),
        Some(admin_token().to_string()),
        file,
    )
    .await
    .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let sampling = format!("{base}/api/v1/analytics/sampling");

    let from_file: Value = client
        .get(&sampling)
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(from_file, json!({"sample_rate": 0.25}));

    let updated = client
        .put(format!("{base}/api/v1/logging-settings"))
        .bearer_auth(admin_token())
        .json(&json!({
            "sample_rate": 0.5,
            "payload_capture_enabled": false,
            "payload_capture_max_bytes": 32768,
            "payload_capture_redact_fields": [],
            "payload_capture_models": [],
            "payload_capture_virtual_key_ids": [],
            "retention_days": 90,
            "payload_retention_hours": 168
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), 200);

    let stored: Value = client
        .get(&sampling)
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stored, json!({"sample_rate": 0.5}));
}

/// The analytics screens say their figures are estimates when the request log
/// is sampled, and every role that opens them must be able to learn the rate,
/// not only the superadmin who reads the logging policy (#3057).
#[tokio::test]
async fn every_analytics_reader_learns_the_sample_rate_and_nothing_else() {
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
    let sampling = format!("{base}/api/v1/analytics/sampling");

    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let viewer = seed_user(&pool, "viewer@sampling.test", false).await;
    seed_membership(&pool, viewer, Some(org), None, None, "viewer").await;
    let viewer_token = seed_session(&pool, viewer, "sampling_viewer").await;

    assert_eq!(
        client.get(&sampling).send().await.unwrap().status(),
        401,
        "an anonymous caller learns nothing"
    );

    let unsampled: Value = client
        .get(&sampling)
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unsampled, json!({"sample_rate": 1.0}));

    let updated = client
        .put(format!("{base}/api/v1/logging-settings"))
        .bearer_auth(admin_token())
        .json(&json!({
            "sample_rate": 0.25,
            "payload_capture_enabled": true,
            "payload_capture_max_bytes": 4096,
            "payload_capture_redact_fields": ["authorization"],
            "payload_capture_models": ["gpt-4o"],
            "payload_capture_virtual_key_ids": [],
            "retention_days": 30,
            "payload_retention_hours": 24
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), 200);

    // the viewer still cannot read the policy itself
    let policy = client
        .get(format!("{base}/api/v1/logging-settings"))
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap();
    assert_eq!(policy.status(), 403);

    // but learns the rate, and only the rate
    let sampled: Value = client
        .get(&sampling)
        .bearer_auth(&viewer_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sampled, json!({"sample_rate": 0.25}));

    let as_admin_token: Value = client
        .get(&sampling)
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(as_admin_token, json!({"sample_rate": 0.25}));
}

#[tokio::test]
async fn runtime_policy_is_superadmin_only_and_audited() {
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

    let denied = client
        .get(format!("{base}/api/v1/runtime-policy"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);

    let baseline: Value = client
        .get(format!("{base}/api/v1/runtime-policy"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
        .json(&json!({"anthropic_version": "latest", "default_max_tokens": 1024}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 400);

    let updated: Value = client
        .put(format!("{base}/api/v1/compatibility-policy"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 400);
    // a malformed identity is refused on the same grounds as a missing one
    let malformed = client
        .post(format!("{base}/internal/adaptive-telemetry"))
        .bearer_auth(admin_token())
        .header("x-rolter-node-id", "   ")
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
    let empty: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth(admin_token())
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
            .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
        .header("x-rolter-node-id", "gw-1")
        .json(&json!({"routes": []}))
        .send()
        .await
        .unwrap();
    assert!(emptied.status().is_success());
    let view: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let nodes = view["routes"][0]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["node_id"], "gw-2");

    // two orgs' `gpt-4o` are two routes (#3458): a report naming each with its
    // org stores both, and the next report that drops one org's route drops
    // that row alone
    let same_name = |orgs: &[&str]| {
        json!({"routes": orgs.iter().map(|org| json!({
            "org_id": org, "model": "gpt-4o", "engaged": *org == "org-a", "observed": 1
        })).collect::<Vec<_>>()})
    };
    for body in [same_name(&["org-a", "org-b"]), same_name(&["org-b"])] {
        let accepted = client
            .post(format!("{base}/internal/adaptive-telemetry"))
            .bearer_auth(admin_token())
            .header("x-rolter-node-id", "gw-3")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(accepted.status().is_success());
        let view: Value = client
            .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut routes: Vec<(String, String)> = view["routes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["nodes"][0]["node_id"] == "gw-3")
            .map(|r| {
                (
                    r["org_id"].as_str().unwrap().to_string(),
                    r["model"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        routes.sort();
        let expected: Vec<(String, String)> = body["routes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["org_id"].as_str().unwrap().to_string(),
                    "gpt-4o".to_string(),
                )
            })
            .collect();
        assert_eq!(routes, expected, "{view}");
    }

    // a sample older than the freshness window is not current state
    sqlx::query(
        "update adaptive_routing_telemetry set reported_at = now() - interval '10 minutes'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let stale: Value = client
        .get(format!("{base}/api/v1/adaptive-routing-telemetry"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stale["routes"].as_array().unwrap().len(), 0);
}

/// The two global catalogs are readable by any authenticated caller with no
/// membership anywhere, and refused outright without authentication (#766).
#[tokio::test]
async fn global_catalogs_take_authentication_but_no_membership() {
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
    let app =
        rolter_control::test_app_with_admin_token(pool.clone(), Some(admin_token().to_string()))
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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

/// #1968: connector bodies go through `SafeJson` like every other handler.
#[tokio::test]
async fn a_connector_name_with_a_nul_byte_is_rejected() {
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

    let resp = client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "bad\u{0}name",
            "kind": "otlp_http",
            "endpoint": "https://collector.example.com/v1/logs",
            "enabled": true,
            "sampling_rate": 1.0,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"]["message"].is_string(), "{body}");
}

#[tokio::test]
async fn collector_config_renders_a_managed_secret_as_a_bearer_header() {
    skip_without_db!();
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

    client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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

/// #2402: an update keeps, replaces or removes the stored bearer secret.
#[tokio::test]
async fn a_connector_update_keeps_replaces_or_clears_its_secret() {
    skip_without_db!();
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
    let (sink, _) = serve_capturing_sink().await;
    let created: Value = client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Sink",
            "kind": "otlp_http",
            "endpoint": format!("http://{sink}/v1/logs"),
            "enabled": true,
            "sampling_rate": 1.0,
            "managed_auth_secret": random_password(),
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let put = |secret: Option<Value>| {
        let client = client.clone();
        let url = format!("{base}/api/v1/connectors/{id}");
        let endpoint = format!("http://{sink}/v1/logs");
        async move {
            let mut body = json!({
                "name": "Sink",
                "kind": "otlp_http",
                "endpoint": endpoint,
                "enabled": true,
                "sampling_rate": 1.0,
            });
            if let Some(secret) = secret {
                body["managed_auth_secret"] = secret;
            }
            let response = client
                .put(url)
                .bearer_auth(admin_token())
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

    // absent keeps
    assert_eq!(put(None).await["auth_secret_configured"], true);
    assert_eq!(columns().await, (true, true));
    // a new value replaces
    let replaced = put(Some(random_password().into())).await;
    assert_eq!(replaced["auth_secret_configured"], true);
    // an explicit null removes both columns together
    let cleared = put(Some(Value::Null)).await;
    assert_eq!(cleared["auth_secret_configured"], false);
    assert_eq!(columns().await, (false, false));
    // a blank string removes too, and clearing nothing stays a no-op
    put(Some(random_password().into())).await;
    assert_eq!(
        put(Some("  ".into())).await["auth_secret_configured"],
        false
    );
    assert_eq!(columns().await, (false, false));
    assert_eq!(
        put(Some(Value::Null)).await["auth_secret_configured"],
        false
    );
}

/// #2403: a connector's stored secret belongs to the endpoint's origin.
#[tokio::test]
async fn a_connector_moved_to_another_origin_drops_its_secret_unless_given_a_new_one() {
    skip_without_db!();
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
    let (old_sink, _) = serve_capturing_sink().await;
    let (new_sink, seen) = serve_capturing_sink().await;
    let old_secret = random_password();
    let new_secret = random_password();

    let created: Value = client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth(admin_token())
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
                .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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
        .bearer_auth(admin_token())
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

/// #1810: drives the real route against a store that fails, so a handler that
/// echoes the driver error fails here; the unit test it replaces only
/// rendered an `ApiError` the test had built itself.
#[tokio::test]
async fn collector_config_redacts_a_failing_connector_query() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("sekrit".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    // the schema is private to this test; a missing table makes sqlx return a
    // driver error naming the relation
    sqlx::query("drop table observability_connectors cascade")
        .execute(&pool)
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/connectors/collector-config"))
        .bearer_auth("sekrit")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 500);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("failed to query observability connectors"),
        "{body}"
    );
    assert!(!body.contains("observability_connectors"), "{body}");
    assert!(!body.contains("does not exist"), "{body}");
}

/// #2404: health describes one endpoint and credential, so changing either
/// resets it to `unknown`; an unrelated edit keeps it.
#[tokio::test]
async fn a_connector_edit_resets_health_only_when_endpoint_or_secret_changes() {
    skip_without_db!();
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
    let (sink, _) = serve_capturing_sink().await;
    let (other_sink, _) = serve_capturing_sink().await;
    let endpoint = format!("http://{sink}/v1/logs");

    let created: Value = client
        .post(format!("{base}/api/v1/connectors"))
        .bearer_auth(admin_token())
        .json(&json!({
            "name": "Sink",
            "kind": "otlp_http",
            "endpoint": endpoint,
            "enabled": true,
            "sampling_rate": 1.0,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let put = |name: &'static str, endpoint: String, secret: Option<String>| {
        let client = client.clone();
        let url = format!("{base}/api/v1/connectors/{id}");
        async move {
            let mut body = json!({
                "name": name,
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
                .bearer_auth(admin_token())
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            response.json::<Value>().await.unwrap()
        }
    };
    let test = || async {
        let tested = client
            .post(format!("{base}/api/v1/connectors/{id}/test"))
            .bearer_auth(admin_token())
            .send()
            .await
            .unwrap();
        assert_eq!(tested.status(), 200);
    };
    let health = || async {
        sqlx::query_as::<_, (String, bool, bool)>(
            "select health_status, health_checked_at is not null, health_error is not null \
             from observability_connectors",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    test().await;
    assert_eq!(health().await, ("healthy".to_string(), true, false));

    // a rename and the same endpoint keep the result
    let body = put("Renamed", endpoint.clone(), None).await;
    assert_eq!(body["health_status"], "healthy");
    assert_eq!(health().await, ("healthy".to_string(), true, false));

    // a new secret on the same endpoint resets it
    put("Renamed", endpoint.clone(), Some(random_password())).await;
    assert_eq!(health().await, ("unknown".to_string(), false, false));

    // so does a new endpoint, error included
    test().await;
    assert_eq!(health().await.0, "healthy");
    let body = put("Renamed", format!("http://{other_sink}/v1/logs"), None).await;
    assert_eq!(body["health_status"], "unknown");
    assert!(body["health_checked_at"].is_null());
    assert_eq!(health().await, ("unknown".to_string(), false, false));
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

/// #2383: the SSO issuer, the guardrail webhook url and a plugin endpoint are
/// operator-written URLs the control plane or the gateway later fetches, so a
/// cloud-metadata address must be a 400 at save, naming the field.
#[tokio::test]
async fn operator_written_urls_the_egress_policy_denies_are_refused_at_save() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app_with_admin_token(
        db.pool().clone(),
        Some(admin_token().to_string()),
    )
    .await
    .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let denied = "https://169.254.169.254/latest/meta-data/";

    let org: Value = client
        .post(format!("{base}/api/v1/orgs"))
        .bearer_auth(admin_token())
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
            .bearer_auth(admin_token())
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
            .bearer_auth(admin_token())
            .json(&webhook(denied))
            .send()
            .await
            .unwrap(),
        "guardrail provider url",
    )
    .await;
    let ok = client
        .post(format!("{base}/api/v1/guardrails/providers"))
        .bearer_auth(admin_token())
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
            .bearer_auth(admin_token())
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
            .bearer_auth(admin_token())
            .json(&plugin(denied))
            .send()
            .await
            .unwrap(),
        "plugin endpoint",
    )
    .await;
    let ok = client
        .post(format!("{base}/api/v1/orgs/{org_id}/plugins"))
        .bearer_auth(admin_token())
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
            .bearer_auth(admin_token())
            .json(&plugin(denied))
            .send()
            .await
            .unwrap(),
        "plugin endpoint",
    )
    .await;
}

/// #3100: the budgets and rate limits of `GET /api/v1/config`, and the lines of
/// `GET /api/v1/config/problems`, carry other tenants' scope ids, limits and
/// provider names. A caller reads those of the organizations they hold a role
/// in, a superadmin reads all, and a lone org sees what it always did.
#[tokio::test]
async fn the_config_budgets_limits_and_problems_show_only_the_callers_organizations() {
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
    async fn get_as(client: &reqwest::Client, url: String, token: &str) -> Value {
        let response = client.get(url).bearer_auth(token).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        response.json().await.unwrap()
    }
    fn scope_ids(rows: &Value) -> Vec<String> {
        let mut out: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        out.sort();
        out
    }
    fn sorted(mut ids: Vec<String>) -> Vec<String> {
        ids.sort();
        ids
    }

    // per org: an org budget and limit, a project budget and limit, a provider
    // whose api_base is malformed, and a budget period nothing recognises
    struct Tenant {
        org: String,
        project: String,
        sentinel: String,
    }
    let mut tenants = Vec::new();
    for slug in ["limit-a", "limit-b"] {
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
        let project = post_as(
            &client,
            format!(
                "{base}/api/v1/teams/{}/projects",
                team["id"].as_str().unwrap()
            ),
            json!({"name": "main"}),
        )
        .await;
        let project_id = project["id"].as_str().unwrap().to_string();
        for (kind, scope_id) in [("org", &org_id), ("project", &project_id)] {
            post_as(
                &client,
                format!("{base}/api/v1/budgets"),
                json!({"scope_type": kind, "scope_id": scope_id, "limit_usd": "10"}),
            )
            .await;
            post_as(
                &client,
                format!("{base}/api/v1/rate-limits"),
                json!({"scope_type": kind, "scope_id": scope_id, "rpm": 60}),
            )
            .await;
        }
        let sentinel = format!("{slug}-upstream");
        post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/providers"),
            json!({"name": sentinel, "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
        .await;
        sqlx::query("update providers set api_base = 'not-a-url' where name = $1")
            .bind(&sentinel)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ('team', $1, 25, '7d')",
        )
        .bind(team["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
        tenants.push(Tenant {
            org: org_id,
            project: project_id,
            sentinel,
        });
    }
    let org_a: uuid::Uuid = tenants[0].org.parse().unwrap();
    let org_b: uuid::Uuid = tenants[1].org.parse().unwrap();
    let project_a: uuid::Uuid = tenants[0].project.parse().unwrap();

    let member_a = seed_user(&pool, "limit-member-a@example.com", false).await;
    seed_membership(&pool, member_a, Some(org_a), None, None, "viewer").await;
    let member_a_token = seed_session(&pool, member_a, "limitmembera").await;
    let member_b = seed_user(&pool, "limit-member-b@example.com", false).await;
    seed_membership(&pool, member_b, Some(org_b), None, None, "viewer").await;
    let member_b_token = seed_session(&pool, member_b, "limitmemberb").await;
    let project_member = seed_user(&pool, "limit-project@example.com", false).await;
    seed_membership(&pool, project_member, None, None, Some(project_a), "viewer").await;
    let project_token = seed_session(&pool, project_member, "limitproject").await;
    let nobody = seed_user(&pool, "limit-nobody@example.com", false).await;
    let nobody_token = seed_session(&pool, nobody, "limitnobody").await;

    let config_url = format!("{base}/api/v1/config");
    let problems_url = format!("{base}/api/v1/config/problems");
    let ids_of = |tenant: &Tenant| vec![tenant.org.clone(), tenant.project.clone()];

    // the superadmin sees both orgs: four budgets (org, project, and the team
    // one written by hand) per the two tenants, and every problem line
    let config = get_as(&client, config_url.clone(), admin_token()).await;
    assert_eq!(config["budgets"].as_array().unwrap().len(), 6, "{config}");
    assert_eq!(
        config["rate_limits"].as_array().unwrap().len(),
        4,
        "{config}"
    );
    let problems = get_as(&client, problems_url.clone(), admin_token()).await;
    let text = problems.to_string();
    for tenant in &tenants {
        assert!(text.contains(&tenant.sentinel), "{text}");
    }

    // (token, tenant it may read in full, tenant it must not read at all)
    for (token, mine, theirs) in [
        (&member_a_token, &tenants[0], &tenants[1]),
        (&member_b_token, &tenants[1], &tenants[0]),
    ] {
        let config = get_as(&client, config_url.clone(), token).await;
        // org + project + team budget; org + project limits
        assert_eq!(config["budgets"].as_array().unwrap().len(), 3, "{config}");
        let limits = scope_ids(&config["rate_limits"]);
        assert_eq!(limits, sorted(ids_of(mine)), "{config}");
        let problems = get_as(&client, problems_url.clone(), token).await;
        let text = format!("{config}{problems}");
        for id in ids_of(theirs) {
            assert!(!text.contains(&id), "{id} leaked: {text}");
        }
        assert!(!text.contains(&theirs.sentinel), "{text}");
        let lines = problems["problems"].as_array().unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.as_str().unwrap().contains(&mine.sentinel)),
            "the caller's own problems stay: {problems}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.as_str().unwrap().contains("period '7d'")),
            "{problems}"
        );
    }

    // a role on one project reads its own rows and the org and team above it,
    // which throttle its keys (#2527), and nothing of the other org
    let config = get_as(&client, config_url.clone(), &project_token).await;
    assert_eq!(
        scope_ids(&config["rate_limits"]),
        sorted(ids_of(&tenants[0])),
        "{config}"
    );
    assert_eq!(config["budgets"].as_array().unwrap().len(), 3, "{config}");
    let text = config.to_string();
    for id in ids_of(&tenants[1]) {
        assert!(!text.contains(&id), "{id} leaked: {text}");
    }
    let problems = get_as(&client, problems_url.clone(), &project_token).await;
    assert!(
        !problems.to_string().contains(&tenants[1].sentinel),
        "{problems}"
    );

    // no role anywhere, no tenant rows
    let config = get_as(&client, config_url, &nobody_token).await;
    assert_eq!(config["budgets"], json!([]), "{config}");
    assert_eq!(config["rate_limits"], json!([]), "{config}");
    let problems = get_as(&client, problems_url, &nobody_token).await;
    assert_eq!(problems["problems"], json!([]), "{problems}");
}

/// #3094: `/api/v1/models` and `/api/v1/config` list the merged store, which
/// holds every organization's rows. A caller sees the rows of the organizations
/// and projects they hold a role in, a superadmin sees all, and the
/// deployment-wide settings stay whole.
#[tokio::test]
async fn the_models_and_config_reads_show_only_the_callers_organizations() {
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
    async fn get_as(client: &reqwest::Client, url: String, token: &str) -> Value {
        let response = client.get(url).bearer_auth(token).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        response.json().await.unwrap()
    }
    fn names(rows: &Value, key: &str) -> Vec<String> {
        let mut out: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[key].as_str().unwrap().to_string())
            .collect();
        out.sort();
        out
    }

    // org A with two projects, org B with one; every org owns a provider and
    // each project a route pointing at it
    let mut orgs = Vec::new();
    for slug in ["scope-a", "scope-b"] {
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
        let provider = post_as(
            &client,
            format!("{base}/api/v1/orgs/{org_id}/providers"),
            json!({"name": format!("{slug}-upstream"), "kind": "openai_compatible", "api_base": "http://127.0.0.1:9"}),
        )
        .await;
        let provider_id = provider["id"].as_str().unwrap().to_string();
        let mut projects = Vec::new();
        let project_names: &[&str] = if slug == "scope-a" {
            &["one", "two"]
        } else {
            &["only"]
        };
        for name in project_names {
            let project = post_as(
                &client,
                format!("{base}/api/v1/teams/{team_id}/projects"),
                json!({"name": name}),
            )
            .await;
            let project_id = project["id"].as_str().unwrap().to_string();
            let route = post_as(
                &client,
                format!("{base}/api/v1/projects/{project_id}/routes"),
                json!({"model": format!("{slug}-{name}"), "strategy": "round_robin"}),
            )
            .await;
            post_as(
                &client,
                format!(
                    "{base}/api/v1/routes/{}/targets",
                    route["id"].as_str().unwrap()
                ),
                json!({"provider_id": provider_id, "upstream_model": "gpt-x", "weight": 1}),
            )
            .await;
            projects.push(project_id);
        }
        orgs.push((org_id, projects));
    }
    let org_a: uuid::Uuid = orgs[0].0.parse().unwrap();
    let org_b: uuid::Uuid = orgs[1].0.parse().unwrap();
    let project_a_one: uuid::Uuid = orgs[0].1[0].parse().unwrap();

    let member_a = seed_user(&pool, "scope-member-a@example.com", false).await;
    seed_membership(&pool, member_a, Some(org_a), None, None, "viewer").await;
    let member_a_token = seed_session(&pool, member_a, "scopemembera").await;
    let member_b = seed_user(&pool, "scope-member-b@example.com", false).await;
    seed_membership(&pool, member_b, Some(org_b), None, None, "viewer").await;
    let member_b_token = seed_session(&pool, member_b, "scopememberb").await;
    // a role on one project of org A shows that project's routes, not its sibling's
    let project_member = seed_user(&pool, "scope-project@example.com", false).await;
    seed_membership(
        &pool,
        project_member,
        None,
        None,
        Some(project_a_one),
        "viewer",
    )
    .await;
    let project_token = seed_session(&pool, project_member, "scopeproject").await;
    let nobody = seed_user(&pool, "scope-nobody@example.com", false).await;
    let nobody_token = seed_session(&pool, nobody, "scopenobody").await;

    let models_url = format!("{base}/api/v1/models");
    let config_url = format!("{base}/api/v1/config");

    let all_models = ["scope-a-one", "scope-a-two", "scope-b-only"];
    let all_providers = ["scope-a-upstream", "scope-b-upstream"];
    let models = get_as(&client, models_url.clone(), admin_token()).await;
    assert_eq!(names(&models, "model"), all_models);
    let config = get_as(&client, config_url.clone(), admin_token()).await;
    assert_eq!(names(&config["routes"], "model"), all_models);
    assert_eq!(names(&config["providers"], "name"), all_providers);

    // (token, models, providers) each caller may see
    let cases: [(&str, &[&str], &[&str]); 4] = [
        (
            &member_a_token,
            &["scope-a-one", "scope-a-two"],
            &["scope-a-upstream"],
        ),
        (&member_b_token, &["scope-b-only"], &["scope-b-upstream"]),
        // the project member holds no role at the org, so the org-wide provider
        // is not theirs; the route of their project is
        (&project_token, &["scope-a-one"], &["scope-a-upstream"]),
        (&nobody_token, &[], &[]),
    ];
    for (token, want_models, want_providers) in cases {
        let models = get_as(&client, models_url.clone(), token).await;
        assert_eq!(names(&models, "model"), want_models, "{models}");
        let config = get_as(&client, config_url.clone(), token).await;
        assert_eq!(names(&config["routes"], "model"), want_models, "{config}");
        assert_eq!(
            names(&config["providers"], "name"),
            want_providers,
            "{config}"
        );
        // another org's targets and ids appear nowhere in either body
        let text = format!("{models}{config}");
        for other in all_models.iter().filter(|m| !want_models.contains(m)) {
            assert!(!text.contains(other), "{other} leaked: {text}");
        }
    }
}

/// #3476: two organizations each serve a `gpt-4o`, and a label on one org's
/// model stays on it. `?org_id=` lists what applies to that org's models, its
/// own label before the deployment-wide one of the same key, and a member of
/// one org never reads the other's labels.
#[tokio::test]
async fn model_labels_name_the_organization_whose_model_they_are_on() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let admin = "model-labels-admin";
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some(admin.to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // two orgs, each with a project serving `gpt-4o`
    let mut orgs = Vec::new();
    for slug in ["acme", "globex"] {
        let org: uuid::Uuid =
            sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                .bind(slug)
                .fetch_one(&pool)
                .await
                .unwrap();
        let team: uuid::Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project: uuid::Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into routes (project_id, model, strategy) values ($1, 'gpt-4o', 'power_of_two')")
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        orgs.push(org);
    }
    let (acme, globex) = (orgs[0], orgs[1]);

    let create = |body: Value| {
        let client = client.clone();
        let url = format!("{base}/api/v1/model-labels");
        async move {
            client
                .post(url)
                .bearer_auth(admin)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    let deployment = create(json!({"model": "gpt-4o", "key": "tier", "value": "standard"})).await;
    assert_eq!(deployment.status(), 201);
    let deployment: Value = deployment.json().await.unwrap();
    assert!(deployment["org_id"].is_null(), "{deployment}");
    for (org, value) in [(acme, "gold"), (globex, "bronze")] {
        let own =
            create(json!({"model": "gpt-4o", "org_id": org, "key": "tier", "value": value})).await;
        assert_eq!(own.status(), 201, "a label on {org}'s gpt-4o");
        let own: Value = own.json().await.unwrap();
        assert_eq!(own["org_id"], org.to_string());
    }
    // each org's label is in that org's own audit log, the deployment's in none
    let audited: Vec<Option<uuid::Uuid>> = sqlx::query_scalar(
        "select org_id from audit_log where action = 'model_label.create' order by org_id nulls first",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut expected = vec![Some(acme), Some(globex)];
    expected.sort();
    expected.insert(0, None);
    assert_eq!(audited, expected);
    // the same key twice for one org is the 409 it always was
    let again = create(json!({"model": "gpt-4o", "org_id": acme, "key": "tier"})).await;
    assert_eq!(again.status(), 409);
    // and an org's label is on a model that org serves
    let unserved = create(json!({"model": "claude", "org_id": acme, "key": "tier"})).await;
    assert_eq!(unserved.status(), 404);

    let list = |token: String, query: String| {
        let client = client.clone();
        let url = format!("{base}/api/v1/model-labels{query}");
        async move {
            let response = client.get(url).bearer_auth(token).send().await.unwrap();
            assert_eq!(response.status(), 200);
            let rows: Value = response.json().await.unwrap();
            let mut pairs: Vec<(String, String)> = rows
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row["org_id"].as_str().unwrap_or("").to_string(),
                        row["value"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect();
            pairs.sort();
            pairs
        }
    };
    let sorted = |mut pairs: Vec<(String, String)>| {
        pairs.sort();
        pairs
    };

    // what applies to each org: its own tier, never the other's
    assert_eq!(
        list(admin.to_string(), format!("?org_id={acme}")).await,
        vec![(acme.to_string(), "gold".to_string())]
    );
    assert_eq!(
        list(
            admin.to_string(),
            format!("?org_id={globex}&subject_id=gpt-4o")
        )
        .await,
        vec![(globex.to_string(), "bronze".to_string())]
    );
    // the whole catalog, to a deployment admin, holds all three
    assert_eq!(
        list(admin.to_string(), String::new()).await,
        sorted(vec![
            (String::new(), "standard".to_string()),
            (acme.to_string(), "gold".to_string()),
            (globex.to_string(), "bronze".to_string()),
        ])
    );

    // a member of acme reads the deployment's label and acme's, not globex's
    let member = seed_user(&pool, "label-member@acme.test", false).await;
    seed_membership(&pool, member, Some(acme), None, None, "viewer").await;
    let member = seed_session(&pool, member, "model_labels_member").await;
    assert_eq!(
        list(member.clone(), String::new()).await,
        sorted(vec![
            (String::new(), "standard".to_string()),
            (acme.to_string(), "gold".to_string()),
        ])
    );
    assert!(
        list(member.clone(), format!("?org_id={globex}"))
            .await
            .is_empty(),
        "another org's labels stay hidden even when asked for by id"
    );
    // without an override of its own, an org reads the deployment's label
    sqlx::query("delete from labels where org_id = $1")
        .bind(acme)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        list(member, format!("?org_id={acme}")).await,
        vec![(String::new(), "standard".to_string())]
    );
}

/// #3520: a label on an organization's own model is that organization's act,
/// so its admin writes it under the org's `label` capability. The same admin
/// cannot label another org's model, and a deployment-wide label stays a
/// superadmin's.
#[tokio::test]
async fn an_org_admin_labels_its_own_orgs_models_and_nothing_else() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let superadmin = "model-labels-superadmin";
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some(superadmin.to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let mut orgs = Vec::new();
    for slug in ["acme", "globex"] {
        let org: uuid::Uuid =
            sqlx::query_scalar("insert into orgs (name, slug) values ($1, $1) returning id")
                .bind(slug)
                .fetch_one(&pool)
                .await
                .unwrap();
        let team: uuid::Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project: uuid::Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into routes (project_id, model, strategy) values ($1, 'gpt-4o', 'power_of_two')")
            .bind(project)
            .execute(&pool)
            .await
            .unwrap();
        orgs.push(org);
    }
    let (acme, globex) = (orgs[0], orgs[1]);
    let org_admin = seed_user(&pool, "label-admin@acme.test", false).await;
    seed_membership(&pool, org_admin, Some(acme), None, None, "admin").await;
    let org_admin = seed_session(&pool, org_admin, "model_labels_org_admin").await;

    let send = |method: reqwest::Method, path: String, token: String, body: Option<Value>| {
        let client = client.clone();
        let url = format!("{base}{path}");
        async move {
            let request = client.request(method, url).bearer_auth(token);
            match body {
                Some(body) => request.json(&body),
                None => request,
            }
            .send()
            .await
            .unwrap()
        }
    };
    let create = |token: String, body: Value| {
        send(
            reqwest::Method::POST,
            "/api/v1/model-labels".to_string(),
            token,
            Some(body),
        )
    };

    // the org admin labels acme's gpt-4o
    let own = create(
        org_admin.clone(),
        json!({"model": "gpt-4o", "org_id": acme, "key": "tier", "value": "gold"}),
    )
    .await;
    assert_eq!(own.status(), 201);
    let own: Value = own.json().await.unwrap();
    assert_eq!(own["org_id"], acme.to_string());
    // but not globex's, and not the deployment's
    let other = create(
        org_admin.clone(),
        json!({"model": "gpt-4o", "org_id": globex, "key": "tier"}),
    )
    .await;
    assert_eq!(other.status(), 403);
    let deployment = create(org_admin.clone(), json!({"model": "gpt-4o", "key": "tier"})).await;
    assert_eq!(deployment.status(), 403);

    // the same split holds for the labels a superadmin wrote
    let theirs: Value = create(
        superadmin.to_string(),
        json!({"model": "gpt-4o", "org_id": globex, "key": "tier"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let wide: Value = create(
        superadmin.to_string(),
        json!({"model": "gpt-4o", "key": "tier", "value": "standard"}),
    )
    .await
    .json()
    .await
    .unwrap();
    for label in [&theirs, &wide] {
        let path = format!("/api/v1/model-labels/{}", label["id"].as_str().unwrap());
        let update = send(
            reqwest::Method::PUT,
            path.clone(),
            org_admin.clone(),
            Some(json!({"value": "mine"})),
        )
        .await;
        assert_eq!(update.status(), 403, "{label}");
        let delete = send(reqwest::Method::DELETE, path, org_admin.clone(), None).await;
        assert_eq!(delete.status(), 403, "{label}");
    }

    // and the org's own label is the admin's to change and remove
    let path = format!("/api/v1/model-labels/{}", own["id"].as_str().unwrap());
    let update = send(
        reqwest::Method::PUT,
        path.clone(),
        org_admin.clone(),
        Some(json!({"value": "platinum"})),
    )
    .await;
    assert_eq!(update.status(), 200);
    let update: Value = update.json().await.unwrap();
    assert_eq!(update["value"], "platinum");
    let delete = send(reqwest::Method::DELETE, path, org_admin.clone(), None).await;
    assert_eq!(delete.status(), 204);
    let left: i64 = sqlx::query_scalar("select count(*) from labels where org_id = $1")
        .bind(acme)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
