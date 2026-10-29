//! The record of subject-scoped log erasures (#1085).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::LogErasure;
use super::support::store_err;

const COLUMNS: &str = "id, scope_kind, scope_id, org_id, request_ts, include_logs, reason, \
                       requested_by, status, statements, error, stores, created_at, completed_at";

/// What to record when an erasure is requested.
#[derive(Debug, Clone)]
pub struct NewLogErasure<'a> {
    pub id: Uuid,
    /// `virtual_key`, `user`, `org` or `request`
    pub scope_kind: &'a str,
    pub scope_id: &'a str,
    pub org_id: Option<Uuid>,
    pub request_ts: Option<DateTime<Utc>>,
    pub include_logs: bool,
    pub reason: &'a str,
    pub requested_by: Option<Uuid>,
    /// the stores searched and skipped, as `[{store, searched, code?, reason?}]`
    pub stores: serde_json::Value,
}

/// A subject the erasure form can name: its id, what it is called and a line
/// that tells two of the same name apart (#3385).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ErasureSubject {
    pub id: Uuid,
    pub name: String,
    pub detail: String,
}

/// Most subjects one lookup returns. `find_subjects` reads one more, so a
/// caller can tell a list that was cut from one that was exactly this long.
pub const SUBJECT_LOOKUP_LIMIT: i64 = 500;

/// Erasure requests. Rows are never deleted: the record of an erasure has to
/// outlive the data it removed.
pub struct LogErasureRepo<'a>(pub &'a PgPool);

impl LogErasureRepo<'_> {
    pub async fn create(&self, new: NewLogErasure<'_>) -> Result<LogErasure> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into log_erasures (id, scope_kind, scope_id, org_id, request_ts, \
                 include_logs, reason, requested_by, stores) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9) returning {COLUMNS}"
        )))
        .bind(new.id)
        .bind(new.scope_kind)
        .bind(new.scope_id)
        .bind(new.org_id)
        .bind(new.request_ts)
        .bind(new.include_logs)
        .bind(new.reason)
        .bind(new.requested_by)
        .bind(new.stores)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<LogErasure>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {COLUMNS} from log_erasures where id = $1"
        )))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, limit: i64) -> Result<Vec<LogErasure>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {COLUMNS} from log_erasures order by created_at desc limit $1"
        )))
        .bind(limit)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Record how the submission went: `pending` once every statement is in,
    /// `failed` with the reason when one was refused.
    pub async fn record_submission(
        &self,
        id: Uuid,
        statements: i32,
        error: Option<&str>,
    ) -> Result<LogErasure> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update log_erasures set statements = $2, error = $3, \
                 status = case when $3::text is null then 'pending' else 'failed' end, \
                 completed_at = case when $3::text is null then null else now() end \
             where id = $1 returning {COLUMNS}"
        )))
        .bind(id)
        .bind(statements)
        .bind(error)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Move a `pending` erasure to `completed` or `unverified`. `None` when it
    /// was not pending, so a late poll cannot rewrite a settled row.
    pub async fn settle(&self, id: Uuid, status: &str) -> Result<Option<LogErasure>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update log_erasures set status = $2, completed_at = now() \
             where id = $1 and status = 'pending' returning {COLUMNS}"
        )))
        .bind(id)
        .bind(status)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// The virtual keys a user created and that still exist. Request logs name
    /// a key, never a person, so this is how a user's rows are found; a key
    /// deleted since has left no row to resolve.
    pub async fn key_ids_created_by(&self, user_id: Uuid) -> Result<Vec<Uuid>> {
        sqlx::query_scalar("select id from virtual_keys where created_by = $1")
            .bind(user_id)
            .fetch_all(self.0)
            .await
            .map_err(store_err)
    }

    /// Subjects of `kind` (`virtual_key`, `user` or `org`) whose name or email
    /// contains `text`, or the one with id `id`, for the erasure form's picker
    /// (#3385). Matching is case-insensitive and `%`, `_` and `\` in `text`
    /// match themselves. A key is told apart by its place in the tenancy
    /// (`org / team / project`), since two projects may each have a "prod".
    pub async fn find_subjects(
        &self,
        kind: &str,
        text: &str,
        id: Option<Uuid>,
    ) -> Result<Vec<ErasureSubject>> {
        let sql = match kind {
            "virtual_key" => {
                "select k.id, coalesce(nullif(k.name, ''), k.key_prefix) as name, \
                        o.name || ' / ' || t.name || ' / ' || p.name as detail \
                 from virtual_keys k \
                 join projects p on p.id = k.project_id \
                 join teams t on t.id = p.team_id \
                 join orgs o on o.id = t.org_id \
                 where ($2::uuid is null or k.id = $2) \
                   and ($1 = '' or k.name ilike '%' || $1 || '%' \
                        or k.key_prefix ilike '%' || $1 || '%') \
                 order by name, k.created_at limit $3"
            }
            "user" => {
                "select u.id, u.email as name, coalesce(u.display_name, '') as detail \
                 from users u \
                 where ($2::uuid is null or u.id = $2) \
                   and ($1 = '' or u.email ilike '%' || $1 || '%' \
                        or u.display_name ilike '%' || $1 || '%') \
                 order by u.email limit $3"
            }
            "org" => {
                "select o.id, o.name, o.slug as detail from orgs o \
                 where ($2::uuid is null or o.id = $2) \
                   and ($1 = '' or o.name ilike '%' || $1 || '%' or o.slug ilike '%' || $1 || '%') \
                 order by o.name limit $3"
            }
            other => {
                return Err(rolter_core::Error::Store(format!(
                    "unknown subject kind {other}"
                )))
            }
        };
        let escaped = text
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(escaped)
            .bind(id)
            .bind(SUBJECT_LOOKUP_LIMIT + 1)
            .fetch_all(self.0)
            .await
            .map_err(store_err)
    }
}
