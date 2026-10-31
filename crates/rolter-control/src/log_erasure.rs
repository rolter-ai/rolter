//! Erasing one subject's request logs and captured payloads on request (#1085).
//!
//! Retention is a clock; this is a command. `POST /api/v1/log-erasures` names a
//! subject — a virtual key, a user, an organization, or a single request — and
//! deletes what ClickHouse holds about it: the `request_logs` rows, the raw
//! bodies in `request_payloads` and the MCP tool calls in `mcp_tool_call_logs`
//! (#3278). Nothing else derives from those rows (there is
//! no rollup or materialized view over them), so deleting them is deleting
//! everything the control plane can reconstitute them from.
//!
//! ClickHouse applies `alter table … delete` as an asynchronous mutation, so
//! the call answers `202` with a record whose `status` is `pending`, and
//! `GET /api/v1/log-erasures/{id}` reads `system.mutations` to report when every
//! submitted mutation is done. The record itself lives in Postgres and the
//! action is audited there, so the erasure is never undone by what it erased.
//! The audit entry names the scope and the reason, never the content.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use rolter_store::postgres::models::LogErasure;
use rolter_store::postgres::repo::{
    AuditLogRepo, LogErasureRepo, NewLogErasure, SUBJECT_LOOKUP_LIMIT,
};

use crate::analytics::{client_or_503, query_failed, ClickHouseClient};
use crate::crud::{invalid_field, pool, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/log-erasures",
            get(list_erasures).post(create_erasure),
        )
        .route("/api/v1/log-erasures/subjects", get(find_subjects))
        .route("/api/v1/log-erasures/{id}", get(get_erasure))
}

#[derive(Deserialize)]
struct SubjectQuery {
    /// `virtual_key`, `user` or `org`
    kind: String,
    /// part of a name or email; empty lists the first few
    #[serde(default)]
    q: String,
    /// one subject by id, to name an id someone pasted
    id: Option<Uuid>,
}

/// `GET /api/v1/log-erasures/subjects`: the named virtual keys, users or
/// organizations the erasure form offers, found by part of a name or email
/// (#3385). An id typed by hand is a mistake waiting to erase another tenant's
/// logs, and an id says nothing of whose data it is; this puts a name beside
/// it. Gated as the erasure itself is, so it reads nothing a caller who could
/// not erase could use.
async fn find_subjects(
    principal: Principal,
    State(state): State<ControlState>,
    crate::crud::Query(query): crate::crud::Query<SubjectQuery>,
) -> ApiResult<Json<Value>> {
    authorize_superadmin(&principal, superadmin_cap!("log_erasure", Create))?;
    if !matches!(query.kind.as_str(), "virtual_key" | "user" | "org") {
        return Err(invalid_field(
            "kind",
            "kind must be one of virtual_key|user|org",
        ));
    }
    let mut subjects = LogErasureRepo(pool(&state))
        .find_subjects(&query.kind, query.q.trim(), query.id)
        .await?;
    // the store reads one past the cap, which is how a cut list is told apart
    let truncated = subjects.len() as i64 > SUBJECT_LOOKUP_LIMIT;
    subjects.truncate(SUBJECT_LOOKUP_LIMIT as usize);
    Ok(Json(json!({ "data": subjects, "truncated": truncated })))
}

/// The most payload rows from before the `virtual_key_id` column that one
/// request will chase through their log rows.
const LEGACY_PAYLOAD_LIMIT: u32 = 10_000;

/// How far from a request's instant its MCP tool-call row may be stamped, in
/// seconds, for a `request` scope that names `ts`.
const MCP_TS_WINDOW_SECONDS: u32 = 300;

/// How long a `pending` erasure may go without ClickHouse listing any of its
/// mutations before it is reported `unverified`: the server was replaced or
/// pruned its mutation history, and completion can no longer be read.
const UNVERIFIED_AFTER: chrono::Duration = chrono::Duration::hours(1);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ScopeBody {
    /// every row sent with this virtual key
    VirtualKey { id: Uuid },
    /// every row sent with a virtual key this user created and that still exists
    User { id: Uuid },
    /// every row of this organization
    Org { id: Uuid },
    /// one request. `request_id` is whatever the caller sent and is not unique,
    /// so the organization is required and `ts` narrows it to one instant
    Request {
        request_id: String,
        org_id: Uuid,
        #[serde(default)]
        ts: Option<DateTime<Utc>>,
    },
}

#[derive(Deserialize)]
struct CreateErasure {
    scope: ScopeBody,
    /// why, for the audit trail; required
    reason: String,
    /// `false` erases the captured bodies only and keeps the log metadata,
    /// which is the usual answer to a secret pasted into one prompt
    #[serde(default = "yes")]
    include_logs: bool,
}

fn yes() -> bool {
    true
}

/// What the ClickHouse statements filter on, as SQL text. Built only from
/// validated ids and escaped strings, since `alter table … delete` takes no
/// query parameters.
#[derive(Debug, PartialEq, Eq)]
struct Predicates {
    logs: String,
    payloads: String,
    /// the same subject in `mcp_tool_call_logs`, which carries `org_id`,
    /// `virtual_key_id`, `user_id` and `request_id` itself. Its `user_id` is the
    /// key's owner, so a user scope also matches the keys that user created
    mcp: String,
    /// the payload column that is empty on rows written before it existed, and
    /// so cannot be matched by `payloads`; `None` for a request scope
    legacy_column: Option<&'static str>,
}

fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn key_list(keys: &[Uuid]) -> String {
    keys.iter()
        .map(|key| quoted(&key.to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn predicates(scope: &ScopeBody, user_keys: &[Uuid]) -> Predicates {
    match scope {
        ScopeBody::VirtualKey { id } => {
            let p = format!("virtual_key_id = {}", quoted(&id.to_string()));
            Predicates {
                logs: p.clone(),
                payloads: p.clone(),
                mcp: p,
                legacy_column: Some("virtual_key_id"),
            }
        }
        ScopeBody::User { id } if user_keys.is_empty() => {
            // no key to find request rows by: only the MCP log, whose rows carry
            // the owner's own id, has anything of this user's (#3278). `1 = 0`
            // keeps the request statements valid while matching nothing
            Predicates {
                logs: "1 = 0".to_string(),
                payloads: "1 = 0".to_string(),
                mcp: format!("user_id = {}", quoted(&id.to_string())),
                legacy_column: None,
            }
        }
        ScopeBody::User { id } => {
            let p = format!("virtual_key_id in ({})", key_list(user_keys));
            Predicates {
                logs: p.clone(),
                payloads: p.clone(),
                mcp: format!("user_id = {} or {p}", quoted(&id.to_string())),
                legacy_column: Some("virtual_key_id"),
            }
        }
        ScopeBody::Org { id } => {
            let p = format!("org_id = {}", quoted(&id.to_string()));
            Predicates {
                logs: p.clone(),
                payloads: p.clone(),
                mcp: p,
                legacy_column: Some("org_id"),
            }
        }
        ScopeBody::Request {
            request_id,
            org_id,
            ts,
        } => {
            let mut p = format!(
                "request_id = {} and org_id = {}",
                quoted(request_id),
                quoted(&org_id.to_string())
            );
            let mut mcp = p.clone();
            if let Some(ts) = ts {
                let instant = quoted(&ts.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
                p.push_str(&format!(" and ts = toDateTime64({instant}, 3, 'UTC')"));
                // the MCP row is stamped when the call was made, not with the
                // instant of the request-log row the caller read `ts` from, so
                // it is matched inside a window around it
                mcp.push_str(&format!(
                    " and ts between toDateTime64({instant}, 3, 'UTC') - interval {MCP_TS_WINDOW_SECONDS} second \
                     and toDateTime64({instant}, 3, 'UTC') + interval {MCP_TS_WINDOW_SECONDS} second"
                ));
            }
            Predicates {
                logs: p.clone(),
                payloads: p,
                mcp,
                legacy_column: None,
            }
        }
    }
}

/// Which stores an erasure searched and which it skipped, as the list stored on
/// the record: `{store, searched, code?, reason?}` (#3436). Request logs are found by
/// virtual key, so a user who holds none leaves them unsearched while their MCP
/// tool calls, which carry `user_id`, are still erased; a payloads-only erasure
/// keeps the log rows. Saying so is what stops the record reading as complete
/// over every store.
fn coverage(scope: &ScopeBody, user_keys: &[Uuid], include_logs: bool) -> Value {
    let keyless = matches!(scope, ScopeBody::User { .. }) && user_keys.is_empty();
    let found = |store: &str| json!({"store": store, "searched": true});
    // `code` is the stable name a client translates; `reason` is the English
    // sentence for one that does not know the code
    let skipped = |store: &str, code: &str, reason: &str| json!({"store": store, "searched": false, "code": code, "reason": reason});
    const NO_KEYS: &str = "the user holds no virtual keys, and request logs are found by key";
    let logs = if keyless {
        skipped("request_logs", "no_key", NO_KEYS)
    } else if include_logs {
        found("request_logs")
    } else {
        skipped(
            "request_logs",
            "bodies_only",
            "only captured payloads were asked for; the log rows are kept",
        )
    };
    let payloads = if keyless {
        skipped("request_payloads", "no_key", NO_KEYS)
    } else {
        found("request_payloads")
    };
    json!([logs, payloads, found("mcp_tool_call_logs")])
}

/// The statements that carry out one erasure. `marker` is the erasure's id: it
/// rides in each `where` as a constant-true clause so the mutation can be found
/// again in `system.mutations`, whose `command` keeps the statement's text.
/// `legacy_log_ids` are the log ids of payload rows that predate the column the
/// predicate reads.
fn statements(
    predicates: &Predicates,
    marker: Uuid,
    include_logs: bool,
    legacy_log_ids: &[String],
) -> Vec<String> {
    let marker = quoted(&marker.to_string());
    let mut out = Vec::new();
    if !legacy_log_ids.is_empty() {
        let ids = legacy_log_ids
            .iter()
            .map(|id| quoted(id))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "alter table request_payloads delete where log_id in ({ids}) and {marker} != ''"
        ));
    }
    out.push(format!(
        "alter table request_payloads delete where ({}) and {marker} != ''",
        predicates.payloads
    ));
    if include_logs {
        out.push(format!(
            "alter table request_logs delete where ({}) and {marker} != ''",
            predicates.logs
        ));
        out.push(format!(
            "alter table mcp_tool_call_logs delete where ({}) and {marker} != ''",
            predicates.mcp
        ));
    } else {
        // the captured arguments and result sit in the same row as the call's
        // metadata, so keeping the metadata means blanking those two columns
        out.push(format!(
            "alter table mcp_tool_call_logs update arguments = '', result = '' \
             where ({}) and {marker} != ''",
            predicates.mcp
        ));
    }
    out
}

/// The `select` that finds payload rows written before their subject column
/// existed, through the log rows that do carry it.
fn legacy_lookup_sql(predicates: &Predicates, column: &str) -> String {
    format!(
        "select distinct log_id from request_payloads \
         where {column} = '' and log_id != '' \
           and log_id in (select log_id from request_logs where ({}) and log_id != '') \
         limit {LEGACY_PAYLOAD_LIMIT} format JSON",
        predicates.logs
    )
}

fn validate(body: &CreateErasure) -> ApiResult<()> {
    let reason = body.reason.trim();
    if reason.is_empty() || reason.len() > 500 || reason.chars().any(char::is_control) {
        return Err(invalid_field(
            "reason",
            "reason must be 1-500 visible characters",
        ));
    }
    if let ScopeBody::Request { request_id, .. } = &body.scope {
        if request_id.is_empty()
            || request_id.len() > 256
            || request_id.chars().any(char::is_control)
        {
            return Err(invalid_field(
                "request_id",
                "request_id must be 1-256 visible characters",
            ));
        }
    }
    Ok(())
}

fn scope_parts(scope: &ScopeBody) -> (&'static str, String, Option<Uuid>, Option<DateTime<Utc>>) {
    match scope {
        ScopeBody::VirtualKey { id } => ("virtual_key", id.to_string(), None, None),
        ScopeBody::User { id } => ("user", id.to_string(), None, None),
        ScopeBody::Org { id } => ("org", id.to_string(), Some(*id), None),
        ScopeBody::Request {
            request_id,
            org_id,
            ts,
        } => ("request", request_id.clone(), Some(*org_id), *ts),
    }
}

/// The erasure as the API shows it: the record, plus how far ClickHouse has got.
#[derive(Serialize)]
struct ErasureView {
    #[serde(flatten)]
    erasure: LogErasure,
    /// mutations ClickHouse reports done, of `erasure.statements` submitted;
    /// absent when nothing was asked of ClickHouse
    #[serde(skip_serializing_if = "Option::is_none")]
    mutations_done: Option<u32>,
    /// the reason a mutation last failed on the server, while one is retrying
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

impl ErasureView {
    fn plain(erasure: LogErasure) -> Self {
        Self {
            erasure,
            mutations_done: None,
            last_error: None,
        }
    }
}

async fn create_erasure(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateErasure>,
) -> ApiResult<Response> {
    authorize_superadmin(&principal, superadmin_cap!("log_erasure", Create))?;
    validate(&body)?;
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return Ok(response),
    };
    let repo = LogErasureRepo(pool(&state));
    let user_keys = match &body.scope {
        ScopeBody::User { id } => repo.key_ids_created_by(*id).await?,
        _ => Vec::new(),
    };
    let predicates = predicates(&body.scope, &user_keys);
    let (kind, scope_id, org_id, request_ts) = scope_parts(&body.scope);
    let id = Uuid::new_v4();
    repo.create(NewLogErasure {
        id,
        scope_kind: kind,
        scope_id: &scope_id,
        org_id,
        request_ts,
        include_logs: body.include_logs,
        reason: body.reason.trim(),
        requested_by: principal.account_id(),
        stores: coverage(&body.scope, &user_keys, body.include_logs),
    })
    .await?;

    let submitted = submit(ch, &predicates, id, body.include_logs).await;
    let (statements, error) = match &submitted {
        Ok(count) => (*count as i32, None),
        Err(message) => (0, Some(message.as_str())),
    };
    let record = repo.record_submission(id, statements, error).await?;

    // what the entry holds is who, what scope and why — the ids that name the
    // subject, never a prompt, a completion or a key. it is written whether or
    // not ClickHouse accepted the request, since asking is itself the event
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            org_id,
            principal.account_id(),
            "log_erasure.request",
            Some("log_erasure"),
            Some(id),
            Some(json!({
                "scope": kind,
                "scope_id": scope_id,
                "include_logs": body.include_logs,
                "reason": record.reason,
                "statements": record.statements,
                "stores": record.stores,
                "outcome": record.status,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write log erasure audit entry");
    }
    let status = if record.status == "failed" {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, Json(ErasureView::plain(record))).into_response())
}

/// Send the statements to ClickHouse. A refusal is returned as text safe to
/// store and show: the server's own words carry the SQL, so the record gets a
/// fixed sentence and the detail goes to the log.
async fn submit(
    ch: &ClickHouseClient,
    predicates: &Predicates,
    marker: Uuid,
    include_logs: bool,
) -> Result<usize, String> {
    let mut legacy = Vec::new();
    if let Some(column) = predicates.legacy_column {
        let rows = ch
            .query(&legacy_lookup_sql(predicates, column), &[])
            .await
            .map_err(|err| {
                tracing::warn!(error = %format!("{err:#}"), "log erasure could not look up older payload rows");
                "clickhouse refused the lookup of older payload rows; apply \
                 clickhouse/020_request_payloads_key.sql and retry"
                    .to_string()
            })?;
        legacy = rows
            .iter()
            .filter_map(|row| {
                row.get("log_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
    }
    let all = statements(predicates, marker, include_logs, &legacy);
    for (index, statement) in all.iter().enumerate() {
        if let Err(err) = ch.execute_ddl(statement).await {
            tracing::warn!(error = %format!("{err:#}"), submitted = index, "log erasure statement failed");
            return Err(format!(
                "clickhouse refused statement {} of {}; earlier statements were submitted, \
                 so submit the erasure again",
                index + 1,
                all.len()
            ));
        }
    }
    Ok(all.len())
}

async fn list_erasures(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Value>> {
    authorize_superadmin(&principal, superadmin_cap!("log_erasure", Read))?;
    let rows = LogErasureRepo(pool(&state)).list(100).await?;
    Ok(Json(json!({ "data": rows })))
}

async fn get_erasure(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    authorize_superadmin(&principal, superadmin_cap!("log_erasure", Read))?;
    let repo = LogErasureRepo(pool(&state));
    let Some(record) = repo.get(id).await? else {
        return Err(ApiError::Core(rolter_core::Error::NotFound(
            "log erasure not found".to_string(),
        )));
    };
    if record.status != "pending" {
        return Ok(Json(ErasureView::plain(record)).into_response());
    }
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return Ok(response),
    };
    let rows = match ch.query(&progress_sql(id), &[]).await {
        Ok(rows) => rows,
        Err(err) => return Ok(query_failed("log erasure progress read failed", &err)),
    };
    let progress = Progress::from_rows(&rows);
    let verdict = progress.verdict(
        record.statements.max(0) as u32,
        Utc::now() - record.created_at,
    );
    let mut view = ErasureView::plain(record);
    view.mutations_done = Some(progress.done);
    view.last_error = progress.last_error;
    if let Some(status) = verdict {
        if let Some(settled) = repo.settle(id, status).await? {
            if status == "completed" {
                audit_completion(&state, &settled).await;
            }
            view.erasure = settled;
        }
    }
    Ok(Json(view).into_response())
}

async fn audit_completion(state: &ControlState, record: &LogErasure) {
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            record.org_id,
            None,
            "log_erasure.complete",
            Some("log_erasure"),
            Some(record.id),
            Some(json!({
                "scope": record.scope_kind,
                "scope_id": record.scope_id,
                "completed_at": record
                    .completed_at
                    .map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true)),
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write log erasure completion audit entry");
    }
}

fn progress_sql(marker: Uuid) -> String {
    format!(
        "select is_done, latest_fail_reason from system.mutations \
         where database = currentDatabase() and table in ('request_logs', 'request_payloads', 'mcp_tool_call_logs') \
           and position(command, {}) > 0 format JSON",
        quoted(&marker.to_string())
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Progress {
    seen: u32,
    done: u32,
    last_error: Option<String>,
}

impl Progress {
    fn from_rows(rows: &[Value]) -> Self {
        let mut progress = Self::default();
        for row in rows {
            progress.seen += 1;
            // ClickHouse's JSON output writes UInt8 as a number
            let done = match row.get("is_done") {
                Some(Value::Number(n)) => n.as_u64() == Some(1),
                Some(Value::Bool(b)) => *b,
                _ => false,
            };
            if done {
                progress.done += 1;
            } else if let Some(reason) = row
                .get("latest_fail_reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            {
                // the server's own words can name tables and paths; the first
                // line is the reason
                progress.last_error = Some(reason.lines().next().unwrap_or_default().to_string());
            }
        }
        progress
    }

    /// The status the erasure moves to, if it moves: `completed` once every
    /// submitted mutation is done, `unverified` when none can be found after
    /// [`UNVERIFIED_AFTER`].
    fn verdict(&self, submitted: u32, age: chrono::Duration) -> Option<&'static str> {
        if submitted > 0 && self.done >= submitted {
            Some("completed")
        } else if self.seen == 0 && age > UNVERIFIED_AFTER {
            Some("unverified")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn a_key_scope_deletes_payloads_before_logs_and_marks_each_statement() {
        let scope = ScopeBody::VirtualKey { id: uuid(7) };
        let p = predicates(&scope, &[]);
        let sql = statements(&p, uuid(9), true, &[]);
        assert_eq!(sql.len(), 3);
        assert!(sql[0].starts_with("alter table request_payloads delete where"));
        assert!(sql[1].starts_with("alter table request_logs delete where"));
        assert!(sql[2].starts_with("alter table mcp_tool_call_logs delete where"));
        for statement in &sql {
            assert!(statement.contains(&format!("'{}'", uuid(7))));
            assert!(statement.contains(&format!("and '{}' != ''", uuid(9))));
        }
    }

    #[test]
    fn payloads_only_leaves_the_log_rows_alone() {
        let p = predicates(&ScopeBody::Org { id: uuid(1) }, &[]);
        let sql = statements(&p, uuid(2), false, &[]);
        assert_eq!(sql.len(), 2);
        assert!(sql[0].contains("request_payloads"));
        assert!(!sql
            .iter()
            .any(|s| s.starts_with("alter table request_logs")));
    }

    #[test]
    fn payloads_only_blanks_the_mcp_bodies_and_keeps_their_rows() {
        let p = predicates(&ScopeBody::Org { id: uuid(1) }, &[]);
        let sql = statements(&p, uuid(2), false, &[]);
        assert_eq!(
            sql[1],
            format!(
                "alter table mcp_tool_call_logs update arguments = '', result = '' \
                 where (org_id = '{}') and '{}' != ''",
                uuid(1),
                uuid(2)
            )
        );
    }

    #[test]
    fn each_scope_reaches_the_mcp_rows_through_the_columns_that_table_carries() {
        assert_eq!(
            predicates(&ScopeBody::VirtualKey { id: uuid(7) }, &[]).mcp,
            format!("virtual_key_id = '{}'", uuid(7))
        );
        assert_eq!(
            predicates(&ScopeBody::Org { id: uuid(7) }, &[]).mcp,
            format!("org_id = '{}'", uuid(7))
        );
        // a key's owner is the MCP row's user_id, which may not be its creator
        assert_eq!(
            predicates(&ScopeBody::User { id: uuid(5) }, &[uuid(1)]).mcp,
            format!(
                "user_id = '{}' or virtual_key_id in ('{}')",
                uuid(5),
                uuid(1)
            )
        );
        let request = predicates(
            &ScopeBody::Request {
                request_id: "r".into(),
                org_id: uuid(3),
                ts: None,
            },
            &[],
        );
        assert_eq!(request.mcp, request.logs);
    }

    #[test]
    fn the_mcp_mutation_is_found_again_by_its_marker() {
        assert!(progress_sql(uuid(4)).contains("'mcp_tool_call_logs'"));
    }

    #[test]
    fn rows_older_than_the_payload_column_are_deleted_by_log_id() {
        let p = predicates(&ScopeBody::VirtualKey { id: uuid(1) }, &[]);
        let sql = statements(&p, uuid(2), true, &["a'b".to_string()]);
        assert_eq!(sql.len(), 4);
        assert!(sql[0].contains("log_id in ('a\\'b')"));
        assert!(legacy_lookup_sql(&p, "virtual_key_id").contains("virtual_key_id = ''"));
    }

    #[test]
    fn a_user_scope_is_every_key_the_user_created() {
        let scope = ScopeBody::User { id: uuid(5) };
        let p = predicates(&scope, &[uuid(1), uuid(2)]);
        assert_eq!(
            p.logs,
            format!("virtual_key_id in ('{}', '{}')", uuid(1), uuid(2))
        );
    }

    #[test]
    fn a_request_scope_pins_the_tenant_and_escapes_the_callers_id() {
        let scope = ScopeBody::Request {
            request_id: "x' or 1=1 --\\".to_string(),
            org_id: uuid(3),
            ts: None,
        };
        let p = predicates(&scope, &[]);
        assert_eq!(
            p.logs,
            format!(
                "request_id = 'x\\' or 1=1 --\\\\' and org_id = '{}'",
                uuid(3)
            )
        );
        assert_eq!(p.legacy_column, None);
    }

    #[test]
    fn a_request_instant_is_written_as_a_millisecond_literal() {
        let ts = DateTime::parse_from_rfc3339("2026-10-27T12:30:45.123Z")
            .unwrap()
            .with_timezone(&Utc);
        let scope = ScopeBody::Request {
            request_id: "r".into(),
            org_id: uuid(3),
            ts: Some(ts),
        };
        let p = predicates(&scope, &[]);
        assert!(p
            .logs
            .ends_with("ts = toDateTime64('2026-10-27 12:30:45.123', 3, 'UTC')"));
        // the MCP row has its own timestamp, so it is matched in a window
        assert!(!p.mcp.contains("and ts = "));
        assert!(p.mcp.contains(
            "ts between toDateTime64('2026-10-27 12:30:45.123', 3, 'UTC') - interval 300 second"
        ));
    }

    #[test]
    fn the_record_says_which_stores_a_keyless_user_erasure_skipped() {
        let user = ScopeBody::User { id: uuid(5) };
        let stores = coverage(&user, &[], true);
        assert_eq!(stores[0]["store"], "request_logs");
        assert_eq!(stores[0]["searched"], false);
        assert!(stores[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no virtual keys"));
        assert_eq!(stores[0]["code"], "no_key");
        assert_eq!(stores[1]["searched"], false);
        assert_eq!(
            stores[2],
            json!({"store": "mcp_tool_call_logs", "searched": true})
        );
        // with a key, every store is searched
        let stores = coverage(&user, &[uuid(1)], true);
        assert!(stores
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["searched"] == true));
        // payloads only keeps the log rows, and says why
        let stores = coverage(&user, &[uuid(1)], false);
        assert_eq!(stores[0]["searched"], false);
        assert!(stores[0]["reason"].as_str().unwrap().contains("payloads"));
        assert_eq!(stores[0]["code"], "bodies_only");
        assert_eq!(stores[1]["searched"], true);
    }

    #[test]
    fn a_user_with_no_keys_still_reaches_their_mcp_rows() {
        let p = predicates(&ScopeBody::User { id: uuid(5) }, &[]);
        assert_eq!(p.mcp, format!("user_id = '{}'", uuid(5)));
        // valid SQL that matches nothing, and no lookup of older payload rows
        assert_eq!((p.logs.as_str(), p.legacy_column), ("1 = 0", None));
    }

    #[test]
    fn progress_completes_only_when_every_mutation_is_done() {
        let rows = |flags: &[u8]| -> Vec<Value> {
            flags
                .iter()
                .map(|f| json!({"is_done": f, "latest_fail_reason": ""}))
                .collect()
        };
        let none = chrono::Duration::minutes(1);
        assert_eq!(
            Progress::from_rows(&rows(&[1, 1])).verdict(2, none),
            Some("completed")
        );
        assert_eq!(Progress::from_rows(&rows(&[1, 0])).verdict(2, none), None);
        // not listed yet, and not listed an hour on
        assert_eq!(Progress::from_rows(&[]).verdict(2, none), None);
        assert_eq!(
            Progress::from_rows(&[]).verdict(2, chrono::Duration::hours(2)),
            Some("unverified")
        );
    }

    #[test]
    fn a_failing_mutation_surfaces_its_first_line_only() {
        let rows = vec![json!({"is_done": 0, "latest_fail_reason": "Code: 1. boom\nstack"})];
        assert_eq!(
            Progress::from_rows(&rows).last_error.as_deref(),
            Some("Code: 1. boom")
        );
    }

    #[test]
    fn validation_requires_a_reason_and_a_sane_request_id() {
        let body = |reason: &str, request_id: &str| CreateErasure {
            scope: ScopeBody::Request {
                request_id: request_id.to_string(),
                org_id: uuid(1),
                ts: None,
            },
            reason: reason.to_string(),
            include_logs: true,
        };
        assert!(validate(&body("erasure request 42", "req-1")).is_ok());
        assert!(validate(&body("  ", "req-1")).is_err());
        assert!(validate(&body("ok", "")).is_err());
        assert!(validate(&body("ok", "a\nb")).is_err());
    }
}
