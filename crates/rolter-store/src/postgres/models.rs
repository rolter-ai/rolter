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

/// A project listed org-wide, carrying the team it belongs to.
///
/// Projects are named per team, so two teams may each own a "prod" and an
/// ungrouped org-wide list would offer the same label twice. `team_name` rides
/// along with the row so a caller can group by team without a second request
/// per team, which is the whole point of listing org-wide in the first place.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OrgProject {
    pub id: Uuid,
    pub team_id: Uuid,
    pub team_name: String,
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

/// A business unit as its org-wide listing returns it, with the number of live
/// virtual keys attributed to it.
///
/// A unit that shows no spend may simply have no key pointing at it, and the
/// dashboard cannot tell those apart from the row alone (#2581). The count
/// rides along with the row, flattened beside its columns, so the listing
/// answers it in the same query rather than one request per unit. A key is
/// live while it is neither disabled nor past its `expires_at`.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BusinessUnitListing {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub unit: BusinessUnit,
    pub live_key_count: i64,
}

/// A customer as its org-wide listing returns it, with the number of live
/// virtual keys attributed to it. See [`BusinessUnitListing`] for why the
/// count is part of the listing and what counts as live.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct CustomerListing {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub customer: Customer,
    pub live_key_count: i64,
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

/// `providers.egress_proxies` as the dashboard and the repo see it: the string
/// entries of the stored array.
///
/// The column only has to be a `jsonb` array (migration 0018), so a row written
/// by SQL, a seed or an import can hold a non-string. Decoding it as a typed
/// `Vec<String>` failed the whole row, which 500ed the provider listing and the
/// create/update `returning`, leaving the row impossible to repair through the
/// API that owns the column (#2946). This decode keeps the strings and drops the
/// rest (and reads a non-array as empty), so the row can still be listed and
/// rewritten. The snapshot loader reads the raw value on its own and leaves such
/// a provider out of the snapshot (#2943); that is what keeps traffic from
/// reaching an upstream around a pool this view cannot show in full.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EgressProxies(pub Vec<String>);

impl EgressProxies {
    /// Keep the string entries of `value`, returning how many entries (or `1`
    /// for a value that is not an array) were dropped.
    fn from_value(value: &serde_json::Value) -> (Self, usize) {
        match value.as_array() {
            Some(entries) => {
                let kept: Vec<String> = entries
                    .iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect();
                let dropped = entries.len() - kept.len();
                (Self(kept), dropped)
            }
            None => (Self::default(), 1),
        }
    }
}

impl sqlx::Type<sqlx::Postgres> for EgressProxies {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <sqlx::types::Json<serde_json::Value> as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <sqlx::types::Json<serde_json::Value> as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for EgressProxies {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let sqlx::types::Json(raw) =
            <sqlx::types::Json<serde_json::Value> as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        let (pool, dropped) = Self::from_value(&raw);
        if dropped > 0 {
            tracing::warn!(
                dropped,
                "a provider's egress_proxies holds entries that are not strings; \
                 they are not shown, and GET /api/v1/config/problems names the provider"
            );
        }
        Ok(pool)
    }
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
    pub egress_proxies: EgressProxies,
    /// vLLM kv-event stream for `precise_cache_aware`, as the json object the
    /// file config's `kv_events` table carries; `None` has no stream. Kept as
    /// raw json so one unreadable row cannot fail a whole listing (#2236)
    pub kv_events: Option<serde_json::Value>,
    /// LMCache controller for `lmcache_aware`; see `kv_events`
    pub lmcache: Option<serde_json::Value>,
    /// the project the provider is scoped to; `None` is org-wide. Only keys
    /// minted in that project may reach it (#1919)
    pub project_id: Option<Uuid>,
    /// opt out of the host pin a hosted kind carries, as the file config's
    /// field of the same name does (ADR-0029, #1133)
    #[serde(default)]
    pub allow_custom_api_base: bool,
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
    /// failure class tokens this target takes over (#1584); empty for an
    /// ordinary member of the pool
    #[serde(default)]
    pub takes_over: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// One weighted variant of a route (#391), the stored form of a
/// `[[routes.variants]]` entry. `position` is the declared order, which is the
/// fallback chain after the sampled primary.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RouteVariant {
    pub id: Uuid,
    pub route_id: Uuid,
    pub name: String,
    pub weight: i32,
    pub position: i32,
    pub params: serde_json::Value,
    pub created_at: DateTime<Utc>,
    /// the variant's targets in routing order; filled in by the repository
    #[sqlx(skip)]
    #[serde(default)]
    pub targets: Vec<RouteVariantTarget>,
}

/// One target of a [`RouteVariant`], tried in `position` order.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RouteVariantTarget {
    pub id: Uuid,
    pub variant_id: Uuid,
    pub provider_id: Uuid,
    pub upstream_model: Option<String>,
    pub weight: i32,
    pub position: i32,
    /// failure class tokens this target takes over (#3373), as
    /// [`RouteTarget::takes_over`]; empty for an ordinary member of the variant
    #[serde(default)]
    pub takes_over: Vec<String>,
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
    /// the project the group is scoped to; `None` is org-wide (#1919)
    pub project_id: Option<Uuid>,
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

/// What the last probe of a provider observed about prompt-cache reporting
/// (#984). A provider with no row has never been probed.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProviderCacheProbe {
    pub provider_id: Uuid,
    /// the probed response carried a prompt-cache usage field
    pub reports: bool,
    /// the field it was found under, as a JSON pointer
    pub spelling: Option<String>,
    /// the upstream model the probe asked
    pub model: String,
    pub probed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct VirtualKey {
    pub id: Uuid,
    pub project_id: Uuid,
    /// peppered digest of the secret. the gateway learns it through the
    /// snapshot; no API response carries it (#3525)
    #[serde(default, skip_serializing)]
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
    /// why the key exists when a person did not mint it by hand: `playground`
    /// for a short-lived key the dashboard minted for its own request, `NULL`
    /// for every key an operator created deliberately (#1640)
    pub purpose: Option<String>,
    pub created_at: DateTime<Utc>,
    /// peppered digest of the secret an admin rotation replaced, while its
    /// overlap window is open. a digest like `key_hash`, so it never leaves the
    /// API: only the prefix and the end of the window do (#1837)
    #[serde(default, skip_serializing)]
    pub previous_key_hash: Option<String>,
    /// first characters of the retired secret, so the Keys screen can say which
    /// one still authenticates
    #[serde(default)]
    pub previous_key_prefix: Option<String>,
    /// last instant the retired secret authenticates; `None` when no overlap is
    /// running
    #[serde(default)]
    pub previous_expires_at: Option<DateTime<Utc>>,
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
    /// `playground` for a key the dashboard minted for its own request, so the
    /// Keys screen can say what it is rather than leaving a reader to infer it
    /// from a short expiry (#1640)
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Budget {
    pub id: Uuid,
    /// one of `org` | `team` | `project` | `virtual_key` | `business_unit` |
    /// `customer` | `user`
    pub scope_type: String,
    pub scope_id: Uuid,
    /// decimal(12,4), returned as text to avoid a numeric-crate dependency
    pub limit_usd: String,
    pub period: String,
    /// this budget's own `ignore` | `warn` | `block` answer to unpriced
    /// traffic; `None` inherits the deployment-wide setting (#996)
    pub unpriced_policy: Option<String>,
    pub created_at: DateTime<Utc>,
    /// narrows a `user` budget to the keys the person minted in this org
    /// (#1830); null on every other scope
    pub within_org_id: Option<Uuid>,
    /// narrows a `user` budget to the keys the person minted in this project
    pub within_project_id: Option<Uuid>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RateLimit {
    pub id: Uuid,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub rpm: Option<i32>,
    pub tpm: Option<i32>,
    pub created_at: DateTime<Utc>,
    /// see [`Budget::within_org_id`]
    pub within_org_id: Option<Uuid>,
    /// see [`Budget::within_project_id`]
    pub within_project_id: Option<Uuid>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ModelPrice {
    pub id: Uuid,
    pub model: String,
    /// decimal(12,6), returned as text to avoid a numeric-crate dependency
    pub input_per_mtok: String,
    pub output_per_mtok: String,
    pub cached_input_per_mtok: Option<String>,
    /// rate for input tokens written to the prompt cache (#2876); `None` means
    /// the input rate
    pub cache_write_per_mtok: Option<String>,
    /// rate for input tokens written to the 1 hour prompt cache (#2891); `None`
    /// means `cache_write_per_mtok`
    pub cache_write_1h_per_mtok: Option<String>,
    /// price of one generated image (#2937); `None` charges nothing for images
    pub image_per_unit: Option<String>,
    /// price of one minute of input audio (#2937)
    pub audio_input_per_minute: Option<String>,
    /// price per million characters of text-to-speech input (#2937)
    pub speech_per_mchar: Option<String>,
    /// price per million audio input tokens (#1882); `None` means
    /// `input_per_mtok`
    pub audio_input_per_mtok: Option<String>,
    /// price per million audio output tokens (#1882); `None` means
    /// `output_per_mtok`
    pub audio_output_per_mtok: Option<String>,
    /// price per million cached audio input tokens (#1882); `None` means the
    /// cached rate, then the audio input rate
    pub cached_audio_input_per_mtok: Option<String>,
    pub currency: String,
    pub created_at: DateTime<Utc>,
    /// the catalogue this price was imported from (#966); `None` is a price an
    /// operator set, which is an override that an import never replaces
    pub source: Option<String>,
    /// the day the catalogue snapshot the price came from was taken
    pub source_imported_at: Option<DateTime<Utc>>,
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
    /// the oldest snapshot format the node stated it reads on its last poll
    /// that stated one; 1 for a node from before negotiation (ADR-0038)
    pub formats_min: i32,
    /// the newest snapshot format the node stated it reads
    pub formats_max: i32,
    /// the format the node's last poll was answered in, or `None` when that
    /// poll was refused for its format (or predates this column)
    pub served_format: Option<i32>,
}

/// the latest adaptive-routing state one node reported for one route (#751).
/// A scoreboard row, overwritten by every report rather than appended to.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AdaptiveRoutingTelemetry {
    pub node_id: String,
    /// the org that owns the route, empty for a deployment-wide route or a
    /// gateway that predates the column (#3458)
    pub org_id: String,
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

/// an LDAP directory registered for one org (#1826).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct LdapDirectory {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub url: String,
    pub bind_dn: String,
    /// sealed service-account password. read only by the directory bind,
    /// through [`super::repo::LdapRepo::bind_password`]; what is serialized in
    /// its place is the derived boolean `has_bind_password`, so an operator can
    /// see that one is stored without anything being able to see it
    #[serde(rename = "has_bind_password", serialize_with = "is_present")]
    #[serde(skip_deserializing)]
    pub bind_secret_ciphertext: Option<Vec<u8>>,
    #[serde(skip)]
    pub bind_secret_nonce: Option<Vec<u8>>,
    pub base_dn: String,
    pub user_filter: String,
    pub email_attr: String,
    pub display_name_attr: String,
    pub group_attr: String,
    pub require_tls: bool,
    /// role granted when no group mapping matches; `None` denies login to a
    /// user the directory has not put in a mapped group
    pub default_role: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// a directory group granting a role at a scope
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct LdapGroupMapping {
    pub id: Uuid,
    pub directory_id: Uuid,
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

/// a redeemed sso exchange code (#2297): who the browser's sign-in was for and
/// what the mapped groups granted, which is all the session is minted from.
#[derive(Debug, Clone, FromRow)]
pub struct SsoExchangeCode {
    pub user_id: Uuid,
    pub provider_id: Uuid,
    pub granted_roles: Vec<String>,
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

/// a scoped control-plane API token (#1836): a service account that holds one
/// role at an org, team or project. `token_hash` is a peppered sha-256; the
/// plaintext is returned once, at mint or rotation, and never stored.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: Uuid,
    pub org_id: Uuid,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub name: String,
    /// a built-in role (`admin` | `member` | `viewer`), or `None` when the
    /// token holds a custom role
    pub role: Option<String>,
    pub custom_role_id: Option<Uuid>,
    #[serde(skip_serializing)]
    pub token_hash: String,
    /// the first characters of the value, to recognise a token in a list
    pub token_prefix: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
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
    /// one of `sse` | `streamable_http` | `websocket` (stdio was dropped in #783)
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
    /// how rolter authenticates to this server: `none` | `bearer` | `header` |
    /// `oauth` (#952). `oauth` is the consent flow above; the other two present
    /// a static credential that the schema requires to be present for them and
    /// absent otherwise
    pub auth_kind: String,
    /// the header a `header`-kind api key is presented in, e.g. `X-Api-Key`.
    /// never `Authorization` — that one belongs to the bearer path, and the
    /// `mcp_servers_auth_header_name_shape` constraint refuses it
    pub auth_header_name: Option<String>,
    /// whether a sealed static credential is stored. the credential itself is
    /// deliberately **not** on this struct, for the same reason the client
    /// secret is not
    pub has_credential: bool,
    /// per-server overrides of the org's `mcp_gateway_settings`; `None`
    /// inherits, so an operator changing the org default still moves every
    /// server that never asked to differ
    pub connect_timeout_ms: Option<i32>,
    pub request_timeout_ms: Option<i32>,
    pub max_retries: Option<i32>,
    /// the authorization server's issuer identifier as an operator pinned it,
    /// for a server whose authorization server publishes no metadata (#1347).
    /// RFC 9207 validation has nothing to compare against without one
    pub oauth_issuer: Option<String>,
    /// `auto` (try RFC 9728 discovery, fall back to the columns above) or
    /// `manual` (use the columns above and never probe)
    pub oauth_discovery: String,
    /// what the last successful discovery resolved. a cache: the interactive
    /// authorize refreshes it, and the refresher and token exchange read it so
    /// they never have to probe an upstream of their own accord
    pub oauth_discovered_issuer: Option<String>,
    pub oauth_discovered_authorize_url: Option<String>,
    pub oauth_discovered_token_url: Option<String>,
    /// whether that metadata carried
    /// `authorization_response_iss_parameter_supported: true`, which is the
    /// row of the RFC 9207 §2.4 table a missing `iss` is judged by
    pub oauth_discovered_iss_supported: bool,
    pub oauth_discovered_at: Option<DateTime<Utc>>,
    /// tools in `tools` that the last successful `tools/list` no longer
    /// returned. Kept rather than pruned so a tool group that names one still
    /// validates, and flagged so a card can say it may be gone (#1283)
    pub tools_stale: Vec<String>,
    /// `pending` (never asked), `discovered` (the server answered, possibly
    /// with no tools) or `failed` (the last attempt got no answer)
    pub tools_discovery_status: String,
    /// a bounded category for a failed attempt, never the upstream's text
    pub tools_discovery_error: Option<String>,
    /// when `tools/list` last succeeded
    pub tools_discovered_at: Option<DateTime<Utc>>,
    /// when discovery last ran, whether or not it succeeded
    pub tools_discovery_attempted_at: Option<DateTime<Utc>>,
    /// when the control plane last got an MCP handshake out of the server, by
    /// a status probe or a discovery (#951)
    pub last_contact_at: Option<DateTime<Utc>>,
    /// what that handshake negotiated and who answered it
    pub last_protocol_version: Option<String>,
    pub last_server_name: Option<String>,
    /// the reason of the last failed contact (`unreachable`, `handshake_failed`,
    /// `auth_rejected`, `egress_denied`) and when it happened. Kept after a
    /// later success, so a status can say when it last broke
    pub last_error_kind: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
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
    /// the issuer of the authorization server this request was sent to, as
    /// RFC 9207 §2.4 requires it be recorded before the browser leaves (#1347)
    pub expected_issuer: Option<String>,
    /// whether that authorization server advertises the `iss` parameter
    pub iss_supported: bool,
    /// the RFC 8707 canonical resource identifier the authorization request
    /// carried; the token request must carry the same one
    pub resource: Option<String>,
    /// the token endpoint of the authorization server whose issuer was
    /// recorded, so the code goes back to the server that issued it
    pub token_url: Option<String>,
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

/// a virtual key, project or team an MCP tool group is assigned to (#3295)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct McpToolGroupAssignment {
    pub group_id: Uuid,
    pub subject_kind: String,
    pub subject_id: Uuid,
    pub created_at: DateTime<Utc>,
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
    /// built-in value detectors that scan captured text (#1835)
    pub payload_capture_redact_detectors: Vec<String>,
    /// operator-supplied value patterns, applied after the detectors
    pub payload_capture_redact_patterns:
        sqlx::types::Json<Vec<rolter_core::payload_redaction::RedactPattern>>,
    pub payload_capture_models: Vec<String>,
    pub payload_capture_virtual_key_ids: Vec<String>,
    /// how long request-log metadata is kept in clickhouse
    pub retention_days: i32,
    /// how long captured raw payloads are kept; always the shorter clock
    pub payload_retention_hours: i32,
    /// whether the control plane accepts dashboard UX events at all; `false`
    /// is the deployment-level opt-out behind `logging.ui_events`
    pub ui_events: bool,
    pub updated_at: DateTime<Utc>,
}

impl LoggingSettings {
    /// Write the stored half of `[logging]` onto `logging`, the projection a
    /// snapshot makes. The sink and batching keys have no column and are left
    /// as they are.
    pub fn project_onto(&self, logging: &mut rolter_core::LoggingConfig) {
        logging.sample_rate = self.sample_rate;
        logging.payload_capture.enabled = self.payload_capture_enabled;
        logging.payload_capture.max_bytes = self.payload_capture_max_bytes.max(0) as usize;
        logging.payload_capture.redact_fields = self.payload_capture_redact_fields.clone();
        logging.payload_capture.redact_detectors = self.payload_capture_redact_detectors.clone();
        logging.payload_capture.redact_patterns = self.payload_capture_redact_patterns.0.clone();
        logging.payload_capture.models = self.payload_capture_models.clone();
        logging.payload_capture.virtual_key_ids = self.payload_capture_virtual_key_ids.clone();
        logging.ui_events = self.ui_events;
    }

    /// The `[logging]` a snapshot serves when the bootstrap file holds `file`:
    /// the file's own while this row still holds its seeded defaults, this
    /// row's otherwise (see [`crate::merge_logging`]).
    pub fn served(&self, file: &rolter_core::LoggingConfig) -> rolter_core::LoggingConfig {
        let mut stored = rolter_core::LoggingConfig::default();
        self.project_onto(&mut stored);
        crate::merge_logging(file.clone(), stored)
    }
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

/// one external guardrail service; several can be enabled, and the scoped
/// policies pick which of them judge a request (#459)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct GuardrailProvider {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    /// `webhook` or `local` (#3329)
    pub kind: String,
    /// empty for a local provider
    pub url: String,
    /// a local provider's weights file on the gateways' file system
    pub model_path: Option<String>,
    /// a local provider's pinned sha256, 64 lowercase hex digits
    pub model_sha256: Option<String>,
    pub stage: String,
    pub timeout_ms: i32,
    pub max_retries: i32,
    pub failure_mode: String,
    pub max_body_bytes: i32,
    /// total bytes a local provider may score, across overlapping body windows
    pub max_scored_bytes: Option<i32>,
    /// `score` or `fail_closed` for text past `max_scored_bytes`
    pub on_truncated: String,
    pub auth_kind: String,
    pub auth_env: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// one scoped semantic guardrail policy (#459): the providers it consults, in
/// order, and the thresholds their scored categories are held to
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct GuardrailPolicyRow {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    /// `deployment`, `org`, `project`, `route` or `key`
    pub scope_kind: String,
    /// the org / project / key id, or the route's public model name; `None`
    /// for the deployment scope
    pub scope_id: Option<String>,
    /// the providers the policy consults, in order
    pub provider_ids: Vec<Uuid>,
    /// category -> threshold in `[0, 1]`
    pub thresholds: serde_json::Value,
    /// `block` or `annotate`
    pub on_violation: String,
    /// `reject` or `buffer`
    pub streaming: String,
    pub description: Option<String>,
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
    /// self-service profile (#1823): 1..=80 chars, trimmed, no control characters
    #[serde(default)]
    pub display_name: Option<String>,
    /// self-service profile (#1823): up to 500 chars, trimmed
    #[serde(default)]
    pub bio: Option<String>,
    /// delegable deployment role (#1834): `operator`, `security_auditor`, or
    /// none. Read only by the guard on deployment-wide capabilities; it grants
    /// nothing at any org, team or project
    #[serde(default)]
    pub deployment_role: Option<String>,
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
    /// email of the account in `invited_by`, resolved by the list query so a
    /// caller who cannot list that user still sees who sent the invitation
    /// (#2325). `None` when the sender is unknown or was deleted; absent from
    /// the rows the other queries return
    #[sqlx(default)]
    #[serde(default)]
    pub invited_by_email: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// how emailing the link went (#1828): `not_configured` (no mail transport
    /// when it was created), `sent` or `failed`. The link is returned to the
    /// admin whichever it is
    pub mail_status: String,
    pub mail_attempted_at: Option<DateTime<Utc>>,
    /// why a `failed` send failed, in words safe to show an admin
    pub mail_error: Option<String>,
}

/// a one-time password reset an administrator issued for an account (#2240)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PasswordReset {
    pub id: Uuid,
    pub user_id: Uuid,
    /// peppered digest of the one-time token; never serialized
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub created_by: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// how one org's members are allowed to authenticate
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OrgAuthPolicy {
    pub org_id: Uuid,
    pub allow_password_login: bool,
    pub allow_sso: bool,
    /// one of `off`, `optional`, `required_superadmin`, `required_all`
    /// (`migrations/0067_totp_second_factor.sql`). Stored as text rather than
    /// an enum so widening the policy set is a migration, not a code change
    /// every consumer has to be recompiled for
    pub mfa_policy: String,
    /// When a `required_*` policy starts sending unenrolled members through
    /// enrolment before they get a session (#1852). `None` means it already
    /// does; a future value is the grace window an org announces the
    /// requirement with. Always `None` under `off` and `optional`
    pub mfa_enforce_after: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// One org's second-factor policy as it applies to one of its members, read
/// by the login exchange to decide what that member owes (#1852).
#[derive(Debug, Clone, FromRow)]
pub struct MfaPolicyBinding {
    /// `off`, `optional`, `required_superadmin` or `required_all`
    pub policy: String,
    pub org_id: Uuid,
    /// the org's grace window; `None` once the policy is in force
    pub enforce_after: Option<DateTime<Utc>>,
}

/// One org member's second-factor state, for an admin deciding what a
/// `required_*` policy would lock out (#1590). No secret and no hash: only
/// whether a factor is armed and how many recovery codes are left.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct OrgMemberMfa {
    pub user_id: Uuid,
    pub email: String,
    /// the account is a deployment superadmin, which is who `required_superadmin`
    /// binds
    pub is_superadmin: bool,
    /// an armed (confirmed) factor, which is what a `required_*` policy asks
    /// for; a half-finished enrolment is not one
    pub enabled: bool,
    /// unspent recovery codes; always `0` without an armed factor
    pub recovery_codes_remaining: i64,
}

/// What the dashboard and the login exchange may know about a user's second
/// factor. Deliberately not the row: the sealed secret has no representation
/// here at all, so there is no serialisation path that could leak it (#1078).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TotpFactorStatus {
    pub user_id: Uuid,
    /// null while an enrolment is in progress. An unconfirmed factor grants
    /// nothing and is not enforced at login
    pub confirmed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A live second-factor challenge handed out by the login exchange. It
/// authenticates nothing on its own -- it only names which user is halfway
/// through signing in, and what they may do next
/// ([`crate::postgres::repo::ChallengePurpose`]).
#[derive(Debug, Clone, FromRow)]
pub struct MfaChallenge {
    pub id: Uuid,
    pub user_id: Uuid,
    pub attempts: i32,
    pub expires_at: DateTime<Utc>,
    /// whether the password step that minted it followed a lockout, so the
    /// session it is redeemed for can say so on its `auth.login` row
    pub after_lock: bool,
}

/// one subject-scoped erasure of request logs and captured payloads (#1085)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct LogErasure {
    pub id: Uuid,
    /// `virtual_key`, `user`, `org` or `request`
    pub scope_kind: String,
    pub scope_id: String,
    pub org_id: Option<Uuid>,
    pub request_ts: Option<DateTime<Utc>>,
    pub include_logs: bool,
    pub reason: String,
    pub requested_by: Option<Uuid>,
    /// `pending` while clickhouse mutations run, then `completed`; `failed`
    /// when a statement was refused, `unverified` when completion could not be
    /// confirmed from `system.mutations`
    pub status: String,
    pub statements: i32,
    pub error: Option<String>,
    /// which stores the erasure searched and which it skipped, as a list of
    /// `{store, searched, code?, reason?}`; `[]` on a record from before it was kept
    pub stores: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
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

/// Global control-plane security settings. The retired dashboard-password
/// columns (#2356) and `virtual_key_required` (#2357) still exist in the table
/// but are not part of this DTO.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub allowed_origins: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub required_headers: serde_json::Value,
    pub auth_bypass_routes: Vec<String>,
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

    /// every endpoint that returns a key serialises this type, so pinning it
    /// here keeps the digests out of all of them at once (#3525)
    #[test]
    fn a_virtual_key_serialises_without_either_digest() {
        let key = VirtualKey {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            key_hash: "digest-of-the-secret".to_string(),
            key_prefix: "rk_live_ab".to_string(),
            name: Some("ci".to_string()),
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache_enabled: None,
            created_by: None,
            business_unit_id: None,
            customer_id: None,
            purpose: None,
            created_at: DateTime::<Utc>::UNIX_EPOCH,
            previous_key_hash: Some("digest-of-the-old-secret".to_string()),
            previous_key_prefix: Some("rk_live_cd".to_string()),
            previous_expires_at: None,
        };
        let json = serde_json::to_value(&key).expect("a key serialises");
        assert!(json.get("key_hash").is_none(), "{json}");
        assert!(json.get("previous_key_hash").is_none(), "{json}");
        assert_eq!(json["key_prefix"], "rk_live_ab");
        assert_eq!(json["previous_key_prefix"], "rk_live_cd");
        let text = json.to_string();
        assert!(!text.contains("digest-of-the"), "{text}");
    }

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

/// one label attached to a provider, provider group, route or model (#985).
///
/// `subject_id` is text because a model is addressed by name while everything
/// else is addressed by id. `source` is `auto` for a fact rolter established
/// itself — those carry `observed_at` and `observation` and no operator can
/// write one — or `custom` for an operator's own, which rolter stores and
/// displays without interpreting.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Label {
    pub id: Uuid,
    pub subject_type: String,
    pub subject_id: String,
    /// the organization whose model a model label is on (#3476); `None` for a
    /// deployment-wide label, and always for a provider, group or route, which
    /// belong to an organization through their own row
    pub org_id: Option<Uuid>,
    pub key: String,
    pub value: Option<String>,
    pub source: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub observation: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// a named filter preset a user saved on one screen (#1825)
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SavedView {
    pub id: Uuid,
    pub user_id: Uuid,
    /// `llm_logs` or `dashboard`
    pub surface: String,
    pub name: String,
    /// a json object; the control plane allow-lists its keys per surface
    pub filters: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// the breaker state one node reported for one `(model, target)` (#2254). A
/// scoreboard row, replaced by every report from that node rather than appended.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BreakerState {
    pub node_id: String,
    /// the org that owns the route, empty for a deployment-wide route or a
    /// gateway that predates the column (#3458)
    pub org_id: String,
    /// public model name of the route
    pub model: String,
    /// position in the route's target list on that node
    pub target_index: i32,
    pub provider: String,
    pub upstream_model: String,
    /// `closed`, `open` or `half_open`
    pub state: String,
    pub consecutive_failures: i32,
    /// seconds until an open breaker admits its probe
    pub retry_after_secs: Option<i32>,
    pub reported_at: DateTime<Utc>,
}
