//! Virtual keys: the credentials callers present to the gateway.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::VirtualKey;
use rolter_store::postgres::repo::{BusinessUnitRepo, CustomerRepo, NewVirtualKey, VirtualKeyRepo};

use super::{
    authorize_virtual_key, invalid_field, log_audit, pool, project_teams, publish_config_change,
    require_reach, row_chain, ApiError, ApiResult, KeyBudgetBody, KeyCapsBody, KeyRateLimitBody,
    SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::cap;
use crate::ControlState;

use super::customers::NullableUuid;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/virtual-keys",
            get(list_virtual_keys).post(create_virtual_key),
        )
        .route(
            "/api/v1/orgs/{org_id}/virtual-keys",
            get(list_org_virtual_keys),
        )
        .route(
            "/api/v1/virtual-keys/{id}",
            put(set_virtual_key_disabled).delete(delete_virtual_key),
        )
        .route("/api/v1/virtual-keys/{id}/rotate", post(rotate_virtual_key))
        .route(
            "/api/v1/virtual-keys/{id}/previous-secret",
            delete(revoke_previous_secret),
        )
        .route(
            "/api/v1/virtual-keys/{id}/cache",
            put(set_virtual_key_cache),
        )
        .route(
            "/api/v1/virtual-keys/{id}/providers",
            put(set_virtual_key_providers),
        )
        .route(
            "/api/v1/virtual-keys/{id}/attribution",
            put(set_virtual_key_attribution),
        )
}

// --- virtual keys ---

async fn list_virtual_keys(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<VirtualKey>>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    authorize(&state, &principal, chain, cap!("virtual_key", Read)).await?;
    Ok(Json(VirtualKeyRepo(pool(&state)).list(project_id).await?))
}

/// A virtual key as the organization-wide listing returns it: the row without
/// any digest of a secret, `key_hash` and the retired secret's included. A
/// screen that picks keys needs to name them, not to check them.
#[derive(Serialize)]
struct OrgVirtualKey {
    id: Uuid,
    project_id: Uuid,
    key_prefix: String,
    name: Option<String>,
    models: Vec<String>,
    providers: Vec<String>,
    disabled: bool,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    cache_enabled: Option<bool>,
    created_by: Option<Uuid>,
    business_unit_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    purpose: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl From<VirtualKey> for OrgVirtualKey {
    fn from(key: VirtualKey) -> Self {
        Self {
            id: key.id,
            project_id: key.project_id,
            key_prefix: key.key_prefix,
            name: key.name,
            models: key.models,
            providers: key.providers,
            disabled: key.disabled,
            expires_at: key.expires_at,
            cache_enabled: key.cache_enabled,
            created_by: key.created_by,
            business_unit_id: key.business_unit_id,
            customer_id: key.customer_id,
            purpose: key.purpose,
            created_at: key.created_at,
        }
    }
}

/// The keys of every project of an organization, each carrying its
/// `project_id`, so a screen that names keys across projects reads one list
/// instead of one per project (#3467). Scoped to what the caller may already
/// read: the same `virtual_key:read` floor as the per-project list, decided for
/// each project, so a caller with a role on one team gets that team's projects'
/// keys and a caller with a role nowhere in the organization is refused. The
/// readable projects are settled first and only their keys are read.
async fn list_org_virtual_keys(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<OrgVirtualKey>>> {
    let filter = ScopeFilter::load(&state, &principal, cap!("virtual_key", Read)).await?;
    // the capability is read at a project, so the verdict is per project. a
    // caller with a role nowhere in the organization is refused rather than
    // shown an empty list that would say the organization exists and has no keys
    if !filter.is_superadmin() {
        require_reach(&state, &filter, org_id).await?;
    }
    let teams_of = project_teams(&state, org_id).await?;
    let readable: Vec<Uuid> = teams_of
        .keys()
        .copied()
        .filter(|project| filter.allows(row_chain(org_id, None, Some(*project), &teams_of)))
        .collect();
    let keys = VirtualKeyRepo(pool(&state))
        .list_in_projects(&readable)
        .await?;
    Ok(Json(keys.into_iter().map(OrgVirtualKey::from).collect()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateVirtualKey {
    /// required, and the same rule the self-service mint path applies: the
    /// plaintext is shown once, so an unnamed key is unattributable forever
    /// after (#945)
    name: String,
    #[serde(default)]
    models: Vec<String>,
    /// upstream providers this key may reach; an empty list permits every
    /// provider on an allowed route
    #[serde(default)]
    providers: Vec<String>,
    /// per-key response-cache override; omit/null to inherit the route decision,
    /// false to bypass, true to cache even on a route that didn't opt in
    #[serde(default)]
    cache: Option<bool>,
    /// key lifetime in days; `None` mints a key that never expires, which the
    /// dashboard only sends for an explicit choice
    #[serde(default)]
    expires_in_days: Option<u32>,
    /// a spend and/or rate cap the key is born with (#1171); written with the
    /// key, in one transaction, and edited afterwards through the budgets and
    /// rate-limits endpoints
    #[serde(default)]
    budget: Option<KeyBudgetBody>,
    #[serde(default)]
    rate_limit: Option<KeyRateLimitBody>,
}

#[derive(Serialize)]
struct CreatedVirtualKey {
    #[serde(flatten)]
    row: VirtualKey,
    /// the plaintext key; shown once, never persisted or returned again
    key: String,
}

/// Deployment-wide pepper shared with the gateway (`ROLTER_KEY_PEPPER`). Keys
/// are stored as `rolter_auth::hash_key(pepper, key)` so the gateway can match
/// presented keys by the same peppered digest.
pub(crate) fn key_pepper() -> String {
    std::env::var("ROLTER_KEY_PEPPER").unwrap_or_default()
}

pub(crate) fn generate_virtual_key(pepper: &str) -> (String, String, String) {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    let key = format!("sk-rolter-{}", hex_encode(&bytes));
    let hash = rolter_auth::hash_key(pepper, &key);
    let prefix = key.chars().take(12).collect::<String>();
    (key, hash, prefix)
}

fn hex_encode(bytes: &[u8]) -> String {
    rolter_auth::hex::encode(bytes)
}

async fn create_virtual_key(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateVirtualKey>,
) -> ApiResult<Json<CreatedVirtualKey>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("virtual_key", Create)).await?;
    let (name, expires_at) =
        crate::me::validated_name_and_expiry(&body.name, body.expires_in_days)?;
    let caps = KeyCapsBody {
        budget: body.budget,
        rate_limit: body.rate_limit,
    };
    caps.validate()?;
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create_with_caps(
            project_id,
            NewVirtualKey {
                key_hash: &key_hash,
                key_prefix: &key_prefix,
                name: Some(name.as_str()),
                models: &body.models,
                providers: &body.providers,
                cache_enabled: body.cache,
                created_by: None,
                expires_at,
                // an operator minted this deliberately; `purpose` marks the keys
                // the dashboard mints for itself (#1640)
                purpose: None,
            },
            caps.as_new(),
        )
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.create",
        "virtual_key",
        row.id,
        serde_json::json!({"name": row.name, "key_prefix": key_prefix, "caps": caps.audit()}),
    )
    .await;
    Ok(Json(CreatedVirtualKey { row, key }))
}

/// Longest an old secret may keep authenticating after a rotation: 30 days.
/// Past that a "rotation" is two live keys with nobody tracking the second.
pub(crate) const MAX_ROTATION_OVERLAP_SECONDS: u64 = 30 * 24 * 60 * 60;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RotateVirtualKey {
    /// how long the secret being replaced keeps authenticating, in seconds;
    /// omit or 0 for none, so the old secret stops at once
    #[serde(default)]
    overlap_seconds: u64,
}

/// A personal key (one a member minted for themself) rotates from Account, never
/// here: a project admin who rotated it would hold a working secret the gateway
/// attributes to that member and runs under their access policy.
fn refuse_personal(key: &VirtualKey) -> ApiResult<()> {
    if key.created_by.is_some() {
        return Err(ApiError::Conflict(
            "this key belongs to the member who minted it; they rotate it from Account".to_string(),
        ));
    }
    Ok(())
}

/// Issue a new secret for a shared key (#1837): the same row, so limits,
/// attribution, allow-lists, cache setting and history all carry over, with an
/// optional window during which the previous secret still authenticates.
///
/// Deliberately its own path rather than a variant of the self-service rotate in
/// `me.rs`, which mints a second row and disables the first.
async fn rotate_virtual_key(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    body: Option<SafeJson<RotateVirtualKey>>,
) -> ApiResult<Json<CreatedVirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    refuse_personal(&VirtualKeyRepo(pool(&state)).get(id).await?)?;
    let body = body.map(|SafeJson(body)| body).unwrap_or_default();
    if body.overlap_seconds > MAX_ROTATION_OVERLAP_SECONDS {
        return Err(invalid_field(
            "overlap_seconds",
            "overlap_seconds must be at most 2592000 (30 days)",
        ));
    }
    let overlap_until = (body.overlap_seconds > 0).then(|| {
        chrono::Utc::now()
            + chrono::Duration::seconds(i64::try_from(body.overlap_seconds).unwrap_or(0))
    });
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .rotate(id, &key_hash, &key_prefix, overlap_until)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.rotate",
        "virtual_key",
        id,
        serde_json::json!({
            "key_prefix": key_prefix,
            "previous_key_prefix": row.previous_key_prefix,
            "overlap_until": overlap_until,
        }),
    )
    .await;
    Ok(Json(CreatedVirtualKey { row, key }))
}

/// End a rotation's overlap early: the previous secret stops authenticating at
/// the next snapshot. Idempotent, so a retry after a timeout is safe.
async fn revoke_previous_secret(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    let repo = VirtualKeyRepo(pool(&state));
    let before = repo.get(id).await?;
    refuse_personal(&before)?;
    let row = repo.revoke_previous(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.revoke_previous_secret",
        "virtual_key",
        id,
        serde_json::json!({
            "previous_key_prefix": before.previous_key_prefix,
            "previous_expires_at": before.previous_expires_at,
        }),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVirtualKeyProviders {
    /// empty restores the permissive default
    #[serde(default)]
    providers: Vec<String>,
}

async fn set_virtual_key_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyProviders>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_providers(id, &body.providers)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_providers",
        "virtual_key",
        id,
        serde_json::json!({"providers": body.providers}),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVirtualKeyAttribution {
    /// Omit to leave unchanged, null to clear, UUID to set.
    #[serde(default)]
    business_unit_id: NullableUuid,
    /// Omit to leave unchanged, null to clear, UUID to set.
    #[serde(default)]
    customer_id: NullableUuid,
}

/// Point a key's spend at a business unit and/or customer. Both dimensions must
/// live in the key's own org, and a customer already owned by a business unit
/// may not be paired with a different one.
async fn set_virtual_key_attribution(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyAttribution>,
) -> ApiResult<Json<VirtualKey>> {
    let repo = VirtualKeyRepo(pool(&state));
    let existing = repo.get(id).await?;
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    let business_unit_id = match body.business_unit_id {
        NullableUuid::Missing => existing.business_unit_id,
        NullableUuid::Null => None,
        NullableUuid::Value(unit_id) => {
            let unit = BusinessUnitRepo(pool(&state)).get(unit_id).await?;
            if Some(unit.org_id) != org_id {
                return Err(ApiError::Core(Error::Config(
                    "business_unit_id must belong to the same org".to_string(),
                )));
            }
            Some(unit_id)
        }
    };
    let customer_id = match body.customer_id {
        NullableUuid::Missing => existing.customer_id,
        NullableUuid::Null => None,
        NullableUuid::Value(customer_id) => {
            let customer = CustomerRepo(pool(&state)).get(customer_id).await?;
            if Some(customer.org_id) != org_id {
                return Err(invalid_field(
                    "customer_id",
                    "customer_id must belong to the same org",
                ));
            }
            Some(customer_id)
        }
    };
    if let (Some(unit_id), Some(customer_id)) = (business_unit_id, customer_id) {
        let customer = CustomerRepo(pool(&state)).get(customer_id).await?;
        if customer
            .business_unit_id
            .is_some_and(|owner| owner != unit_id)
        {
            return Err(invalid_field(
                "customer_id",
                "customer_id belongs to a different business unit",
            ));
        }
    }
    let row = repo
        .set_attribution(id, business_unit_id, customer_id)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_attribution",
        "virtual_key",
        id,
        serde_json::json!({
            "business_unit_id": row.business_unit_id,
            "customer_id": row.customer_id,
        }),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVirtualKeyDisabled {
    disabled: bool,
}

async fn set_virtual_key_disabled(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyDisabled>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_disabled(id, body.disabled)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_disabled",
        "virtual_key",
        id,
        serde_json::json!({"disabled": body.disabled}),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVirtualKeyCache {
    /// null clears the override (inherit the route); false/true force it
    #[serde(default)]
    cache: Option<bool>,
}

async fn set_virtual_key_cache(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyCache>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_cache(id, body.cache)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_cache",
        "virtual_key",
        id,
        serde_json::json!({"cache": body.cache}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_virtual_key(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Delete)).await?;
    VirtualKeyRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.delete",
        "virtual_key",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod virtual_key_tests {
    use super::*;

    #[test]
    fn generates_expected_format() {
        let pepper = "test_pepper";
        let (key, hash, prefix) = generate_virtual_key(pepper);

        // Check key format
        assert!(key.starts_with("sk-rolter-"));
        // 24 bytes hex encoded = 48 chars. + 10 chars for "sk-rolter-" = 58 chars
        assert_eq!(key.len(), 58);
        assert!(key["sk-rolter-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));

        // Check prefix
        assert_eq!(prefix, key.chars().take(12).collect::<String>());
        assert_eq!(prefix.len(), 12);

        // Check hash consistency
        assert_eq!(hash, rolter_auth::hash_key(pepper, &key));
    }

    #[test]
    fn distinct_peppers_produce_different_hashes() {
        let (key1, hash1, _) = generate_virtual_key("pepper_one");
        let hash2 = rolter_auth::hash_key("pepper_two", &key1);

        assert_ne!(hash1, hash2);
    }
}
