//! SCIM 2.0 Groups provisioning and group→team mapping (#540).
//!
//! The Users slice ([`crate::scim`]) answers *who exists*. This one answers
//! *what they may do*: an IdP drives `/scim/v2/Groups` with the same org-scoped
//! provisioning token, and an operator writes org-scoped mappings that turn a
//! group name into a role at a team, a project, or the org itself.
//!
//! The mapping model is deliberately the SSO one
//! ([`crate::sso`] / `sso_group_mappings`), down to the vocabulary: a
//! `group_name` grants a `role` at the most specific non-null scope id, only
//! `admin`/`member`/`viewer` are mappable, and reconciliation touches only the
//! memberships provisioning itself created. Two divergent group-mapping models
//! in one control plane would be two things for an operator to learn and two
//! places for a privilege bug to hide.
//!
//! Three properties shape the implementation:
//!
//! * **Reconciliation converges.** Every write recomputes the affected users'
//!   `source = 'scim'` memberships from scratch. An IdP re-sending the same
//!   group state changes nothing, which is exactly what a scheduled sync does
//!   all day.
//! * **A grant an operator made is never undone.** Only `scim`-sourced rows are
//!   reconciled away; a `manual` grant from the admin API or an invitation
//!   survives a sync, the same rule SSO logins follow.
//! * **A member must already be provisioned here.** A `members` entry that is
//!   not a user with a SCIM identity in the token's org is refused, so a group
//!   cannot be used to reach across tenants or to smuggle in an account the
//!   Users surface never created.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rolter_store::postgres::models::{Membership, ScimGroup, ScimGroupMapping};
use rolter_store::postgres::repo::{
    MembershipRepo, ScimGroupMappingRepo, ScimGroupRepo, ScimIdentityRepo,
};

use crate::crud::{log_audit, pool, require_non_empty, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::scim::{
    audit_scim, ListQuery, PatchBody, PatchOp, ScimError, ScimPrincipal, ScimResult, LIST_SCHEMA,
    MAX_PAGE, PATCH_SCHEMA,
};
use crate::ControlState;

const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";

/// Marks the memberships this module owns. Anything else in the org — an
/// operator's grant, an SSO login's grant — is left alone by reconciliation.
const SCIM_SOURCE: &str = "scim";

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        // the IdP-facing surface, authenticated by a provisioning token
        .route("/scim/v2/Groups", get(list_groups).post(create_group))
        .route(
            "/scim/v2/Groups/{id}",
            get(get_group)
                .put(replace_group)
                .patch(patch_group)
                .delete(delete_group),
        )
        // the operator-facing mapping lifecycle, authenticated as usual
        .route(
            "/api/v1/orgs/{org_id}/scim-group-mappings",
            post(create_mapping).get(list_mappings),
        )
        .route(
            "/api/v1/scim-group-mappings/{id}",
            axum::routing::delete(delete_mapping),
        )
}

// ---------------------------------------------------------------------------
// resource rendering
// ---------------------------------------------------------------------------

/// Render a group as a SCIM Groups resource. `members` carries each member's
/// `userName` as `display`, which is what an IdP shows when it reconciles.
fn group_resource(group: &ScimGroup, members: &[(Uuid, String)]) -> Value {
    let mut resource = json!({
        "schemas": [GROUP_SCHEMA],
        "id": group.id.to_string(),
        "displayName": group.display_name,
        "members": members.iter().map(|(id, user_name)| json!({
            "value": id.to_string(),
            "display": user_name,
            "$ref": format!("/scim/v2/Users/{id}"),
            "type": "User",
        })).collect::<Vec<_>>(),
        "meta": {
            "resourceType": "Group",
            "created": group.created_at.to_rfc3339(),
            "lastModified": group.updated_at.to_rfc3339(),
            "location": format!("/scim/v2/Groups/{}", group.id),
        },
    });
    if let Some(external_id) = &group.external_id {
        resource["externalId"] = json!(external_id);
    }
    resource
}

/// The group's members with their `userName`s, in membership order.
async fn members_of(
    state: &ControlState,
    org_id: Uuid,
    group_id: Uuid,
) -> ScimResult<Vec<(Uuid, String)>> {
    let pool = pool(state);
    let mut out = Vec::new();
    for user_id in ScimGroupRepo(pool).members(group_id).await? {
        // a member whose identity was deprovisioned is dropped from the view
        // rather than rendered as a dangling id
        if let Some(identity) = ScimIdentityRepo(pool).get(user_id, org_id).await? {
            out.push((user_id, identity.user_name));
        }
    }
    Ok(out)
}

async fn rendered(state: &ControlState, org_id: Uuid, group: &ScimGroup) -> ScimResult<Value> {
    let members = members_of(state, org_id, group.id).await?;
    Ok(group_resource(group, &members))
}

// ---------------------------------------------------------------------------
// list / read
// ---------------------------------------------------------------------------

/// Parse the one filter shape group reconciliation needs:
/// `displayName eq "x"`. Anything else is refused rather than ignored — an IdP
/// handed the whole directory for an unmatched filter reads it as "no such
/// group" and creates a duplicate.
fn parse_display_name_filter(filter: &str) -> ScimResult<String> {
    let trimmed = filter.trim();
    let rest = trimmed
        .strip_prefix("displayName eq ")
        .or_else(|| trimmed.strip_prefix("displayname eq "))
        .or_else(|| trimmed.strip_prefix("displayName Eq "))
        .ok_or_else(|| {
            ScimError::new(
                StatusCode::BAD_REQUEST,
                Some("invalidFilter"),
                "only `displayName eq \"value\"` filters are supported",
            )
        })?;
    let value = rest.trim().trim_matches('"');
    if value.is_empty() {
        return Err(ScimError::new(
            StatusCode::BAD_REQUEST,
            Some("invalidFilter"),
            "filter value must not be empty",
        ));
    }
    Ok(value.to_string())
}

async fn list_groups(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Query(query): Query<ListQuery>,
) -> ScimResult<Json<Value>> {
    let repo = ScimGroupRepo(pool(&state));
    let groups = match &query.filter {
        Some(filter) => {
            let display_name = parse_display_name_filter(filter)?;
            repo.find_by_display_name(principal.org_id, &display_name)
                .await?
                .into_iter()
                .collect()
        }
        None => repo.list(principal.org_id).await?,
    };
    let total = groups.len();
    // SCIM paging is 1-based
    let start = query.start_index.unwrap_or(1).max(1) - 1;
    let count = query.count.unwrap_or(MAX_PAGE).min(MAX_PAGE);
    let mut resources = Vec::new();
    for group in groups.into_iter().skip(start).take(count) {
        resources.push(rendered(&state, principal.org_id, &group).await?);
    }
    Ok(Json(json!({
        "schemas": [LIST_SCHEMA],
        "totalResults": total,
        "startIndex": start + 1,
        "itemsPerPage": resources.len(),
        "Resources": resources,
    })))
}

/// Look a group up inside the token's org. A group in another tenant is a
/// `404`, never a redacted row.
async fn resolve(
    state: &ControlState,
    principal: &ScimPrincipal,
    id: &str,
) -> ScimResult<ScimGroup> {
    let group_id: Uuid = id.parse().map_err(|_| ScimError::not_found(id))?;
    ScimGroupRepo(pool(state))
        .get(group_id, principal.org_id)
        .await?
        .ok_or_else(|| ScimError::not_found(id))
}

async fn get_group(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> ScimResult<Json<Value>> {
    let group = resolve(&state, &principal, &id).await?;
    Ok(Json(rendered(&state, principal.org_id, &group).await?))
}

// ---------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ScimGroupBody {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "externalId")]
    external_id: Option<String>,
    /// absent means "do not touch the membership"; present (even empty) means
    /// "the group's members are exactly this"
    members: Option<Value>,
}

/// Resolve one `members` entry to a local user. The id must be a user the same
/// token provisioned: an unprovisioned or foreign id is an `invalidValue`, so a
/// group can never be a side door into another tenant.
async fn resolve_member(state: &ControlState, org_id: Uuid, value: &str) -> ScimResult<Uuid> {
    let user_id: Uuid = value
        .parse()
        .map_err(|_| ScimError::invalid(format!("member '{value}' is not a valid resource id")))?;
    ScimIdentityRepo(pool(state))
        .get(user_id, org_id)
        .await?
        .map(|identity| identity.user_id)
        .ok_or_else(|| {
            ScimError::invalid(format!(
                "member '{value}' is not a user provisioned in this org"
            ))
        })
}

/// Read the member ids out of the shapes IdPs send: an array of
/// `{"value": id}` objects, an array of bare id strings, or a single object.
fn member_values(value: &Value) -> ScimResult<Vec<String>> {
    fn one(item: &Value) -> ScimResult<String> {
        match item {
            Value::String(id) => Ok(id.clone()),
            Value::Object(_) => item
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ScimError::invalid("a member entry is missing its `value`")),
            other => Err(ScimError::invalid(format!(
                "a member entry must be an object or an id, got {other}"
            ))),
        }
    }
    match value {
        Value::Array(items) => items.iter().map(one).collect(),
        Value::Null => Ok(Vec::new()),
        other => Ok(vec![one(other)?]),
    }
}

/// Resolve a whole `members` payload to local user ids, rejecting duplicates by
/// collapsing them: an IdP listing the same member twice must not double-count.
async fn resolve_members(
    state: &ControlState,
    org_id: Uuid,
    value: &Value,
) -> ScimResult<Vec<Uuid>> {
    let mut ids = Vec::new();
    for raw in member_values(value)? {
        let id = resolve_member(state, org_id, &raw).await?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

async fn create_group(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Json(body): Json<ScimGroupBody>,
) -> ScimResult<Response> {
    let display_name = body
        .display_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| ScimError::invalid("displayName is required"))?;
    let repo = ScimGroupRepo(pool(&state));
    // an IdP replaying a create must not produce a second group
    if repo
        .find_by_display_name(principal.org_id, &display_name)
        .await?
        .is_some()
    {
        return Err(ScimError::new(
            StatusCode::CONFLICT,
            Some("uniqueness"),
            format!("displayName {display_name} already exists"),
        ));
    }
    // resolve members before writing anything, so a bad member id leaves no
    // half-created group behind
    let members = match &body.members {
        Some(value) => resolve_members(&state, principal.org_id, value).await?,
        None => Vec::new(),
    };
    let group = repo
        .create(principal.org_id, body.external_id.as_deref(), &display_name)
        .await?;
    repo.set_members(group.id, &members).await?;
    reconcile_users(&state, principal.org_id, &members).await?;
    audit_scim(
        &state,
        &principal,
        "scim.group.create",
        "scim_group",
        group.id,
        json!({"display_name": group.display_name, "members": members.len()}),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(rendered(&state, principal.org_id, &group).await?),
    )
        .into_response())
}

async fn replace_group(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
    Json(body): Json<ScimGroupBody>,
) -> ScimResult<Json<Value>> {
    let group = resolve(&state, &principal, &id).await?;
    let repo = ScimGroupRepo(pool(&state));
    let display_name = body
        .display_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| group.display_name.clone());
    if display_name != group.display_name
        && repo
            .find_by_display_name(principal.org_id, &display_name)
            .await?
            .is_some()
    {
        return Err(ScimError::new(
            StatusCode::CONFLICT,
            Some("uniqueness"),
            format!("displayName {display_name} already exists"),
        ));
    }
    // whoever was in the group before is reconciled too: a member PUT drops is
    // losing whatever the group granted them
    let before = repo.members(group.id).await?;
    let updated = repo
        .update(
            group.id,
            principal.org_id,
            body.external_id.as_deref().or(group.external_id.as_deref()),
            &display_name,
        )
        .await?;
    // a PUT with no `members` at all is a rename, not "empty the group"
    if let Some(value) = &body.members {
        let members = resolve_members(&state, principal.org_id, value).await?;
        repo.set_members(group.id, &members).await?;
    }
    let touched = touched_users(&state, group.id, &before).await?;
    reconcile_users(&state, principal.org_id, &touched).await?;
    audit_scim(
        &state,
        &principal,
        "scim.group.update",
        "scim_group",
        group.id,
        json!({"display_name": updated.display_name}),
    )
    .await;
    Ok(Json(rendered(&state, principal.org_id, &updated).await?))
}

/// The union of a group's members before and after a write — everyone whose
/// effective roles the write could have changed.
async fn touched_users(
    state: &ControlState,
    group_id: Uuid,
    before: &[Uuid],
) -> ScimResult<Vec<Uuid>> {
    let mut touched = before.to_vec();
    for user_id in ScimGroupRepo(pool(state)).members(group_id).await? {
        if !touched.contains(&user_id) {
            touched.push(user_id);
        }
    }
    Ok(touched)
}

/// One `members` patch path an IdP may send, as a member id when the path
/// carries a filter (`members[value eq "…"]`).
fn member_from_path(path: &str) -> Option<String> {
    let rest = path.trim().strip_prefix("members[")?.strip_suffix(']')?;
    let value = rest
        .trim()
        .strip_prefix("value eq ")
        .or_else(|| rest.trim().strip_prefix("value Eq "))?;
    Some(value.trim().trim_matches(['"', '\'']).to_string())
}

/// Apply one patch operation to a group. Returns whether anything was applied,
/// so a request whose operations all did nothing can be refused rather than
/// answered with a success the IdP would read as "the change landed".
async fn apply_group_op(
    state: &ControlState,
    principal: &ScimPrincipal,
    group: &ScimGroup,
    op: &PatchOp,
) -> ScimResult<bool> {
    let repo = ScimGroupRepo(pool(state));
    let verb = op.op.to_ascii_lowercase();
    let path = op.path.as_deref().map(str::trim).unwrap_or_default();

    // remove is the only op whose path may carry the member it targets, and the
    // only one that is meaningful without a value
    if verb == "remove" {
        if let Some(value) = member_from_path(path) {
            let user_id = resolve_member(state, principal.org_id, &value).await?;
            repo.remove_member(group.id, user_id).await?;
            return Ok(true);
        }
        if path != "members" {
            return Err(ScimError::invalid(format!(
                "unsupported remove path '{path}'"
            )));
        }
        return match &op.value {
            // `remove` on the whole attribute empties the group
            None | Some(Value::Null) => {
                repo.set_members(group.id, &[]).await?;
                Ok(true)
            }
            Some(value) => {
                for user_id in resolve_members(state, principal.org_id, value).await? {
                    repo.remove_member(group.id, user_id).await?;
                }
                Ok(true)
            }
        };
    }

    if verb != "add" && verb != "replace" {
        return Err(ScimError::invalid(format!("unsupported op '{}'", op.op)));
    }
    let value = op
        .value
        .as_ref()
        .ok_or_else(|| ScimError::invalid("operation is missing a value"))?;

    match path {
        "members" => {
            let members = resolve_members(state, principal.org_id, value).await?;
            if verb == "replace" {
                repo.set_members(group.id, &members).await?;
            } else {
                for user_id in members {
                    repo.add_member(group.id, user_id).await?;
                }
            }
            Ok(true)
        }
        "displayName" | "externalId" => {
            let text = value
                .as_str()
                .ok_or_else(|| ScimError::invalid(format!("{path} must be a string")))?;
            rename(state, principal, group, path, text).await?;
            Ok(true)
        }
        // the bare form, where the value object names the attributes
        "" => {
            let object = value
                .as_object()
                .ok_or_else(|| ScimError::invalid("a pathless operation needs an object value"))?;
            let mut applied = false;
            for (key, item) in object {
                match key.as_str() {
                    "members" => {
                        let members = resolve_members(state, principal.org_id, item).await?;
                        if verb == "replace" {
                            repo.set_members(group.id, &members).await?;
                        } else {
                            for user_id in members {
                                repo.add_member(group.id, user_id).await?;
                            }
                        }
                        applied = true;
                    }
                    "displayName" | "externalId" => {
                        let text = item
                            .as_str()
                            .ok_or_else(|| ScimError::invalid(format!("{key} must be a string")))?;
                        rename(state, principal, group, key, text).await?;
                        applied = true;
                    }
                    other => {
                        return Err(ScimError::invalid(format!(
                            "unsupported attribute '{other}'"
                        )))
                    }
                }
            }
            if !applied {
                return Err(ScimError::invalid("the operation names no attribute"));
            }
            Ok(true)
        }
        other => Err(ScimError::invalid(format!("unsupported path '{other}'"))),
    }
}

/// Set `displayName` or `externalId`, re-reading the group first so two
/// operations in one PATCH body compose instead of overwriting each other.
async fn rename(
    state: &ControlState,
    principal: &ScimPrincipal,
    group: &ScimGroup,
    attribute: &str,
    value: &str,
) -> ScimResult<()> {
    let repo = ScimGroupRepo(pool(state));
    let current = repo
        .get(group.id, principal.org_id)
        .await?
        .ok_or_else(|| ScimError::not_found(&group.id.to_string()))?;
    let (display_name, external_id) = if attribute == "displayName" {
        (value.to_string(), current.external_id.clone())
    } else {
        (current.display_name.clone(), Some(value.to_string()))
    };
    if display_name.trim().is_empty() {
        return Err(ScimError::invalid("displayName must not be empty"));
    }
    if display_name != current.display_name
        && repo
            .find_by_display_name(principal.org_id, &display_name)
            .await?
            .is_some()
    {
        return Err(ScimError::new(
            StatusCode::CONFLICT,
            Some("uniqueness"),
            format!("displayName {display_name} already exists"),
        ));
    }
    repo.update(
        group.id,
        principal.org_id,
        external_id.as_deref(),
        &display_name,
    )
    .await?;
    Ok(())
}

async fn patch_group(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> ScimResult<Json<Value>> {
    if !body.schemas.is_empty() && !body.schemas.iter().any(|s| s == PATCH_SCHEMA) {
        return Err(ScimError::invalid(format!(
            "schemas must include {PATCH_SCHEMA}"
        )));
    }
    let group = resolve(&state, &principal, &id).await?;
    let before = ScimGroupRepo(pool(&state)).members(group.id).await?;
    let mut applied = false;
    for op in &body.operations {
        applied |= apply_group_op(&state, &principal, &group, op).await?;
    }
    if !applied {
        return Err(ScimError::invalid("no supported operation in the request"));
    }
    let touched = touched_users(&state, group.id, &before).await?;
    reconcile_users(&state, principal.org_id, &touched).await?;
    let updated = resolve(&state, &principal, &id).await?;
    audit_scim(
        &state,
        &principal,
        "scim.group.update",
        "scim_group",
        group.id,
        json!({"display_name": updated.display_name}),
    )
    .await;
    Ok(Json(rendered(&state, principal.org_id, &updated).await?))
}

/// SCIM `DELETE` removes the group and, with it, everything the group granted:
/// its members are reconciled after the row is gone, so the roles it implied
/// disappear in the same request rather than at the next sync.
async fn delete_group(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> ScimResult<StatusCode> {
    let group = resolve(&state, &principal, &id).await?;
    let repo = ScimGroupRepo(pool(&state));
    let members = repo.members(group.id).await?;
    repo.delete(group.id, principal.org_id).await?;
    reconcile_users(&state, principal.org_id, &members).await?;
    audit_scim(
        &state,
        &principal,
        "scim.group.delete",
        "scim_group",
        group.id,
        json!({"display_name": group.display_name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// reconciliation
// ---------------------------------------------------------------------------

/// One membership a group mapping implies: `(org, team, project, role)`, with
/// the same "most specific non-null id" convention as `memberships` and the SSO
/// mapping model.
type ScopedGrant = (Option<Uuid>, Option<Uuid>, Option<Uuid>, String);

/// The grant a mapping row stands for. A mapping that names neither a team nor
/// a project is an org grant; a narrower scope carries the org implicitly.
fn grant_of(mapping: &ScimGroupMapping) -> ScopedGrant {
    if mapping.team_id.is_none() && mapping.project_id.is_none() {
        (Some(mapping.org_id), None, None, mapping.role.clone())
    } else {
        (
            None,
            mapping.team_id,
            mapping.project_id,
            mapping.role.clone(),
        )
    }
}

fn grant_matches(membership: &Membership, want: &ScopedGrant) -> bool {
    membership.org_id == want.0
        && membership.team_id == want.1
        && membership.project_id == want.2
        && membership.role == want.3
}

/// Make one user's `scim`-sourced memberships inside `org_id` be exactly what
/// their current group membership implies.
///
/// This is the whole convergence guarantee: the wanted set is derived from the
/// database every time, never from the request, so replaying a sync is a no-op
/// and a dropped group membership revokes the role that group granted. The
/// one exception is an org's last active admin grant, which is kept and
/// audited until the org has another admin (#2558). `protect` is false only
/// when a superadmin deletes a mapping, which may revoke that grant like
/// `delete_membership` does (#2673); every IdP-driven sync passes true.
pub(crate) async fn reconcile_user(
    state: &ControlState,
    org_id: Uuid,
    user_id: Uuid,
    protect: bool,
) -> ApiResult<Vec<String>> {
    let db = pool(state);
    let mut wanted: Vec<ScopedGrant> = Vec::new();
    for group in ScimGroupRepo(db).groups_for_user(org_id, user_id).await? {
        for mapping in ScimGroupMappingRepo(db)
            .list_for_group(org_id, &group.display_name)
            .await?
        {
            let grant = grant_of(&mapping);
            if !wanted.contains(&grant) {
                wanted.push(grant);
            }
        }
    }

    let repo = MembershipRepo(db);
    let existing: Vec<Membership> = repo
        .list_in_org(org_id)
        .await?
        .into_iter()
        .filter(|m| m.user_id == user_id)
        .collect();

    let mut granted = Vec::new();
    for stale in existing
        .iter()
        .filter(|m| m.source == SCIM_SOURCE)
        .filter(|m| !wanted.iter().any(|w| grant_matches(m, w)))
    {
        // the org's last admin grant outlives the group change rather than
        // failing the sync; the next sync revokes it once another admin exists
        // (#2558)
        if crate::crud::revoke_idp_grant(state, stale, protect).await? {
            granted.push(stale.role.clone());
        }
    }

    for want in &wanted {
        granted.push(want.3.clone());
        // an equivalent grant from any source already covers this one; creating
        // a second row would only make the operator's grant look duplicated
        if existing.iter().any(|m| grant_matches(m, want)) {
            continue;
        }
        repo.create_with_source(user_id, want.0, want.1, want.2, &want.3, SCIM_SOURCE)
            .await?;
    }
    Ok(granted)
}

async fn reconcile_users(state: &ControlState, org_id: Uuid, user_ids: &[Uuid]) -> ApiResult<()> {
    for user_id in user_ids {
        reconcile_user(state, org_id, *user_id, true).await?;
    }
    Ok(())
}

/// Drop a user out of every group in the org and reconcile. Called when the
/// Users surface deprovisions them, so a `DELETE /scim/v2/Users/{id}` takes the
/// group-granted roles with it instead of leaving them behind.
pub(crate) async fn forget_user(
    state: &ControlState,
    org_id: Uuid,
    user_id: Uuid,
) -> ApiResult<()> {
    let db = pool(state);
    let repo = ScimGroupRepo(db);
    for group in repo.groups_for_user(org_id, user_id).await? {
        repo.remove_member(group.id, user_id).await?;
    }
    reconcile_user(state, org_id, user_id, true).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// mapping lifecycle (operator-facing)
// ---------------------------------------------------------------------------

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.into()))
}

#[derive(Debug, Deserialize)]
struct CreateMapping {
    group_name: String,
    role: String,
    #[serde(default)]
    team_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn create_mapping(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateMapping>,
) -> ApiResult<Json<ScimGroupMapping>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("scim_group_mapping", Create),
    )
    .await?;
    require_non_empty(&body.group_name, "group_name")?;
    crate::sso::parse_role(&body.role)?;
    // a mapping may only grant inside its own org, or an IdP admin could hand
    // their users a foothold in someone else's tenant
    let scope_org = match (body.team_id, body.project_id) {
        (_, Some(project)) => ScopeChain::from_project(pool(&state), project)
            .await?
            .org
            .unwrap_or_default(),
        (Some(team), None) => ScopeChain::from_team(pool(&state), team)
            .await?
            .org
            .unwrap_or_default(),
        (None, None) => org_id,
    };
    if scope_org != org_id {
        return Err(invalid(
            "a group mapping may only grant a role inside its own org",
        ));
    }
    let mapping = ScimGroupMappingRepo(pool(&state))
        .create(
            org_id,
            &body.group_name,
            body.team_id,
            body.project_id,
            &body.role,
        )
        .await?;
    // take effect now rather than at the next sync. a new mapping only adds
    // grants, so the last-admin guard stays on whoever created it
    reconcile_group_members(&state, org_id, &mapping.group_name, true).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "scim_group_mapping.create",
        "scim_group_mapping",
        mapping.id,
        json!({"group": mapping.group_name, "role": mapping.role}),
    )
    .await;
    Ok(Json(mapping))
}

async fn list_mappings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ScimGroupMapping>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("scim_group_mapping", Read),
    )
    .await?;
    Ok(Json(ScimGroupMappingRepo(pool(&state)).list(org_id).await?))
}

async fn delete_mapping(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = ScimGroupMappingRepo(pool(&state));
    let mapping = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(mapping.org_id),
        cap!("scim_group_mapping", Delete),
    )
    .await?;
    repo.delete(id).await?;
    // the role the mapping granted goes away with it. a superadmin may take an
    // org's last admin grant with it, as delete_membership allows (#2673)
    let protect = !matches!(principal, Principal::Superadmin { .. });
    reconcile_group_members(&state, mapping.org_id, &mapping.group_name, protect).await?;
    log_audit(
        &state,
        &principal,
        Some(mapping.org_id),
        "scim_group_mapping.delete",
        "scim_group_mapping",
        id,
        json!({"group": mapping.group_name, "role": mapping.role}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Reconcile everyone in the named group, if the IdP has told us about it yet.
/// A mapping may legitimately be written before the group is provisioned.
async fn reconcile_group_members(
    state: &ControlState,
    org_id: Uuid,
    group_name: &str,
    protect: bool,
) -> ApiResult<()> {
    let repo = ScimGroupRepo(pool(state));
    let Some(group) = repo.find_by_display_name(org_id, group_name).await? else {
        return Ok(());
    };
    for user_id in repo.members(group.id).await? {
        reconcile_user(state, org_id, user_id, protect).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn mapping(team: Option<Uuid>, project: Option<Uuid>, role: &str) -> ScimGroupMapping {
        ScimGroupMapping {
            id: Uuid::nil(),
            org_id: Uuid::from_u128(1),
            group_name: "platform".into(),
            team_id: team,
            project_id: project,
            role: role.into(),
            created_at: Utc::now(),
        }
    }

    fn membership(org: Option<Uuid>, team: Option<Uuid>, role: &str, source: &str) -> Membership {
        Membership {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            org_id: org,
            team_id: team,
            project_id: None,
            role: role.into(),
            source: source.into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn parses_the_display_name_filter_idps_send() {
        assert_eq!(
            parse_display_name_filter("displayName eq \"platform\"").unwrap(),
            "platform"
        );
        // casing varies between IdPs
        assert_eq!(
            parse_display_name_filter("displayname eq \"sre\"").unwrap(),
            "sre"
        );
    }

    #[test]
    fn rejects_filters_it_cannot_honour() {
        // answering an unsupported filter with the whole directory reads to an
        // IdP as "no such group", and it then creates a duplicate
        for filter in [
            "userName eq \"ada\"",
            "displayName sw \"p\"",
            "displayName eq \"\"",
            "",
        ] {
            assert!(
                parse_display_name_filter(filter).is_err(),
                "accepted {filter}"
            );
        }
    }

    #[test]
    fn reads_members_in_every_shape_idps_send() {
        let objects = json!([{"value": "a", "display": "Ada"}, {"value": "b"}]);
        assert_eq!(member_values(&objects).unwrap(), vec!["a", "b"]);

        // azure sends bare ids in some connector versions
        let bare = json!(["a", "b"]);
        assert_eq!(member_values(&bare).unwrap(), vec!["a", "b"]);

        let single = json!({"value": "a"});
        assert_eq!(member_values(&single).unwrap(), vec!["a"]);

        assert!(member_values(&json!([])).unwrap().is_empty());
        assert!(member_values(&Value::Null).unwrap().is_empty());
        // a member entry with no id at all cannot be honoured silently
        assert!(member_values(&json!([{"display": "Ada"}])).is_err());
    }

    #[test]
    fn reads_the_member_out_of_a_filtered_remove_path() {
        assert_eq!(
            member_from_path("members[value eq \"11111111-1111-1111-1111-111111111111\"]")
                .as_deref(),
            Some("11111111-1111-1111-1111-111111111111")
        );
        // okta sends single quotes
        assert_eq!(
            member_from_path("members[value eq 'abc']").as_deref(),
            Some("abc")
        );
        // the plain attribute path carries no member and is handled elsewhere
        assert!(member_from_path("members").is_none());
        assert!(member_from_path("displayName").is_none());
    }

    #[test]
    fn a_mapping_grants_at_its_most_specific_scope() {
        let org = Uuid::from_u128(1);
        let team = Uuid::from_u128(2);
        // no team and no project means the grant is on the org itself
        assert_eq!(
            grant_of(&mapping(None, None, "viewer")),
            (Some(org), None, None, "viewer".to_string())
        );
        // a narrower scope carries the org implicitly, exactly as memberships
        // and sso_group_mappings do
        assert_eq!(
            grant_of(&mapping(Some(team), None, "member")),
            (None, Some(team), None, "member".to_string())
        );
    }

    #[test]
    fn only_an_equivalent_grant_counts_as_already_held() {
        let org = Uuid::from_u128(1);
        let team = Uuid::from_u128(2);
        let want = grant_of(&mapping(Some(team), None, "member"));
        assert!(grant_matches(
            &membership(None, Some(team), "member", "scim"),
            &want
        ));
        // a different role, or the same role at a wider scope, is not the same
        // grant and must not suppress the one the mapping asks for
        assert!(!grant_matches(
            &membership(None, Some(team), "viewer", "scim"),
            &want
        ));
        assert!(!grant_matches(
            &membership(Some(org), None, "member", "scim"),
            &want
        ));
    }
}
