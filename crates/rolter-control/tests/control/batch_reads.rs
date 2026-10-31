//! Batch reads that replace one request per row on the dashboard (#1996).
//! Routes embed their targets and complexity policy
//! (`the_routes_list_embeds_targets_and_complexity_within_the_callers_scope`);
//! these cover the access-profile list.

use crate::support::*;
use serde_json::{json, Value};

async fn send(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: String,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = client.request(method, url).bearer_auth(admin_token());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_access_profile_list_embeds_what_the_detail_returns() {
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
    let post = reqwest::Method::POST;

    let (_, org) = send(
        &client,
        post.clone(),
        format!("{base}/api/v1/orgs"),
        Some(json!({"name": "batch", "slug": "batch"})),
    )
    .await;
    let org_id = org["id"].as_str().unwrap().to_string();
    let (_, role) = send(
        &client,
        post.clone(),
        format!("{base}/api/v1/orgs/{org_id}/custom-roles"),
        Some(json!({"name": "Wrangler", "base_role": "member"})),
    )
    .await;
    let role_id = role["id"].as_str().unwrap().to_string();
    for (name, with_role, policy) in [
        ("Composed", true, Some(json!({"allowed_models": ["gpt-*"]}))),
        ("Bare", false, None),
    ] {
        let mut body = json!({"name": name});
        if with_role {
            body["roles"] = json!([{"role_id": role_id, "org_id": org_id}]);
        }
        if let Some(policy) = policy {
            body["policy"] = policy;
        }
        let (status, created) = send(
            &client,
            post.clone(),
            format!("{base}/api/v1/orgs/{org_id}/access-profiles"),
            Some(body),
        )
        .await;
        assert_eq!(status, 200, "{created}");
    }

    let list_url = format!("{base}/api/v1/orgs/{org_id}/access-profiles");
    // a plain list is the body it always was: no embedded keys at all
    let (_, plain) = send(&client, reqwest::Method::GET, list_url.clone(), None).await;
    for profile in plain.as_array().unwrap() {
        assert!(profile.get("roles").is_none() && profile.get("policy").is_none());
    }

    // one request answers what N detail reads did
    let (status, embedded) = send(
        &client,
        reqwest::Method::GET,
        format!("{list_url}?include=roles,assignments,policy"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{embedded}");
    let embedded = embedded.as_array().unwrap();
    assert_eq!(embedded.len(), 2);
    for profile in embedded {
        let id = profile["id"].as_str().unwrap();
        let (_, detail) = send(
            &client,
            reqwest::Method::GET,
            format!("{base}/api/v1/access-profiles/{id}"),
            None,
        )
        .await;
        for field in ["roles", "assignments", "policy"] {
            assert_eq!(profile[field], detail[field], "{} {field}", profile["name"]);
        }
    }
    let composed = embedded.iter().find(|p| p["name"] == "Composed").unwrap();
    assert_eq!(composed["roles"].as_array().map(Vec::len), Some(1));
    assert_eq!(composed["policy"]["allowed_models"], json!(["gpt-*"]));
    let bare = embedded.iter().find(|p| p["name"] == "Bare").unwrap();
    assert_eq!(bare["roles"], json!([]));
    assert_eq!(
        bare["policy"],
        Value::Null,
        "no policy is null, as in the detail"
    );

    // each extra is asked for on its own, and an unknown one is refused
    let (_, only_roles) = send(
        &client,
        reqwest::Method::GET,
        format!("{list_url}?include=roles"),
        None,
    )
    .await;
    assert!(only_roles[0].get("policy").is_none(), "{only_roles}");
    let (status, refused) = send(
        &client,
        reqwest::Method::GET,
        format!("{list_url}?include=secrets"),
        None,
    )
    .await;
    assert_eq!(status, 400, "{refused}");
}
