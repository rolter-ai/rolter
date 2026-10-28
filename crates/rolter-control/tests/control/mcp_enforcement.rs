//! What the gateway needs to enforce MCP policy reaches it through the snapshot
//! (#1403, #1404): enabled tool groups and, once saved, the organization's MCP
//! settings, with every edit waking snapshot polling.

use serde_json::{json, Value};

use crate::support::*;

async fn snapshot_config(client: &reqwest::Client, base: &str) -> Value {
    let snap: Value = client
        .get(format!("{base}/internal/snapshot"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    snap["config"].clone()
}

#[tokio::test]
async fn enabled_tool_groups_reach_the_snapshot_and_every_edit_bumps_the_version() {
    skip_without_db!();
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
            "name": "Docs", "slug": "docs", "url": "http://127.0.0.1:9/mcp",
            "tools": ["search", "fetch"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let server_id = server["id"].as_str().unwrap().to_string();

    let before = config_version(&pool).await;
    let group: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp/tool-groups"))
        .json(&json!({
            "name": "Readers", "slug": "readers",
            "tools": [{"server_id": server_id, "tool": "search"}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let group_id = group["id"].as_str().unwrap().to_string();
    let after_create = config_version(&pool).await;
    assert!(
        after_create > before,
        "creating a group must bump the version"
    );

    let config = snapshot_config(&client, &base).await;
    let groups = config["mcp_tool_groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{config}");
    assert_eq!(groups[0]["org_id"], org_id);
    assert_eq!(
        groups[0]["tools"],
        json!([{"server_id": server_id, "tool": "search"}])
    );

    // a disabled group governs nothing, so it is not carried; the edit bumps
    let updated = client
        .put(format!("{base}/api/v1/mcp/tool-groups/{group_id}"))
        .json(&json!({"name": "Readers", "enabled": false, "tools": group["tools"]}))
        .send()
        .await
        .unwrap();
    assert!(updated.status().is_success());
    assert!(config_version(&pool).await > after_create);
    let config = snapshot_config(&client, &base).await;
    assert!(config["mcp_tool_groups"].as_array().unwrap().is_empty());

    // deleting one that was enabled again bumps too
    client
        .put(format!("{base}/api/v1/mcp/tool-groups/{group_id}"))
        .json(&json!({"name": "Readers", "enabled": true, "tools": group["tools"]}))
        .send()
        .await
        .unwrap();
    let before_delete = config_version(&pool).await;
    client
        .delete(format!("{base}/api/v1/mcp/tool-groups/{group_id}"))
        .send()
        .await
        .unwrap();
    assert!(config_version(&pool).await > before_delete);
    assert!(snapshot_config(&client, &base).await["mcp_tool_groups"]
        .as_array()
        .unwrap()
        .is_empty());
}

/// #3295: a group is assigned to a key, project or team of its own organization
/// only, the assignment reaches the snapshot with the group and bumps the
/// version, and a disabled group carries none.
#[tokio::test]
async fn tool_group_assignments_reach_the_snapshot_and_stay_inside_the_organization() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let mut org_ids = Vec::new();
    for slug in ["acme", "other"] {
        let org: Value = client
            .post(format!("{base}/api/v1/orgs"))
            .json(&json!({"name": slug, "slug": slug}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        org_ids.push(org["id"].as_str().unwrap().to_string());
    }
    let (org_id, other_org) = (&org_ids[0], &org_ids[1]);
    let mut subjects = Vec::new();
    for org in [org_id, other_org] {
        let team: (uuid::Uuid,) = sqlx::query_as(
            "insert into teams (org_id, name) values ($1::uuid, 'core') returning id",
        )
        .bind(org)
        .fetch_one(&pool)
        .await
        .unwrap();
        let project: (uuid::Uuid,) =
            sqlx::query_as("insert into projects (team_id, name) values ($1, 'api') returning id")
                .bind(team.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        let key: (uuid::Uuid,) = sqlx::query_as(
            "insert into virtual_keys (project_id, key_hash, key_prefix) \
             values ($1, gen_random_uuid()::text, 'sk-test') returning id",
        )
        .bind(project.0)
        .fetch_one(&pool)
        .await
        .unwrap();
        subjects.push((team.0, project.0, key.0));
    }
    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({
            "name": "Docs", "slug": "docs", "url": "http://127.0.0.1:9/mcp",
            "tools": ["search", "fetch"],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let group: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp/tool-groups"))
        .json(&json!({
            "name": "Readers", "slug": "readers",
            "tools": [{"server_id": server["id"], "tool": "search"}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let group_id = group["id"].as_str().unwrap().to_string();
    let url = format!("{base}/api/v1/mcp/tool-groups/{group_id}/assignments");

    // a subject of another organization is refused, and nothing is written
    let refused = client
        .put(&url)
        .json(&json!({"assignments": [{"kind": "team", "id": subjects[1].0}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let unknown_kind = client
        .put(&url)
        .json(&json!({"assignments": [{"kind": "user", "id": subjects[0].0}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_kind.status(), 400);

    let before = config_version(&pool).await;
    let saved: Value = client
        .put(&url)
        .json(&json!({"assignments": [
            {"kind": "team", "id": subjects[0].0},
            {"kind": "project", "id": subjects[0].1},
            {"kind": "virtual_key", "id": subjects[0].2},
            {"kind": "virtual_key", "id": subjects[0].2},
        ]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved.as_array().unwrap().len(), 3, "{saved}");
    assert!(
        config_version(&pool).await > before,
        "the edit bumps the version"
    );

    let listed: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{org_id}/mcp/tool-group-assignments"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 3);
    let other_listed: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{other_org}/mcp/tool-group-assignments"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(other_listed.as_array().unwrap().is_empty());

    let config = snapshot_config(&client, &base).await;
    let assignments = config["mcp_tool_groups"][0]["assignments"]
        .as_array()
        .unwrap();
    assert_eq!(assignments.len(), 3, "{config}");
    assert!(assignments.contains(&json!({"kind": "team", "id": subjects[0].0.to_string()})));

    // a disabled group governs nothing, so it carries no assignment either
    client
        .put(format!("{base}/api/v1/mcp/tool-groups/{group_id}"))
        .json(&json!({"name": "Readers", "enabled": false, "tools": group["tools"]}))
        .send()
        .await
        .unwrap();
    assert!(snapshot_config(&client, &base).await["mcp_tool_groups"]
        .as_array()
        .unwrap()
        .is_empty());

    // an empty set hands the group back to the whole organization
    let cleared: Value = client
        .put(&url)
        .json(&json!({"assignments": []}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cleared.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn saved_mcp_settings_reach_the_snapshot_and_set_a_new_servers_transport() {
    skip_without_db!();
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

    // never saved: the organization is absent and the gateway uses defaults
    assert!(snapshot_config(&client, &base).await["mcp_settings"]
        .as_array()
        .unwrap()
        .is_empty());

    let before = config_version(&pool).await;
    let saved = client
        .put(format!("{base}/api/v1/orgs/{org_id}/mcp/settings"))
        .json(&json!({
            "default_transport": "sse",
            "connect_timeout_ms": 1500,
            "request_timeout_ms": 20000,
            "max_retries": 2,
            "default_failure_mode": "fail_open",
            "allow_unlisted_tools": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(saved.status().is_success());
    assert!(config_version(&pool).await > before, "saving must bump");
    let config = snapshot_config(&client, &base).await;
    assert_eq!(
        config["mcp_settings"],
        json!([{
            "org_id": org_id,
            "connect_timeout_ms": 1500,
            "request_timeout_ms": 20000,
            "max_retries": 2,
            "fail_open": true,
            "allow_unlisted_tools": true,
        }])
    );

    // a server registered without a transport takes the saved default
    let server: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({"name": "Docs", "slug": "docs", "url": "http://127.0.0.1:9/mcp", "tools": ["a"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(server["transport"], "sse");
    // and the declared manifest travels with the server
    let config = snapshot_config(&client, &base).await;
    assert_eq!(config["mcp_servers"][0]["tools"], json!(["a"]));
    // one that names its own keeps it
    let explicit: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
        .json(&json!({"name": "Two", "slug": "two", "url": "http://127.0.0.1:9/mcp", "transport": "streamable_http"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(explicit["transport"], "streamable_http");
}
