//! Row types returned by the repository layer. These mirror `migrations/`
//! column-for-column; domain interpretation (e.g. parsing `strategy` into
//! [`rolter_core::BalancingStrategy`]) is left to callers such as the
//! control-plane API and [`super::PostgresConfigStore`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Org {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Team {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub team_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Provider {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    /// stable, URL-safe identity for `provider-slug/model` addressing;
    /// `unique(org_id, slug)` and immutable by default
    pub slug: String,
    /// a supported provider kind such as `openai`, `ollama`, `openrouter`, or `tei`
    pub kind: String,
    pub api_base: String,
    pub api_key_env: Option<String>,
    pub egress_proxy: Option<String>,
    pub egress_proxies: sqlx::types::Json<Vec<String>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Route {
    pub id: Uuid,
    pub project_id: Uuid,
    pub model: String,
    /// one of `round_robin` | `random` | `power_of_two` | `consistent_hash` | `cache_aware` | `weighted` | `pipeline`
    pub strategy: String,
    pub enabled: bool,
    /// admin default inference params (jsonb object); mirrors config `[routes.params]`
    pub params: serde_json::Value,
    /// override policy (jsonb `{mode, allow, deny}`); mirrors config `[routes.param_policy]`
    pub param_policy: serde_json::Value,
    /// catalog metadata and per-model execution policy; mirrors `[routes.advanced]`
    pub advanced: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RouteTarget {
    pub id: Uuid,
    pub route_id: Uuid,
    pub provider_id: Uuid,
    pub upstream_model: Option<String>,
    pub weight: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct VirtualKey {
    pub id: Uuid,
    pub project_id: Uuid,
    pub key_hash: String,
    pub key_prefix: String,
    pub name: Option<String>,
    pub models: Vec<String>,
    /// empty means the key may reach every provider on an allowed route
    pub providers: Vec<String>,
    pub disabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// per-key response-cache override; `NULL` inherits the route decision
    pub cache_enabled: Option<bool>,
    /// local account that minted this key via the self-service panel; `NULL`
    /// for admin-created or bootstrap-config keys (ROL-224)
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// a virtual key owned by the current user, enriched with the project/org names
/// it belongs to so the self-service panel can label it without needing admin
/// read access to the tenancy tables. never carries the key hash.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OwnedVirtualKey {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_name: String,
    pub org_name: String,
    pub key_prefix: String,
    pub name: Option<String>,
    pub models: Vec<String>,
    pub disabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Budget {
    pub id: Uuid,
    /// one of `org` | `team` | `project` | `virtual_key`
    pub scope_type: String,
    pub scope_id: Uuid,
    /// decimal(12,4), returned as text to avoid a numeric-crate dependency
    pub limit_usd: String,
    pub period: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RateLimit {
    pub id: Uuid,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub rpm: Option<i32>,
    pub tpm: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ModelPrice {
    pub id: Uuid,
    pub model: String,
    /// decimal(12,6), returned as text to avoid a numeric-crate dependency
    pub input_per_mtok: String,
    pub output_per_mtok: String,
    pub cached_input_per_mtok: Option<String>,
    pub currency: String,
    pub created_at: DateTime<Utc>,
}

/// a local account. `password_hash` is `None` for sso-only users (a later
/// phase) and is never serialized back to a client
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub is_superadmin: bool,
    /// set when an admin deactivates the account; a non-null value blocks login
    /// while keeping the row, memberships and audit trail intact
    pub deactivated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// a role grant at a scope; scope is the most specific non-null id among
/// `org_id`/`team_id`/`project_id`
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Membership {
    pub id: Uuid,
    pub user_id: Uuid,
    pub org_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// one of `admin` | `member` | `viewer`
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// a record of an admin/CRUD/auth action, for the audit-log API
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub detail: Option<serde_json::Value>,
    pub at: DateTime<Utc>,
}

/// a login session. `token_hash` is the peppered digest of the opaque bearer
/// token handed to the client; the plaintext token is never stored
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}
