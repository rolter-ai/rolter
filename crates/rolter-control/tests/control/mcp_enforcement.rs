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
