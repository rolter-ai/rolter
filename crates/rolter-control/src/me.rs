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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_store::postgres::models::{OwnedVirtualKey, VirtualKey};
use rolter_store::postgres::repo::{RouteRepo, VirtualKeyRepo};

use crate::analytics::{client_or_503, run, window_params, WindowQuery, WHERE_WINDOW};
use crate::auth::CurrentUser;
use crate::crud::{
    generate_virtual_key, key_pepper, pool, publish_config_change, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/me/virtual-keys", get(list_my_keys))
        .route(
            "/api/v1/me/projects/{project_id}/virtual-keys",
            post(mint_my_key),
        )
        .route(
            "/api/v1/me/projects/{project_id}/playground-key",
            post(mint_playground_key),
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
    /// required, 1..=`MAX_KEY_NAME_LEN` characters after trimming. the plaintext
    /// is shown exactly once, so a key that arrives unnamed can never be told
    /// apart from its siblings again (#945)
    name: String,
    #[serde(default)]
    models: Vec<String>,
    /// upstream providers this key may reach; empty permits every provider on
    /// an allowed route
    #[serde(default)]
    providers: Vec<String>,
    /// per-key response-cache override; omit/null to inherit the route decision
    #[serde(default)]
    cache: Option<bool>,
    /// how long the key lives, in days. `None` mints a key that never expires,
    /// which the dashboard only sends for an explicit "never" choice — it is
    /// not what omitting the field in a form produces
    #[serde(default)]
    expires_in_days: Option<u32>,
}

/// longest a virtual key name may be; long enough for "prod checkout service
/// — eu" and short enough to render in a card without wrapping twice
pub(crate) const MAX_KEY_NAME_LEN: usize = 64;
/// widest TTL the mint path accepts, in days (~5 years). past this a caller is
/// asking for an immortal key without saying so
pub(crate) const MAX_KEY_TTL_DAYS: u32 = 1826;

/// `Error::Config` is what the control plane's error mapping renders as a 400
fn bad_request(message: &str) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.to_string()))
}

/// Validate a mint request's name and turn its day count into an instant.
///
/// The server computes `expires_at` rather than accepting one, so a client with
/// a wrong clock cannot mint a key that outlives what the operator chose.
pub(crate) fn validated_name_and_expiry(
    name: &str,
    expires_in_days: Option<u32>,
) -> Result<(String, Option<DateTime<Utc>>), ApiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad_request("virtual key name is required"));
    }
    if name.chars().count() > MAX_KEY_NAME_LEN {
        return Err(bad_request(&format!(
            "virtual key name must be at most {MAX_KEY_NAME_LEN} characters"
        )));
    }
    let expires_at = match expires_in_days {
        None => None,
        Some(0) => {
            return Err(bad_request(
                "expires_in_days must be at least 1; omit it for a key that never expires",
            ));
        }
        Some(d) if d > MAX_KEY_TTL_DAYS => {
            return Err(bad_request(&format!(
                "expires_in_days must be at most {MAX_KEY_TTL_DAYS}"
            )));
        }
        Some(d) => Some(Utc::now() + chrono::Duration::days(i64::from(d))),
    };
    Ok((name.to_string(), expires_at))
}

/// mint a key the caller owns, in a project they belong to. requires `member`
/// (not just viewer) at the project so read-only users can't create keys.
async fn mint_my_key(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    SafeJson(body): SafeJson<MintKey>,
) -> ApiResult<Json<MintedKey>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let principal = Principal::User(current.user.clone());
    authorize(&state, &principal, chain, cap!("my_virtual_key", Create)).await?;

    let (name, expires_at) = validated_name_and_expiry(&body.name, body.expires_in_days)?;

    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            Some(name.as_str()),
            &body.models,
            &body.providers,
            body.cache,
            Some(current.user.id),
            expires_at,
            // minted by hand from the self-service panel
            None,
        )
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(MintedKey { row, key }))
}

/// how long a playground key lives.
///
/// Minutes rather than the mint path's days: this key exists for the length of
/// one sitting at the Playground screen, it is held in the browser, and the
/// dashboard asks for a fresh one rather than keeping this one. A key that
/// outlives the tab that asked for it is a credential nobody is watching.
pub(crate) const PLAYGROUND_KEY_TTL_MINUTES: i64 = 30;

/// the `purpose` a playground-minted key carries, so the Keys screen can say
/// what it is instead of leaving a reader to infer it from a short expiry
pub(crate) const PLAYGROUND_PURPOSE: &str = "playground";

/// Mint a short-lived key for the Playground, scoped by the server (#1640).
///
/// Deliberately takes **no body**. The mint endpoint above accepts `models`,
/// `providers` and a TTL from the caller, which is right for a key an operator
/// is configuring on purpose and wrong here: a key the client scopes is not a
/// key that "cannot address a model the user could not already address", since
/// the client can simply ask for everything. So the reach is computed here,
/// from the routes configured in the project the caller is minting against,
/// and the TTL is fixed.
///
/// An empty `models` list on a virtual key means *every* model, so a project
/// with no routes cannot produce a playground key: minting one would hand out
/// the widest key in the system to mean "nothing to address". That is a 400,
/// not an empty list.
async fn mint_playground_key(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<MintedKey>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let principal = Principal::User(current.user.clone());
    // the same capability the self-service mint needs: this is a key the caller
    // mints for themselves, with strictly less reach than one they could mint
    // by hand, so it cannot be the thing that lets a viewer create credentials
    authorize(&state, &principal, chain, cap!("my_virtual_key", Create)).await?;

    let models: Vec<String> = RouteRepo(pool(&state))
        .list(project_id)
        .await?
        .into_iter()
        .map(|route| route.model)
        .collect();
    if models.is_empty() {
        return Err(bad_request(
            "this project has no routes, so there is nothing a playground key could address",
        ));
    }

    let expires_at = Utc::now() + chrono::Duration::minutes(PLAYGROUND_KEY_TTL_MINUTES);
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            Some("Playground"),
            &models,
            // no provider narrowing: the routes already decide which providers
            // a model reaches, and pinning providers here would make the
            // playground key behave unlike the traffic it is meant to imitate
            &[],
            None,
            Some(current.user.id),
            Some(expires_at),
            Some(PLAYGROUND_PURPOSE),
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
            // a rotation replaces a secret, it does not renew a decision: the
            // fresh key expires exactly when the one it replaces would have
            old.expires_at,
            // and it stays the same kind of key it was
            old.purpose.as_deref(),
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

    let in_list = keys.iter().fold(String::new(), |mut acc, k| {
        if !acc.is_empty() {
            acc.push_str(", ");
        }
        use std::fmt::Write;
        let _ = write!(acc, "\'{}\'", k.id);
        acc
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_cannot_be_minted_without_a_name() {
        // the plaintext is shown once; an unnamed key is unattributable the
        // moment there is a second one (#945)
        for blank in ["", "   ", "\t\n"] {
            let err = validated_name_and_expiry(blank, Some(30)).unwrap_err();
            assert!(format!("{err:?}").contains("name is required"), "{err:?}");
        }
    }

    #[test]
    fn a_name_is_trimmed_and_bounded() {
        let (name, _) = validated_name_and_expiry("  prod checkout  ", Some(30)).unwrap();
        assert_eq!(name, "prod checkout");
        // the bound counts characters, not bytes, so a cyrillic name of the
        // same visible length is not rejected where a latin one passes
        let long = "я".repeat(MAX_KEY_NAME_LEN);
        assert!(validated_name_and_expiry(&long, Some(30)).is_ok());
        let too_long = "я".repeat(MAX_KEY_NAME_LEN + 1);
        assert!(validated_name_and_expiry(&too_long, Some(30)).is_err());
    }

    #[test]
    fn a_ttl_becomes_an_instant_the_server_computed() {
        let before = Utc::now();
        let (_, expires) = validated_name_and_expiry("k", Some(30)).unwrap();
        let expires = expires.expect("30 days is not 'never'");
        // the server derives the instant, so a client with a wrong clock cannot
        // mint a key that outlives what the operator chose
        assert!(expires > before + chrono::Duration::days(29));
        assert!(expires < Utc::now() + chrono::Duration::days(31));
    }

    #[test]
    fn never_expires_is_a_choice_and_zero_days_is_a_mistake() {
        // omitting the field is the only way to ask for an immortal key, and it
        // is what the dashboard sends only for an explicit "never"
        assert_eq!(validated_name_and_expiry("k", None).unwrap().1, None);
        // `0` reads like "no expiry" but would mean "already expired"; refusing
        // it keeps that ambiguity from minting a dead or immortal key
        let err = validated_name_and_expiry("k", Some(0)).unwrap_err();
        assert!(format!("{err:?}").contains("at least 1"), "{err:?}");
        assert!(validated_name_and_expiry("k", Some(MAX_KEY_TTL_DAYS)).is_ok());
        assert!(validated_name_and_expiry("k", Some(MAX_KEY_TTL_DAYS + 1)).is_err());
    }
}
