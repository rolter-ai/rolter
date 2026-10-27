//! Virtual keys: the credentials callers present to the gateway.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::VirtualKey;
use rolter_store::postgres::repo::{BusinessUnitRepo, CustomerRepo, NewVirtualKey, VirtualKeyRepo};

use super::{
    authorize_virtual_key, invalid_field, log_audit, pool, publish_config_change, ApiError,
    ApiResult, SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
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
            "/api/v1/virtual-keys/{id}",
            put(set_virtual_key_disabled).delete(delete_virtual_key),
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
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
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
        serde_json::json!({"name": row.name, "key_prefix": key_prefix}),
    )
    .await;
    Ok(Json(CreatedVirtualKey { row, key }))
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
