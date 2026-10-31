//! Rotating a shared project key on the same row, with an optional overlap
//! (#1837).

use crate::support::*;
use serde_json::{json, Value};

struct Fixture {
    pool: sqlx::PgPool,
    base: String,
    admin: String,
    viewer: String,
    org: uuid::Uuid,
    project: uuid::Uuid,
    _db: rolter_store::postgres::test_schema::TestSchema,
}

async fn fixture() -> Fixture {
    let db = fresh_db().await;
    let pool = db.pool().clone();
    let app = rolter_control::test_app_with_admin_token(pool.clone(), Some("rot".to_string()))
        .await
        .unwrap();
    let addr = serve(app).await;
    let org: uuid::Uuid =
        sqlx::query_scalar("insert into orgs (name, slug) values ('Acme', 'acme') returning id")
            .fetch_one(&pool)
            .await
            .unwrap();
    let team: uuid::Uuid =
        sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
    let project: uuid::Uuid =
        sqlx::query_scalar("insert into projects (team_id, name) values ($1, 'app') returning id")
            .bind(team)
            .fetch_one(&pool)
            .await
            .unwrap();
    let admin = seed_user(&pool, "admin@acme.test", false).await;
    seed_membership(&pool, admin, Some(org), None, None, "admin").await;
    let viewer = seed_user(&pool, "viewer@acme.test", false).await;
    seed_membership(&pool, viewer, Some(org), None, None, "viewer").await;
    Fixture {
        base: format!("http://{addr}"),
        admin: seed_session(&pool, admin, "rot_admin").await,
        viewer: seed_session(&pool, viewer, "rot_viewer").await,
        pool,
        org,
        project,
        _db: db,
    }
}

/// The digest the row stores for a key, which no API response carries.
async fn stored_hash(f: &Fixture, id: &str) -> String {
    sqlx::query_scalar("select key_hash from virtual_keys where id = $1")
        .bind(uuid::Uuid::parse_str(id).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

async fn snapshot_key(f: &Fixture, id: &str) -> Value {
    let snap: Value = reqwest::Client::new()
        .get(format!("{}/internal/snapshot", f.base))
        .bearer_auth("rot")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    snap["config"]["db_virtual_keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == id)
        .cloned()
        .expect("key in snapshot")
}

#[tokio::test]
async fn rotation_keeps_the_row_and_overlaps_the_old_secret_until_it_is_revoked() {
    skip_without_db!();
    let f = fixture().await;
    let client = reqwest::Client::new();
    let created: Value = client
        .post(format!(
            "{}/api/v1/projects/{}/virtual-keys",
            f.base, f.project
        ))
        .bearer_auth(&f.admin)
        .json(
            &json!({"name": "billing", "models": ["gpt-4o"], "providers": ["edge"], "cache": true}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    // no digest of the secret leaves the API (#3525): read it from the row
    assert!(created.get("key_hash").is_none());
    let old_hash = stored_hash(&f, &id).await;
    let old_key = created["key"].as_str().unwrap().to_string();
    let before = config_version(&f.pool).await;

    let rotated: Value = client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.admin)
        .json(&json!({"overlap_seconds": 3600}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // same row and every setting carries over; the secret is new
    assert_eq!(rotated["id"], id.as_str());
    assert_eq!(rotated["models"], json!(["gpt-4o"]));
    assert_eq!(rotated["providers"], json!(["edge"]));
    assert_eq!(rotated["cache_enabled"], true);
    assert_ne!(rotated["key"], old_key.as_str());
    assert!(rotated.get("key_hash").is_none());
    let new_hash = stored_hash(&f, &id).await;
    assert_ne!(new_hash, old_hash);
    assert_eq!(rotated["previous_key_prefix"], old_key[..18]);
    assert!(rotated["previous_expires_at"].is_string());
    // the digest of the retired secret never leaves the API
    assert!(rotated.get("previous_key_hash").is_none());
    assert!(config_version(&f.pool).await > before);

    // the gateway is told about the overlap by digest, and nothing else
    let in_snapshot = snapshot_key(&f, &id).await;
    assert_eq!(in_snapshot["key_hash"], new_hash.as_str());
    assert_eq!(in_snapshot["previous"]["key_hash"], old_hash.as_str());
    assert!(in_snapshot["previous"]["valid_until"].is_string());

    // revoking early ends the overlap and is idempotent
    for _ in 0..2 {
        let revoked: Value = client
            .delete(format!(
                "{}/api/v1/virtual-keys/{id}/previous-secret",
                f.base
            ))
            .bearer_auth(&f.admin)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(revoked["previous_expires_at"].is_null());
        assert!(revoked["previous_key_prefix"].is_null());
    }
    assert!(snapshot_key(&f, &id).await.get("previous").is_none());

    // a rotation with no body ends the old secret at once
    let plain: Value = client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(plain["previous_expires_at"].is_null());
    assert!(snapshot_key(&f, &id).await.get("previous").is_none());

    // both the rotation and the early revoke are audited, without any secret
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "select action, detail from audit_log
         where org_id = $1 and target_id = $2 and action like 'virtual_key.r%' order by at",
    )
    .bind(f.org)
    .bind(uuid::Uuid::parse_str(&id).unwrap())
    .fetch_all(&f.pool)
    .await
    .unwrap();
    let actions: Vec<&str> = rows.iter().map(|(a, _)| a.as_str()).collect();
    assert_eq!(
        actions,
        [
            "virtual_key.rotate",
            "virtual_key.revoke_previous_secret",
            "virtual_key.revoke_previous_secret",
            "virtual_key.rotate"
        ]
    );
    assert_eq!(rows[0].1["previous_key_prefix"], old_key[..18]);
    assert!(!rows.iter().any(|(_, d)| d.to_string().contains(&old_hash)));
}

#[tokio::test]
async fn rotation_preserves_a_legacy_display_prefix() {
    skip_without_db!();
    let f = fixture().await;
    let client = reqwest::Client::new();
    let created: Value = client
        .post(format!(
            "{}/api/v1/projects/{}/virtual-keys",
            f.base, f.project
        ))
        .bearer_auth(&f.admin)
        .json(&json!({"name": "legacy"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let legacy = &created["key"].as_str().unwrap()[..12];
    sqlx::query("update virtual_keys set key_prefix = $1 where id = $2")
        .bind(legacy)
        .bind(uuid::Uuid::parse_str(id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();

    let rotated: Value = client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.admin)
        .json(&json!({"overlap_seconds": 3600}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rotated["previous_key_prefix"], legacy);
    assert_eq!(rotated["key_prefix"].as_str().unwrap().len(), 18);
}

#[tokio::test]
async fn rotation_needs_admin_and_bounds_the_overlap() {
    skip_without_db!();
    let f = fixture().await;
    let client = reqwest::Client::new();
    let created: Value = client
        .post(format!(
            "{}/api/v1/projects/{}/virtual-keys",
            f.base, f.project
        ))
        .bearer_auth(&f.admin)
        .json(&json!({"name": "billing"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let before = stored_hash(&f, id).await;

    // a viewer can read the key but not rotate it or end an overlap
    let denied = client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let denied = client
        .delete(format!(
            "{}/api/v1/virtual-keys/{id}/previous-secret",
            f.base
        ))
        .bearer_auth(&f.viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);

    // thirty days is the ceiling
    let too_long = client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.admin)
        .json(&json!({"overlap_seconds": 30 * 24 * 3600 + 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(too_long.status(), 400);
    // and a refused rotation changed nothing
    assert_eq!(stored_hash(&f, id).await, before);
}

#[tokio::test]
async fn a_personal_key_is_never_rotated_by_a_project_admin() {
    skip_without_db!();
    let f = fixture().await;
    let client = reqwest::Client::new();
    // a key a member minted for themself: it carries its creator
    let member = seed_user(&f.pool, "member@acme.test", false).await;
    seed_membership(&f.pool, member, None, None, Some(f.project), "member").await;
    let key_id: uuid::Uuid = sqlx::query_scalar(
        "insert into virtual_keys (project_id, key_hash, key_prefix, name, created_by)
         values ($1, 'personal-hash', 'sk-rolter-per', 'mine', $2) returning id",
    )
    .bind(f.project)
    .bind(member)
    .fetch_one(&f.pool)
    .await
    .unwrap();

    let rotate = client
        .post(format!("{}/api/v1/virtual-keys/{key_id}/rotate", f.base))
        .bearer_auth(&f.admin)
        .json(&json!({"overlap_seconds": 60}))
        .send()
        .await
        .unwrap();
    assert_eq!(rotate.status(), 409);
    let revoke = client
        .delete(format!(
            "{}/api/v1/virtual-keys/{key_id}/previous-secret",
            f.base
        ))
        .bearer_auth(&f.admin)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 409);
    // and nothing about the key changed
    let hash: String = sqlx::query_scalar("select key_hash from virtual_keys where id = $1")
        .bind(key_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(hash, "personal-hash");
}

#[tokio::test]
async fn a_viewer_is_refused_before_the_overlap_is_checked() {
    skip_without_db!();
    let f = fixture().await;
    let client = reqwest::Client::new();
    let created: Value = client
        .post(format!(
            "{}/api/v1/projects/{}/virtual-keys",
            f.base, f.project
        ))
        .bearer_auth(&f.admin)
        .json(&json!({"name": "billing"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    // an oversized overlap from someone who may not rotate at all is a 403, not
    // a 400 that tells them what the limit is
    let denied = client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.viewer)
        .json(&json!({"overlap_seconds": 99_999_999_999u64}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
}

#[tokio::test]
async fn disabling_a_key_ends_the_overlap_so_re_enabling_does_not_revive_the_old_secret() {
    skip_without_db!();
    let f = fixture().await;
    let client = reqwest::Client::new();
    let created: Value = client
        .post(format!(
            "{}/api/v1/projects/{}/virtual-keys",
            f.base, f.project
        ))
        .bearer_auth(&f.admin)
        .json(&json!({"name": "billing"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    client
        .post(format!("{}/api/v1/virtual-keys/{id}/rotate", f.base))
        .bearer_auth(&f.admin)
        .json(&json!({"overlap_seconds": 3600}))
        .send()
        .await
        .unwrap();
    assert!(snapshot_key(&f, &id).await.get("previous").is_some());

    for disabled in [true, false] {
        let row: Value = client
            .put(format!("{}/api/v1/virtual-keys/{id}", f.base))
            .bearer_auth(&f.admin)
            .json(&json!({"disabled": disabled}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(row["previous_expires_at"].is_null(), "disabled={disabled}");
    }
    // enabled again, and the old secret is still gone
    assert!(snapshot_key(&f, &id).await.get("previous").is_none());
}
