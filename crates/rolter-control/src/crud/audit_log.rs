//! Audit log: the org-scoped and deployment-wide listings and their exports.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_store::postgres::models::AuditLogEntry;
use rolter_store::postgres::repo::{
    AuditLogCursor, AuditLogDirection, AuditLogFilter, AuditLogPage, AuditLogRepo,
};

use super::{invalid_field, normalized_filter, pool, ApiResult, Query};
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain};
use crate::rbac_matrix::{cap, superadmin_cap};
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/orgs/{org_id}/audit-log", get(list_audit_log))
        .route("/api/v1/audit-log", get(list_deployment_audit_log))
        .route(
            "/api/v1/orgs/{org_id}/audit-log/export",
            get(crate::audit_export::export_org_audit_log),
        )
        .route(
            "/api/v1/audit-log/export",
            get(crate::audit_export::export_deployment_audit_log),
        )
}

async fn list_audit_log(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<AuditLogQuery>,
) -> ApiResult<Json<AuditLogPageResponse>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("audit_log", Read),
    )
    .await?;
    let (filter, limit) = audit_log_filter(&query)?;
    let repo = AuditLogRepo(pool(&state));
    let page = repo.list_page(org_id, &filter, limit).await?;
    let total = if query.include_total {
        Some(repo.count(org_id, &filter).await?)
    } else {
        None
    };
    Ok(Json(audit_log_response(page, &filter, total)))
}

/// Every audit row in the deployment, org-less account events included. Those
/// are the ones no org read returns: a superadmin's own sign-ins, attempts
/// against an unregistered address and the events of someone removed from
/// every org (#1858).
// a security auditor (#1834) reads it alongside a superadmin, by the capability table
async fn list_deployment_audit_log(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<AuditLogQuery>,
) -> ApiResult<Json<AuditLogPageResponse>> {
    authorize_superadmin(&principal, superadmin_cap!("deployment_audit_log", Read))?;
    let (filter, limit) = audit_log_filter(&query)?;
    let repo = AuditLogRepo(pool(&state));
    let page = repo.list_page_all(&filter, limit).await?;
    let total = if query.include_total {
        Some(repo.count_all(&filter).await?)
    } else {
        None
    };
    Ok(Json(audit_log_response(page, &filter, total)))
}

/// Parse the audit-log query string into a store filter and a clamped page
/// size; shared by the per-org and the deployment-wide read.
fn audit_log_filter(query: &AuditLogQuery) -> ApiResult<(AuditLogFilter, i64)> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let filter = AuditLogFilter {
        actor_user_id: query.actor,
        action: normalized_filter(query.action.clone(), "action")?,
        target_type: normalized_filter(query.target_type.clone(), "target_type")?,
        start_at: query.start_at,
        end_at: query.end_at,
        cursor: query
            .cursor
            .as_deref()
            .map(parse_audit_cursor)
            .transpose()?,
        direction: query.direction.unwrap_or_default().into(),
    };
    if filter
        .start_at
        .is_some_and(|start| filter.end_at.is_some_and(|end| start > end))
    {
        return Err(invalid_field(
            "start_at",
            "start_at must be before or equal to end_at",
        ));
    }
    Ok((filter, limit))
}

fn audit_log_response(
    page: AuditLogPage,
    filter: &AuditLogFilter,
    total: Option<i64>,
) -> AuditLogPageResponse {
    let previous = matches!(filter.direction, AuditLogDirection::Previous);
    let has_next = if previous {
        !page.entries.is_empty()
    } else {
        page.has_more
    };
    let has_previous = if previous {
        page.has_more
    } else {
        filter.cursor.is_some()
    };
    let next_cursor = has_next
        .then(|| page.entries.last().map(encode_audit_cursor))
        .flatten();
    let previous_cursor = has_previous
        .then(|| page.entries.first().map(encode_audit_cursor))
        .flatten();
    AuditLogPageResponse {
        items: page.entries,
        next_cursor,
        previous_cursor,
        has_next,
        has_previous,
        total,
    }
}

#[derive(Deserialize)]
struct AuditLogQuery {
    limit: Option<i64>,
    #[serde(alias = "actor_user_id")]
    actor: Option<Uuid>,
    action: Option<String>,
    target_type: Option<String>,
    #[serde(alias = "from")]
    start_at: Option<DateTime<Utc>>,
    #[serde(alias = "to")]
    end_at: Option<DateTime<Utc>>,
    cursor: Option<String>,
    direction: Option<AuditLogQueryDirection>,
    #[serde(default)]
    include_total: bool,
}

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum AuditLogQueryDirection {
    #[default]
    Next,
    Previous,
}

impl From<AuditLogQueryDirection> for AuditLogDirection {
    fn from(direction: AuditLogQueryDirection) -> Self {
        match direction {
            AuditLogQueryDirection::Next => Self::Next,
            AuditLogQueryDirection::Previous => Self::Previous,
        }
    }
}

#[derive(Serialize)]
struct AuditLogPageResponse {
    items: Vec<AuditLogEntry>,
    next_cursor: Option<String>,
    previous_cursor: Option<String>,
    has_next: bool,
    has_previous: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<i64>,
}

pub(super) fn parse_audit_cursor(cursor: &str) -> ApiResult<AuditLogCursor> {
    let (at, id) = cursor
        .rsplit_once('|')
        .ok_or_else(|| invalid_field("cursor", "cursor must contain timestamp and id"))?;
    let at = DateTime::parse_from_rfc3339(at)
        .map_err(|_| invalid_field("cursor", "cursor has an invalid timestamp"))?
        .with_timezone(&Utc);
    let id =
        Uuid::parse_str(id).map_err(|_| invalid_field("cursor", "cursor has an invalid id"))?;
    Ok(AuditLogCursor { at, id })
}

pub(super) fn encode_audit_cursor(entry: &AuditLogEntry) -> String {
    format!(
        "{}|{}",
        entry.at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        entry.id
    )
}

#[cfg(test)]
mod audit_cursor_tests {
    use super::*;
    use crate::crud::ApiError;
    use rolter_core::Error;

    /// a 400 refusal, coded per field or not
    fn is_config_err<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(
            res,
            Err(ApiError::Core(Error::Config(_)) | ApiError::InvalidField { .. })
        )
    }

    #[test]
    fn audit_cursor_roundtrips_and_rejects_malformed_values() {
        let entry = AuditLogEntry {
            id: Uuid::nil(),
            org_id: None,
            actor_user_id: None,
            action: "route.create".to_string(),
            target_type: Some("route".to_string()),
            target_id: None,
            detail: None,
            at: Utc::now(),
        };
        let cursor = encode_audit_cursor(&entry);
        let parsed = parse_audit_cursor(&cursor).unwrap();
        assert_eq!(parsed.id, entry.id);
        assert_eq!(parsed.at, entry.at);
        assert!(is_config_err(parse_audit_cursor("bad")));
    }

    #[test]
    fn audit_filter_rejects_control_characters() {
        assert!(is_config_err(normalized_filter(
            Some("route\ncreate".to_string()),
            "action"
        )));
    }
}
