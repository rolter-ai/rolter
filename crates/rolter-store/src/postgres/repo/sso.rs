//! OIDC single sign-on providers and their login state.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{SsoExchangeCode, SsoGroupMapping, SsoLoginState, SsoProvider};
use super::lockout::{lock_org_sign_in, other_enabled_provider, passwords_off, LockoutGuard};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// OIDC identity providers, their group→role mappings, and the short-lived
/// state rows that make the authorization-code flow replay-safe.
pub struct SsoRepo<'a>(pub &'a PgPool);

/// Most rows one SSO sweep statement deletes. Keeps a single delete short
/// enough that it never holds a long lock, however large the backlog.
pub const SSO_SWEEP_BATCH: i64 = 1000;

const SSO_PROVIDER_COLUMNS: &str = "id, org_id, name, slug, issuer, client_id, secret_ciphertext, \
     secret_nonce, scopes, group_claim, default_role, enabled, created_at";

/// What to do with the sealed client secret on an update: leave the stored one
/// alone, clear it (a provider that becomes a public PKCE client), or replace
/// it with freshly sealed bytes (a rotation at the IdP).
#[derive(Debug, Clone, Copy)]
pub enum SecretUpdate<'a> {
    Keep,
    Clear,
    Set(&'a [u8], &'a [u8]),
}

/// The mutable half of an `sso_providers` row.
///
/// A struct rather than an argument list because the row has eight editable
/// columns and four of them are strings - positional arguments there are a
/// silent swap waiting to happen.
#[derive(Debug, Clone, Copy)]
pub struct SsoProviderUpdate<'a> {
    pub name: &'a str,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub secret: SecretUpdate<'a>,
    pub scopes: &'a [String],
    pub group_claim: &'a str,
    pub default_role: Option<&'a str>,
    pub enabled: bool,
}

/// The identity provider and sign-in settings of an SSO provider being registered.
#[derive(Debug, Clone, Copy)]
pub struct NewSsoProvider<'a> {
    pub name: &'a str,
    pub slug: &'a str,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub secret: Option<(&'a [u8], &'a [u8])>,
    pub scopes: &'a [String],
    pub group_claim: &'a str,
    pub default_role: Option<&'a str>,
}

impl SsoRepo<'_> {
    pub async fn create_provider(
        &self,
        org_id: Uuid,
        values: NewSsoProvider<'_>,
    ) -> Result<SsoProvider> {
        let NewSsoProvider {
            name,
            slug,
            issuer,
            client_id,
            secret,
            scopes,
            group_claim,
            default_role,
        } = values;
        let (ciphertext, nonce) = match secret {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into sso_providers (org_id, name, slug, issuer, client_id, \
                    secret_ciphertext, secret_nonce, scopes, group_claim, default_role) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             returning {SSO_PROVIDER_COLUMNS}"
        )))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(issuer)
        .bind(client_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(scopes)
        .bind(group_claim)
        .bind(default_role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Update the mutable half of a provider row in place.
    ///
    /// `slug` is deliberately not updatable: it is in the login URL, so
    /// changing it would break every bookmark, IdP redirect and org runbook
    /// already pointing at it (#1233).
    ///
    /// The sealed secret is three-valued - kept, cleared, or replaced - which
    /// the `case when $5` pair expresses in one statement, so a rotation can
    /// never leave the row with a ciphertext from one write and a nonce from
    /// another.
    pub async fn update_provider(
        &self,
        id: Uuid,
        update: SsoProviderUpdate<'_>,
    ) -> Result<LockoutGuard<SsoProvider>> {
        let (replace, ciphertext, nonce) = match update.secret {
            SecretUpdate::Keep => (false, None, None),
            SecretUpdate::Clear => (true, None, None),
            SecretUpdate::Set(c, n) => (true, Some(c), Some(n)),
        };
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let (org_id, was_enabled): (Uuid, bool) = fetch_optional_or_not_found(
            sqlx::query_as("select org_id, enabled from sso_providers where id = $1").bind(id),
            &mut *tx,
            || format!("sso provider {id}"),
        )
        .await?;
        if was_enabled && !update.enabled {
            lock_org_sign_in(&mut tx, org_id).await?;
            if passwords_off(&mut tx, org_id).await?
                && !other_enabled_provider(&mut tx, org_id, id).await?
            {
                return Ok(LockoutGuard::WouldLockOut);
            }
        }
        let row = fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update sso_providers set \
                    name = $2, issuer = $3, client_id = $4, \
                    secret_ciphertext = case when $5 then $6 else secret_ciphertext end, \
                    secret_nonce = case when $5 then $7 else secret_nonce end, \
                    scopes = $8, group_claim = $9, default_role = $10, enabled = $11 \
             where id = $1 \
             returning {SSO_PROVIDER_COLUMNS}"
            )))
            .bind(id)
            .bind(update.name)
            .bind(update.issuer)
            .bind(update.client_id)
            .bind(replace)
            .bind(ciphertext)
            .bind(nonce)
            .bind(update.scopes)
            .bind(update.group_claim)
            .bind(update.default_role)
            .bind(update.enabled),
            &mut *tx,
            || format!("sso provider {id}"),
        )
        .await?;
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done(row))
    }

    pub async fn list_providers(&self, org_id: Uuid) -> Result<Vec<SsoProvider>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_provider(&self, id: Uuid) -> Result<SsoProvider> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {SSO_PROVIDER_COLUMNS} from sso_providers where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("sso provider {id}"),
        )
        .await
    }

    /// Resolve the provider a login URL names. Slugs are unique per org, and a
    /// login URL carries no org, so a duplicate slug across orgs is ambiguous
    /// and rejected rather than resolved arbitrarily.
    pub async fn find_provider_by_slug(&self, slug: &str) -> Result<Option<SsoProvider>> {
        let mut rows: Vec<SsoProvider> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers where slug = $1 and enabled limit 2"
        )))
        .bind(slug)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        if rows.len() > 1 {
            return Err(Error::Store(format!(
                "sso slug '{slug}' is registered by more than one org; log in through the \
                 org-specific url"
            )));
        }
        Ok(rows.pop())
    }

    /// Every provider the login screen may offer: enabled, and owned by an org
    /// whose auth policy allows single sign-on. An org with no policy row
    /// allows it, as [`OrgAuthPolicyRepo::get`](super::auth::OrgAuthPolicyRepo::get) reads it, so a deployment that
    /// never set a policy keeps its buttons. Returns names and slugs the login
    /// URL already exposes; never secrets.
    pub async fn list_sign_in_providers(&self) -> Result<Vec<SsoProvider>> {
        // `not exists` rather than a join, so the shared column list needs no
        // table prefix and a missing policy row reads as sso on (#2339)
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers
             where enabled
               and not exists (
                   select 1 from org_auth_policies p
                   where p.org_id = sso_providers.org_id and not p.allow_sso
               )
             order by name"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Delete a provider, refusing when it is the org's last enabled one and
    /// password sign-in is off (#2233).
    pub async fn delete_provider(&self, id: Uuid) -> Result<LockoutGuard<()>> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let (org_id, was_enabled): (Uuid, bool) = fetch_optional_or_not_found(
            sqlx::query_as("select org_id, enabled from sso_providers where id = $1").bind(id),
            &mut *tx,
            || format!("sso provider {id}"),
        )
        .await?;
        if was_enabled {
            lock_org_sign_in(&mut tx, org_id).await?;
            if passwords_off(&mut tx, org_id).await?
                && !other_enabled_provider(&mut tx, org_id, id).await?
            {
                return Ok(LockoutGuard::WouldLockOut);
            }
        }
        require_affected(
            sqlx::query("delete from sso_providers where id = $1").bind(id),
            &mut *tx,
            || format!("sso provider {id}"),
        )
        .await?;
        tx.commit().await.map_err(store_err)?;
        Ok(LockoutGuard::Done(()))
    }

    /// Open the sealed client secret. Returns `None` for a public client.
    pub async fn client_secret(
        &self,
        kek: &super::super::crypto::Kek,
        provider: &SsoProvider,
    ) -> Result<Option<String>> {
        match (&provider.secret_ciphertext, &provider.secret_nonce) {
            (Some(c), Some(n)) => Ok(Some(kek.decrypt(c, n)?)),
            _ => Ok(None),
        }
    }

    pub async fn add_mapping(
        &self,
        provider_id: Uuid,
        group_name: &str,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
    ) -> Result<SsoGroupMapping> {
        sqlx::query_as(
            "insert into sso_group_mappings (provider_id, group_name, org_id, team_id, \
                    project_id, role) \
             values ($1, $2, $3, $4, $5, $6) \
             returning id, provider_id, group_name, org_id, team_id, project_id, role, created_at",
        )
        .bind(provider_id)
        .bind(group_name)
        .bind(org_id)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list_mappings(&self, provider_id: Uuid) -> Result<Vec<SsoGroupMapping>> {
        sqlx::query_as(
            "select id, provider_id, group_name, org_id, team_id, project_id, role, created_at \
             from sso_group_mappings where provider_id = $1 order by group_name",
        )
        .bind(provider_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete_mapping(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from sso_group_mappings where id = $1").bind(id),
            self.0,
            || format!("sso group mapping {id}"),
        )
        .await?;
        Ok(())
    }

    pub async fn start_login(
        &self,
        state: &str,
        provider_id: Uuid,
        code_verifier: &str,
        nonce: &str,
        redirect_uri: &str,
    ) -> Result<()> {
        sqlx::query(
            "insert into sso_login_states (state, provider_id, code_verifier, nonce, redirect_uri) \
             values ($1, $2, $3, $4, $5)",
        )
        .bind(state)
        .bind(provider_id)
        .bind(code_verifier)
        .bind(nonce)
        .bind(redirect_uri)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Consume a login state exactly once. A replayed callback finds nothing,
    /// so an intercepted `code`+`state` pair cannot be redeemed twice. States
    /// older than `max_age_secs` are treated as absent and swept.
    pub async fn consume_login(
        &self,
        state: &str,
        max_age_secs: i64,
    ) -> Result<Option<SsoLoginState>> {
        // opportunistic sweep: expired rows are worthless. bounded, so a large
        // backlog costs one batch here and the periodic sweeper takes the rest
        let _ = self.sweep_login_states(max_age_secs, SSO_SWEEP_BATCH).await;
        // the row is spent either way, but an expired one is refused here
        // rather than trusted to the sweep above: a bounded sweep may stop
        // before it reaches this state
        sqlx::query_as(
            "with spent as (delete from sso_login_states where state = $1 \
                 returning state, provider_id, code_verifier, nonce, redirect_uri, created_at) \
             select state, provider_id, code_verifier, nonce, redirect_uri, created_at from spent \
             where created_at >= now() - ($2 || ' seconds')::interval",
        )
        .bind(state)
        .bind(max_age_secs.to_string())
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Delete at most `limit` login states older than `max_age_secs`, oldest
    /// first, and return how many went. A login abandoned at the identity
    /// provider is never consumed, so without this its row would stay forever.
    ///
    /// Rows another transaction holds are skipped rather than waited on, so
    /// two control-plane replicas sweeping at once split the work instead of
    /// queueing behind each other.
    pub async fn sweep_login_states(&self, max_age_secs: i64, limit: i64) -> Result<u64> {
        let res = sqlx::query(
            "delete from sso_login_states where state in ( \
                 select state from sso_login_states \
                 where created_at < now() - ($1 || ' seconds')::interval \
                 order by created_at limit $2 for update skip locked)",
        )
        .bind(max_age_secs.to_string())
        .bind(limit)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected())
    }

    /// Delete at most `limit` exchange codes past their expiry, oldest first,
    /// and return how many went. A code whose browser never came back to
    /// redeem it would otherwise stay forever; locked rows are skipped as in
    /// [`SsoRepo::sweep_login_states`].
    pub async fn sweep_exchange_codes(&self, limit: i64) -> Result<u64> {
        let res = sqlx::query(
            "delete from sso_exchange_codes where code_hash in ( \
                 select code_hash from sso_exchange_codes where expires_at < now() \
                 order by expires_at limit $1 for update skip locked)",
        )
        .bind(limit)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected())
    }

    /// Record a one-time exchange code for a completed browser sign-in. Only
    /// the digest of the code is stored; `ttl_secs` counts from the database
    /// clock, like every other expiry here.
    pub async fn issue_exchange(
        &self,
        code_hash: &str,
        user_id: Uuid,
        provider_id: Uuid,
        granted_roles: &[String],
        ttl_secs: i64,
    ) -> Result<()> {
        sqlx::query(
            "insert into sso_exchange_codes (code_hash, user_id, provider_id, granted_roles, expires_at) \
             values ($1, $2, $3, $4, now() + ($5 || ' seconds')::interval)",
        )
        .bind(code_hash)
        .bind(user_id)
        .bind(provider_id)
        .bind(granted_roles)
        .bind(ttl_secs.to_string())
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Redeem an exchange code exactly once. An unknown, already-redeemed or
    /// expired code yields `None`; the delete is the single-use guarantee, so
    /// two concurrent redemptions cannot both succeed.
    pub async fn redeem_exchange(&self, code_hash: &str) -> Result<Option<SsoExchangeCode>> {
        // opportunistic bounded sweep, as for login states
        let _ = self.sweep_exchange_codes(SSO_SWEEP_BATCH).await;
        sqlx::query_as(
            "delete from sso_exchange_codes where code_hash = $1 and expires_at > now() \
             returning user_id, provider_id, granted_roles",
        )
        .bind(code_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }
}
