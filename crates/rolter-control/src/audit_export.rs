//! Streaming export of the filtered audit log as CSV or JSON (#3051).
//!
//! The dashboard's own export reads the log 500 rows at a time in the browser
//! and stops at 10,000. This is the same read without the cap: the handler
//! authorizes exactly like the page read it mirrors, then streams the body by
//! walking the log with the keyset cursor, one page in memory at a time, so a
//! year of changes costs one page of memory whatever its length.
//!
//! A browser without the File System Access API downloads it through a
//! single-use ticket instead (#3545, ADR-0041): `export_tickets` mints one for
//! either path after [`authorize_export`] and [`check_query`] have run, and
//! [`run_redeemed`] runs the export for it under the same capability check.

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::AuditLogEntry;
use rolter_store::postgres::repo::{
    AuditLogCursor, AuditLogDirection, AuditLogFilter, AuditLogRepo, OrgRepo, UserRepo,
};

use crate::analytics_export::{csv_cell, ExportKind, ExportProgress};
use crate::crud::{normalized_filter, pool, ApiError, ApiResult};
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain};
use crate::rbac_matrix::{cap, superadmin_cap};
use crate::ControlState;

/// Rows read per query. A page is the unit of memory, so this bounds what one
/// export holds however long the log is.
const PAGE_SIZE: i64 = 1000;

/// The action recorded when an export starts.
pub(crate) const EXPORT_ACTION: &str = "audit_log.export";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Json => "application/json",
        }
    }
}

/// The filters of the page read, minus the paging: an export has no cursor,
/// limit or direction of its own.
#[derive(Deserialize)]
pub(crate) struct ExportQuery {
    format: ExportFormat,
    #[serde(alias = "actor_user_id")]
    actor: Option<Uuid>,
    action: Option<String>,
    target_type: Option<String>,
    #[serde(alias = "from")]
    start_at: Option<DateTime<Utc>>,
    #[serde(alias = "to")]
    end_at: Option<DateTime<Utc>>,
}

impl ExportQuery {
    fn filter(&self) -> ApiResult<AuditLogFilter> {
        if self
            .start_at
            .is_some_and(|start| self.end_at.is_some_and(|end| start > end))
        {
            return Err(ApiError::Core(Error::Config(
                "start_at must be before or equal to end_at".to_string(),
            )));
        }
        Ok(AuditLogFilter {
            actor_user_id: self.actor,
            action: normalized_filter(self.action.clone(), "action")?,
            target_type: normalized_filter(self.target_type.clone(), "target_type")?,
            start_at: self.start_at,
            end_at: self.end_at,
            cursor: None,
            direction: AuditLogDirection::Next,
        })
    }
}

/// One exported row. Field order is the CSV column order.
#[derive(Serialize)]
struct ExportRow {
    at: String,
    actor_email: String,
    actor_user_id: String,
    action: String,
    target_type: String,
    target_id: String,
    org_id: String,
    entry_id: String,
    detail: Value,
}

const COLUMNS: [&str; 9] = [
    "at",
    "actor_email",
    "actor_user_id",
    "action",
    "target_type",
    "target_id",
    "org_id",
    "entry_id",
    "detail",
];

impl ExportRow {
    fn new(entry: &AuditLogEntry, emails: &HashMap<Uuid, String>) -> Self {
        let text = |id: Option<Uuid>| id.map(|id| id.to_string()).unwrap_or_default();
        Self {
            at: entry.at.to_rfc3339_opts(SecondsFormat::Millis, true),
            actor_email: entry
                .actor_user_id
                .and_then(|id| emails.get(&id).cloned())
                .unwrap_or_default(),
            actor_user_id: text(entry.actor_user_id),
            action: entry.action.clone(),
            target_type: entry.target_type.clone().unwrap_or_default(),
            target_id: text(entry.target_id),
            org_id: text(entry.org_id),
            entry_id: entry.id.to_string(),
            detail: entry.detail.clone().unwrap_or(Value::Null),
        }
    }

    fn cells(&self) -> [String; 9] {
        [
            self.at.clone(),
            self.actor_email.clone(),
            self.actor_user_id.clone(),
            self.action.clone(),
            self.target_type.clone(),
            self.target_id.clone(),
            self.org_id.clone(),
            self.entry_id.clone(),
            match &self.detail {
                Value::Null => String::new(),
                Value::String(text) => text.clone(),
                other => other.to_string(),
            },
        ]
    }
}

fn csv_header() -> String {
    format!("{}\r\n", COLUMNS.join(","))
}

fn csv_line(row: &ExportRow) -> String {
    let cells: Vec<String> = row.cells().iter().map(|c| csv_cell(c)).collect();
    format!("{}\r\n", cells.join(","))
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "org".to_string()
    } else {
        out.to_string()
    }
}

/// `audit-log-<org>-<from day>_to_<to day>.<ext>`, the dashboard's name for an
/// export: `all` stands for an open lower bound and today for an open upper one.
fn file_name(
    org: &str,
    filter: &AuditLogFilter,
    now: DateTime<Utc>,
    format: ExportFormat,
) -> String {
    let day = |at: DateTime<Utc>| at.format("%Y-%m-%d").to_string();
    let from = filter.start_at.map_or_else(|| "all".to_string(), day);
    let to = day(filter.end_at.unwrap_or(now));
    format!(
        "audit-log-{}-{from}_to_{to}.{}",
        slug(org),
        format.extension()
    )
}

/// Where the walk reads: one org's log, or the whole deployment's.
#[derive(Clone, Copy)]
enum Source {
    Org(Uuid),
    Deployment,
}

struct Walk {
    pool: PgPool,
    source: Source,
    filter: AuditLogFilter,
    format: ExportFormat,
    emails: HashMap<Uuid, String>,
    /// where a download ticket's export reports the rows written so far
    progress: Option<Arc<ExportProgress>>,
    /// data rows written so far
    written: u64,
    /// the header (or `[`) has not gone out yet
    started: bool,
    /// a JSON row has gone out, so the next one needs a separator
    wrote_row: bool,
    done: bool,
}

impl Walk {
    /// The next chunk of the body, or `None` once it is complete.
    async fn next_chunk(&mut self) -> Result<Option<Bytes>, Error> {
        if self.done {
            return Ok(None);
        }
        let mut out = String::new();
        if !self.started {
            self.started = true;
            match self.format {
                ExportFormat::Csv => out.push_str(&csv_header()),
                ExportFormat::Json => out.push_str("[\n"),
            }
        }
        let repo = AuditLogRepo(&self.pool);
        let page = match self.source {
            Source::Org(org_id) => repo.list_page(org_id, &self.filter, PAGE_SIZE).await?,
            Source::Deployment => repo.list_page_all(&self.filter, PAGE_SIZE).await?,
        };
        self.resolve_emails(&page.entries).await?;
        self.written += page.entries.len() as u64;
        if let Some(progress) = &self.progress {
            progress.set_rows(self.written);
        }
        for entry in &page.entries {
            let row = ExportRow::new(entry, &self.emails);
            match self.format {
                ExportFormat::Csv => out.push_str(&csv_line(&row)),
                ExportFormat::Json => {
                    if self.wrote_row {
                        out.push_str(",\n");
                    }
                    self.wrote_row = true;
                    let line = serde_json::to_string(&row).map_err(|err| {
                        Error::Config(format!("audit row did not serialize: {err}"))
                    })?;
                    out.push_str(&line);
                }
            }
        }
        match page.entries.last() {
            Some(last) if page.has_more => {
                // keyset: the next page starts strictly after this row, so rows
                // written during the export never shift or repeat a page
                self.filter.cursor = Some(AuditLogCursor {
                    at: last.at,
                    id: last.id,
                });
            }
            _ => {
                self.done = true;
                if self.format == ExportFormat::Json {
                    out.push_str("\n]\n");
                }
            }
        }
        Ok(Some(Bytes::from(out)))
    }

    /// Names the actors of `entries` that earlier pages did not. An id with no
    /// account (deleted since) stays blank, as it does in the dashboard.
    async fn resolve_emails(&mut self, entries: &[AuditLogEntry]) -> Result<(), Error> {
        let missing: Vec<Uuid> = entries
            .iter()
            .filter_map(|e| e.actor_user_id)
            .filter(|id| !self.emails.contains_key(id))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        for (id, email) in UserRepo(&self.pool).emails_by_ids(&missing).await? {
            self.emails.insert(id, email);
        }
        Ok(())
    }
}

fn response(walk: Walk, name: &str, format: ExportFormat) -> ApiResult<Response> {
    // an error mid-walk ends the body with an io error, so the client sees a
    // failed download rather than a file that looks complete and is not
    let body = Body::from_stream(stream::unfold(walk, |mut walk| async move {
        match walk.next_chunk().await {
            Ok(Some(chunk)) => Some((Ok::<_, io::Error>(chunk), walk)),
            Ok(None) => None,
            Err(err) => {
                tracing::error!(error = %err, "audit export failed part way");
                walk.done = true;
                Some((Err(io::Error::other("audit export failed")), walk))
            }
        }
    }));
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
        .map_err(|_| ApiError::Curated("export file name is not a valid header".to_string()))?;
    headers.insert(header::CONTENT_DISPOSITION, disposition);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

/// Records who started an export and under which filters. Written before the
/// first byte goes out: an export the client abandons part way still happened.
async fn record(
    state: &ControlState,
    principal: &Principal,
    org_id: Option<Uuid>,
    query: &ExportQuery,
    filter: &AuditLogFilter,
) -> ApiResult<()> {
    let detail = json!({
        "format": query.format,
        "scope": if org_id.is_some() { "org" } else { "deployment" },
        "actor": filter.actor_user_id,
        "action": filter.action,
        "target_type": filter.target_type,
        "start_at": filter.start_at,
        "end_at": filter.end_at,
    });
    AuditLogRepo(pool(state))
        .create(
            org_id,
            principal.account_id(),
            EXPORT_ACTION,
            Some("audit_log"),
            org_id,
            Some(detail),
        )
        .await?;
    Ok(())
}

/// The capability check an export of `kind` takes: the page read's, so an
/// export is never wider than what the caller can page. Run by the bearer
/// routes, by the mint of a download ticket and again when it is redeemed.
pub(crate) async fn authorize_export(
    state: &ControlState,
    principal: &Principal,
    kind: ExportKind,
) -> ApiResult<()> {
    match kind {
        ExportKind::OrgAudit(org_id) => {
            authorize(
                state,
                principal,
                ScopeChain::org(org_id),
                cap!("audit_log", Read),
            )
            .await
        }
        ExportKind::DeploymentAudit => {
            authorize_superadmin(principal, superadmin_cap!("deployment_audit_log", Read))
        }
        // not an audit export: nothing here may run it
        ExportKind::Attribution | ExportKind::Timeseries | ExportKind::Invocations => {
            Err(ApiError::Forbidden)
        }
    }
}

/// Parse a stored query as the export would, so a ticket for a query it would
/// refuse is refused when it is minted.
#[allow(clippy::result_large_err)]
pub(crate) async fn check_query(parts: &mut Parts, state: &ControlState) -> Result<(), Response> {
    let Query(query) = Query::<ExportQuery>::from_request_parts(parts, state)
        .await
        .map_err(IntoResponse::into_response)?;
    query
        .filter()
        .map(drop)
        .map_err(IntoResponse::into_response)
}

/// Run the export `kind` for a redeemed download ticket, as the bearer route
/// does: the capability check again for the principal the ticket re-resolved
/// to, the audit row and the walk, reporting its rows to `progress`.
pub(crate) async fn run_redeemed(
    state: ControlState,
    kind: ExportKind,
    mut parts: Parts,
    principal: Principal,
    progress: Arc<ExportProgress>,
) -> Response {
    let query = match Query::<ExportQuery>::from_request_parts(&mut parts, &state).await {
        Ok(Query(query)) => query,
        Err(refused) => return refused.into_response(),
    };
    export(state, principal, kind, query, Some(progress))
        .await
        .into_response()
}

/// An export of `kind` for `principal`: authorized, recorded, then streamed.
async fn export(
    state: ControlState,
    principal: Principal,
    kind: ExportKind,
    query: ExportQuery,
    progress: Option<Arc<ExportProgress>>,
) -> ApiResult<Response> {
    authorize_export(&state, &principal, kind).await?;
    let filter = query.filter()?;
    let (org_id, source, label) = match kind {
        ExportKind::OrgAudit(org_id) => {
            let org = OrgRepo(pool(&state)).get(org_id).await?;
            (Some(org_id), Source::Org(org_id), org.slug)
        }
        _ => (None, Source::Deployment, "deployment".to_string()),
    };
    record(&state, &principal, org_id, &query, &filter).await?;
    let name = file_name(&label, &filter, Utc::now(), query.format);
    let walk = Walk {
        pool: pool(&state).clone(),
        source,
        filter,
        format: query.format,
        emails: HashMap::new(),
        progress,
        written: 0,
        started: false,
        wrote_row: false,
        done: false,
    };
    response(walk, &name, query.format)
}

/// `GET /api/v1/orgs/{org_id}/audit-log/export`: gated and scoped exactly as
/// the page read, so an export is never wider than what the caller can page.
pub(crate) async fn export_org_audit_log(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Response> {
    export(state, principal, ExportKind::OrgAudit(org_id), query, None).await
}

/// `GET /api/v1/audit-log/export`: the deployment-wide log, superadmin only
/// like the page read.
pub(crate) async fn export_deployment_audit_log(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Response> {
    export(state, principal, ExportKind::DeploymentAudit, query, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_formula_cell_is_defused_like_the_dashboard_does() {
        for lead in ["=", "+", "-", "@", "\t", "\r"] {
            let cell = csv_cell(&format!("{lead}cmd"));
            assert!(
                cell.trim_start_matches('"').starts_with('\''),
                "{lead:?} -> {cell:?}"
            );
        }
        assert_eq!(csv_cell("=1+1"), "'=1+1");
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("line\nbreak"), "\"line\nbreak\"");
        // the carriage return both defuses and forces quoting
        assert_eq!(csv_cell("\rx"), "\"'\rx\"");
        assert_eq!(csv_cell(""), "");
    }

    #[test]
    fn a_name_matches_the_dashboards() {
        let now = Utc.with_ymd_and_hms(2026, 10, 13, 8, 0, 0).unwrap();
        let open = AuditLogFilter::default();
        assert_eq!(
            file_name("Acme Corp!", &open, now, ExportFormat::Csv),
            "audit-log-acme-corp-all_to_2026-10-13.csv"
        );
        let bounded = AuditLogFilter {
            start_at: Some(Utc.with_ymd_and_hms(2026, 1, 2, 3, 0, 0).unwrap()),
            end_at: Some(Utc.with_ymd_and_hms(2026, 2, 3, 0, 0, 0).unwrap()),
            ..Default::default()
        };
        assert_eq!(
            file_name("", &bounded, now, ExportFormat::Json),
            "audit-log-org-2026-01-02_to_2026-02-03.json"
        );
    }

    #[test]
    fn a_row_without_detail_leaves_the_cell_empty() {
        let entry = AuditLogEntry {
            id: Uuid::nil(),
            org_id: None,
            actor_user_id: None,
            action: "auth.login".to_string(),
            target_type: None,
            target_id: None,
            detail: None,
            at: Utc.with_ymd_and_hms(2026, 10, 13, 8, 0, 0).unwrap(),
        };
        let row = ExportRow::new(&entry, &HashMap::new());
        assert_eq!(row.cells()[0], "2026-10-13T08:00:00.000Z");
        assert_eq!(row.cells()[8], "");
    }
}
