//! End-user self-service API (ROL-224): a logged-in local account manages its
//! own virtual keys and sees its own usage, without any admin role.
//!
//! Every route here authenticates via [`CurrentUser`] (a live session token),
//! not the admin [`Principal`] path — these are for end users, so they are only
//! reachable once local-account login is configured. Key mutation is gated two
//! ways: the caller must be at least a `member` of the project the key lives in
//! (so a viewer can't mint keys), and rotate/delete/usage additionally require
//! that the key was minted by the caller (`created_by = me`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_store::postgres::models::User;
use rolter_store::postgres::models::{OwnedVirtualKey, VirtualKey};
use rolter_store::postgres::repo::{
    AuditLogRepo, OrgRepo, RouteRepo, ScimIdentityRepo, UserPreferencesRepo, UserRepo,
    VirtualKeyRepo,
};

use crate::analytics::{client_or_503, run, window_params, WindowQuery, WHERE_WINDOW};
use crate::auth::CurrentUser;
use crate::crud::{
    generate_virtual_key, key_pepper, pool, publish_config_change, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::cap;
use crate::time_bounds::Query;
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
        .route("/api/v1/me/profile", patch(update_my_profile))
        .route(
            "/api/v1/me/preferences",
            get(get_my_preferences).put(put_my_preferences),
        )
}

/// longest display name, in characters; matches `users_display_name_shape`
pub(crate) const MAX_DISPLAY_NAME_LEN: usize = 80;
/// longest bio, in characters; matches `users_bio_shape`
pub(crate) const MAX_BIO_LEN: usize = 500;

/// Keep "field omitted" apart from "field sent as null": a plain `Option`
/// collapses both to `None`, and a PATCH must leave the first alone while the
/// second clears the value.
fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct ProfilePatch {
    #[serde(default, deserialize_with = "present")]
    display_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    bio: Option<Option<String>>,
}

/// Normalise one profile field: `None` and `""` clear it, anything else is
/// trimmed and bounded. A value that is only whitespace is refused rather than
/// quietly cleared, since a client that sent it did not mean "remove".
fn normalise_field(
    field: &str,
    raw: Option<String>,
    max: usize,
    allow_newlines: bool,
) -> Result<Option<String>, ApiError> {
    let Some(raw) = raw else { return Ok(None) };
    if raw.is_empty() {
        return Ok(None);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_request(&format!(
            "{field} must not be only whitespace; send an empty string or null to clear it"
        )));
    }
    if let Some(bad) = trimmed
        .chars()
        .find(|c| c.is_control() && !(allow_newlines && matches!(c, '\n' | '\r' | '\t')))
    {
        return Err(bad_request(&format!(
            "{field} must not contain control characters (found U+{:04X})",
            bad as u32
        )));
    }
    if trimmed.chars().count() > max {
        return Err(bad_request(&format!(
            "{field} must be at most {max} characters"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

/// Coerce an IdP-supplied name into something `users.display_name` accepts.
/// Unlike [`normalise_field`] this never refuses: a directory owns the value,
/// and a provisioning call must not fail because of how a name is spelled.
pub(crate) fn sanitise_directory_name(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let trimmed: String = cleaned.trim().chars().take(MAX_DISPLAY_NAME_LEN).collect();
    let trimmed = trimmed.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// What `PATCH /me/profile` returns: the editable fields and whether the
/// name is owned by a directory.
#[derive(Serialize)]
struct ProfileResponse {
    display_name: Option<String>,
    bio: Option<String>,
    display_name_managed: bool,
}

/// Whether a SCIM-provisioned identity owns this account's display name.
///
/// SCIM is the only source treated as authoritative: its `displayName` is
/// written into `users.display_name` on every provision and replace, so a
/// local edit would be overwritten on the next sync. OIDC's claim is read at
/// login but never persisted, and LDAP's is likewise unused, so those accounts
/// keep an editable name.
pub(crate) async fn display_name_managed(
    state: &ControlState,
    user_id: Uuid,
) -> Result<bool, rolter_core::Error> {
    ScimIdentityRepo(pool(state)).exists_for_user(user_id).await
}

/// change the caller's own profile. any signed-in account may do this at any
/// role: it writes only the caller's row, so it needs no capability and does
/// not widen `user:update`.
async fn update_my_profile(
    current: CurrentUser,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<ProfilePatch>,
) -> ApiResult<Json<ProfileResponse>> {
    let user: &User = &current.user;
    let managed = display_name_managed(&state, user.id).await?;

    let display_name = match body.display_name {
        Some(raw) => Some(normalise_field(
            "display_name",
            raw,
            MAX_DISPLAY_NAME_LEN,
            false,
        )?),
        None => None,
    };
    let bio = match body.bio {
        Some(raw) => Some(normalise_field("bio", raw, MAX_BIO_LEN, true)?),
        None => None,
    };

    // only fields that actually change count, so a form that resubmits both
    // values still works for a directory-managed name it did not touch
    let name_changes = display_name
        .as_ref()
        .is_some_and(|new| *new != user.display_name);
    let bio_changes = bio.as_ref().is_some_and(|new| *new != user.bio);
    if managed && name_changes {
        return Err(ApiError::Conflict(
            "display_name is managed by your identity provider (SCIM) and cannot be changed here"
                .to_string(),
        ));
    }

    let mut changed = Vec::new();
    if name_changes {
        changed.push("display_name");
    }
    if bio_changes {
        changed.push("bio");
    }
    let updated = if changed.is_empty() {
        user.clone()
    } else {
        let updated = UserRepo(pool(&state))
            .set_profile(
                user.id,
                display_name
                    .as_ref()
                    .filter(|_| name_changes)
                    .map(|v| v.as_deref()),
                bio.as_ref().filter(|_| bio_changes).map(|v| v.as_deref()),
            )
            .await?;
        // field names only: a bio is free text the user may not want in a log
        if let Err(err) = AuditLogRepo(pool(&state))
            .create(
                None,
                Some(user.id),
                "user.profile.update",
                Some("user"),
                Some(user.id),
                Some(serde_json::json!({ "fields": changed })),
            )
            .await
        {
            tracing::warn!(error = %err, "failed to write profile audit entry");
        }
        updated
    };
    Ok(Json(ProfileResponse {
        display_name: updated.display_name,
        bio: updated.bio,
        display_name_managed: managed,
    }))
}

/// dashboard locale codes a preference may name. one per catalog in
/// `ui/src/lib/i18n/locales/*.json`; add a code here in the same change that
/// adds its catalog
pub(crate) const LANGUAGES: &[&str] = &["en", "ru"];
/// longest default playground model name, in characters
pub(crate) const MAX_MODEL_LEN: usize = 200;
/// longest IANA zone name accepted; the longest real ones are under 40
const MAX_TIME_ZONE_LEN: usize = 64;

/// The preferences document. A key that is `null` or absent means "use the
/// deployment default", so the stored object simply omits it.
///
/// `PUT` replaces the whole document, which is why this same shape is both the
/// request body and (plus `effective_default_scope`) the response.
#[derive(Deserialize, Serialize, Default, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
struct Preferences {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    default_org_id: Option<Uuid>,
    #[serde(default)]
    default_team_id: Option<Uuid>,
    #[serde(default)]
    default_project_id: Option<Uuid>,
    #[serde(default)]
    default_playground_model: Option<String>,
    #[serde(default)]
    chart_time_zone: Option<String>,
}

/// a scope the caller can read right now, computed on every `GET`
#[derive(Serialize, PartialEq, Debug)]
struct EffectiveScope {
    org_id: Option<Uuid>,
    team_id: Option<Uuid>,
    project_id: Option<Uuid>,
}

#[derive(Serialize)]
struct PreferencesResponse {
    #[serde(flatten)]
    preferences: Preferences,
    /// the stored default scope if the caller can still read it, else the first
    /// scope they can read, else `null`. The dashboard uses this and never the
    /// raw `default_*_id` fields, which can name a scope access was lost to
    effective_default_scope: Option<EffectiveScope>,
}

impl Preferences {
    /// refuse what the dashboard could not render; trims the free-text fields
    fn validated(mut self) -> Result<Self, ApiError> {
        if let Some(lang) = &self.language {
            if !LANGUAGES.contains(&lang.as_str()) {
                return Err(bad_request(&format!(
                    "language must be one of: {}",
                    LANGUAGES.join(", ")
                )));
            }
        }
        if let Some(model) = self.default_playground_model.take() {
            self.default_playground_model = normalise_field(
                "default_playground_model",
                Some(model),
                MAX_MODEL_LEN,
                false,
            )?;
        }
        if let Some(zone) = &self.chart_time_zone {
            if !is_iana_zone_shape(zone) {
                return Err(bad_request(
                    "chart_time_zone must be an IANA time zone name such as Europe/Berlin or UTC",
                ));
            }
        }
        Ok(self)
    }

    /// keys set to a value, as the stored object; unset keys are omitted
    fn to_document(&self) -> serde_json::Value {
        let mut doc = serde_json::to_value(self).unwrap_or_default();
        if let Some(map) = doc.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        doc
    }
}

/// Whether `zone` has the shape of an IANA name: `UTC`, `Europe/Berlin`,
/// `America/Argentina/Buenos_Aires`, `Etc/GMT+5`. The workspace carries no
/// tz database (`chrono-tz` is not a dependency and is heavy), so this checks
/// shape only; the browser's `Intl` rejects a well-formed name it does not
/// know, and the dashboard falls back to the local zone when it does.
fn is_iana_zone_shape(zone: &str) -> bool {
    if zone.is_empty() || zone.len() > MAX_TIME_ZONE_LEN {
        return false;
    }
    let segments: Vec<&str> = zone.split('/').collect();
    segments.len() <= 3
        && segments.iter().all(|seg| {
            !seg.is_empty()
                && seg.starts_with(|c: char| c.is_ascii_alphabetic())
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
        })
}

/// The scope the dashboard should open on, from what the caller can read now.
///
/// Preferences never widen access: the stored ids are only ever a hint, and a
/// scope is returned only if the caller's memberships and custom roles still
/// reach it, the same check the org/team/project listings use. The most
/// specific stored id that is still readable wins; failing that the first
/// scope the caller holds a role at (membership order); failing that `None`.
/// A stored id whose row was deleted counts as unreadable.
async fn effective_default_scope(
    state: &ControlState,
    user: &User,
    prefs: &Preferences,
) -> ApiResult<Option<EffectiveScope>> {
    let principal = Principal::for_user(user.clone());
    let filter = ScopeFilter::load(state, &principal, cap!("project", Read)).await?;
    let pool = pool(state);

    let stored_chain = async {
        if let Some(project) = prefs.default_project_id {
            if let Ok(chain) = ScopeChain::from_project(pool, project).await {
                if filter.allows(chain) {
                    return Some(chain);
                }
            }
        }
        if let Some(team) = prefs.default_team_id {
            if let Ok(chain) = ScopeChain::from_team(pool, team).await {
                if filter.allows(chain) {
                    return Some(chain);
                }
            }
        }
        if let Some(org) = prefs.default_org_id {
            let chain = ScopeChain::org(org);
            // the listings let a caller who only holds a role below the org
            // navigate to it, so reach counts here too; a superadmin reaches
            // only orgs that exist
            let reachable = if filter.is_superadmin() {
                OrgRepo(pool).get(org).await.is_ok()
            } else {
                filter.allows(chain)
                    || filter
                        .reach(pool)
                        .await
                        .is_ok_and(|reach| crate::rbac::reaches_org(&reach, org))
            };
            if reachable {
                return Some(chain);
            }
        }
        None
    }
    .await;

    let chain = match stored_chain {
        Some(chain) => Some(chain),
        None if filter.is_superadmin() => {
            let first = OrgRepo(pool).list().await?.into_iter().next();
            first.map(|org| ScopeChain::org(org.id))
        }
        None => filter.reach(pool).await?.into_iter().next(),
    };
    Ok(chain.map(|c| EffectiveScope {
        org_id: c.org,
        team_id: c.team,
        project_id: c.project,
    }))
}

async fn preferences_response(
    state: &ControlState,
    user: &User,
    preferences: Preferences,
) -> ApiResult<Json<PreferencesResponse>> {
    let effective_default_scope = effective_default_scope(state, user, &preferences).await?;
    Ok(Json(PreferencesResponse {
        preferences,
        effective_default_scope,
    }))
}

/// the caller's own preferences. there is no route to read anyone else's: the
/// row is keyed by the session's user and nothing in the request names one
async fn get_my_preferences(
    current: CurrentUser,
    State(state): State<ControlState>,
) -> ApiResult<Json<PreferencesResponse>> {
    let stored = UserPreferencesRepo(pool(&state))
        .get(current.user.id)
        .await?;
    // a document this build cannot read (written by a newer one, say) reads as
    // empty rather than failing the dashboard's first paint
    let preferences = stored
        .and_then(|doc| serde_json::from_value::<Preferences>(doc).ok())
        .unwrap_or_default();
    preferences_response(&state, &current.user, preferences).await
}

/// replace the caller's whole preferences document. keys left out are cleared.
/// a default scope the caller cannot read is stored all the same, since access
/// can change later; it is [`effective_default_scope`] that keeps it harmless
async fn put_my_preferences(
    current: CurrentUser,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<Preferences>,
) -> ApiResult<Json<PreferencesResponse>> {
    let new = body.validated()?;
    let repo = UserPreferencesRepo(pool(&state));
    let old = repo.get(current.user.id).await?.unwrap_or_default();
    let new_doc = new.to_document();

    let empty = serde_json::Map::new();
    let old_map = old.as_object().unwrap_or(&empty);
    let new_map = new_doc.as_object().unwrap_or(&empty);
    let mut changed: Vec<&str> = old_map
        .keys()
        .chain(new_map.keys())
        .map(String::as_str)
        .filter(|key| old_map.get(*key) != new_map.get(*key))
        .collect();
    changed.sort_unstable();
    changed.dedup();

    if !changed.is_empty() {
        repo.put(current.user.id, &new_doc).await?;
        // key names only: a model name or zone is the user's own business
        if let Err(err) = AuditLogRepo(pool(&state))
            .create(
                None,
                Some(current.user.id),
                "user.preferences.update",
                Some("user"),
                Some(current.user.id),
                Some(serde_json::json!({ "keys": changed })),
            )
            .await
        {
            tracing::warn!(error = %err, "failed to write preferences audit entry");
        }
    }
    preferences_response(&state, &current.user, new).await
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
    let principal = Principal::for_user(current.user.clone());
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
    let principal = Principal::for_user(current.user.clone());
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
    fn time_zone_shape_accepts_iana_names_and_nothing_else() {
        for ok in [
            "UTC",
            "Europe/Berlin",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+5",
        ] {
            assert!(is_iana_zone_shape(ok), "{ok}");
        }
        for bad in [
            "",
            "/",
            "Europe/",
            "../etc",
            "a/b/c/d",
            "Europe Berlin",
            "5/x",
            "Europe/Berlin\n",
        ] {
            assert!(!is_iana_zone_shape(bad), "{bad}");
        }
        assert!(!is_iana_zone_shape(&"a".repeat(65)));
    }

    #[test]
    fn unset_keys_are_left_out_of_the_stored_document() {
        let prefs = Preferences {
            language: Some("en".into()),
            ..Default::default()
        };
        assert_eq!(prefs.to_document(), serde_json::json!({"language": "en"}));
    }

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
