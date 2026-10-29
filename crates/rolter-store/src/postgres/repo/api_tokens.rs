//! Scoped control-plane API tokens (#1836).

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::ApiToken;
use super::support::{fetch_optional_or_not_found, store_err};

const API_TOKEN_COLUMNS: &str = "id, org_id, team_id, project_id, name, role, custom_role_id, \
     token_hash, token_prefix, expires_at, created_by, created_at, rotated_at, last_used_at, \
     revoked_at";

/// What a mint writes. The digest is computed by the caller; the plaintext
/// never reaches this layer.
pub struct NewApiToken<'a> {
    pub org_id: Uuid,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub name: &'a str,
    pub role: Option<&'a str>,
    pub custom_role_id: Option<Uuid>,
    pub token_hash: &'a str,
    pub token_prefix: &'a str,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_by: Option<Uuid>,
}

/// API tokens. Lookup is by peppered digest only.
pub struct ApiTokenRepo<'a>(pub &'a PgPool);

impl ApiTokenRepo<'_> {
    pub async fn create(&self, new: NewApiToken<'_>) -> Result<ApiToken> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into api_tokens (org_id, team_id, project_id, name, role, custom_role_id, \
             token_hash, token_prefix, expires_at, created_by) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) returning {API_TOKEN_COLUMNS}"
        )))
        .bind(new.org_id)
        .bind(new.team_id)
        .bind(new.project_id)
        .bind(new.name)
        .bind(new.role)
        .bind(new.custom_role_id)
        .bind(new.token_hash)
        .bind(new.token_prefix)
        .bind(new.expires_at)
        .bind(new.created_by)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// every token of an org, newest first, revoked and expired ones included
    /// so the audit trail's names still resolve
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ApiToken>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {API_TOKEN_COLUMNS} from api_tokens where org_id = $1 \
             order by created_at desc"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ApiToken> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {API_TOKEN_COLUMNS} from api_tokens where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("api token {id}"),
        )
        .await
    }

    /// Resolve a presented token by digest. A revoked or expired token never
    /// resolves, so both take effect on the next request with no cache to
    /// invalidate.
    pub async fn find_active_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {API_TOKEN_COLUMNS} from api_tokens \
             where token_hash = $1 and revoked_at is null \
               and (expires_at is null or expires_at > now())"
        )))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// [`Self::find_active_by_hash`] by the token's row id, for a download
    /// ticket that names the token it was minted with rather than its secret
    /// (#3504).
    pub async fn find_active_by_id(&self, id: Uuid) -> Result<Option<ApiToken>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {API_TOKEN_COLUMNS} from api_tokens \
             where id = $1 and revoked_at is null \
               and (expires_at is null or expires_at > now())"
        )))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Replace the secret of a live token. The old digest stops resolving in
    /// the same statement; a revoked token is not revived (`None`).
    pub async fn rotate(
        &self,
        id: Uuid,
        token_hash: &str,
        token_prefix: &str,
    ) -> Result<Option<ApiToken>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "update api_tokens set token_hash = $2, token_prefix = $3, rotated_at = now() \
             where id = $1 and revoked_at is null returning {API_TOKEN_COLUMNS}"
        )))
        .bind(id)
        .bind(token_hash)
        .bind(token_prefix)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<ApiToken> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update api_tokens set revoked_at = coalesce(revoked_at, now()) \
                 where id = $1 returning {API_TOKEN_COLUMNS}"
            )))
            .bind(id),
            self.0,
            || format!("api token {id}"),
        )
        .await
    }

    /// Best-effort last-used stamp, written at most once a minute per token so
    /// a busy pipeline does not turn every request into a write.
    pub async fn touch(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "update api_tokens set last_used_at = now() where id = $1 \
             and (last_used_at is null or last_used_at < now() - interval '1 minute')",
        )
        .bind(id)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }
}
