//! Guardrail policy trials and the snapshot they feed.

use crate::support::*;
use serde_json::{json, Value};

/// Pausing a dashboard guardrail rule that a route still names in an override
/// must not turn every later snapshot into a 500: the override is pruned and
/// reported instead (#2306).
#[tokio::test]
async fn pausing_a_guardrail_rule_named_by_a_route_override_keeps_the_snapshot_served() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    async fn send(req: reqwest::RequestBuilder, what: &str) -> Value {
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let json: Value = resp.json().await.unwrap_or(Value::Null);
        assert!(status.is_success(), "{what} failed ({status}): {json}");
        json
    }
    let rule_body = |enabled: bool| {
        json!({
            "name": "no-secrets", "enabled": enabled, "source_type": "pattern",
            "pattern": "secret", "stage": "pre_call", "action": "block",
            "include_system": false, "position": 0
        })
    };

    let org = send(
        client
            .post(format!("{base}/api/v1/orgs"))
            .json(&json!({"name": "Acme", "slug": "acme"})),
        "org",
    )
    .await;
    let org_id = org["id"].as_str().expect("org id");
    let team = send(
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/teams"))
            .json(&json!({"name": "Platform"})),
        "team",
    )
    .await;
    let team_id = team["id"].as_str().expect("team id");
    let project = send(
        client
            .post(format!("{base}/api/v1/teams/{team_id}/projects"))
            .json(&json!({"name": "Gateway"})),
        "project",
    )
    .await;
    let project_id = project["id"].as_str().expect("project id");
    let provider = send(
        client
            .post(format!("{base}/api/v1/orgs/{org_id}/providers"))
            .json(
                &json!({"name": "openai", "kind": "openai", "api_base": "https://api.openai.com"}),
            ),
        "provider",
    )
    .await;
    let provider_id = provider["id"].as_str().expect("provider id");
    let route = send(
        client
            .post(format!("{base}/api/v1/projects/{project_id}/routes"))
            .json(&json!({"model": "gpt-4o", "strategy": "round_robin"})),
        "route",
    )
    .await;
    let route_id = route["id"].as_str().expect("route id");
    send(
        client
            .post(format!("{base}/api/v1/routes/{route_id}/targets"))
            .json(&json!({"provider_id": provider_id, "weight": 1})),
        "target",
    )
    .await;
    let rule = send(
        client
            .post(format!("{base}/api/v1/guardrails/rules"))
            .json(&rule_body(true)),
        "rule",
    )
    .await;
    let rule_id = rule["id"].as_str().expect("rule id");
    send(
        client
            .put(format!("{base}/api/v1/routes/{route_id}/advanced"))
            .json(&json!({"advanced": {"guardrails": {"disable": ["no-secrets"]}}})),
        "advanced",
    )
    .await;

    let snapshot = |client: reqwest::Client, base: String| async move {
        client
            .get(format!("{base}/internal/snapshot"))
            .send()
            .await
            .unwrap()
    };
    let live = snapshot(client.clone(), base.clone()).await;
    assert_eq!(live.status(), 200);
    let live: Value = live.json().await.unwrap();
    let overrides = live["config"]["routes"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["model"] == "gpt-4o"))
        .map(|r| r["advanced"]["guardrails"]["disable"].clone())
        .expect("route in snapshot");
    assert_eq!(
        overrides,
        json!(["no-secrets"]),
        "precondition: override served"
    );

    // pause the rule: it leaves the effective set while the override stays
    send(
        client
            .put(format!("{base}/api/v1/guardrails/rules/{rule_id}"))
            .json(&rule_body(false)),
        "pause",
    )
    .await;

    let paused = snapshot(client.clone(), base.clone()).await;
    assert_eq!(paused.status(), 200, "snapshot must survive the pause");
    let paused: Value = paused.json().await.unwrap();
    let route = paused["config"]["routes"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["model"] == "gpt-4o"))
        .expect("route still served");
    assert!(
        route["advanced"]["guardrails"]["disable"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "override pruned: {route}"
    );

    let problems: Value = send(
        client.get(format!("{base}/api/v1/config/problems")),
        "problems",
    )
    .await;
    let listed = problems["problems"].as_array().expect("problems array");
    assert!(
        listed.iter().any(|p| p
            .as_str()
            .is_some_and(|p| p.contains("gpt-4o") && p.contains("no-secrets"))),
        "pruned override must be reported: {problems}"
    );
}

/// The effective endpoint names the file as the source of a rule and a webhook
/// that are exact copies of an enabled dashboard row, which comparing fields
/// cannot (#2249).
#[tokio::test]
async fn effective_guardrails_name_the_file_as_the_source_of_an_identical_copy() {
    skip_without_db!();
    let db = fresh_db().await;
    let file_config = rolter_core::GatewayConfig::from_toml_str(
        r#"
        [guardrails]
        enabled = true
        [[guardrails.rules]]
        name = "email"
        builtin = "email"
        action = "redact"

        [guardrail_webhook]
        enabled = true
        url = "https://guard.internal/check"
        "#,
    )
    .expect("file config");
    let app = rolter_control::test_app_with_file_config(db.pool().clone(), None, file_config)
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let rule = client
        .post(format!("{base}/api/v1/guardrails/rules"))
        .json(&json!({
            "name": "email", "enabled": true, "source_type": "builtin", "builtin": "email",
            "stage": "pre_call", "action": "redact", "include_system": false, "position": 0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(rule.status(), 201);
    let rule: Value = rule.json().await.unwrap();
    let extra = client
        .post(format!("{base}/api/v1/guardrails/rules"))
        .json(&json!({
            "name": "phone", "enabled": true, "source_type": "builtin", "builtin": "phone",
            "stage": "pre_call", "action": "block", "include_system": false, "position": 1
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(extra.status(), 201);
    let extra: Value = extra.json().await.unwrap();
    let provider = client
        .post(format!("{base}/api/v1/guardrails/providers"))
        .json(&json!({
            "name": "hook", "enabled": true, "url": "https://guard.internal/check",
            "stage": "pre_call", "timeout_ms": 2000, "max_retries": 0,
            "failure_mode": "fail_open", "max_body_bytes": 65536, "auth_kind": "none"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(provider.status(), 201);
    let provider: Value = provider.json().await.unwrap();

    let effective: Value = client
        .get(format!("{base}/api/v1/guardrails/effective"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(effective["guardrails"], true, "{effective}");
    let rules = effective["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2, "{effective}");
    assert_eq!(rules[0]["name"], "email");
    assert_eq!(rules[0]["source"], "file");
    assert_eq!(rules[0]["overrides"], rule["id"], "{effective}");
    assert_eq!(rules[1]["name"], "phone");
    assert_eq!(rules[1]["source"], "registry");
    assert_eq!(rules[1]["rule_id"], extra["id"], "{effective}");
    assert_eq!(effective["source"], "file", "{effective}");
    // since #459 the file's webhook runs beside the registry's providers
    // rather than displacing one
    assert_eq!(effective["overrides"], Value::Null, "{effective}");
    assert_eq!(effective["webhook"]["url"], "https://guard.internal/check");
    let running = effective["semantic"]["providers"]
        .as_array()
        .expect("providers");
    assert_eq!(running.len(), 2, "{effective}");
    assert_eq!(running[0]["source"], "file");
    assert_eq!(running[1]["source"], "registry");
    assert_eq!(running[1]["provider_id"], provider["id"], "{effective}");
}

/// Scoped policies (#459): several providers can be enabled at once, a policy
/// is validated and stored with its ordered providers, one enabled policy per
/// scope, the snapshot carries it, and a listed provider cannot be deleted.
#[tokio::test]
async fn scoped_policies_select_several_enabled_providers() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let provider = |name: &str, stage: &str| {
        json!({
            "name": name, "enabled": true, "url": format!("https://{name}.internal/check"),
            "stage": stage, "timeout_ms": 500, "max_retries": 1,
            "failure_mode": "fail_closed", "max_body_bytes": 65536, "auth_kind": "none"
        })
    };
    let mut ids = Vec::new();
    for (name, stage) in [("injection", "pre_call"), ("toxicity", "post_call")] {
        let created = client
            .post(format!("{base}/api/v1/guardrails/providers"))
            .json(&provider(name, stage))
            .send()
            .await
            .unwrap();
        assert_eq!(created.status(), 201);
        let created: Value = created.json().await.unwrap();
        ids.push(created["id"].as_str().unwrap().to_string());
    }
    // both stay enabled: saving one no longer pauses the other
    let listed: Value = client
        .get(format!("{base}/api/v1/guardrails/providers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["enabled"] == true),
        "{listed}"
    );

    let policy = |name: &str, scope_id: &str, thresholds: Value| {
        json!({
            "name": name, "enabled": true, "scope_kind": "project", "scope_id": scope_id,
            "provider_ids": [ids[1], ids[0]], "thresholds": thresholds,
            "on_violation": "annotate", "streaming": "buffer"
        })
    };
    let bad = client
        .post(format!("{base}/api/v1/guardrails/policies"))
        .json(&policy("support", "p-1", json!({"violence": 1.5})))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    let created = client
        .post(format!("{base}/api/v1/guardrails/policies"))
        .json(&policy(
            "support",
            "p-1",
            json!({"violence": 0.8, "*": 0.95}),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    // the order the policy gave is the order it keeps
    assert_eq!(
        created["provider_ids"],
        json!([ids[1], ids[0]]),
        "{created}"
    );
    let clash = client
        .post(format!("{base}/api/v1/guardrails/policies"))
        .json(&policy("support-2", "p-1", json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(clash.status(), 409, "one enabled policy per scope");

    let snapshot = rolter_store::postgres::PostgresConfigStore::new(db.pool().clone());
    let config = rolter_store::ConfigStore::load(&snapshot).await.unwrap();
    assert_eq!(config.semantic_guards.providers.len(), 2);
    let stored = &config.semantic_guards.policies[0];
    assert_eq!(stored.providers, ["toxicity", "injection"]);
    assert_eq!(stored.thresholds.get("violence"), Some(&0.8));
    let problems = config.semantic_guards.validate(&config.guardrail_webhook);
    assert!(problems.is_empty(), "the snapshot must load: {problems:?}");

    let refused = client
        .delete(format!("{base}/api/v1/guardrails/providers/{}", ids[0]))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409, "a listed provider cannot be deleted");
    let deleted = client
        .delete(format!(
            "{base}/api/v1/guardrails/policies/{}",
            created["id"].as_str().unwrap()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    let freed = client
        .delete(format!("{base}/api/v1/guardrails/providers/{}", ids[0]))
        .send()
        .await
        .unwrap();
    assert_eq!(freed.status(), 204);
}

/// A webhook double: answers every POST with `answer` (after `delay`) and hands
/// the headers and body of the call it saw to the test.
async fn guardrail_webhook_double(
    answer: Value,
    delay: std::time::Duration,
    seen: std::sync::Arc<std::sync::Mutex<Vec<(axum::http::HeaderMap, Value)>>>,
) -> String {
    use axum::routing::post;
    let app = axum::Router::new().route(
        "/check",
        post(
            move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                let answer = answer.clone();
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push((headers, body));
                    tokio::time::sleep(delay).await;
                    axum::Json(answer)
                }
            },
        ),
    );
    let addr = serve(app).await;
    format!("http://{addr}/check")
}

async fn post_json(base: &str, path: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// #2159: an unsaved rule is run against a sample with the gateway's matcher,
/// and a pattern the gateway would refuse comes back as a 400 on `pattern`.
#[tokio::test]
async fn an_unsaved_guardrail_rule_is_tried_against_a_sample() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let base = format!("http://{}", serve(app).await);
    let path = "/api/v1/guardrails/rules/test";

    let (status, run) = post_json(
        &base,
        path,
        json!({"builtin": "email", "action": "redact", "sample": "write to a@b.io now"}),
    )
    .await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["outcome"], "redacted");
    assert_eq!(run["match_count"], 1);
    assert_eq!(run["spans"], json!([{"start": 9, "end": 15}]));
    assert_eq!(run["redacted"], "write to [REDACTED:EMAIL] now");

    let (_, run) = post_json(
        &base,
        path,
        json!({"pattern": "secret", "action": "block", "sample": "a secret"}),
    )
    .await;
    assert_eq!(run["outcome"], "blocked");

    let (_, run) = post_json(
        &base,
        path,
        json!({"pattern": "secret", "action": "block", "sample": "nothing"}),
    )
    .await;
    assert_eq!(run["outcome"], "no_match");

    let (status, err) = post_json(
        &base,
        path,
        json!({"pattern": "[", "action": "block", "sample": "x"}),
    )
    .await;
    assert_eq!(status, 400, "{err}");
    assert!(err.to_string().contains("pattern"), "{err}");

    let (status, _) = post_json(
        &base,
        path,
        json!({"pattern": "x", "sample": "x".repeat(70_000)}),
    )
    .await;
    assert_eq!(status, 400, "an oversized sample is refused");

    // nothing was written
    let rules: Value = reqwest::get(format!("{base}/api/v1/guardrails/rules"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rules, json!([]));
}

/// #3084: the whole effective rule set runs in order on a sample, config-file
/// rules first, a draft slotted at its position, and nothing is written.
#[tokio::test]
async fn the_guardrail_policy_is_tried_in_order_on_a_sample() {
    skip_without_db!();
    let db = fresh_db().await;
    let file_config = rolter_core::GatewayConfig::from_toml_str(
        r#"
        [guardrails]
        enabled = true
        [[guardrails.rules]]
        name = "file-pager"
        pattern = "PAGER"
        action = "annotate"
        "#,
    )
    .expect("file config");
    let app = rolter_control::test_app_with_file_config(db.pool().clone(), None, file_config)
        .await
        .expect("build app");
    let base = format!("http://{}", serve(app).await);
    let client = reqwest::Client::new();
    let dashboard = |name: &str, position: i32, action: &str, enabled: bool| {
        json!({"name": name, "enabled": enabled, "source_type": "builtin",
               "builtin": "email", "stage": "pre_call", "action": action,
               "replacement": null, "include_system": false, "position": position})
    };
    for body in [
        dashboard("mask", 10, "redact", true),
        dashboard("deny", 20, "block", true),
        dashboard("off", 15, "block", false),
    ] {
        let resp = client
            .post(format!("{base}/api/v1/guardrails/rules"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201, "{}", resp.text().await.unwrap());
    }
    let path = "/api/v1/guardrails/policy/test";
    let names = |run: &Value| -> Vec<String> {
        run["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["rule"].as_str().unwrap().to_string())
            .collect()
    };

    // the redaction hides the address from the block rule behind it, and the
    // disabled dashboard rule is not part of the policy
    let (status, run) = post_json(&base, path, json!({"sample": "PAGER a@b.io"})).await;
    assert_eq!(status, 200, "{run}");
    assert_eq!(names(&run), ["file-pager", "mask", "deny"]);
    assert_eq!(run["outcome"], "redacted");
    assert_eq!(run["redacted"], "PAGER [REDACTED:EMAIL]");
    assert_eq!(run["steps"][0]["source"], "file");
    assert_eq!(run["steps"][0]["outcome"], "annotated");
    assert_eq!(run["steps"][1]["source"], "dashboard");
    assert_eq!(run["steps"][1]["outcome"], "redacted");
    assert_eq!(run["steps"][2]["outcome"], "no_match");

    // a draft block ahead of the redaction sees the address and ends the scan
    let (_, run) = post_json(
        &base,
        path,
        json!({"sample": "a@b.io", "draft": {"name": "early", "position": 5,
               "builtin": "email", "action": "block"}}),
    )
    .await;
    assert_eq!(names(&run), ["file-pager", "early", "mask", "deny"]);
    assert_eq!(run["outcome"], "blocked");
    assert_eq!(run["blocked_by"], "early");
    assert_eq!(run["steps"][1]["source"], "draft");
    assert_eq!(run["steps"][2]["skip"], "after_block");

    // a draft carrying a saved rule's name replaces it
    let (_, run) = post_json(
        &base,
        path,
        json!({"sample": "a@b.io", "draft": {"name": "mask", "position": 10,
               "builtin": "email", "action": "annotate"}}),
    )
    .await;
    assert_eq!(names(&run), ["file-pager", "mask", "deny"]);
    assert_eq!(run["outcome"], "blocked");
    assert_eq!(run["steps"][1]["source"], "draft");

    // a file rule's name wins, a bad draft and an oversized sample are 400s
    let (status, _) = post_json(
        &base,
        path,
        json!({"sample": "x", "draft": {"name": "file-pager", "pattern": "x"}}),
    )
    .await;
    assert_eq!(status, 400);
    let (status, err) = post_json(
        &base,
        path,
        json!({"sample": "x", "draft": {"pattern": "["}}),
    )
    .await;
    assert_eq!(status, 400, "{err}");
    assert!(err.to_string().contains("pattern"), "{err}");
    let (status, _) = post_json(&base, path, json!({"sample": "x".repeat(70_000)})).await;
    assert_eq!(status, 400);

    // nothing was written
    let rules: Value = reqwest::get(format!("{base}/api/v1/guardrails/rules"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rules.as_array().unwrap().len(), 3);
}

/// #2265: an unsaved provider is sent the gateway's envelope and the answer is
/// reported as a result; failures are results, not 5xx.
#[tokio::test]
async fn an_unsaved_guardrail_provider_is_tried_with_a_sample_envelope() {
    skip_without_db!();
    let (app, _db) = fresh_app().await;
    let base = format!("http://{}", serve(app).await);
    let path = "/api/v1/guardrails/providers/test";
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let none = std::time::Duration::ZERO;

    // pass, with a credential read from an environment variable that is set
    let allow = guardrail_webhook_double(json!({"action": "allow"}), none, seen.clone()).await;
    let (status, out) = post_json(
        &base,
        path,
        json!({"url": allow, "auth_kind": "bearer", "auth_env": "PATH",
               "sample": "hello there"}),
    )
    .await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["decision"]["action"], "allow");
    assert_eq!(out["credential"], "sent");
    assert_eq!(out["http_status"], 200);
    let (headers, envelope) = seen.lock().unwrap().remove(0);
    assert!(headers["authorization"]
        .to_str()
        .unwrap()
        .starts_with("Bearer "));
    assert_eq!(envelope["direction"], "request");
    assert_eq!(envelope["stage"], "pre_call");
    assert_eq!(envelope["content"]["messages"][0]["content"], "hello there");
    // the answer never echoes the credential
    assert!(!out.to_string().contains(&std::env::var("PATH").unwrap()));

    // block carries its reason
    let block = guardrail_webhook_double(
        json!({"action": "block", "reason": "pii detected"}),
        none,
        seen.clone(),
    )
    .await;
    let (_, out) = post_json(&base, path, json!({"url": block})).await;
    assert_eq!(out["decision"]["action"], "block");
    assert_eq!(out["decision"]["reason"], "pii detected");

    // a slow webhook is a timeout result
    let slow = guardrail_webhook_double(
        json!({"action": "allow"}),
        std::time::Duration::from_secs(3),
        seen.clone(),
    )
    .await;
    let (status, out) = post_json(&base, path, json!({"url": slow, "timeout_ms": 200})).await;
    assert_eq!(status, 200);
    assert_eq!(out["ok"], false);
    assert_eq!(out["error"]["kind"], "timeout", "{out}");

    // an answer that is not a decision is reported, not read as allow
    let junk = guardrail_webhook_double(json!("fine"), none, seen.clone()).await;
    let (_, out) = post_json(&base, path, json!({"url": junk})).await;
    assert_eq!(out["error"]["kind"], "unparsable", "{out}");

    // refused egress: the metadata address is never contacted
    let (status, out) = post_json(
        &base,
        path,
        json!({"url": "http://169.254.169.254/latest/meta-data"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(out["error"]["kind"], "egress_denied", "{out}");

    // a credential variable the control plane does not have: not contacted
    let before = seen.lock().unwrap().len();
    let (_, out) = post_json(
        &base,
        path,
        json!({"url": allow, "auth_kind": "bearer",
               "auth_env": "TEST_GUARDRAIL_TOKEN_NOT_SET"}),
    )
    .await;
    assert_eq!(out["error"]["kind"], "credential_unset", "{out}");
    assert_eq!(out["credential"], "unset");
    assert!(out["error"]["message"]
        .as_str()
        .unwrap()
        .contains("TEST_GUARDRAIL_TOKEN_NOT_SET is not set on the control plane"));
    assert_eq!(
        seen.lock().unwrap().len(),
        before,
        "the webhook was not called"
    );

    // ... unless the caller asks to leave the credential out
    let (_, out) = post_json(
        &base,
        path,
        json!({"url": allow, "auth_kind": "shared_secret",
               "auth_env": "TEST_GUARDRAIL_TOKEN_NOT_SET", "skip_credential": true}),
    )
    .await;
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["credential"], "skipped");

    // nothing listening
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    };
    let (_, out) = post_json(
        &base,
        path,
        json!({"url": format!("http://{closed}/check")}),
    )
    .await;
    assert_eq!(out["error"]["kind"], "connection", "{out}");

    // the control plane's own secrets cannot be named, and bad drafts are 400s
    let (status, _) = post_json(
        &base,
        path,
        json!({"url": allow, "auth_kind": "bearer", "auth_env": "ROLTER_KEK"}),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = post_json(&base, path, json!({"url": "ftp://nope"})).await;
    assert_eq!(status, 400);

    // an oversized sample is cut to max_body_bytes, as the gateway cuts it
    let (_, out) = post_json(
        &base,
        path,
        json!({"url": allow, "max_body_bytes": 64, "sample": "y".repeat(500)}),
    )
    .await;
    assert_eq!(out["truncated"], true, "{out}");
}

#[tokio::test]
async fn a_local_provider_is_stored_validated_and_reaches_the_snapshot() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/guardrails/providers");
    let digest = "ab".repeat(32);
    let local = json!({
        "name": "injection", "enabled": true, "kind": "local", "url": "",
        "model_path": "/etc/rolter/models/injection.json", "model_sha256": digest,
        "stage": "pre_call", "timeout_ms": 50, "max_retries": 0,
        "failure_mode": "fail_closed", "max_body_bytes": 65536, "auth_kind": "none"
    });

    // a local provider with a url, or without its pin, is refused by field
    for (field, value) in [
        ("url", json!("https://x.internal")),
        ("model_sha256", json!(null)),
    ] {
        let mut bad = local.clone();
        bad[field] = value;
        let refused = client.post(&url).json(&bad).send().await.unwrap();
        assert_eq!(refused.status(), 400, "{field}");
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["error"]["field"], field, "{body}");
    }

    let created = client.post(&url).json(&local).send().await.unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["kind"], "local");
    assert_eq!(created["url"], "");
    assert_eq!(created["model_sha256"], digest);

    // a webhook saved beside it keeps working, and carries no model fields
    let webhook = client
        .post(&url)
        .json(&json!({
            "name": "toxicity", "enabled": true, "url": "https://tox.internal/check",
            "stage": "post_call", "timeout_ms": 500, "max_retries": 0,
            "failure_mode": "fail_open", "max_body_bytes": 65536, "auth_kind": "none"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(webhook.status(), 201);
    let webhook: Value = webhook.json().await.unwrap();
    assert_eq!(webhook["kind"], "webhook");
    assert_eq!(webhook["model_path"], Value::Null);

    let store = rolter_store::postgres::PostgresConfigStore::new(db.pool().clone());
    let config = rolter_store::ConfigStore::load(&store).await.unwrap();
    let loaded = config
        .semantic_guards
        .providers
        .iter()
        .find(|p| p.name == "injection")
        .expect("the local provider is in the snapshot");
    assert_eq!(loaded.kind, rolter_core::GuardProviderKind::Local);
    assert_eq!(loaded.model_path, "/etc/rolter/models/injection.json");
    assert_eq!(loaded.model_sha256, digest);
    assert!(config.validate_snapshot().is_ok());

    // the schema refuses what the API refuses, for rows written around it
    let raw = sqlx::query(
        "insert into guardrail_providers (name, kind, url, model_path, model_sha256) \
         values ('raw', 'local', 'https://x.internal', '/m.json', $1)",
    )
    .bind(&digest)
    .execute(db.pool())
    .await;
    assert!(raw.is_err(), "a local row with a url breaks the kind check");
}

#[tokio::test]
async fn local_truncation_settings_round_trip_and_bump_the_snapshot_version() {
    skip_without_db!();
    let db = fresh_db().await;
    let app = rolter_control::test_app(db.pool().clone())
        .await
        .expect("build app");
    let addr = serve(app).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/guardrails/providers");
    let local = json!({
        "name": "injection", "enabled": true, "kind": "local", "url": "",
        "model_path": "/etc/rolter/models/injection.json", "model_sha256": "ab".repeat(32),
        "stage": "pre_call", "timeout_ms": 50, "max_retries": 0,
        "failure_mode": "fail_closed", "max_body_bytes": 65536,
        "max_scored_bytes": 1048576, "on_truncated": "fail_closed", "auth_kind": "none"
    });

    let before = config_version(db.pool()).await;
    let created = client.post(&url).json(&local).send().await.unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["max_scored_bytes"], 1_048_576);
    assert_eq!(created["on_truncated"], "fail_closed");
    assert_eq!(config_version(db.pool()).await, before + 1);

    let provider_id = created["id"].as_str().expect("provider id");
    let updated = client
        .put(format!("{url}/{provider_id}"))
        .json(&json!({
            "name": "injection", "enabled": true, "kind": "local", "url": "",
            "model_path": "/etc/rolter/models/injection.json", "model_sha256": "ab".repeat(32),
            "stage": "pre_call", "timeout_ms": 50, "max_retries": 0,
            "failure_mode": "fail_closed", "max_body_bytes": 65536,
            "max_scored_bytes": 2097152, "on_truncated": "score", "auth_kind": "none"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), 200);
    let updated: Value = updated.json().await.unwrap();
    assert_eq!(updated["max_scored_bytes"], 2_097_152);
    assert_eq!(updated["on_truncated"], "score");
    assert_eq!(config_version(db.pool()).await, before + 2);

    let store = rolter_store::postgres::PostgresConfigStore::new(db.pool().clone());
    let config = rolter_store::ConfigStore::load(&store).await.unwrap();
    let loaded = config
        .semantic_guards
        .providers
        .iter()
        .find(|provider| provider.name == "injection")
        .expect("the local provider reaches the snapshot");
    assert_eq!(loaded.max_scored_bytes, Some(2_097_152));
    assert_eq!(loaded.on_truncated, rolter_core::OnTruncated::Score);

    let mut cleared = local.clone();
    cleared["max_scored_bytes"] = Value::Null;
    cleared["on_truncated"] = json!("score");
    let cleared_response = client
        .put(format!("{url}/{provider_id}"))
        .json(&cleared)
        .send()
        .await
        .unwrap();
    assert_eq!(cleared_response.status(), 200);
    assert_eq!(config_version(db.pool()).await, before + 3);
    let config = rolter_store::ConfigStore::load(&store).await.unwrap();
    let loaded = config
        .semantic_guards
        .providers
        .iter()
        .find(|provider| provider.name == "injection")
        .expect("the local provider stays in the snapshot");
    assert_eq!(loaded.max_scored_bytes, None);
    assert_eq!(loaded.on_truncated, rolter_core::OnTruncated::Score);
}
