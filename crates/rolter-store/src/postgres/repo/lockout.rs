//! Guards and advisory locks shared by the writes that must never strand a deployment
//! without an administrator or a way to sign in.

use uuid::Uuid;

use rolter_core::Result;

use super::super::models::User;
use super::support::store_err;

/// Outcome of a write that is refused when it would lock someone out for good:
/// an org with no way to sign in (#2233), or a deployment with no active
/// superadmin (#2344).
#[derive(Debug, Clone, PartialEq)]
pub enum LockoutGuard<T> {
    Done(T),
    /// the write would leave nothing behind: no way to sign in to the org
    /// (password sign-in is off and no enabled sso provider would remain), or
    /// no active superadmin account
    WouldLockOut,
}

/// Outcome of [`UserRepo::set_deactivated_guarding_org_admins`](super::users::UserRepo::set_deactivated_guarding_org_admins): the
/// deactivation went through, or it is refused because the account is the last
/// active superadmin (#2344) or holds the last active org-scoped `admin` grant
/// of the named org (#2558).
#[derive(Debug, Clone)]
pub enum DeactivationGuard {
    Done(User),
    LastSuperadmin,
    /// the org the account is the last active admin of
    LastOrgAdmin(Uuid),
}

/// Serialise every write that decides whether an org keeps a sign-in method.
///
/// The two guards read different rows (the provider list, the policy row), so
/// a row lock on either cannot stop a provider write racing a policy write;
/// one advisory lock per org, held to the end of the transaction, orders them
/// all. Two concurrent disables of different providers queue behind it, and
/// the second sees the first's commit.
pub(super) async fn lock_org_sign_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
) -> Result<()> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('org_sign_in:' || $1::text, 0))")
        .bind(org_id)
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    Ok(())
}

/// Whether the org's policy refuses passwords. No row is the permissive
/// default, matching [`OrgAuthPolicyRepo::get`].
pub(super) async fn passwords_off(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
) -> Result<bool> {
    let allow: Option<bool> =
        sqlx::query_scalar("select allow_password_login from org_auth_policies where org_id = $1")
            .bind(org_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(store_err)?;
    Ok(allow == Some(false))
}

pub(super) async fn other_enabled_provider(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    except: Uuid,
) -> Result<bool> {
    sqlx::query_scalar(
        "select exists (select 1 from sso_providers where org_id = $1 and enabled and id <> $2)",
    )
    .bind(org_id)
    .bind(except)
    .fetch_one(&mut **tx)
    .await
    .map_err(store_err)
}

/// Serialise every write that can shrink the set of active superadmins.
///
/// A row lock on the target is not enough: two concurrent demotions of two
/// different superadmins each lock only their own row, each sees the other
/// still active, and both commit. One advisory lock for the whole set, held to
/// the end of the transaction, makes the second writer wait and then count the
/// first one's commit. Writes that only grow the set never take it.
pub(super) async fn lock_superadmins(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('superadmins', 0))")
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    Ok(())
}

/// Serialise every write that can shrink the set of active admins of `org`
/// (#2311, #2558): a revoked org-scoped `admin` grant, or a deactivated account
/// holding one. Like [`lock_superadmins`], one advisory lock per org held to
/// the end of the transaction, so the second of two concurrent writers counts
/// the first one's commit.
pub(super) async fn lock_org_admins(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org: Uuid,
) -> Result<()> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('org_admins:' || $1::text, 0))")
        .bind(org)
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    Ok(())
}

/// Whether `id` is an active superadmin and no other account is. Call it under
/// [`lock_superadmins`]; an unknown id answers `false` and the write reports
/// the not-found itself.
pub(super) async fn last_active_superadmin(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<bool> {
    sqlx::query_scalar(
        "select exists (
             select 1 from users where id = $1 and is_superadmin and deactivated_at is null
         ) and not exists (
             select 1 from users where id <> $1 and is_superadmin and deactivated_at is null
         )",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await
    .map_err(store_err)
}
