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

/// #3334: an organization created now enforces its manifests until it saves
/// settings of its own, one that existed before keeps allowing unlisted tools,
/// and creating the organization writes no settings row.
#[tokio::test]
async fn a_new_organization_enforces_manifests_and_an_existing_one_does_not() {
    skip_without_db!();
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app(pool.clone()).await.unwrap();
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let mut orgs = Vec::new();
    for slug in ["fresh", "veteran"] {
        let org: Value = client
            .post(format!("{base}/api/v1/orgs"))
            .json(&json!({"name": slug, "slug": slug}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let org_id = org["id"].as_str().unwrap().to_string();
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/mcp-servers"))
            .json(&json!({
                "name": slug, "slug": slug, "url": "http://127.0.0.1:9/mcp",
                "tools": ["search"],
            }))
            .send()
            .await
            .unwrap();
        orgs.push(org_id);
    }
    // the second org stands for one that existed before the column did
    sqlx::query("update orgs set mcp_enforce_manifests = false where id = $1::uuid")
        .bind(&orgs[1])
        .execute(&pool)
        .await
        .unwrap();

    let settings = |org: &str| format!("{base}/api/v1/orgs/{org}/mcp/settings");
    let fresh: Value = client
        .get(settings(&orgs[0]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let veteran: Value = client
        .get(settings(&orgs[1]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(fresh["allow_unlisted_tools"], false);
    assert_eq!(veteran["allow_unlisted_tools"], true);
    // creating the org stored nothing, so the other defaults stay unpinned
    let rows: (i64,) = sqlx::query_as("select count(*) from mcp_gateway_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows.0, 0);

    let config = snapshot_config(&client, &base).await;
    let servers = config["mcp_servers"].as_array().unwrap();
    let flag = |org: &str| {
        servers
            .iter()
            .find(|server| server["org_id"] == org)
            .map(|server| server["enforce_manifest_by_default"].clone())
    };
    assert_eq!(flag(&orgs[0]), Some(json!(true)), "{config}");
    assert_eq!(flag(&orgs[1]), Some(json!(false)), "{config}");

    // changing the marker reaches the gateway
    let before = config_version(&pool).await;
    sqlx::query("update orgs set mcp_enforce_manifests = true where id = $1::uuid")
        .bind(&orgs[1])
        .execute(&pool)
        .await
        .unwrap();
    assert!(config_version(&pool).await > before);
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

    // a disabled group governs no one else, but its callers stay narrowed: it
    // travels marked disabled, with its assignments, so they are not handed the
    // organization-wide set back
    client
        .put(format!("{base}/api/v1/mcp/tool-groups/{group_id}"))
        .json(&json!({"name": "Readers", "enabled": false, "tools": group["tools"]}))
        .send()
        .await
        .unwrap();
    let config = snapshot_config(&client, &base).await;
    let carried = config["mcp_tool_groups"].as_array().unwrap();
    assert_eq!(carried.len(), 1, "{config}");
    assert_eq!(carried[0]["disabled"], json!(true));
    assert_eq!(carried[0]["assignments"].as_array().unwrap().len(), 3);

    // deleting a subject takes its assignments with it in the same
    // transaction (#3382), so the listing and the next snapshot lose it at
    // once, and naming the deleted key again is refused like any subject that
    // is not in the organization
    sqlx::query("delete from virtual_keys where id = $1")
        .bind(subjects[0].2)
        .execute(&pool)
        .await
        .unwrap();
    let swept: Value = client
        .get(format!(
            "{base}/api/v1/orgs/{org_id}/mcp/tool-group-assignments"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(swept.as_array().unwrap().len(), 2, "{swept}");
    let config = snapshot_config(&client, &base).await;
    assert_eq!(
        config["mcp_tool_groups"][0]["assignments"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "{config}"
    );
    let refused = client
        .put(&url)
        .json(&json!({"assignments": [
            {"kind": "team", "id": subjects[0].0},
            {"kind": "virtual_key", "id": subjects[0].2},
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);

    // a row that outlived its subject (one written around the delete, or from
    // before the sweep) is still dropped instead of blocking the save
    sqlx::query(
        "insert into mcp_tool_group_assignments (group_id, subject_kind, subject_id) \
         values ($1, 'virtual_key', $2)",
    )
    .bind(group_id.parse::<uuid::Uuid>().unwrap())
    .bind(subjects[0].2)
    .execute(&pool)
    .await
    .unwrap();
    let resaved: Value = client
        .put(&url)
        .json(&json!({"assignments": [
            {"kind": "team", "id": subjects[0].0},
            {"kind": "virtual_key", "id": subjects[0].2},
        ]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resaved.as_array().unwrap().len(), 1, "{resaved}");

    // a group created with assignments has them from its first snapshot, and a
    // subject outside the organization refuses the whole create
    let created: Value = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp/tool-groups"))
        .json(&json!({
            "name": "Narrow", "slug": "narrow",
            "tools": [{"server_id": server["id"], "tool": "fetch"}],
            "assignments": [{"kind": "team", "id": subjects[0].0}],
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let config = snapshot_config(&client, &base).await;
    let narrow = config["mcp_tool_groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == created["id"])
        .unwrap_or_else(|| panic!("{config}"));
    assert_eq!(narrow["assignments"].as_array().unwrap().len(), 1);
    let refused = client
        .post(format!("{base}/api/v1/orgs/{org_id}/mcp/tool-groups"))
        .json(&json!({
            "name": "Foreign", "slug": "foreign",
            "tools": [{"server_id": server["id"], "tool": "fetch"}],
            "assignments": [{"kind": "team", "id": subjects[1].0}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let groups: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/tool-groups"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        groups.as_array().unwrap().len(),
        2,
        "nothing of the refused create is left"
    );

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
    // the deprecated failure mode is kept and returned as saved (#3527) ...
    let saved: Value = saved.json().await.unwrap();
    assert_eq!(saved["default_failure_mode"], "fail_open", "{saved}");
    let read: Value = client
        .get(format!("{base}/api/v1/orgs/{org_id}/mcp/settings"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read["default_failure_mode"], "fail_open", "{read}");
    // ... but the snapshot always says fail closed, so a gateway that still
    // honours the field refuses an unreadable body during a rolling upgrade
    let config = snapshot_config(&client, &base).await;
    assert_eq!(
        config["mcp_settings"],
        json!([{
            "org_id": org_id,
            "connect_timeout_ms": 1500,
            "request_timeout_ms": 20000,
            "max_retries": 2,
            "fail_open": false,
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
