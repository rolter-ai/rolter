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
    /// one of `openai` | `anthropic` | `openai_compatible` | `ollama` | `llama_cpp`
    pub kind: String,
    pub api_base: String,
    pub api_key_env: Option<String>,
    pub egress_proxy: Option<String>,
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
    pub disabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// per-key response-cache override; `NULL` inherits the route decision
    pub cache_enabled: Option<bool>,
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
