//! The audit log.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::AuditLogEntry;
use super::support::store_err;

/// Every column of `audit_log` that its row type reads.
const AUDIT_LOG_COLUMNS: &str =
    "id, org_id, actor_user_id, action, target_type, target_id, detail, at";

/// Everyone holding a role anywhere in the org bound as `$1`: at the org, one
/// of its teams or one of its projects.
///
/// Account events — sign-ins, failed sign-ins, second-factor changes, a
/// break-glass reset — are written with no org, because an account is not any
/// one org's. An org's audit log still has to show them for its own people,
/// or the rows are written and nobody can read them (#1854); `org_audit_scope`
/// is the predicate that uses this set.
macro_rules! org_members_cte {
    () => {
        "with members as (
     select m.user_id from memberships m
       left join teams t on t.id = m.team_id
       left join projects p on p.id = m.project_id
       left join teams pt on pt.id = p.team_id
      where m.org_id = $1 or t.org_id = $1 or pt.org_id = $1)
 "
    };
}

/// The rows an org's audit log returns: its own (`org_id = $1`), plus the
/// account events of its people, which are written with no org.
///
/// An org-less row is returned only when its action is in an account-event
/// family, `auth.*` or `user.*`, **and** its actor or target user is one of
/// the org's `members`. Naming the families rather than excluding the
/// deployment-wide actions keeps a deployment action added later private by
/// default (#2857): the actor of a deployment setting is a superadmin, who may
/// hold a role in some org, and a rule that looked at the actor alone handed
/// that org's admins every `security.settings.update` and `cluster_node.forget`
/// the superadmin made. A new account-event family has to be added here to
/// become readable, which is the failure that shows up in a test rather than
/// as a leak.
macro_rules! org_audit_scope {
    () => {
        "(org_id = $1 or (org_id is null
                and (action like 'auth.%' or action like 'user.%')
                and (actor_user_id in (select user_id from members)
                     or (target_type = 'user' and target_id in (select user_id from members)))))"
    };
}

pub struct AuditLogRepo<'a>(pub &'a PgPool);

/// The immutable boundary for a stable keyset page. Audit records are ordered
/// by `(at desc, id desc)` so records written after a page is read do not shift
/// its older results.
#[derive(Debug, Clone, Copy)]
pub struct AuditLogCursor {
    pub at: DateTime<Utc>,
    pub id: Uuid,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum AuditLogDirection {
    #[default]
    Next,
    Previous,
}

#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub actor_user_id: Option<Uuid>,
    pub action: Option<String>,
    pub target_type: Option<String>,
    /// Inclusive lower bound on `at`.
    pub start_at: Option<DateTime<Utc>>,
    /// Exclusive upper bound on `at`, the convention the analytics reads use:
    /// adjacent windows `[a, b)` and `[b, c)` never count a row twice.
    pub end_at: Option<DateTime<Utc>>,
    pub cursor: Option<AuditLogCursor>,
    pub direction: AuditLogDirection,
}

#[derive(Debug)]
pub struct AuditLogPage {
    pub entries: Vec<AuditLogEntry>,
    pub has_more: bool,
}

impl AuditLogRepo<'_> {
    pub async fn create(
        &self,
        org_id: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<Uuid>,
        detail: Option<serde_json::Value>,
    ) -> Result<AuditLogEntry> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
             values ($1, $2, $3, $4, $5, $6)
             returning {AUDIT_LOG_COLUMNS}"
        )))
        .bind(org_id)
        .bind(actor_user_id)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(detail)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid, limit: i64) -> Result<Vec<AuditLogEntry>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {AUDIT_LOG_COLUMNS}
             from audit_log where org_id = $1 order by at desc limit $2"
        )))
        .bind(org_id)
        .bind(limit)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Query one cursor page. `Previous` scans newer records in ascending
    /// order, then reverses them before returning so every response remains
    /// newest-first to API clients.
    pub async fn list_page(
        &self,
        org_id: Uuid,
        filter: &AuditLogFilter,
        limit: i64,
    ) -> Result<AuditLogPage> {
        let query = match filter.direction {
            AuditLogDirection::Next => {
                concat!(
                    org_members_cte!(),
                    "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
                 from audit_log
                 where ",
                    org_audit_scope!(),
                    "
                   and ($2::uuid is null or actor_user_id = $2)
                   and ($3::text is null or action = $3)
                   and ($4::text is null or target_type = $4)
                   and ($5::timestamptz is null or at >= $5)
                   and ($6::timestamptz is null or at < $6)
                   and ($7::timestamptz is null or (at, id) < ($7, $8))
                 order by at desc, id desc limit $9"
                )
            }
            AuditLogDirection::Previous => {
                concat!(
                    org_members_cte!(),
                    "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
                 from audit_log
                 where ",
                    org_audit_scope!(),
                    "
                   and ($2::uuid is null or actor_user_id = $2)
                   and ($3::text is null or action = $3)
                   and ($4::text is null or target_type = $4)
                   and ($5::timestamptz is null or at >= $5)
                   and ($6::timestamptz is null or at < $6)
                   and ($7::timestamptz is null or (at, id) > ($7, $8))
                 order by at asc, id asc limit $9"
                )
            }
        };
        let entries: Vec<AuditLogEntry> = sqlx::query_as(query)
            .bind(org_id)
            .bind(filter.actor_user_id)
            .bind(filter.action.as_deref())
            .bind(filter.target_type.as_deref())
            .bind(filter.start_at)
            .bind(filter.end_at)
            .bind(filter.cursor.map(|cursor| cursor.at))
            .bind(filter.cursor.map(|cursor| cursor.id))
            .bind(limit + 1)
            .fetch_all(self.0)
            .await
            .map_err(store_err)?;
        Ok(Self::finish_page(entries, filter, limit))
    }

    /// Trim the probe row and restore newest-first order after a `Previous`
    /// scan, shared by the per-org and the deployment-wide read.
    fn finish_page(
        mut entries: Vec<AuditLogEntry>,
        filter: &AuditLogFilter,
        limit: i64,
    ) -> AuditLogPage {
        let has_more = entries.len() as i64 > limit;
        if has_more {
            entries.pop();
        }
        if matches!(filter.direction, AuditLogDirection::Previous) {
            entries.reverse();
        }
        AuditLogPage { entries, has_more }
    }

    /// Query one cursor page across the whole deployment: every row, org-less
    /// account events included. Same ordering, filters and cursor as
    /// [`Self::list_page`], minus the org scoping.
    pub async fn list_page_all(&self, filter: &AuditLogFilter, limit: i64) -> Result<AuditLogPage> {
        let query = match filter.direction {
            AuditLogDirection::Next => {
                "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
                 from audit_log
                 where ($1::uuid is null or actor_user_id = $1)
                   and ($2::text is null or action = $2)
                   and ($3::text is null or target_type = $3)
                   and ($4::timestamptz is null or at >= $4)
                   and ($5::timestamptz is null or at < $5)
                   and ($6::timestamptz is null or (at, id) < ($6, $7))
                 order by at desc, id desc limit $8"
            }
            AuditLogDirection::Previous => {
                "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
                 from audit_log
                 where ($1::uuid is null or actor_user_id = $1)
                   and ($2::text is null or action = $2)
                   and ($3::text is null or target_type = $3)
                   and ($4::timestamptz is null or at >= $4)
                   and ($5::timestamptz is null or at < $5)
                   and ($6::timestamptz is null or (at, id) > ($6, $7))
                 order by at asc, id asc limit $8"
            }
        };
        let entries: Vec<AuditLogEntry> = sqlx::query_as(query)
            .bind(filter.actor_user_id)
            .bind(filter.action.as_deref())
            .bind(filter.target_type.as_deref())
            .bind(filter.start_at)
            .bind(filter.end_at)
            .bind(filter.cursor.map(|cursor| cursor.at))
            .bind(filter.cursor.map(|cursor| cursor.id))
            .bind(limit + 1)
            .fetch_all(self.0)
            .await
            .map_err(store_err)?;
        Ok(Self::finish_page(entries, filter, limit))
    }

    /// Count matching records across the deployment, ignoring the cursor.
    pub async fn count_all(&self, filter: &AuditLogFilter) -> Result<i64> {
        sqlx::query_scalar(
            "select count(*) from audit_log
             where ($1::uuid is null or actor_user_id = $1)
               and ($2::text is null or action = $2)
               and ($3::text is null or target_type = $3)
               and ($4::timestamptz is null or at >= $4)
               and ($5::timestamptz is null or at < $5)",
        )
        .bind(filter.actor_user_id)
        .bind(filter.action.as_deref())
        .bind(filter.target_type.as_deref())
        .bind(filter.start_at)
        .bind(filter.end_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Count matching records without applying a cursor. Callers opt in to
    /// this extra query because a precise total is not needed for normal
    /// next/previous navigation.
    pub async fn count(&self, org_id: Uuid, filter: &AuditLogFilter) -> Result<i64> {
        sqlx::query_scalar(concat!(
            org_members_cte!(),
            "select count(*) from audit_log
             where ",
            org_audit_scope!(),
            "
               and ($2::uuid is null or actor_user_id = $2)
               and ($3::text is null or action = $3)
               and ($4::text is null or target_type = $4)
               and ($5::timestamptz is null or at >= $5)
               and ($6::timestamptz is null or at < $6)",
        ))
        .bind(org_id)
        .bind(filter.actor_user_id)
        .bind(filter.action.as_deref())
        .bind(filter.target_type.as_deref())
        .bind(filter.start_at)
        .bind(filter.end_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}
