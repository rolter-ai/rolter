//! The gateway fleet roll-up every signed-in caller may read (#2013).

use crate::support::*;
use serde_json::Value;

async fn summary(client: &reqwest::Client, base: &str, token: Option<&str>) -> (u16, Value) {
    let mut request = client.get(format!("{base}/api/v1/cluster/summary"));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn poll(client: &reqwest::Client, base: &str, node: &str, version: i64) {
    let response = client
        .get(format!("{base}/internal/snapshot?version={version}"))
        .bearer_auth(admin_token())
        .header("x-rolter-node-id", node)
        .header("x-rolter-node-role", "gateway")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success() || response.status() == 304);
}

#[tokio::test]
async fn a_viewer_reads_counts_of_the_fleet_but_no_node_is_named() {
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

    // a signed-in caller with no membership anywhere
    let nobody = seed_user(&pool, "summary-nobody@example.com", false).await;
    let token = seed_session(&pool, nobody, "summarynobody").await;

    // anonymous callers are refused
    assert_eq!(summary(&client, &base, None).await.0, 401);

    // an empty inventory is a fleet of zero, and says it counted
    let (status, empty) = summary(&client, &base, Some(&token)).await;
    assert_eq!(status, 200, "{empty}");
    assert_eq!(empty["inventory"], true);
    assert_eq!(empty["gateways"]["live"], 0);

    // gw-1 applied the current version, gw-2 reports an older one
    let current = config_version(&pool).await;
    poll(&client, &base, "gw-1", current).await;
    poll(&client, &base, "gw-2", current - 1).await;
    let (_, view) = summary(&client, &base, Some(&token)).await;
    assert_eq!(view["gateways"]["live"], 2, "{view}");
    assert_eq!(view["gateways"]["lagging"], 1, "{view}");
    assert_eq!(view["gateways"]["stale"], 0, "{view}");
    assert_eq!(view["config_version"], current);
    assert!(view["checked_at"].is_string());
    // counts only: nothing in the body identifies a node or a build
    let body = view.to_string();
    assert!(!body.contains("gw-1") && !body.contains("gw-2"), "{body}");

    // a gateway that went quiet is stale until the horizon, then forgotten
    sqlx::query(
        "update cluster_nodes set last_seen_at = now() - interval '5 minutes' where id = 'gw-2'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, view) = summary(&client, &base, Some(&token)).await;
    assert_eq!(view["gateways"]["live"], 1, "{view}");
    assert_eq!(view["gateways"]["stale"], 1, "{view}");
    sqlx::query(
        "update cluster_nodes set last_seen_at = now() - interval '2 hours' where id = 'gw-2'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, view) = summary(&client, &base, Some(&token)).await;
    assert_eq!(view["gateways"]["stale"], 0, "{view}");

    // the node list itself is still superadmin-only
    let nodes = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(nodes.status(), 403);
}

/// #3351: the inventory records the snapshot formats each node states it
/// reads and the format it was last served. A poll with no `formats` is a
/// gateway from before negotiation, recorded as format 1; a refused poll keeps
/// the range it stated and is served none.
#[tokio::test]
async fn the_inventory_records_each_nodes_snapshot_format() {
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
    let current = config_version(&pool).await;
    let own = rolter_core::snapshot_format::SNAPSHOT_FORMAT;

    let poll_stating = |node: &'static str, formats: Option<String>| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let mut url = format!("{base}/internal/snapshot?version={current}");
            if let Some(formats) = formats {
                url.push_str(&format!("&formats={formats}"));
            }
            client
                .get(url)
                .bearer_auth(admin_token())
                .header("x-rolter-node-id", node)
                .header("x-rolter-node-role", "gateway")
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(poll_stating("gw-states", Some(own.to_string())).await, 304);
    assert_eq!(poll_stating("gw-silent", None).await, 304);
    let ahead = own + 7;
    assert_eq!(poll_stating("gw-ahead", Some(ahead.to_string())).await, 406);

    let nodes: Value = client
        .get(format!("{base}/api/v1/cluster/nodes"))
        .bearer_auth(admin_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let node = |id: &str| {
        nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap_or_else(|| panic!("{id} not in {nodes}"))
            .clone()
    };
    let states = node("gw-states");
    assert_eq!(states["formats_min"], own, "{states}");
    assert_eq!(states["formats_max"], own, "{states}");
    assert_eq!(states["served_format"], own, "{states}");
    assert_eq!(states["converged"], true, "{states}");
    let silent = node("gw-silent");
    assert_eq!(silent["formats_min"], 1, "{silent}");
    assert_eq!(silent["formats_max"], 1, "{silent}");
    assert_eq!(silent["served_format"], 1, "{silent}");
    let refused = node("gw-ahead");
    assert_eq!(refused["formats_min"], ahead, "{refused}");
    assert_eq!(refused["served_format"], Value::Null, "{refused}");

    // a statement that does not parse leaves the recorded range alone
    assert_eq!(poll_stating("gw-states", Some("x".to_string())).await, 400);
    let row: (i32, i32, Option<i32>) = sqlx::query_as(
        "select formats_min, formats_max, served_format from cluster_nodes where id = 'gw-states'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let own = i32::try_from(own).unwrap();
    assert_eq!(row, (own, own, None));
}
