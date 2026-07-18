//! End-user self-service API (ROL-224): a logged-in local account manages its
//! own virtual keys and sees its own usage, without any admin role.
//!
//! Every route here authenticates via [`CurrentUser`] (a live session token),
//! not the admin [`Principal`] path — these are for end users, so they are only
//! reachable once local-account login is configured. Key mutation is gated two
//! ways: the caller must be at least a `member` of the project the key lives in
//! (so a viewer can't mint keys), and rotate/delete/usage additionally require
//! that the key was minted by the caller (`created_by = me`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_auth::Role;
use rolter_store::postgres::models::{OwnedVirtualKey, VirtualKey};
use rolter_store::postgres::repo::VirtualKeyRepo;

use crate::analytics::{client_or_503, run, window_params, WindowQuery, WHERE_WINDOW};
use crate::auth::CurrentUser;
use crate::crud::{
    generate_virtual_key, key_pepper, pool, publish_config_change, ApiError, ApiResult,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::ControlState;

pub fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/me/virtual-keys", get(list_my_keys))
        .route(
            "/api/v1/me/projects/{project_id}/virtual-keys",
            post(mint_my_key),
        )
        .route("/api/v1/me/virtual-keys/{id}/rotate", post(rotate_my_key))
        .route(
            "/api/v1/me/virtual-keys/{id}",
            axum::routing::delete(delete_my_key),
        )
        .route("/api/v1/me/usage", get(my_usage))
}

/// the plaintext key is returned once on mint/rotate and never again
#[derive(Serialize)]
struct MintedKey {
    #[serde(flatten)]
    row: VirtualKey,
    key: String,
}

async fn list_my_keys(
    current: CurrentUser,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<OwnedVirtualKey>>> {
    Ok(Json(
        VirtualKeyRepo(pool(&state))
            .list_for_user(current.user.id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct MintKey {
    name: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    /// per-key response-cache override; omit/null to inherit the route decision
    #[serde(default)]
    cache: Option<bool>,
}

/// mint a key the caller owns, in a project they belong to. requires `member`
/// (not just viewer) at the project so read-only users can't create keys.
async fn mint_my_key(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<MintKey>,
) -> ApiResult<Json<MintedKey>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let principal = Principal::User(current.user.clone());
    authorize(&state, &principal, chain, Role::Member).await?;

    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            body.name.as_deref(),
            &body.models,
            &[],
            body.cache,
            Some(current.user.id),
        )
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(MintedKey { row, key }))
}

/// require that virtual key `id` was minted by the caller, returning the row.
async fn owned_key(state: &ControlState, current: &CurrentUser, id: Uuid) -> ApiResult<VirtualKey> {
    let vk = VirtualKeyRepo(pool(state)).get(id).await?;
    if vk.created_by != Some(current.user.id) {
        // don't distinguish "not yours" from "doesn't exist" to avoid probing
        return Err(ApiError::Forbidden);
    }
    Ok(vk)
}

/// rotate a key: mint a fresh secret with the same project/name/models/cache and
/// disable the old one, so a leaked key can be replaced without losing config.
async fn rotate_my_key(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<MintedKey>> {
    let old = owned_key(&state, &current, id).await?;

    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            old.project_id,
            &key_hash,
            &key_prefix,
            old.name.as_deref(),
            &old.models,
            &old.providers,
            old.cache_enabled,
            Some(current.user.id),
        )
        .await?;
    VirtualKeyRepo(pool(&state)).set_disabled(id, true).await?;
    publish_config_change(&state).await?;
    Ok(Json(MintedKey { row, key }))
}

async fn delete_my_key(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    owned_key(&state, &current, id).await?;
    VirtualKeyRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// per-key usage/spend over the window for the caller's own keys. depends on the
/// ClickHouse `request_logs` table; returns 503 when analytics isn't configured.
///
/// the key ids spliced into the `in (...)` list are strongly-typed [`Uuid`]s
/// loaded from our own database (their `Display` is only hex + hyphens), so this
/// cannot carry a SQL injection — the time window still binds as parameters.
async fn my_usage(
    current: CurrentUser,
    State(state): State<ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(resp) => return resp,
    };

    let keys = match VirtualKeyRepo(pool(&state))
        .list_for_user(current.user.id)
        .await
    {
        Ok(keys) => keys,
        Err(err) => return run(Err(anyhow::anyhow!(err.to_string()))),
    };
    if keys.is_empty() {
        // no keys → no rows, and nothing to build an `in ()` list from
        return Json(serde_json::json!({ "data": [] })).into_response();
    }

    let in_list = keys
        .iter()
        .map(|k| format!("'{}'", k.id))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "select toString(virtual_key_id) as virtual_key_id, \
                count() as requests, \
                sum(total_tokens) as tokens, \
                round(sum(cost_usd), 6) as cost_usd, \
                countIf(status >= 400) as errors \
         from request_logs \
         where {WHERE_WINDOW} and virtual_key_id in ({in_list}) \
         group by virtual_key_id format JSON"
    );
    run(ch.query(&sql, &window_params(&q)).await)
}
