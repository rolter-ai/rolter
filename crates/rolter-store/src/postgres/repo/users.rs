//! Users and their preferences.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::User;
use super::lockout::{
    last_active_superadmin, lock_org_admins, lock_superadmins, DeactivationGuard, LockoutGuard,
};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Every column of `users` that its row type reads.
const USER_COLUMNS: &str = "id, email, password_hash, is_superadmin, deactivated_at, created_at, \
     display_name, bio, deployment_role";

/// local accounts. see [`super::super::models::User`]; `find_by_email` backs login.
pub struct UserRepo<'a>(pub &'a PgPool);

impl UserRepo<'_> {
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {USER_COLUMNS}
             from users where email = $1"
        )))
        .bind(email)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// the e-mail of each account in `ids` that still exists, for naming the
    /// actors of an audit export a page at a time; an unknown id is left out
    pub async fn emails_by_ids(&self, ids: &[Uuid]) -> Result<Vec<(Uuid, String)>> {
        sqlx::query_as("select id, email from users where id = any($1)")
            .bind(ids)
            .fetch_all(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<User> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {USER_COLUMNS}
             from users where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("user {id}"),
        )
        .await
    }

    /// list every user that holds at least one membership anywhere in `org_id`'s
    /// tree (org, its teams, or their projects). this is the admin-visible set of
    /// accounts for an org; superadmins with no org membership are not included.
    pub async fn list_in_org(&self, org_id: Uuid) -> Result<Vec<User>> {
        sqlx::query_as(
            "select distinct u.id, u.email, u.password_hash, u.is_superadmin,
                    u.deactivated_at, u.created_at, u.display_name, u.bio, u.deployment_role
             from users u
             join memberships m on m.user_id = u.id
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             where m.org_id = $1 or t.org_id = $1 or pt.org_id = $1
             order by u.email",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// [`Self::list_in_org`] plus every account that holds no membership
    /// anywhere, in one email-ordered list.
    ///
    /// The seeded superadmin (`rolter-seed --admin-email`) belongs to no org, so
    /// the plain listing never shows it: nobody can find the account to edit,
    /// deactivate or delete it. An account with a membership in a *different*
    /// org is not added, since another org's people are not this org's to list.
    pub async fn list_in_org_with_unassigned(&self, org_id: Uuid) -> Result<Vec<User>> {
        sqlx::query_as(
            "select distinct u.id, u.email, u.password_hash, u.is_superadmin,
                    u.deactivated_at, u.created_at, u.display_name, u.bio, u.deployment_role
             from users u
             left join memberships m on m.user_id = u.id
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             where m.id is null or m.org_id = $1 or t.org_id = $1 or pt.org_id = $1
             order by u.email",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// every account in the deployment, whatever it belongs to, in one
    /// email-ordered list: members of any org, members of none, deactivated
    /// ones.
    ///
    /// The listings above answer for one org, so an account that only belongs
    /// to another org is invisible to them. This is the read behind the
    /// deployment-wide audit log, where an actor can be from anywhere (#2871).
    /// It names no org, so the caller must be a superadmin.
    pub async fn list_all(&self) -> Result<Vec<User>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {USER_COLUMNS}
             from users
             order by email, id"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// create a local account. `password_hash` is a pre-computed argon2id digest
    /// (the repo never sees plaintext); pass `None` for an sso-only shell account
    pub async fn create(
        &self,
        email: &str,
        password_hash: Option<&str>,
        is_superadmin: bool,
    ) -> Result<User> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into users (email, password_hash, is_superadmin)
             values ($1, $2, $3)
             returning {USER_COLUMNS}"
        )))
        .bind(email)
        .bind(password_hash)
        .bind(is_superadmin)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// update mutable account fields. each `Some` is applied via `coalesce`, so
    /// `None` leaves the stored value untouched. `password_hash` follows the same
    /// rule; there is no way to clear a password back to null through this path.
    ///
    /// `deactivated` stamps or clears `deactivated_at` in the same transaction.
    /// the write is refused with [`LockoutGuard::WouldLockOut`] when it would
    /// demote or deactivate the last active superadmin (#2344); the caller is
    /// responsible for deleting live sessions when deactivating.
    pub async fn update_account(
        &self,
        id: Uuid,
        email: Option<&str>,
        password_hash: Option<&str>,
        is_superadmin: Option<bool>,
        deactivated: Option<bool>,
    ) -> Result<LockoutGuard<User>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if is_superadmin == Some(false) || deactivated == Some(true) {
            lock_superadmins(&mut tx).await?;
            if last_active_superadmin(&mut tx, id).await? {
                return Ok(LockoutGuard::WouldLockOut);
            }
        }
        let user: User = fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update users set
                 email = coalesce($2, email),
                 password_hash = coalesce($3, password_hash),
                 is_superadmin = coalesce($4, is_superadmin),
                 deactivated_at = case
                     when $5::boolean is null then deactivated_at
                     when $5 then coalesce(deactivated_at, now())
                     else null end
             where id = $1
             returning {USER_COLUMNS}"
            )))
            .bind(id)
            .bind(email)
            .bind(password_hash)
            .bind(is_superadmin)
            .bind(deactivated),
            &mut *tx,
            || format!("user {id}"),
        )
        .await?;
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done(user))
    }

    /// flip the deactivation flag. `true` stamps `deactivated_at = now()` (login
    /// blocked); `false` clears it back to null (re-enabled). the caller is
    /// responsible for deleting live sessions when deactivating. refused when it
    /// would deactivate the last active superadmin (#2344).
    pub async fn set_deactivated(&self, id: Uuid, deactivated: bool) -> Result<LockoutGuard<User>> {
        self.update_account(id, None, None, None, Some(deactivated))
            .await
    }

    /// flip the deactivation flag like [`Self::set_deactivated`], and also
    /// refuse to deactivate an account that holds an org's last org-scoped
    /// `admin` grant among active accounts (#2558).
    ///
    /// Used where the caller is not a superadmin who could repair the org
    /// afterwards: SCIM deprovisioning and `active: false`. The account is
    /// global, so every org it administers is checked, not only the one the
    /// caller is scoped to. Each org is counted under the same per-org advisory
    /// lock `MembershipRepo::delete_guarded` takes, in ascending org order so
    /// two deactivations cannot deadlock, all in the update's transaction: a
    /// concurrent revoke or deactivation of the org's other admin waits and
    /// then counts this one.
    pub async fn set_deactivated_guarding_org_admins(
        &self,
        id: Uuid,
        deactivated: bool,
    ) -> Result<DeactivationGuard> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if deactivated {
            lock_superadmins(&mut tx).await?;
            if last_active_superadmin(&mut tx, id).await? {
                return Ok(DeactivationGuard::LastSuperadmin);
            }
            let orgs: Vec<Uuid> = sqlx::query_scalar(
                "select distinct m.org_id from memberships m join users u on u.id = m.user_id
                 where m.user_id = $1 and m.role = 'admin' and m.org_id is not null
                   and u.deactivated_at is null
                 order by m.org_id",
            )
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_err)?;
            for org in orgs {
                lock_org_admins(&mut tx, org).await?;
                let others: bool = sqlx::query_scalar(
                    "select exists (
                         select 1 from memberships m join users u on u.id = m.user_id
                         where m.user_id <> $1 and m.org_id = $2 and m.role = 'admin'
                           and u.deactivated_at is null
                     )",
                )
                .bind(id)
                .bind(org)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                if !others {
                    return Ok(DeactivationGuard::LastOrgAdmin(org));
                }
            }
        }
        let user: User = fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update users set
                 deactivated_at = case when $2 then coalesce(deactivated_at, now()) else null end
             where id = $1
             returning {USER_COLUMNS}"
            )))
            .bind(id)
            .bind(deactivated),
            &mut *tx,
            || format!("user {id}"),
        )
        .await?;
        tx.commit().await.map_err(store_err)?;
        Ok(DeactivationGuard::Done(user))
    }

    /// set the self-service profile. each `Some(x)` replaces the column with `x`
    /// (`Some(None)` clears it); `None` leaves it alone. deliberately separate
    /// from [`Self::update_account`]: that one names `is_superadmin` in its `set` list
    /// and so fires the `config_version` trigger, which a name edit must not.
    /// callers validate and normalise; the table's check constraints are the
    /// backstop
    pub async fn set_profile(
        &self,
        id: Uuid,
        display_name: Option<Option<&str>>,
        bio: Option<Option<&str>>,
    ) -> Result<User> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update users set
                 display_name = case when $2 then $3 else display_name end,
                 bio = case when $4 then $5 else bio end
             where id = $1
             returning {USER_COLUMNS}"
            )))
            .bind(id)
            .bind(display_name.is_some())
            .bind(display_name.flatten())
            .bind(bio.is_some())
            .bind(bio.flatten()),
            self.0,
            || format!("user {id}"),
        )
        .await
    }

    /// set or clear the account's deployment role (#1834). deliberately its own
    /// statement: the column is not one the snapshot reads, so it must stay out
    /// of [`Self::update_account`], whose `set` list fires the `config_version`
    /// trigger. callers validate the role; the table's check constraint is the
    /// backstop
    pub async fn set_deployment_role(&self, id: Uuid, role: Option<&str>) -> Result<User> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update users set deployment_role = $2 where id = $1
                 returning {USER_COLUMNS}"
            )))
            .bind(id)
            .bind(role),
            self.0,
            || format!("user {id}"),
        )
        .await
    }

    /// give an account that has no display name the one its identity provider
    /// asserted. The `is null` guard is in the statement so a name set between
    /// the caller's read and this write is never overwritten. Returns whether
    /// a name was written.
    pub async fn default_display_name(&self, id: Uuid, display_name: &str) -> Result<bool> {
        let res = sqlx::query(
            "update users set display_name = $2 where id = $1 and display_name is null",
        )
        .bind(id)
        .bind(display_name)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected() > 0)
    }

    /// delete the account, refused when it is the last active superadmin
    /// (#2344)
    pub async fn delete(&self, id: Uuid) -> Result<LockoutGuard<()>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        lock_superadmins(&mut tx).await?;
        if last_active_superadmin(&mut tx, id).await? {
            return Ok(LockoutGuard::WouldLockOut);
        }
        require_affected(
            sqlx::query("delete from users where id = $1").bind(id),
            &mut *tx,
            || format!("user {id}"),
        )
        .await?;
        // the caps on a person carry no foreign key to them (#1830)
        for table in ["budgets", "rate_limits"] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "delete from {table} where scope_type = 'user' and scope_id = $1"
            )))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done(()))
    }
}

/// per-user preference documents (#1824). one row per user, created on first
/// save; the document is an object whose keys the API validates
pub struct UserPreferencesRepo<'a>(pub &'a PgPool);

impl UserPreferencesRepo<'_> {
    /// the stored document, or `None` when the user never saved one
    pub async fn get(&self, user_id: Uuid) -> Result<Option<serde_json::Value>> {
        sqlx::query_scalar("select prefs from user_preferences where user_id = $1")
            .bind(user_id)
            .fetch_optional(self.0)
            .await
            .map_err(store_err)
    }

    /// replace the whole document. `prefs` must be a json object; the table's
    /// check constraint refuses anything else
    pub async fn put(&self, user_id: Uuid, prefs: &serde_json::Value) -> Result<()> {
        sqlx::query(
            "insert into user_preferences (user_id, prefs) values ($1, $2)
             on conflict (user_id) do update set prefs = excluded.prefs, updated_at = now()",
        )
        .bind(user_id)
        .bind(prefs)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }
}
