//! Per-org sign-in policy and sessions.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{MfaPolicyBinding, OrgAuthPolicy, Session};
use super::lockout::{lock_org_sign_in, other_enabled_provider, LockoutGuard};
use super::support::store_err;

/// Every column of `sessions` that its row type reads.
const SESSION_COLUMNS: &str = "id, user_id, token_hash, created_at, expires_at, last_seen_at";

/// Every column of `org_auth_policies` that its row type reads.
const ORG_AUTH_POLICY_COLUMNS: &str =
    "org_id, allow_password_login, allow_sso, mfa_policy, mfa_enforce_after, updated_at";

/// How strict an `mfa_policy` value is, for picking the strictest across a
/// user's orgs: `off` 0, `optional` 1, `required_superadmin` 2,
/// `required_all` 3.
///
/// Expressed here and not as a SQL `order by`, because the policy values are
/// words rather than a rank the database knows. An unknown value ranks with
/// `off`, which the column's check constraint makes unreachable anyway.
pub fn mfa_policy_rank(policy: &str) -> u8 {
    match policy {
        "required_all" => 3,
        "required_superadmin" => 2,
        "optional" => 1,
        _ => 0,
    }
}

/// per-org login policy: whether members may use a local password, and
/// whether SSO callbacks for the org's providers are honoured.
pub struct OrgAuthPolicyRepo<'a>(pub &'a PgPool);

impl OrgAuthPolicyRepo<'_> {
    /// The policy for `org_id`, or the permissive default when no row exists.
    /// Orgs created before single sign-on existed have no row, and must keep
    /// logging in exactly as they did.
    pub async fn get(&self, org_id: Uuid) -> Result<OrgAuthPolicy> {
        let found: Option<OrgAuthPolicy> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ORG_AUTH_POLICY_COLUMNS}
             from org_auth_policies where org_id = $1"
        )))
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(found.unwrap_or(OrgAuthPolicy {
            org_id,
            allow_password_login: true,
            allow_sso: true,
            mfa_policy: "off".to_string(),
            mfa_enforce_after: None,
            updated_at: chrono::Utc::now(),
        }))
    }

    /// Write the policy, refusing to turn passwords off while the org has no
    /// enabled sso provider. The check runs under the same per-org lock as the
    /// provider writes, so the two guards cannot both pass against each other
    /// (#2233).
    pub async fn set(
        &self,
        org_id: Uuid,
        allow_password_login: bool,
        allow_sso: bool,
        mfa_policy: &str,
        mfa_enforce_after: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<LockoutGuard<OrgAuthPolicy>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        lock_org_sign_in(&mut tx, org_id).await?;
        if !allow_password_login && !other_enabled_provider(&mut tx, org_id, Uuid::nil()).await? {
            return Ok(LockoutGuard::WouldLockOut);
        }
        let row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into org_auth_policies
                 (org_id, allow_password_login, allow_sso, mfa_policy, mfa_enforce_after)
             values ($1, $2, $3, $4, $5)
             on conflict (org_id) do update
                 set allow_password_login = excluded.allow_password_login,
                     allow_sso = excluded.allow_sso,
                     mfa_policy = excluded.mfa_policy,
                     mfa_enforce_after = excluded.mfa_enforce_after,
                     updated_at = now()
             returning {ORG_AUTH_POLICY_COLUMNS}"
        )))
        .bind(org_id)
        .bind(allow_password_login)
        .bind(allow_sso)
        .bind(mfa_policy)
        .bind(mfa_enforce_after)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done(row))
    }

    /// The strictest second-factor policy across every org this user belongs
    /// to, as `(policy, org_id)`.
    ///
    /// Strictest wins rather than first-found: a user who is an admin in a
    /// hardened org and a viewer in a relaxed one must not be able to sign in
    /// without their factor because the relaxed membership was read first.
    /// Membership scopes are org/team/project, so a team's and a project's
    /// owning org counts too -- the same join `password_login_blocked_for_user`
    /// walks.
    pub async fn strictest_mfa_policy_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Option<(String, Uuid)>> {
        Ok(self
            .mfa_policies_for_user(user_id)
            .await?
            .into_iter()
            .map(|binding| (binding.policy, binding.org_id))
            .max_by_key(|(policy, _)| mfa_policy_rank(policy))
            .filter(|(policy, _)| mfa_policy_rank(policy) > 0))
    }

    /// Every org policy that applies to this user, one per membership, with
    /// the grace window each carries (#1852).
    ///
    /// Returned whole rather than reduced to the strictest, because two
    /// hardened orgs can disagree about *when* they start to bite, and the
    /// earlier one has to win -- a later grace window in one org must not
    /// postpone a requirement another org already enforces.
    pub async fn mfa_policies_for_user(&self, user_id: Uuid) -> Result<Vec<MfaPolicyBinding>> {
        sqlx::query_as(
            "select ap.mfa_policy as policy, ap.org_id, ap.mfa_enforce_after as enforce_after
             from memberships m
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             join org_auth_policies ap
               on ap.org_id = coalesce(m.org_id, t.org_id, pt.org_id)
             where m.user_id = $1",
        )
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// True when at least one org this user belongs to forbids password login.
    /// Membership scopes are org/team/project, so the team's and project's
    /// owning org counts too.
    pub async fn password_login_blocked_for_user(&self, user_id: Uuid) -> Result<bool> {
        let blocked: Option<(bool,)> = sqlx::query_as(
            "select true from memberships m
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             join org_auth_policies ap
               on ap.org_id = coalesce(m.org_id, t.org_id, pt.org_id)
             where m.user_id = $1 and not ap.allow_password_login
             limit 1",
        )
        .bind(user_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(blocked.is_some())
    }

    /// True when any org allows password login, or when no org has a policy
    /// row at all. Drives the login page: a deployment that turned password
    /// login off everywhere should not render a password form.
    pub async fn any_password_login_allowed(&self) -> Result<bool> {
        let row: Option<(bool,)> = sqlx::query_as(
            "select true from orgs o
             left join org_auth_policies ap on ap.org_id = o.id
             where ap.allow_password_login is not false
             limit 1",
        )
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(row.is_some())
    }
}

/// login sessions backing bearer-token auth. see [`super::super::models::Session`]
/// and the rationale in `migrations/0013_sessions.sql` for why these are
/// stateful (postgres-backed) rather than a stateless jwt.
pub struct SessionRepo<'a>(pub &'a PgPool);

impl SessionRepo<'_> {
    pub async fn create(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Session> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into sessions (user_id, token_hash, expires_at)
             values ($1, $2, $3)
             returning {SESSION_COLUMNS}"
        )))
        .bind(user_id)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// look up a live (unexpired) session by its token digest and bump
    /// `last_seen_at`; returns `None` for a missing, wrong, or expired token
    pub async fn find_active_by_hash(&self, token_hash: &str) -> Result<Option<Session>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update sessions set last_seen_at = now()
             where token_hash = $1 and expires_at > now()
             returning {SESSION_COLUMNS}"
        )))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// look up a live (unexpired) session by its row id, without touching
    /// `last_seen_at`: a download ticket names the session it was minted on and
    /// must find it revoked or expired by the time it is redeemed (#3504)
    pub async fn find_active_by_id(&self, id: Uuid) -> Result<Option<Session>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SESSION_COLUMNS} from sessions where id = $1 and expires_at > now()"
        )))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// delete a session by its token digest (logout); a no-op if it's
    /// already gone or expired
    pub async fn delete_by_hash(&self, token_hash: &str) -> Result<()> {
        sqlx::query("delete from sessions where token_hash = $1")
            .bind(token_hash)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// revoke every live session for a user (used when an account is
    /// deactivated or deleted so access is cut immediately)
    pub async fn delete_for_user(&self, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from sessions where user_id = $1")
            .bind(user_id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// revoke every session for a user except the one whose digest is
    /// `keep_token_hash`, returning how many went (used on a password reset,
    /// so the caller resetting their own password stays signed in while every
    /// other session opened with the old password ends)
    pub async fn delete_for_user_except(
        &self,
        user_id: Uuid,
        keep_token_hash: Option<&str>,
    ) -> Result<u64> {
        let done = sqlx::query(
            "delete from sessions
             where user_id = $1 and ($2::text is null or token_hash <> $2)",
        )
        .bind(user_id)
        .bind(keep_token_hash)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(done.rows_affected())
    }
}
