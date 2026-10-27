//! Memberships, password resets and invitations.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{Invitation, Membership, PasswordReset};
use super::lockout::{lock_org_admins, LockoutGuard};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Every column of `memberships` that its row type reads.
const MEMBERSHIP_COLUMNS: &str =
    "id, user_id, org_id, team_id, project_id, role, source, created_at";

/// role grants at an org/team/project scope. see [`super::super::models::Membership`].
pub struct MembershipRepo<'a>(pub &'a PgPool);

impl MembershipRepo<'_> {
    pub async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<Membership>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {MEMBERSHIP_COLUMNS}
             from memberships where user_id = $1 order by created_at"
        )))
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// `(membership id, org, team)` for every membership of `user_id`: the org
    /// and team each one sits under, which a project membership does not name
    /// on its own row.
    pub async fn ancestors_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<(Uuid, Option<Uuid>, Option<Uuid>)>> {
        sqlx::query_as(
            "select m.id, coalesce(m.org_id, t.org_id, pt.org_id), coalesce(m.team_id, p.team_id)
             from memberships m
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             where m.user_id = $1",
        )
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// every membership whose scope falls within `org_id`'s tree (an org-scoped
    /// grant, a grant on one of its teams, or on one of their projects), so an
    /// org admin can see and manage all role assignments under their org
    pub async fn list_in_org(&self, org_id: Uuid) -> Result<Vec<Membership>> {
        sqlx::query_as(
            "select m.id, m.user_id, m.org_id, m.team_id, m.project_id, m.role, m.source, m.created_at
             from memberships m
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             where m.org_id = $1 or t.org_id = $1 or pt.org_id = $1
             order by m.created_at",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Membership> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {MEMBERSHIP_COLUMNS}
             from memberships where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("membership {id}"),
        )
        .await
    }

    /// grant `role` to `user_id` at exactly one scope. exactly one of
    /// `org_id`/`team_id`/`project_id` should be non-null (the most-specific
    /// scope id); enforcement of that invariant is left to the caller.
    pub async fn create(
        &self,
        user_id: Uuid,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
    ) -> Result<Membership> {
        self.create_with_source(user_id, org_id, team_id, project_id, role, "manual")
            .await
    }

    /// grant a role and record where the grant came from. `source` is
    /// `manual` for anything an operator did (admin API, invitation, seed) and
    /// `sso` for a grant an IdP group mapping produced; only `sso` rows are
    /// ever reconciled away by a later login.
    pub async fn create_with_source(
        &self,
        user_id: Uuid,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
        source: &str,
    ) -> Result<Membership> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into memberships (user_id, org_id, team_id, project_id, role, source)
             values ($1, $2, $3, $4, $5, $6)
             returning {MEMBERSHIP_COLUMNS}"
        )))
        .bind(user_id)
        .bind(org_id)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .bind(source)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// delete a grant, refused with [`LockoutGuard::WouldLockOut`] when it is
    /// an org-scoped `admin` grant held by an active account and no other
    /// active account holds one for that org (#2311). a per-org advisory lock
    /// held to the end of the transaction orders two concurrent revocations of
    /// two different admins, so the second one counts the first one's commit.
    /// `protect_last_admin = false` skips the check (the superadmin override).
    pub async fn delete_guarded(
        &self,
        id: Uuid,
        protect_last_admin: bool,
    ) -> Result<LockoutGuard<()>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if protect_last_admin {
            let org: Option<Uuid> = sqlx::query_scalar(
                "select org_id from memberships where id = $1 and role = 'admin'",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?
            .flatten();
            if let Some(org) = org {
                lock_org_admins(&mut tx, org).await?;
                // read again under the lock: a concurrent revoke may have
                // committed while this one waited
                let last: bool = sqlx::query_scalar(
                    "select exists (
                         select 1 from memberships m join users u on u.id = m.user_id
                         where m.id = $1 and m.org_id = $2 and m.role = 'admin'
                           and u.deactivated_at is null
                     ) and not exists (
                         select 1 from memberships m join users u on u.id = m.user_id
                         where m.id <> $1 and m.org_id = $2 and m.role = 'admin'
                           and u.deactivated_at is null
                     )",
                )
                .bind(id)
                .bind(org)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                if last {
                    return Ok(LockoutGuard::WouldLockOut);
                }
            }
        }
        require_affected(
            sqlx::query("delete from memberships where id = $1").bind(id),
            &mut *tx,
            || format!("membership {id}"),
        )
        .await?;
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done(()))
    }

    /// change a grant's role in one statement, returning the row as it was
    /// before and as it is now. Demoting an org-scoped `admin` grant held by
    /// an active account is refused with [`LockoutGuard::WouldLockOut`] when no
    /// other active account holds one for that org, under the same per-org
    /// advisory lock `delete_guarded` takes (#2311); `protect_last_admin =
    /// false` skips the check (the superadmin override).
    pub async fn update_role_guarded(
        &self,
        id: Uuid,
        role: &str,
        protect_last_admin: bool,
    ) -> Result<LockoutGuard<(Membership, Membership)>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if protect_last_admin && role != "admin" {
            let org: Option<Uuid> = sqlx::query_scalar(
                "select org_id from memberships where id = $1 and role = 'admin'",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?
            .flatten();
            if let Some(org) = org {
                lock_org_admins(&mut tx, org).await?;
                let last: bool = sqlx::query_scalar(
                    "select exists (
                         select 1 from memberships m join users u on u.id = m.user_id
                         where m.id = $1 and m.org_id = $2 and m.role = 'admin'
                           and u.deactivated_at is null
                     ) and not exists (
                         select 1 from memberships m join users u on u.id = m.user_id
                         where m.id <> $1 and m.org_id = $2 and m.role = 'admin'
                           and u.deactivated_at is null
                     )",
                )
                .bind(id)
                .bind(org)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                if last {
                    return Ok(LockoutGuard::WouldLockOut);
                }
            }
        }
        let before: Membership = fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {MEMBERSHIP_COLUMNS}
             from memberships where id = $1 for update"
            )))
            .bind(id),
            &mut *tx,
            || format!("membership {id}"),
        )
        .await?;
        // a manual edit is operator intent: it must survive sso reconciliation,
        // which only ever touches rows whose source is 'sso'
        let after: Membership = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update memberships set role = $2, source = 'manual' where id = $1
             returning {MEMBERSHIP_COLUMNS}"
        )))
        .bind(id)
        .bind(role)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done((before, after)))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from memberships where id = $1").bind(id),
            self.0,
            || format!("membership {id}"),
        )
        .await?;
        Ok(())
    }
}

/// password resets: one-time links an administrator issues so a member can
/// choose a new password themselves (#2240).
pub struct PasswordResetRepo<'a>(pub &'a PgPool);

const PASSWORD_RESET_COLUMNS: &str =
    "id, user_id, token_hash, created_by, expires_at, used_at, revoked_at, created_at";

impl PasswordResetRepo<'_> {
    /// Issue a reset for `user_id`, revoking the account's live one in the same
    /// transaction so only the newest link works. Returns the new row and the
    /// id of the reset it replaced, if any.
    pub async fn issue(
        &self,
        user_id: Uuid,
        token_hash: &str,
        created_by: Option<Uuid>,
        expires_at: DateTime<Utc>,
    ) -> Result<(PasswordReset, Option<Uuid>)> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query(
            "select pg_advisory_xact_lock(hashtextextended('password_reset:' || $1::text, 0))",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        let replaced: Option<Uuid> = sqlx::query_scalar(
            "update password_resets set revoked_at = now() \
             where user_id = $1 and used_at is null and revoked_at is null \
             returning id",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let reset = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into password_resets (user_id, token_hash, created_by, expires_at) \
             values ($1, $2, $3, $4) returning {PASSWORD_RESET_COLUMNS}"
        )))
        .bind(user_id)
        .bind(token_hash)
        .bind(created_by)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok((reset, replaced))
    }

    /// Look a reset up by token digest, only while it is live: unused,
    /// unrevoked and unexpired. Spent and wrong tokens look alike to the caller.
    pub async fn find_live_by_hash(&self, token_hash: &str) -> Result<Option<PasswordReset>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PASSWORD_RESET_COLUMNS} from password_resets \
             where token_hash = $1 and used_at is null and revoked_at is null \
               and expires_at > now()"
        )))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Spend a live reset: stamp it used, store the new password hash and end
    /// every session of the account, in one transaction. Returns the number of
    /// sessions ended, or `None` when another request spent the reset first or
    /// the account is deactivated, so redeeming is single-use even under a race.
    pub async fn redeem(
        &self,
        id: Uuid,
        user_id: Uuid,
        password_hash: &str,
    ) -> Result<Option<u64>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let spent = sqlx::query(
            "update password_resets set used_at = now() \
             where id = $1 and used_at is null and revoked_at is null and expires_at > now()",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        if spent.rows_affected() != 1 {
            return Ok(None);
        }
        let updated = sqlx::query(
            "update users set password_hash = $2 where id = $1 and deactivated_at is null",
        )
        .bind(user_id)
        .bind(password_hash)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        if updated.rows_affected() != 1 {
            return Ok(None);
        }
        let sessions = sqlx::query("delete from sessions where user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(Some(sessions.rows_affected()))
    }
}

/// invitations: one-time links that let an invitee create their own account.
pub struct InvitationRepo<'a>(pub &'a PgPool);

const INVITATION_COLUMNS: &str = "id, org_id, email, role, team_id, project_id, token_hash, \
     invited_by, expires_at, accepted_at, revoked_at, created_at";

impl InvitationRepo<'_> {
    /// Create an invitation, replacing the address's live one in the same
    /// transaction. Returns the new row and the id of the invitation it
    /// revoked, if any.
    ///
    /// `invitations_live_email_idx` forbids two unaccepted, unrevoked rows for
    /// one address but ignores `expires_at` (its predicate cannot use
    /// `now()`), so an expired invitation still holds the address. Revoking
    /// whatever holds it first, expired or not, makes the new link the only
    /// live one and the old link stop working. A transaction-scoped advisory
    /// lock on `(org, lower(email))` orders concurrent creates for one
    /// address: the second waits, then revokes the first's row, so neither
    /// trips the index.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: Uuid,
        email: &str,
        role: &str,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        token_hash: &str,
        invited_by: Option<Uuid>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(Invitation, Option<Uuid>)> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("select pg_advisory_xact_lock(hashtextextended('invitation:' || $1::text || ':' || lower($2), 0))")
            .bind(org_id)
            .bind(email)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        let replaced: Option<Uuid> = sqlx::query_scalar(
            "update invitations set revoked_at = now() \
             where org_id = $1 and lower(email) = lower($2) \
               and accepted_at is null and revoked_at is null \
             returning id",
        )
        .bind(org_id)
        .bind(email)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let invitation = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into invitations (org_id, email, role, team_id, project_id, token_hash, \
                    invited_by, expires_at) \
             values ($1, $2, $3, $4, $5, $6, $7, $8) \
             returning {INVITATION_COLUMNS}"
        )))
        .bind(org_id)
        .bind(email)
        .bind(role)
        .bind(team_id)
        .bind(project_id)
        .bind(token_hash)
        .bind(invited_by)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok((invitation, replaced))
    }

    /// the org's invitations, newest first, each with the sender's email
    /// resolved here (#2325): the join is independent of which users the
    /// caller may list, since the invitation itself is already visible to them
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Invitation>> {
        sqlx::query_as(
            "select i.id, i.org_id, i.email, i.role, i.team_id, i.project_id, i.token_hash, \
                    i.invited_by, u.email as invited_by_email, i.expires_at, i.accepted_at, \
                    i.revoked_at, i.created_at \
             from invitations i left join users u on u.id = i.invited_by \
             where i.org_id = $1 order by i.created_at desc",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Look an invitation up by token digest, but only while it is live:
    /// unaccepted, unrevoked and unexpired. Spent invitations are
    /// indistinguishable from wrong ones to the caller, which is the point.
    pub async fn find_live_by_hash(&self, token_hash: &str) -> Result<Option<Invitation>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {INVITATION_COLUMNS} from invitations \
             where token_hash = $1 and accepted_at is null and revoked_at is null \
               and expires_at > now()"
        )))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Mark an invitation accepted, but only if it is still live. Returns
    /// `false` when another request got there first, so acceptance is
    /// single-use even under a race.
    pub async fn mark_accepted(&self, id: Uuid) -> Result<bool> {
        let res = sqlx::query(
            "update invitations set accepted_at = now() \
             where id = $1 and accepted_at is null and revoked_at is null and expires_at > now()",
        )
        .bind(id)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<Invitation> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update invitations set revoked_at = now() \
             where id = $1 and accepted_at is null \
             returning {INVITATION_COLUMNS}"
            )))
            .bind(id),
            self.0,
            || format!("pending invitation {id}"),
        )
        .await
    }

    pub async fn get(&self, id: Uuid) -> Result<Invitation> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {INVITATION_COLUMNS} from invitations where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("invitation {id}"),
        )
        .await
    }
}
