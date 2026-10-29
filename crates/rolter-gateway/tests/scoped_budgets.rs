//! Budgets that scope to a credential or a person rather than a tenancy level.
//!
//! These need the Redis the counters live in (`ROLTER_TEST_REDIS_URL`, set in
//! CI) and skip without one. A budget of zero is a frozen scope, which is the
//! sharpest way to present "a request that exceeds the cap": the first request
//! is already over it, with no spend to arrange first.

use std::net::SocketAddr;

use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BalancingStrategy, BudgetConfig, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind,
    Target, VirtualKeyRecord,
};
use serde_json::{json, Value};

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn upstream() -> SocketAddr {
    serve(Router::new().route(
        "/v1/chat/completions",
        post(|_: Json<Value>| async {
            Json(json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "pong"},
                             "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }))
        }),
    ))
    .await
}

fn redis_url() -> Option<String> {
    std::env::var("ROLTER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

/// ids unique to one run, so a counter left in a shared Redis by an earlier
/// run cannot decide this one
fn unique(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    )
}

fn key_record(config: &GatewayConfig, plaintext: &str, id: &str, user: &str) -> VirtualKeyRecord {
    VirtualKeyRecord {
        key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), plaintext),
        id: id.to_string(),
        org_id: "org-1".to_string(),
        team_id: "team-1".to_string(),
        project_id: "project-1".to_string(),
        user_id: user.to_string(),
        ..Default::default()
    }
}

fn base_config(upstream: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "up".to_string(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: "test-model".to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "up".to_string(),
            ..Default::default()
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config
}

async fn gateway(config: &GatewayConfig, redis: &str) -> SocketAddr {
    let state = rolter_gateway::AppState::with_logging(config, Some(redis));
    serve(rolter_gateway::build_router(
        state,
        &config.server.metrics_path,
        config.server.max_body_bytes,
    ))
    .await
}

async fn chat(gw: SocketAddr, key: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{gw}/v1/chat/completions"))
        .bearer_auth(key)
        .json(&json!({"model": "test-model", "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap()
}

fn budget(scope: &str, id: &str, limit: u32) -> BudgetConfig {
    serde_json::from_value(json!({
        "scope": scope, "id": id, "limit_usd": limit, "period": "monthly"
    }))
    .unwrap()
}

/// #1171: the cap a key is minted with is enforced at the gateway. The capped
/// key is refused with a 402 that names it, and a sibling key in the same
/// project, with no cap, is untouched.
#[tokio::test]
async fn a_key_over_its_cap_is_refused_and_its_sibling_is_not() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let mut config = base_config(upstream().await);
    let capped = unique("vk-capped");
    let free = unique("vk-free");
    config
        .db_virtual_keys
        .push(key_record(&config, "sk-capped", &capped, ""));
    config
        .db_virtual_keys
        .push(key_record(&config, "sk-free", &free, ""));
    config.budgets.push(budget("key", &capped, 0));
    let gw = gateway(&config, &redis).await;

    let refused = chat(gw, "sk-capped").await;
    assert_eq!(refused.status(), 402);
    let body: Value = refused.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&capped), "{message}");
    assert_eq!(chat(gw, "sk-free").await.status(), 200);
}

/// #1830: a cap on a person counts every key they minted. Both of their keys
/// are refused, the 402 names the user budget as the one that tripped, and
/// another person's key in the same project is untouched.
#[tokio::test]
async fn a_user_cap_stops_every_key_the_person_minted() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let mut config = base_config(upstream().await);
    let user = unique("user-a");
    for (plain, id, owner) in [
        ("sk-a1", unique("vk-a1"), user.clone()),
        ("sk-a2", unique("vk-a2"), user.clone()),
        ("sk-b1", unique("vk-b1"), unique("user-b")),
    ] {
        config
            .db_virtual_keys
            .push(key_record(&config, plain, &id, &owner));
    }
    config.budgets.push(budget("user", &user, 0));
    let gw = gateway(&config, &redis).await;

    for key in ["sk-a1", "sk-a2"] {
        let refused = chat(gw, key).await;
        assert_eq!(refused.status(), 402, "{key}");
        let body: Value = refused.json().await.unwrap();
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("User") && message.contains(&user),
            "{message}"
        );
    }
    assert_eq!(chat(gw, "sk-b1").await.status(), 200);
}

/// A user cap narrowed to an org or a project counts only the keys the person
/// minted there: one narrowed to another org never stops them here, and one
/// narrowed to this project does.
#[tokio::test]
async fn a_narrowed_user_cap_counts_only_its_own_org_or_project() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let user = unique("user-n");
    let narrowed = |within: Value| -> BudgetConfig {
        serde_json::from_value(json!({
            "scope": "user", "id": user, "limit_usd": 0, "period": "monthly",
            "within": within,
        }))
        .unwrap()
    };
    // (the narrowing, whether the person's key in org-1/project-1 is stopped)
    for (within, stopped) in [
        (json!({"org": "org-2"}), false),
        (json!({"project": "project-2"}), false),
        (json!({"org": "org-1"}), true),
        (json!({"project": "project-1"}), true),
    ] {
        let mut config = base_config(upstream().await);
        let id = unique("vk-n");
        config
            .db_virtual_keys
            .push(key_record(&config, "sk-n", &id, &user));
        config.budgets.push(narrowed(within.clone()));
        let gw = gateway(&config, &redis).await;
        let expected = if stopped { 402 } else { 200 };
        assert_eq!(chat(gw, "sk-n").await.status(), expected, "{within}");
    }
}

/// #3470: a budget on a scope this gateway does not know, as an older control
/// plane now serves one it cannot read (or a newer one sends), is binding: it
/// counts every request, so a frozen cap refuses a key it does not name.
#[tokio::test]
async fn a_cap_on_an_unknown_scope_refuses_every_request() {
    let Some(redis) = redis_url() else {
        eprintln!("ROLTER_TEST_REDIS_URL unset; skipping");
        return;
    };
    let mut config = base_config(upstream().await);
    let key = unique("vk-anyone");
    config
        .db_virtual_keys
        .push(key_record(&config, "sk-anyone", &key, ""));
    let galaxy = budget("galaxy", &unique("galaxy"), 0);
    assert_eq!(galaxy.scope, rolter_core::BudgetScope::Unknown);
    config.budgets.push(galaxy);
    let gw = gateway(&config, &redis).await;
    assert_eq!(chat(gw, "sk-anyone").await.status(), 402);
}
