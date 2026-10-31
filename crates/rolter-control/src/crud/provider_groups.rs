//! Provider groups: weighted sets of providers addressed by one slug.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::{ProviderGroup, ProviderGroupMember};
use rolter_store::postgres::repo::{ProviderGroupRepo, ProviderRepo};

use super::{
    explicit_null, invalid_field, log_audit, name_taken, pool, publish_config_change,
    require_non_empty, require_not_config_owned, require_row_not_config_owned, resolve_new_slug,
    resolve_slug_change, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

use super::providers::require_providers_usable_from;

use super::providers::visible_in_scope;

use super::routes::STRATEGIES;

use super::routes::default_strategy;

use super::providers::require_scope_project;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/provider-groups",
            get(list_provider_groups).post(create_provider_group),
        )
        .route(
            "/api/v1/provider-groups/{id}",
            put(update_provider_group).delete(delete_provider_group),
        )
}

// --- provider groups (ADR-0017 addendum, ADR-0022) ---

/// A provider group with its resolved membership, as the API returns it.
#[derive(Serialize)]
struct ProviderGroupView {
    #[serde(flatten)]
    group: ProviderGroup,
    members: Vec<ProviderGroupMember>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GroupMemberInput {
    pub(super) provider_id: Uuid,
    /// upstream model rewrite; omit for passthrough of the requested model
    #[serde(default)]
    pub(super) upstream_model: Option<String>,
    #[serde(default = "default_member_weight")]
    pub(super) weight: i32,
}

fn default_member_weight() -> i32 {
    1
}

/// The 409 for a name a readonly row of the bootstrap file holds, which
/// every org's rows are kept off.
///
/// It says only that the name is taken, never where (#1845).
pub(super) fn taken_in_deployment(what: &str, name: &str) -> ApiError {
    name_taken(format!(
        "{what} '{name}' is already in use in this deployment; choose another"
    ))
}

/// The 409 for a name the caller's own org already holds. Names are per org
/// (#1857, ADR-0040), so another org's row of the same name is no conflict,
/// and the refusal never concerns one.
pub(super) fn taken_in_org(what: &str, name: &str) -> ApiError {
    name_taken(format!(
        "{what} '{name}' is already in use in this organization; choose another"
    ))
}

/// Refuse a provider or group slug that already answers an address in
/// `org_id`.
///
/// Providers and provider groups share one slug namespace per org at the
/// gateway (`provider-slug/model`, `group-slug/model`), and a second holder
/// silently takes the address from the first: a group whose slug a provider
/// of its org holds is dropped from routing entirely. Another org may hold the
/// same slug (#1857). A readonly row of the bootstrap file keeps its slug from
/// every org. `except` is the row being renamed, so it does not collide with
/// itself.
pub(super) async fn require_address_slug_free(
    state: &ControlState,
    what: &str,
    slug: &str,
    org_id: Uuid,
    except: Option<Uuid>,
) -> ApiResult<()> {
    if state.config_owned.holds_slug(slug) {
        return Err(taken_in_deployment(what, slug));
    }
    if ProviderRepo(pool(state))
        .address_slug_in_use(slug, Some(org_id), except)
        .await?
    {
        return Err(taken_in_org(what, slug));
    }
    Ok(())
}

/// Refuse any provider that is not in `org_id`.
///
/// A route target or group member on another org's provider would spend that
/// org's credential (#1844). The refusal is the 404 an unknown id gets, so it
/// does not confirm that the provider exists somewhere else.
pub(super) async fn require_providers_in_org(
    state: &ControlState,
    org_id: Uuid,
    provider_ids: &[Uuid],
) -> ApiResult<()> {
    let repo = ProviderRepo(pool(state));
    for &id in provider_ids {
        if repo.get(id).await?.org_id != org_id {
            return Err(ApiError::Core(Error::NotFound(format!("provider {id}"))));
        }
    }
    Ok(())
}

pub(super) fn to_member_tuples(members: &[GroupMemberInput]) -> Vec<(Uuid, Option<String>, i32)> {
    members
        .iter()
        .map(|m| {
            (
                m.provider_id,
                m.upstream_model.clone().filter(|s| !s.trim().is_empty()),
                m.weight.max(1),
            )
        })
        .collect()
}

pub(super) fn validate_strategy(strategy: &str) -> ApiResult<()> {
    if !STRATEGIES.contains(&strategy) {
        return Err(invalid_field(
            "strategy",
            format!("strategy must be one of {STRATEGIES:?}"),
        ));
    }
    Ok(())
}

/// Reject a mutation against a readonly (config-owned) group slug (ADR-0022).
fn require_group_not_config_owned(state: &ControlState, slug: &str) -> ApiResult<()> {
    require_not_config_owned(&state.config_owned.groups, slug, "provider group")
}

/// [`require_group_not_config_owned`] for a stored group of `org`: refused only
/// when the file owns that very row, so an org's group that shares a slug with
/// a global file group stays its own (#3516).
fn require_group_row_not_config_owned(
    state: &ControlState,
    org: Uuid,
    slug: &str,
) -> ApiResult<()> {
    require_row_not_config_owned(&state.config_owned.group_rows, org, slug, "provider group")
}

async fn view_of(state: &ControlState, group: ProviderGroup) -> ApiResult<ProviderGroupView> {
    let members = ProviderGroupRepo(pool(state)).members(group.id).await?;
    Ok(ProviderGroupView { group, members })
}

async fn list_provider_groups(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ProviderGroupView>>> {
    let groups = ProviderGroupRepo(pool(&state)).list(org_id).await?;
    let groups = visible_in_scope(
        &state,
        &principal,
        cap!("provider_group", Read),
        org_id,
        groups,
        |group| group.project_id,
    )
    .await?;
    let mut views = Vec::with_capacity(groups.len());
    for group in groups {
        views.push(view_of(&state, group).await?);
    }
    Ok(Json(views))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateProviderGroup {
    name: String,
    /// stable URL-safe identity; derived from `name` when omitted
    slug: Option<String>,
    #[serde(default = "default_strategy")]
    strategy: String,
    #[serde(default)]
    members: Vec<GroupMemberInput>,
    /// scope the group to one project of the org, as for a provider. A scoped
    /// group may hold that project's providers and org-wide ones; an org-wide
    /// group may hold only org-wide providers. Omit for an org-wide group
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn create_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    let chain = match body.project_id {
        Some(project_id) => {
            require_scope_project(&state, org_id, project_id).await?;
            ScopeChain::from_project(pool(&state), project_id).await?
        }
        None => ScopeChain::org(org_id),
    };
    authorize(&state, &principal, chain, cap!("provider_group", Create)).await?;
    require_non_empty(&body.name, "name")?;
    validate_strategy(&body.strategy)?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    // a readonly config group with this slug shadows any DB row — refuse early
    require_group_not_config_owned(&state, &slug)?;
    let repo = ProviderGroupRepo(pool(&state));
    require_address_slug_free(&state, "provider group slug", &slug, org_id, None).await?;
    let member_providers: Vec<Uuid> = body.members.iter().map(|m| m.provider_id).collect();
    require_providers_in_org(&state, org_id, &member_providers).await?;
    require_providers_usable_from(&state, &member_providers, body.project_id, "this group").await?;
    let group = repo
        .create(org_id, &body.name, &slug, &body.strategy, body.project_id)
        .await?;
    repo.set_members(group.id, &to_member_tuples(&body.members))
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider_group.create",
        "provider_group",
        group.id,
        serde_json::json!({
            "name": group.name, "slug": group.slug, "strategy": group.strategy,
            "project_id": group.project_id
        }),
    )
    .await;
    Ok(Json(view_of(&state, group).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateProviderGroup {
    name: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    allow_slug_change: bool,
    strategy: Option<String>,
    /// when present, replaces the entire membership; omit to leave unchanged
    members: Option<Vec<GroupMemberInput>>,
    /// omit to leave the scope unchanged; a project id scopes the group to that
    /// project; `null` makes it org-wide again
    #[serde(default, deserialize_with = "explicit_null")]
    project_id: Option<Option<Uuid>>,
}

async fn update_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    let repo = ProviderGroupRepo(pool(&state));
    let existing = repo.get(id).await?;
    // changing the scope is an org admin's call; edits inside a scoped group
    // are its project admin's
    let chain = match existing.project_id {
        Some(project_id) if body.project_id.is_none() => {
            ScopeChain::from_project(pool(&state), project_id).await?
        }
        _ => ScopeChain::org(existing.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider_group", Update)).await?;
    require_group_row_not_config_owned(&state, existing.org_id, &existing.slug)?;
    if let Some(Some(project_id)) = body.project_id {
        require_scope_project(&state, existing.org_id, project_id).await?;
    }
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    if let Some(strategy) = &body.strategy {
        validate_strategy(strategy)?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    if let Some(slug) = slug_change.as_deref() {
        require_address_slug_free(
            &state,
            "provider group slug",
            slug,
            existing.org_id,
            Some(id),
        )
        .await?;
    }
    if let Some(members) = &body.members {
        let member_providers: Vec<Uuid> = members.iter().map(|m| m.provider_id).collect();
        require_providers_in_org(&state, existing.org_id, &member_providers).await?;
    }
    // the rule is checked against the group as it will be: its new scope, or
    // the old one, over its new members, or the ones it already has
    if body.members.is_some() || body.project_id.is_some() {
        let scope = body.project_id.unwrap_or(existing.project_id);
        let member_providers: Vec<Uuid> = match &body.members {
            Some(members) => members.iter().map(|m| m.provider_id).collect(),
            None => repo
                .members(id)
                .await?
                .into_iter()
                .map(|m| m.provider_id)
                .collect(),
        };
        require_providers_usable_from(&state, &member_providers, scope, "this group").await?;
    }
    let group = repo
        .update(
            id,
            body.name.as_deref(),
            slug_change.as_deref(),
            body.strategy.as_deref(),
            body.project_id,
        )
        .await?;
    if let Some(members) = &body.members {
        repo.set_members(id, &to_member_tuples(members)).await?;
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "provider_group.update",
        "provider_group",
        id,
        serde_json::json!({"slug": group.slug, "project_id": group.project_id}),
    )
    .await;
    Ok(Json(view_of(&state, group).await?))
}

async fn delete_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = ProviderGroupRepo(pool(&state));
    let existing = repo.get(id).await?;
    let chain = match existing.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(existing.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider_group", Delete)).await?;
    require_group_row_not_config_owned(&state, existing.org_id, &existing.slug)?;
    repo.delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "provider_group.delete",
        "provider_group",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
