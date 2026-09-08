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
pub struct BusinessUnit {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub retired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub org_id: Uuid,
    pub business_unit_id: Option<Uuid>,
    pub name: String,
    pub slug: String,
    pub retired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub published_version: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PromptTemplateVersion {
    pub template_id: Uuid,
    pub version: i32,
    pub variables: serde_json::Value,
    pub decorators: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PromptTemplateScope {
    pub template_id: Uuid,
    pub version: i32,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Skill {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub retired_at: Option<DateTime<Utc>>,
    pub published_version: Option<i32>,
    pub allowed_team_ids: Vec<Uuid>,
    pub minimum_role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SkillVersion {
    pub skill_id: Uuid,
    pub version: i32,
    pub content: Option<String>,
    pub content_ref: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// one configured plugin instance at org or project scope
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PluginInstance {
    pub id: Uuid,
    pub org_id: Uuid,
    pub project_id: Option<Uuid>,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub kind: String,
    pub stage: String,
    pub enabled: bool,
    pub position: i32,
    pub failure_mode: String,
    pub endpoint: String,
    pub secret_env: Option<String>,
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

/// A provider group: a fleet of providers addressable as `group-slug/model`
/// (ADR-0017 addendum, ADR-0022). Org-scoped; the slug shares the provider slug
/// namespace.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProviderGroup {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    /// stable, URL-safe identity; `unique(org_id, slug)`, immutable by default
    pub slug: String,
    /// one of the balancing-strategy keys (`round_robin`, `weighted`, …)
    pub strategy: String,
    pub created_at: DateTime<Utc>,
}

/// One membership row of a [`ProviderGroup`]. `provider_name` is joined in for
/// config assembly; `upstream_model` null means passthrough of the requested
/// model.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProviderGroupMember {
    pub group_id: Uuid,
    pub provider_id: Uuid,
    pub provider_name: String,
    pub upstream_model: Option<String>,
    pub weight: i32,
    pub position: i32,
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
    /// business unit this key's spend rolls up to; `NULL` leaves the key
    /// attributed to its tenancy chain only
    pub business_unit_id: Option<Uuid>,
    /// customer this key's spend rolls up to; `NULL` when unattributed
    pub customer_id: Option<Uuid>,
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
    /// this budget's own `ignore` | `warn` | `block` answer to unpriced
    /// traffic; `None` inherits the deployment-wide setting (#996)
    pub unpriced_policy: Option<String>,
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
/// singleton persisted feature flags that gate supported subsystems
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FeatureFlags {
    pub response_cache: bool,
    pub cache_aware_routing: bool,
    pub circuit_breaker: bool,
    pub active_health_checks: bool,
    pub complexity_routing: bool,
    pub guardrails: bool,
    pub updated_at: DateTime<Utc>,
}

/// singleton persisted runtime policy projected into snapshots
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RuntimePolicy {
    pub retry_max_retries: i32,
    pub retry_base_ms: i32,
    pub retry_max_ms: i32,
    pub timeout_connect_s: i32,
    pub timeout_request_s: i32,
    pub queue_enabled: bool,
    pub queue_capacity: i32,
    pub queue_workers: i32,
    pub queue_backpressure: String,
    pub queue_block_ms: i32,
    pub updated_at: DateTime<Utc>,
}

/// a gateway or control node seen by the control plane, refreshed on every
/// snapshot poll it makes
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ClusterNode {
    pub id: String,
    pub role: String,
    pub build_version: String,
    /// config snapshot version the node reported running
    pub config_version: i64,
    /// operator-requested state: `active` or `draining`
    pub desired_state: String,
    pub state_changed_at: DateTime<Utc>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// the latest adaptive-routing state one node reported for one route (#751).
/// A scoreboard row, overwritten by every report rather than appended to.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AdaptiveRoutingTelemetry {
    pub node_id: String,
    /// public model name of the route
    pub model: String,
    /// whether the blend was routing at sample time
    pub engaged: bool,
    /// picks the route observed since that node built its balancer
    pub observed: i64,
    pub blend_picks: i64,
    pub exploration_picks: i64,
    pub fallback_picks: i64,
    /// the sanitized policy that node actually applies
    pub policy: serde_json::Value,
    /// per-target signals and scores, target-order aligned with the route
    pub targets: serde_json::Value,
    pub reported_at: DateTime<Utc>,
}

/// singleton persisted cross-dialect compatibility policy projected into
/// snapshots
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct CompatibilityPolicy {
    pub anthropic_version: String,
    pub default_max_tokens: i32,
    pub updated_at: DateTime<Utc>,
}

/// singleton client-facing settings projected into snapshots: what the gateway
/// does with headers on the upstream leg, plus the advertised base URL (#564)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ClientSettings {
    pub public_base_url: Option<String>,
    pub forwarded_headers: Vec<String>,
    pub injected_headers: serde_json::Value,
    pub request_id_header: String,
    pub updated_at: DateTime<Utc>,
}

/// the policy half of `security_settings`, and only that half: the dashboard
/// credential columns exist on the table but are deliberately absent here so
/// they cannot reach a snapshot by accident (#1162)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SecurityPolicyRow {
    pub virtual_key_required: bool,
    pub required_headers: serde_json::Value,
    pub auth_bypass_routes: Vec<String>,
}

/// singleton inference-parameter defaults projected into snapshots (#564)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ModelDefaults {
    pub enabled: bool,
    pub default_model: Option<String>,
    pub default_temperature: Option<f64>,
    pub default_top_p: Option<f64>,
    pub default_max_tokens: Option<i32>,
    pub updated_at: DateTime<Utc>,
}

/// serialize a sealed column as "is there one", never as its bytes. used for
/// [`SsoProvider::secret_ciphertext`], which is renamed to `has_client_secret`
/// on the wire (#1231)
fn is_present<S: serde::Serializer>(
    value: &Option<Vec<u8>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_bool(value.is_some())
}

/// an OIDC identity provider registered for one org.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SsoProvider {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub issuer: String,
    pub client_id: String,
    /// sealed client secret. the bytes are read only by the token exchange,
    /// through [`super::repo::SsoRepo::client_secret`], and never leave the
    /// control plane — what is serialized in their place is the derived
    /// boolean `has_client_secret`, so an operator can see that a secret is
    /// stored without anything being able to see the secret (#1231). without
    /// it the first symptom of a provider registered with no secret, or one
    /// dropped because `ROLTER_KEK` was unset at the time, is a failed token
    /// exchange at login
    #[serde(rename = "has_client_secret", serialize_with = "is_present")]
    #[serde(skip_deserializing)]
    pub secret_ciphertext: Option<Vec<u8>>,
    #[serde(skip)]
    pub secret_nonce: Option<Vec<u8>>,
    pub scopes: Vec<String>,
    pub group_claim: String,
    /// role granted when no group mapping matches; `None` denies login to a
    /// user the IdP has not put in a mapped group
    pub default_role: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// an IdP group name granting a role at a scope
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SsoGroupMapping {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub group_name: String,
    pub org_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// one in-flight authorization-code login
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SsoLoginState {
    pub state: String,
    pub provider_id: Uuid,
    pub code_verifier: String,
    pub nonce: String,
    pub redirect_uri: String,
    pub created_at: DateTime<Utc>,
}

/// a SCIM provisioning token. `token_hash` is peppered sha-256; the plaintext
/// is returned once at creation and never stored.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ScimToken {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// the SCIM view of a local user within one org: what the IdP calls them and
/// the stable id it knows them by.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ScimIdentity {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub external_id: Option<String>,
    pub user_name: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// a SCIM group inside one org. `display_name` is the name mappings are keyed
/// on, which is also the name an operator sees in the IdP.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ScimGroup {
    pub id: Uuid,
    pub org_id: Uuid,
    pub external_id: Option<String>,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// a SCIM group name granting a role at an org/team/project scope, with the
/// same "most specific non-null id" convention as [`Membership`] and
/// [`SsoGroupMapping`]
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ScimGroupMapping {
    pub id: Uuid,
    pub org_id: Uuid,
    pub group_name: String,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

/// an MCP server an org has registered; the anchor OAuth grants hang off
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpServer {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub url: String,
    /// one of `stdio` | `sse` | `streamable_http` | `websocket`
    pub transport: String,
    pub description: String,
    /// only enabled servers are projected into gateway snapshots
    pub enabled: bool,
    /// tool names advertised by the registry, never executable code
    pub tools: Vec<String>,
    /// `custom` or `library`
    pub source: String,
    /// OAuth scopes every proxied call to this server must carry
    pub required_scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// authorization endpoint a user's browser is sent to for consent (#707)
    pub authorize_url: Option<String>,
    /// token endpoint the code, refresh and exchange grants are posted to
    pub token_url: Option<String>,
    /// the OAuth client rolter presents; the matching secret is sealed and is
    /// deliberately **not** on this struct, so a serialized `McpServer` cannot
    /// carry it into an API response
    pub client_id: Option<String>,
    /// scopes requested when a consent flow does not name its own
    pub default_scopes: Vec<String>,
    /// whether a sealed client secret is stored, so a UI can show that the
    /// client is confidential without the control plane handing the secret out
    pub has_client_secret: bool,
}

/// One in-flight authorization-code consent, opened by the callback. The PKCE
/// verifier is decrypted here and nowhere else; like
/// [`super::repo::McpSessionTokens`] this is deliberately not `Serialize`.
#[derive(Debug, Clone)]
pub struct McpLoginState {
    pub state: String,
    pub server_id: Uuid,
    pub user_id: Uuid,
    pub code_verifier: String,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    pub created_at: DateTime<Utc>,
}

/// a governed, named bundle of MCP tool references
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpToolGroup {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub enabled: bool,
    pub tools: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// organization defaults managed by the MCP Settings screen
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpGatewaySettings {
    pub org_id: Uuid,
    pub default_transport: String,
    pub connect_timeout_ms: i32,
    pub request_timeout_ms: i32,
    pub max_retries: i32,
    pub default_failure_mode: String,
    pub allow_unlisted_tools: bool,
    pub updated_at: DateTime<Utc>,
}

/// a user's consent grant against one MCP server. Revoked grants are kept so
/// the audit trail survives; `revoked_at` is the live/dead flag.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpOAuthGrant {
    pub id: Uuid,
    pub server_id: Uuid,
    pub user_id: Uuid,
    pub scopes: Vec<String>,
    pub granted_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<Uuid>,
}

/// Session metadata for a grant, with **no** token material on it. The sealed
/// tokens live in the same row but are only ever read through
/// [`super::repo::McpOAuthRepo::open_session`], so a DTO that reaches an API
/// response cannot carry them by accident.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpOAuthSession {
    pub id: Uuid,
    pub grant_id: Uuid,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// whether a refresh token is stored, so a UI can show renewability
    /// without the control plane ever handing the token out
    pub has_refresh_token: bool,
}

/// singleton persisted adaptive-routing policy projected into snapshots
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AdaptiveRoutingPolicy {
    pub enabled: bool,
    pub latency_weight: f32,
    pub cost_weight: f32,
    pub load_weight: f32,
    pub exploration_ratio: f32,
    pub min_samples: i32,
    pub updated_at: DateTime<Utc>,
}

/// singleton persisted request-log policy projected into snapshots
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct LoggingSettings {
    pub sample_rate: f64,
    pub payload_capture_enabled: bool,
    pub payload_capture_max_bytes: i32,
    pub payload_capture_redact_fields: Vec<String>,
    pub payload_capture_models: Vec<String>,
    pub payload_capture_virtual_key_ids: Vec<String>,
    /// how long request-log metadata is kept in clickhouse
    pub retention_days: i32,
    /// how long captured raw payloads are kept; always the shorter clock
    pub payload_retention_hours: i32,
    pub updated_at: DateTime<Utc>,
}

/// one ordered built-in or custom regex rule in the global guardrail policy
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct GuardrailRule {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub source_type: String,
    pub builtin: Option<String>,
    pub pattern: Option<String>,
    pub stage: String,
    pub action: String,
    pub replacement: Option<String>,
    pub include_system: bool,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// one external guardrail service; at most one row can be enabled at a time
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct GuardrailProvider {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub url: String,
    pub stage: String,
    pub timeout_ms: i32,
    pub max_retries: i32,
    pub failure_mode: String,
    pub max_body_bytes: i32,
    pub auth_kind: String,
    pub auth_env: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    /// who granted this: `manual` (invitation, seed, admin API) or `sso` (an
    /// IdP group mapping). SSO reconciliation only ever touches its own rows
    pub source: String,
    pub created_at: DateTime<Utc>,
}

/// a pending (or spent) invitation to join an org at a scope
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Invitation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub role: String,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// peppered digest of the one-time token; never serialized, and never
    /// compared outside [`super::repo::InvitationRepo::find_live_by_hash`]
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub invited_by: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// how one org's members are allowed to authenticate
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OrgAuthPolicy {
    pub org_id: Uuid,
    pub allow_password_login: bool,
    pub allow_sso: bool,
    pub updated_at: DateTime<Utc>,
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

/// Global control-plane security settings. Managed dashboard credentials are
/// encrypted separately and intentionally never appear on this DTO.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub virtual_key_required: bool,
    pub allowed_origins: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub required_headers: serde_json::Value,
    pub auth_bypass_routes: Vec<String>,
    pub dashboard_auth_enabled: bool,
    pub dashboard_credential_ref: Option<String>,
    pub dashboard_secret_configured: bool,
    pub updated_at: DateTime<Utc>,
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

// ---------------------------------------------------------------------------
// configurable rbac (#534): custom roles and access profiles
// ---------------------------------------------------------------------------

/// an org-scoped role an operator defined. `base_role` is the built-in role it
/// is at least equivalent to; the explicit grants in [`CustomRoleGrant`] widen
/// it further
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct CustomRole {
    pub id: Uuid,
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    /// one of `admin` | `member` | `viewer`
    pub base_role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// one explicit `(resource, action)` a custom role grants. `resource` names a
/// row in the control plane's capability table; `action` is one of `read` |
/// `create` | `update` | `delete`
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct CustomRoleGrant {
    pub id: Uuid,
    pub role_id: Uuid,
    pub resource: String,
    pub action: String,
}

/// a reusable bundle of custom roles, each pinned to a scope
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AccessProfile {
    pub id: Uuid,
    pub org_id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// one `(custom role, scope)` pair inside a profile. Scope follows the same
/// most-specific-non-null convention as [`Membership`]
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AccessProfileRole {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub role_id: Uuid,
    pub org_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// a profile handed to a user or to a whole team (exactly one is set)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AccessProfileAssignment {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub user_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// model/route visibility carried by a profile. An empty allow-list means "no
/// restriction"; deny beats allow. Entries are exact names or a trailing-`*`
/// prefix glob
#[derive(Debug, Clone, Default, FromRow, Serialize, Deserialize)]
pub struct AccessProfilePolicy {
    pub profile_id: Uuid,
    pub allowed_models: Vec<String>,
    pub denied_models: Vec<String>,
    pub allowed_routes: Vec<String>,
    pub denied_routes: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

/// One `(custom role at a scope)` a user actually holds, flattened across every
/// profile that reaches them — directly or through a team they belong to.
///
/// `resource`/`action` are `None` for a composition whose role has no explicit
/// grants: the row still carries `base_role`, which is the whole point of a
/// role that only widens the built-in floor.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct EffectiveGrant {
    pub profile_id: Uuid,
    pub role_id: Uuid,
    pub role_slug: String,
    pub base_role: String,
    pub org_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub resource: Option<String>,
    pub action: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(secret: Option<Vec<u8>>) -> SsoProvider {
        SsoProvider {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            name: "Okta".to_string(),
            slug: "okta".to_string(),
            issuer: "https://example.okta.com".to_string(),
            client_id: "client-1".to_string(),
            secret_nonce: secret.as_ref().map(|_| vec![9u8; 12]),
            secret_ciphertext: secret,
            scopes: vec!["openid".to_string()],
            group_claim: "groups".to_string(),
            default_role: None,
            enabled: true,
            created_at: DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is a valid timestamp"),
        }
    }

    #[test]
    fn sso_provider_reports_a_stored_secret_without_serializing_it() {
        let json = serde_json::to_value(provider(Some(b"sealed-bytes".to_vec())))
            .expect("SsoProvider serializes");
        assert_eq!(json["has_client_secret"], serde_json::json!(true));
        // the sealed columns themselves must never appear on the wire, under
        // any name, and neither must their contents (#1231)
        assert!(json.get("secret_ciphertext").is_none());
        assert!(json.get("secret_nonce").is_none());
        assert!(!json.to_string().contains("sealed-bytes"));
    }

    #[test]
    fn sso_provider_without_a_secret_says_so() {
        // the case the dashboard could not see: registered with no secret, or
        // the secret dropped because ROLTER_KEK was unset at the time. the
        // first symptom used to be a failed token exchange at login
        let json = serde_json::to_value(provider(None)).expect("SsoProvider serializes");
        assert_eq!(json["has_client_secret"], serde_json::json!(false));
    }
}
