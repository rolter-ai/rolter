//! Opt-in alert-rule management, bounded ClickHouse evaluation and webhook
//! delivery of state transitions.
//!
//! A rule reads one signal over a trailing window and is `firing` when the
//! value reaches its threshold (`>=`, or `<=` for a `below` rule), `ok`
//! otherwise. A window with nothing to measure is settled by the rule's
//! `no_data` policy instead. A change between the two is a
//! transition: it is POSTed to the rule's channel when that channel is enabled,
//! and recorded in `alert_notification_history` as `delivered`, `failed` or
//! `skipped` either way.
//!
//! Three things are deliberately not left to trust:
//!
//! - **The endpoint goes through [`rolter_core::EgressPolicy`]** when the
//!   channel is saved and again at every delivery, the same check a connector
//!   endpoint gets, and redirects are not followed. It is stored as the HTTP
//!   client parses it, so the address checked is the address dialled.
//! - **The credential is write-only.** A channel secret is sealed with the
//!   deployment KEK, unsealed only to build the `Authorization` header, and
//!   never returned by the API or placed in an audit entry or history row.
//!   It belongs to the endpoint's origin: repointing a channel at another
//!   origin drops it unless the same request supplies a new one.
//! - **A receiver's response body is never stored.** The history keeps the HTTP
//!   status or the class of transport failure, since an error body can echo the
//!   credential it just rejected.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::{EgressPolicy, Error};
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::repo::AuditLogRepo;

use crate::crud::{invalid_field, pool, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, is_deployment_operator, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::{cap, Requirement};
use crate::ControlState;

/// Longest one webhook delivery may take, connect included. Deliveries run
/// after the evaluation's transaction has committed (see [`Outbox`]), so a slow
/// receiver holds neither the rule's row lock nor a database connection, only
/// one of [`MAX_DELIVERIES`] slots.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
/// Deliveries in flight at once across every pass. A scheduled pass hands its
/// deliveries off and moves on, so this is what bounds the sockets a burst of
/// firing rules can open against a dead receiver.
const MAX_DELIVERIES: usize = 16;
/// How long a `pending` history row is taken to be in flight on some replica.
/// Past it the delivery is presumed lost (the process died between the commit
/// and the send) and the state is delivered again. Well above the longest a
/// queued delivery can wait, [`DELIVERY_TIMEOUT`] times the queue depth over
/// [`MAX_DELIVERIES`] for a pass of dead receivers, so a slow one is not sent twice.
const PENDING_GRACE_SECS: f64 = 300.0;
static DELIVERY_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_DELIVERIES);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a signal query may run. The ClickHouse client bounds each request at
/// 15s itself; this outer bound is a backstop so a pass never hangs on one rule.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Why a rule fired or resolved on a window with nothing to measure.
const NO_DATA_DETAIL: &str = "no data in window";
/// How many of a rule's newest history rows the retry decision reads: enough
/// to count the failed attempts behind the longest [`retry_delay`].
const HISTORY_WINDOW: i64 = 16;

/// Stable codes for `alert_notification_history.detail_code`. Part of the API:
/// a code is never renamed or reused.
mod detail_code {
    pub const HTTP_OK: &str = "http_ok";
    pub const HTTP_ERROR: &str = "http_error";
    pub const TIMEOUT: &str = "timeout";
    pub const CONNECT_FAILED: &str = "connect_failed";
    pub const DELIVERY_FAILED: &str = "delivery_failed";
    pub const EGRESS_DENIED: &str = "egress_denied";
    pub const SECRET_UNSEALABLE: &str = "secret_unsealable";
    pub const SECRET_INVALID: &str = "secret_invalid";
    pub const CLIENT_UNAVAILABLE: &str = "client_unavailable";
    pub const NO_CHANNEL: &str = "no_channel";
    pub const CHANNEL_DISABLED: &str = "channel_disabled";
    pub const DELIVERY_PENDING: &str = "delivery_pending";
}

/// Stable codes for `alert_rules.last_error_code`. Part of the API.
mod error_code {
    pub const CLICKHOUSE_NOT_CONFIGURED: &str = "clickhouse_not_configured";
    pub const UNKNOWN_SIGNAL: &str = "unknown_signal";
    pub const QUERY_TIMEOUT: &str = "query_timeout";
    pub const CLICKHOUSE_TIMEOUT: &str = "clickhouse_timeout";
    pub const CLICKHOUSE_UNREACHABLE: &str = "clickhouse_unreachable";
    pub const CLICKHOUSE_BAD_RESPONSE: &str = "clickhouse_bad_response";
    pub const QUERY_FAILED: &str = "query_failed";
    pub const CLICKHOUSE_ERROR: &str = "clickhouse_error";
    pub const SCOPE_GONE: &str = "scope_gone";
}

/// Why a rule's signal could not be read: a stable code, the numbers it
/// interpolates, and the English text stored beside it.
struct SignalError {
    code: &'static str,
    params: Option<serde_json::Value>,
    message: String,
}

impl SignalError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            params: None,
            message: message.into(),
        }
    }
}

const COMPARISONS: &[&str] = &["above", "below"];
const NO_DATA_POLICIES: &[&str] = &["ignore", "fire", "ok"];

/// Whether an empty window is a missing reading for this signal. An error rate
/// or a percentile of zero requests is undefined, not zero. The others are
/// real zeros: no spend, no requests, no failed health events.
fn can_lack_data(signal: &str) -> bool {
    matches!(signal, "error_rate" | "p95_latency_ms")
}

const SIGNALS: &[&str] = &[
    "error_rate",
    "p95_latency_ms",
    "spend_velocity",
    "request_volume",
    "provider_health_flaps",
];

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/alert-channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/v1/alert-channels/{id}",
            axum::routing::put(update_channel).delete(delete_channel),
        )
        .route("/api/v1/alert-rules", get(list_rules).post(create_rule))
        .route(
            "/api/v1/alert-rules/{id}",
            axum::routing::put(update_rule).delete(delete_rule),
        )
        .route("/api/v1/alert-rules/{id}/evaluate", post(evaluate))
        .route("/api/v1/alert-notifications", get(list_history))
}

/// Start the control-plane evaluator. It is deliberately absent when no DB is
/// configured and does no network work until at least one rule is enabled.
pub(crate) fn start_evaluator(state: ControlState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = evaluate_enabled(&state).await {
                tracing::warn!(error = %error, "alert rule evaluation pass failed");
            }
        }
    });
}

#[derive(Serialize, sqlx::FromRow)]
struct Channel {
    id: Uuid,
    name: String,
    kind: String,
    endpoint: String,
    enabled: bool,
    secret_configured: bool,
    /// The org that owns the channel; null for a deployment channel, which only
    /// a superadmin manages (#1829)
    org_id: Option<Uuid>,
    /// Whether the caller may edit or delete it. A deployment channel is listed
    /// to an org admin so a rule can name it, with its endpoint withheld, since
    /// the address may carry a token
    #[sqlx(skip)]
    manageable: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Rule {
    id: Uuid,
    name: String,
    /// The org the rule's scope sits in; null for a deployment-wide rule
    org_id: Option<Uuid>,
    /// What the rule watches (`org`, `team`, `project`, `virtual_key`,
    /// `business_unit` or `customer`); null for the whole deployment. Fixed when
    /// the rule is created
    scope_type: Option<String>,
    scope_id: Option<Uuid>,
    signal: String,
    threshold: f64,
    comparison: String,
    no_data: String,
    window_secs: i32,
    channel_id: Option<Uuid>,
    enabled: bool,
    state: String,
    last_value: Option<f64>,
    last_evaluated_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    /// Stable code of `last_error` (see [`error_code`]); `last_error` is its
    /// English fallback. Null when the last evaluation succeeded
    last_error_code: Option<String>,
    /// The numbers `last_error_code` interpolates, such as the timeout
    last_error_params: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Notification {
    id: Uuid,
    rule_id: Uuid,
    channel_id: Option<Uuid>,
    state: String,
    delivery_status: String,
    detail: Option<String>,
    /// Stable code of `detail` (see [`detail_code`]), which is its English
    /// fallback. Null on rows written before codes existed
    detail_code: Option<String>,
    /// The numbers `detail_code` interpolates, such as the HTTP status
    detail_params: Option<serde_json::Value>,
    /// The reading the transition was written for, in the signal's API unit.
    /// Null for a window with no data and on older rows
    value: Option<f64>,
    /// The rule's threshold when the row was written
    threshold: Option<f64>,
    sent_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ChannelInput {
    name: String,
    endpoint: String,
    /// Owning org, on create only; omitted for a deployment channel. An update
    /// leaves the owner as it is
    #[serde(default)]
    org_id: Option<Uuid>,
    #[serde(default)]
    enabled: bool,
    managed_secret: Option<String>,
}

#[derive(Clone, Deserialize)]
struct RuleInput {
    name: String,
    /// The scope to watch, on create only: both or neither. An update leaves
    /// the scope as it is
    #[serde(default)]
    scope_type: Option<String>,
    #[serde(default)]
    scope_id: Option<Uuid>,
    signal: String,
    threshold: f64,
    /// `above` (the default) fires at `value >= threshold`, `below` at
    /// `value <= threshold`. Omitted on an update, the stored value is kept,
    /// so an editor that predates the field cannot reset it
    comparison: Option<String>,
    /// What a window with no data does: `ignore` (the default), `fire` or
    /// `ok`. Omitted on an update, the stored value is kept
    no_data: Option<String>,
    window_secs: i32,
    channel_id: Option<Uuid>,
    #[serde(default)]
    enabled: bool,
}

fn default_comparison() -> String {
    "above".into()
}

fn default_no_data() -> String {
    "ignore".into()
}

impl RuleInput {
    /// The comparison a create stores: the body's, or the default
    fn comparison(&self) -> String {
        self.comparison.clone().unwrap_or_else(default_comparison)
    }

    /// The no-data policy a create stores: the body's, or the default
    fn no_data(&self) -> String {
        self.no_data.clone().unwrap_or_else(default_no_data)
    }

    /// The no-data policy an update writes, `None` to keep the stored one.
    /// A rule moved to a signal that cannot lack data is written `ignore`,
    /// since a kept `fire` or `ok` would be a policy that never applies
    fn no_data_for_update(&self) -> Option<String> {
        match &self.no_data {
            Some(policy) => Some(policy.clone()),
            None if !can_lack_data(&self.signal) => Some(default_no_data()),
            None => None,
        }
    }
}

/// Names are stored trimmed, so the limit counts the characters of the
/// trimmed value, the unit the error text names.
fn validate_name(value: &str, label: &str) -> ApiResult<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 128 || trimmed.chars().any(char::is_control)
    {
        return Err(invalid_field(
            "name",
            format!("{label} must be 1-128 visible characters"),
        ));
    }
    Ok(())
}

/// Check a channel body and return its endpoint as the HTTP client parses it,
/// which is the form it is stored in.
///
/// Storing the parsed form rather than what was typed means the delivery-time
/// egress check and the request both read one spelling of the host.
fn validate_channel(input: &ChannelInput) -> ApiResult<reqwest::Url> {
    validate_name(&input.name, "channel name")?;
    let endpoint = reqwest::Url::parse(input.endpoint.trim())
        .map_err(|_| invalid_field("endpoint", "channel endpoint must be a valid http(s) URL"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(invalid_field(
            "endpoint",
            "channel endpoint must be an http(s) URL without userinfo",
        ));
    }
    if let Some(secret) = &input.managed_secret {
        if secret.trim().is_empty() {
            return Err(invalid_field(
                "managed_secret",
                "managed_secret must not be empty",
            ));
        }
        // checked here rather than found at the first delivery, where a
        // trailing newline from a file would fail every alert with no hint
        if bearer(secret).is_none() {
            return Err(invalid_field("managed_secret", "managed_secret must be a valid HTTP header value, without control characters or line breaks",
            ));
        }
    }
    Ok(endpoint)
}

/// The same guard a connector endpoint gets: an operator-supplied URL the
/// control plane will POST to is an SSRF surface.
fn check_egress(egress: &EgressPolicy, endpoint: &reqwest::Url) -> ApiResult<()> {
    egress
        .check_url(endpoint.as_str(), "channel endpoint")
        .map_err(|message| invalid_field("endpoint", message))
}

/// The `Authorization` value a channel secret is sent as, or `None` when the
/// secret cannot be one.
fn bearer(secret: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_str(&format!("Bearer {secret}")).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// The `request_logs` column a scope type is matched on. The health stream is
/// keyed by the provider's org alone, so only an org scope can watch it.
fn scope_column(scope_type: &str) -> Option<&'static str> {
    Some(match scope_type {
        "org" => "org_id",
        "team" => "team_id",
        "project" => "project_id",
        "virtual_key" => "virtual_key_id",
        "business_unit" => "business_unit_id",
        "customer" => "customer_id",
        _ => return None,
    })
}

fn check_signal_scope(signal: &str, scope_type: Option<&str>) -> ApiResult<()> {
    match scope_type {
        Some(scope) if scope_column(scope).is_none() => Err(invalid_field(
            "scope_type",
            "scope_type must be one of org, team, project, virtual_key, business_unit, customer",
        )),
        Some(scope) if signal == "provider_health_flaps" && scope != "org" => Err(invalid_field(
            "signal",
            "provider_health_flaps can only watch a whole org or the deployment: provider health is not recorded per team, project, key, business unit or customer",
        )),
        _ => Ok(()),
    }
}

fn validate_rule(input: &RuleInput) -> ApiResult<()> {
    validate_name(&input.name, "rule name")?;
    if input.scope_type.is_some() != input.scope_id.is_some() {
        return Err(invalid_field(
            "scope_id",
            "scope_type and scope_id are given together or not at all",
        ));
    }
    check_signal_scope(&input.signal, input.scope_type.as_deref())?;
    if !SIGNALS.contains(&input.signal.as_str()) {
        return Err(invalid_field(
            "signal",
            format!("signal must be one of {SIGNALS:?}"),
        ));
    }
    if !input.threshold.is_finite() || input.threshold < 0.0 {
        return Err(invalid_field(
            "threshold",
            "threshold must be a finite non-negative number",
        ));
    }
    if !(60..=86_400).contains(&input.window_secs) {
        return Err(invalid_field(
            "window_secs",
            "window_secs must be between 60 and 86400",
        ));
    }
    if !COMPARISONS.contains(&input.comparison().as_str()) {
        return Err(invalid_field(
            "comparison",
            format!("comparison must be one of {COMPARISONS:?}"),
        ));
    }
    if !NO_DATA_POLICIES.contains(&input.no_data().as_str()) {
        return Err(invalid_field(
            "no_data",
            format!("no_data must be one of {NO_DATA_POLICIES:?}"),
        ));
    }
    // rejected rather than stored inert: a policy that can never apply reads
    // as protection the rule does not have
    if input.no_data() != "ignore" && !can_lack_data(&input.signal) {
        return Err(invalid_field("no_data", "no_data applies only to error_rate and p95_latency_ms; the other signals read 0 for an empty window (use comparison below on request_volume to catch stopped traffic)",
        ));
    }
    Ok(())
}

fn seal(secret: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    let Some(kek) = Kek::from_env() else {
        return Err(ApiError::KekUnavailable(format!(
            "storing channel credentials requires {KEK_ENV}"
        )));
    };
    kek.encrypt(secret)
        .map_err(|_| ApiError::Curated("failed to encrypt channel credential".into()))
}

fn channel_columns() -> &'static str {
    "id, name, kind, endpoint, enabled, secret_ciphertext is not null as secret_configured, org_id, created_at, updated_at"
}

fn rule_columns() -> &'static str {
    "id, name, org_id, scope_type, scope_id, signal, threshold, comparison, no_data, window_secs, channel_id, enabled, state, last_value, last_evaluated_at, last_error, last_error_code, last_error_params, created_at, updated_at"
}

/// A deployment-wide channel or rule names no org, so no membership can
/// satisfy it: only a superadmin or a deployment operator may touch it. An API
/// token holds no deployment authority and is refused as a member is.
fn deployment_only(principal: &Principal) -> ApiResult<()> {
    if is_deployment_operator(principal) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Guard a channel at its owning org, or at the deployment when it has none.
async fn authorize_channel(
    state: &ControlState,
    principal: &Principal,
    org_id: Option<Uuid>,
    requirement: Requirement,
) -> ApiResult<()> {
    match org_id {
        Some(org) => authorize(state, principal, ScopeChain::org(org), requirement).await,
        None => deployment_only(principal),
    }
}

/// The scope chain a stored rule is guarded at: its own scope, or its org when
/// the scope row is gone (deleted team, project, key...) so the org's admins
/// can still see and remove the leftover rule. `None` for a deployment rule.
async fn rule_chain(state: &ControlState, rule: &Rule) -> Option<ScopeChain> {
    let org = rule.org_id?;
    if let (Some(scope_type), Some(scope_id)) = (&rule.scope_type, rule.scope_id) {
        if let Ok(chain) = ScopeChain::from_scope(pool(state), scope_type, scope_id).await {
            if chain.org == Some(org) {
                return Some(chain);
            }
        }
    }
    Some(ScopeChain::org(org))
}

async fn authorize_rule(
    state: &ControlState,
    principal: &Principal,
    rule: &Rule,
    requirement: Requirement,
) -> ApiResult<()> {
    match rule_chain(state, rule).await {
        Some(chain) => authorize(state, principal, chain, requirement).await,
        None => deployment_only(principal),
    }
}

async fn load_rule(state: &ControlState, id: Uuid) -> ApiResult<Rule> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {} from alert_rules where id=$1",
        rule_columns()
    )))
    .bind(id)
    .fetch_optional(pool(state))
    .await
    .map_err(rolter_store::postgres::store_err)?
    .ok_or_else(|| ApiError::Core(Error::NotFound(format!("alert rule {id}"))))
}

/// The owner of a channel: `Some(None)` for a deployment channel.
async fn channel_owner(state: &ControlState, id: Uuid) -> ApiResult<Option<Uuid>> {
    sqlx::query_scalar::<_, Option<Uuid>>("select org_id from alert_channels where id=$1")
        .bind(id)
        .fetch_optional(pool(state))
        .await
        .map_err(rolter_store::postgres::store_err)?
        .ok_or_else(|| ApiError::Core(Error::NotFound(format!("alert channel {id}"))))
}

/// Whether a rule owned by `rule_org` may notify a channel owned by
/// `channel_org`: its own org's, or a deployment channel. A deployment-wide
/// rule reads the whole deployment, so it may only notify a deployment channel,
/// never one org's.
fn channel_reaches(channel_org: Option<Uuid>, rule_org: Option<Uuid>) -> bool {
    channel_org.is_none() || channel_org == rule_org
}

/// Refuse a rule that names a channel it may not notify. An unknown channel and
/// another org's are answered alike, so the id cannot be used to probe.
async fn check_rule_channel(
    state: &ControlState,
    channel_id: Option<Uuid>,
    rule_org: Option<Uuid>,
) -> ApiResult<()> {
    let Some(id) = channel_id else { return Ok(()) };
    let reachable = match channel_owner(state, id).await {
        Ok(owner) => channel_reaches(owner, rule_org),
        Err(ApiError::Core(Error::NotFound(_))) => false,
        Err(error) => return Err(error),
    };
    if reachable {
        Ok(())
    } else {
        Err(invalid_field(
            "channel_id",
            "channel_id must name a channel of the rule's own organization or a deployment channel",
        ))
    }
}

/// Where the caller may run alerting: the chain of every place they hold an
/// alert-rule admin role, so the orgs, teams and projects their rules live in.
/// Empty for a caller with none, who is refused the listing (a superadmin never
/// asks).
async fn alerting_reach(state: &ControlState, filter: &ScopeFilter) -> ApiResult<Vec<ScopeChain>> {
    let held: Vec<ScopeChain> = filter
        .reach(pool(state))
        .await?
        .into_iter()
        .filter(|held| filter.allows(*held))
        .collect();
    if held.iter().all(|held| held.org.is_none()) {
        return Err(ApiError::Forbidden);
    }
    Ok(held)
}

/// The orgs in which the caller may run alerting: those of every place they
/// hold an alert-rule admin role.
async fn alerting_orgs(
    state: &ControlState,
    filter: &ScopeFilter,
) -> ApiResult<std::collections::HashSet<Uuid>> {
    Ok(alerting_reach(state, filter)
        .await?
        .into_iter()
        .filter_map(|held| held.org)
        .collect())
}

/// The rules whose scope lies inside one of the places in `reach` (#3361): the
/// rules of an org, of a team (or of a project or key inside it) and of a
/// project (or a key inside it), chosen in SQL so a rule of any other org is
/// never read. A superset of what the caller may see, never a verdict.
async fn rules_in_reach(state: &ControlState, reach: &[ScopeChain]) -> ApiResult<Vec<Rule>> {
    let (mut orgs, mut teams, mut projects) = (Vec::new(), Vec::new(), Vec::new());
    for held in reach {
        match (held.org, held.team, held.project) {
            (_, _, Some(project)) => projects.push(project),
            (_, Some(team), None) => teams.push(team),
            (Some(org), None, None) => orgs.push(org),
            (None, None, None) => {}
        }
    }
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {} from alert_rules where id in ( \
             select r.id from alert_rules r \
             left join projects p on r.scope_type = 'project' and p.id = r.scope_id \
             left join virtual_keys k on r.scope_type = 'virtual_key' and k.id = r.scope_id \
             left join projects kp on kp.id = k.project_id \
             where r.org_id = any($1) \
                or (r.scope_type = 'team' and r.scope_id = any($2)) \
                or coalesce(p.team_id, kp.team_id) = any($2) \
                or coalesce(p.id, kp.id) = any($3)) \
         order by name",
        rule_columns()
    )))
    .bind(orgs)
    .bind(teams)
    .bind(projects)
    .fetch_all(pool(state))
    .await
    .map_err(rolter_store::postgres::store_err)
    .map_err(Into::into)
}

/// The rules the caller may see: all of them for a superadmin or a deployment
/// operator, otherwise those whose scope the caller is admin of.
/// Deployment-wide rules are never theirs.
///
/// A scoped caller's rules are chosen in SQL from their reach (#3361): the
/// rules of the orgs they hold the role at, of the teams, and of the projects,
/// reading a team or a project (or a key's project) scope through its row. So
/// what is read, and how many chains are then resolved, follows the caller's
/// own rules and not the number of rules in other orgs. The reach is a
/// superset on purpose: the verdict on each rule is still [`ScopeFilter`]'s, so
/// a more specific lower role, which beats an org admin at that scope, still
/// hides the rule.
async fn visible_rules(
    state: &ControlState,
    principal: &Principal,
    requirement: Requirement,
) -> ApiResult<Vec<Rule>> {
    let filter = ScopeFilter::load(state, principal, requirement).await?;
    if is_deployment_operator(principal) {
        return sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {} from alert_rules order by name",
            rule_columns()
        )))
        .fetch_all(pool(state))
        .await
        .map_err(rolter_store::postgres::store_err)
        .map_err(Into::into);
    }
    let reach = alerting_reach(state, &filter).await?;
    let rules = rules_in_reach(state, &reach).await?;
    let mut visible = Vec::new();
    for rule in rules {
        if let Some(chain) = rule_chain(state, &rule).await {
            if filter.allows(chain) {
                visible.push(rule);
            }
        }
    }
    Ok(visible)
}

async fn list_channels(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Channel>>> {
    let owned = ScopeFilter::load(&state, &principal, cap!("alert_channel", Read)).await?;
    // who may see a channel is decided by running alerting in its org, which is
    // the rule capability: a team admin names their org's channel on a rule
    // without being able to edit it
    let rules = ScopeFilter::load(&state, &principal, cap!("alert_rule", Read)).await?;
    let orgs = if is_deployment_operator(&principal) {
        None
    } else {
        Some(alerting_orgs(&state, &rules).await?)
    };
    let mut channels: Vec<Channel> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {} from alert_channels order by name",
        channel_columns()
    )))
    .fetch_all(pool(&state))
    .await
    .map_err(rolter_store::postgres::store_err)?;
    if let Some(orgs) = &orgs {
        channels.retain(|c| c.org_id.is_none_or(|org| orgs.contains(&org)));
    }
    for channel in &mut channels {
        channel.manageable = match channel.org_id {
            Some(org) => is_deployment_operator(&principal) || owned.allows(ScopeChain::org(org)),
            None => is_deployment_operator(&principal),
        };
        if !channel.manageable {
            channel.endpoint.clear();
        }
    }
    Ok(Json(channels))
}

async fn create_channel(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(input): SafeJson<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    authorize_channel(
        &state,
        &principal,
        input.org_id,
        cap!("alert_channel", Create),
    )
    .await?;
    let endpoint = validate_channel(&input)?;
    check_egress(&state.egress, &endpoint)?;
    let secret = input.managed_secret.as_deref().map(seal).transpose()?;
    let mut channel: Channel = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into alert_channels (name, kind, endpoint, enabled, secret_ciphertext, secret_nonce, org_id) values ($1, 'webhook', $2, $3, $4, $5, $6) returning {}", channel_columns())))
        .bind(input.name.trim()).bind(endpoint.as_str()).bind(input.enabled)
        .bind(secret.as_ref().map(|(c, _)| c.as_slice())).bind(secret.as_ref().map(|(_, n)| n.as_slice()))
        .bind(input.org_id)
        .fetch_one(pool(&state)).await.map_err(rolter_store::postgres::store_err)?;
    channel.manageable = true;
    audit(
        &state,
        &principal,
        channel.org_id,
        "alert.channel.create",
        channel.id,
        serde_json::json!({"name": channel.name, "enabled": channel.enabled, "org_id": channel.org_id}),
    )
    .await;
    Ok(Json(channel))
}

async fn update_channel(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(input): SafeJson<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    let owner = channel_owner(&state, id).await?;
    authorize_channel(&state, &principal, owner, cap!("alert_channel", Update)).await?;
    let endpoint = validate_channel(&input)?;
    let secret = input.managed_secret.as_deref().map(seal).transpose()?;
    let mut tx = pool(&state)
        .begin()
        .await
        .map_err(rolter_store::postgres::store_err)?;
    // no key update rather than for update: an evaluation delivering to this
    // channel holds it for key share, and an edit need not wait for that
    let (stored, had_secret): (String, bool) = sqlx::query_as(
        "select endpoint, secret_ciphertext is not null from alert_channels where id=$1 for no key update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(rolter_store::postgres::store_err)?
    .ok_or_else(|| Error::NotFound(format!("alert channel {id}")))?;
    let stored = reqwest::Url::parse(&stored).ok();
    let endpoint_changed = stored.as_ref() != Some(&endpoint);
    // a disabled channel sends nothing, so switching off one whose unchanged
    // endpoint a since-tightened policy denies must not be refused by it
    if input.enabled || endpoint_changed {
        check_egress(&state.egress, &endpoint)?;
    }
    // a secret was given for one receiver; repointed at another origin, the
    // channel would hand it to whoever answers there, so it is dropped unless
    // this request brings the new receiver's own
    let origin_changed = stored.is_none_or(|stored| stored.origin() != endpoint.origin());
    let clear_secret = origin_changed && secret.is_none();
    let mut channel: Channel = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update alert_channels set name=$2, endpoint=$3, enabled=$4, \
         secret_ciphertext = case when $7 then null else coalesce($5, secret_ciphertext) end, \
         secret_nonce = case when $7 then null else coalesce($6, secret_nonce) end, \
         updated_at=now() where id=$1 returning {}",
        channel_columns()
    )))
    .bind(id)
    .bind(input.name.trim())
    .bind(endpoint.as_str())
    .bind(input.enabled)
    .bind(secret.as_ref().map(|(c, _)| c.as_slice()))
    .bind(secret.as_ref().map(|(_, n)| n.as_slice()))
    .bind(clear_secret)
    .fetch_one(&mut *tx)
    .await
    .map_err(rolter_store::postgres::store_err)?;
    tx.commit()
        .await
        .map_err(rolter_store::postgres::store_err)?;
    channel.manageable = true;
    // whether the endpoint moved and what became of the secret, never the
    // endpoint itself: its query string can carry a token
    audit(
        &state,
        &principal,
        owner,
        "alert.channel.update",
        id,
        serde_json::json!({
            "name": channel.name,
            "enabled": channel.enabled,
            "endpoint_changed": endpoint_changed,
            "secret_replaced": secret.is_some(),
            "secret_cleared": clear_secret && had_secret,
        }),
    )
    .await;
    Ok(Json(channel))
}

async fn delete_channel(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let owner = channel_owner(&state, id).await?;
    authorize_channel(&state, &principal, owner, cap!("alert_channel", Delete)).await?;
    if sqlx::query("delete from alert_channels where id=$1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(rolter_store::postgres::store_err)?
        .rows_affected()
        == 0
    {
        return Err(ApiError::Core(Error::NotFound(format!(
            "alert channel {id}"
        ))));
    }
    audit(
        &state,
        &principal,
        owner,
        "alert.channel.delete",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_rules(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Rule>>> {
    Ok(Json(
        visible_rules(&state, &principal, cap!("alert_rule", Read)).await?,
    ))
}

async fn create_rule(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(input): SafeJson<RuleInput>,
) -> ApiResult<Json<Rule>> {
    validate_rule(&input)?;
    // creating takes admin at the scope the rule watches; a deployment-wide
    // rule watches every tenant's traffic and stays a superadmin's
    let (org_id, scope) = match (input.scope_type.as_deref(), input.scope_id) {
        (Some(scope_type), Some(scope_id)) => {
            let chain = ScopeChain::from_scope(pool(&state), scope_type, scope_id).await?;
            authorize(&state, &principal, chain, cap!("alert_rule", Create)).await?;
            (chain.org, Some((scope_type, scope_id)))
        }
        _ => {
            deployment_only(&principal)?;
            (None, None)
        }
    };
    check_rule_channel(&state, input.channel_id, org_id).await?;
    let rule: Rule = sqlx::query_as(sqlx::AssertSqlSafe(format!("insert into alert_rules (name, signal, threshold, comparison, no_data, window_secs, channel_id, enabled, org_id, scope_type, scope_id) values ($1,$2,$3,$7,$8,$4,$5,$6,$9,$10,$11) returning {}", rule_columns())))
        .bind(input.name.trim()).bind(&input.signal).bind(input.threshold).bind(input.window_secs).bind(input.channel_id).bind(input.enabled).bind(input.comparison()).bind(input.no_data())
        .bind(org_id).bind(scope.map(|(t, _)| t)).bind(scope.map(|(_, id)| id))
        .fetch_one(pool(&state)).await.map_err(rolter_store::postgres::store_err)?;
    audit(
        &state,
        &principal,
        rule.org_id,
        "alert.rule.create",
        rule.id,
        serde_json::json!({"name": rule.name, "signal": rule.signal, "enabled": rule.enabled, "scope_type": rule.scope_type, "scope_id": rule.scope_id}),
    )
    .await;
    Ok(Json(rule))
}

async fn update_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(input): SafeJson<RuleInput>,
) -> ApiResult<Json<Rule>> {
    let stored = load_rule(&state, id).await?;
    authorize_rule(&state, &principal, &stored, cap!("alert_rule", Update)).await?;
    // the scope is fixed at creation, so what the body says about it is not
    // read; the stored scope is what the signal is checked against
    let mut checked = input.clone();
    checked.scope_type = None;
    checked.scope_id = None;
    validate_rule(&checked)?;
    check_signal_scope(&input.signal, stored.scope_type.as_deref())?;
    check_rule_channel(&state, input.channel_id, stored.org_id).await?;
    let rule: Rule = sqlx::query_as(sqlx::AssertSqlSafe(format!("update alert_rules set name=$2, signal=$3, threshold=$4, window_secs=$5, channel_id=$6, enabled=$7, comparison=coalesce($8, comparison), no_data=coalesce($9, no_data), updated_at=now() where id=$1 returning {}", rule_columns())))
        .bind(id).bind(input.name.trim()).bind(&input.signal).bind(input.threshold).bind(input.window_secs).bind(input.channel_id).bind(input.enabled).bind(&input.comparison).bind(input.no_data_for_update())
        .fetch_optional(pool(&state)).await.map_err(rolter_store::postgres::store_err)?
        .ok_or_else(|| Error::NotFound(format!("alert rule {id}")))?;
    audit(
        &state,
        &principal,
        stored.org_id,
        "alert.rule.update",
        id,
        serde_json::json!({"name": rule.name, "signal": rule.signal, "enabled": rule.enabled}),
    )
    .await;
    Ok(Json(rule))
}

async fn delete_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let stored = load_rule(&state, id).await?;
    authorize_rule(&state, &principal, &stored, cap!("alert_rule", Delete)).await?;
    if sqlx::query("delete from alert_rules where id=$1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(rolter_store::postgres::store_err)?
        .rows_affected()
        == 0
    {
        return Err(ApiError::Core(Error::NotFound(format!("alert rule {id}"))));
    }
    audit(
        &state,
        &principal,
        stored.org_id,
        "alert.rule.delete",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<i64>,
    rule_id: Option<Uuid>,
    /// `firing` or `resolved`
    state: Option<String>,
    /// `delivered`, `failed` or `skipped`
    delivery_status: Option<String>,
    /// Keyset cursor `<sent_at>,<id>` of the last row of the previous page;
    /// the page returned is the rows strictly older than it
    before: Option<String>,
}

const HISTORY_STATES: &[&str] = &["firing", "resolved"];
const DELIVERY_STATUSES: &[&str] = &["pending", "delivered", "failed", "skipped"];

fn bad_param(field: &str, message: String) -> ApiError {
    ApiError::InvalidField {
        field: field.into(),
        message,
    }
}

/// A filter value, checked against the values the column can hold.
fn one_of(field: &str, value: Option<String>, allowed: &[&str]) -> ApiResult<Option<String>> {
    match value {
        Some(value) if !allowed.contains(&value.as_str()) => Err(bad_param(
            field,
            format!("{field} must be one of: {}", allowed.join(", ")),
        )),
        other => Ok(other),
    }
}

/// Parse `before=<sent_at>,<id>`. `sent_at` alone can tie, so the id breaks it.
fn parse_cursor(raw: &str) -> ApiResult<(DateTime<Utc>, Uuid)> {
    raw.split_once(',')
        .and_then(|(at, id)| {
            Some((
                DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc),
                Uuid::parse_str(id).ok()?,
            ))
        })
        .ok_or_else(|| {
            bad_param(
                "before",
                "before must be `<sent_at>,<id>` of the last row of the previous page".into(),
            )
        })
}

async fn list_history(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Notification>>> {
    // history is read through the rules the caller may see, so a scoped admin
    // reads only what their own rules wrote
    let rules = visible_rules(&state, &principal, cap!("alert_history", Read)).await?;
    let rule_ids: Option<Vec<Uuid>> =
        (!is_deployment_operator(&principal)).then(|| rules.iter().map(|r| r.id).collect());
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let status = one_of("state", query.state, HISTORY_STATES)?;
    let delivery = one_of("delivery_status", query.delivery_status, DELIVERY_STATUSES)?;
    let cursor = query.before.as_deref().map(parse_cursor).transpose()?;
    let history = sqlx::query_as(
        "select id, rule_id, channel_id, state, delivery_status, detail, detail_code, detail_params, value, threshold, sent_at \
         from alert_notification_history \
         where ($1::uuid is null or rule_id=$1) \
           and ($7::uuid[] is null or rule_id = any($7)) \
           and ($2::text is null or state=$2) \
           and ($3::text is null or delivery_status=$3) \
           and ($4::timestamptz is null or (sent_at, id) < ($4, $5)) \
         order by sent_at desc, id desc \
         limit $6",
    )
    .bind(query.rule_id)
    .bind(status)
    .bind(delivery)
    .bind(cursor.map(|(at, _)| at))
    .bind(cursor.map(|(_, id)| id))
    .bind(limit)
    .bind(rule_ids)
    .fetch_all(pool(&state))
    .await
    .map_err(rolter_store::postgres::store_err)?;
    Ok(Json(history))
}

#[derive(Serialize)]
struct Evaluation {
    rule: Rule,
    /// Whether this evaluation delivered a transition to a channel. False for a
    /// transition recorded as `failed` or `skipped`.
    notified: bool,
    /// The history row this evaluation wrote, when the reading was a transition.
    notification: Option<Notification>,
}

async fn evaluate(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Evaluation>> {
    let stored = load_rule(&state, id).await?;
    authorize_rule(&state, &principal, &stored, cap!("alert_history", Create)).await?;
    evaluate_one(&state, id, Lock::Wait, Kek::from_env().as_ref())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::Core(Error::NotFound(format!("alert rule {id}"))))
}

async fn evaluate_enabled(state: &ControlState) -> Result<(), Error> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("select id from alert_rules where enabled order by name")
            .fetch_all(pool(state))
            .await
            .map_err(rolter_store::postgres::store_err)?;
    let kek = Kek::from_env();
    for id in ids {
        match evaluate_rule(state, id, Lock::SkipLocked, kek.as_ref()).await {
            // the send is handed off: the pass goes on to the next rule while
            // a slow or dead receiver runs out its timeout in the background
            Ok(Some((_, Some(outbox)))) => hand_off(state, outbox),
            Ok(_) => {}
            Err(error) => tracing::warn!(%id, error = ?error, "alert rule evaluation failed"),
        }
    }
    Ok(())
}

/// Send `outbox` in the background, within the [`MAX_DELIVERIES`] bound. The
/// slot is waited for inside the task so a burst never holds up the caller.
fn hand_off(state: &ControlState, outbox: Outbox) {
    let pool = pool(state).clone();
    let egress = Arc::clone(&state.egress);
    tokio::spawn(async move {
        let Ok(_slot) = DELIVERY_SLOTS.acquire().await else {
            return;
        };
        let rule = outbox.rule_id;
        if let Err(error) = settle(&pool, &egress, outbox).await {
            // the row stays `pending` and is delivered again once it is older
            // than PENDING_GRACE_SECS
            tracing::warn!(%rule, error = ?error, "alert delivery outcome was not recorded");
        }
    });
}

/// How [`evaluate_one`] takes the rule's row lock.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lock {
    /// Wait for it. **Evaluate now** runs after whichever pass holds the rule,
    /// evaluates it whether or not it is enabled, and retries a state that was
    /// not delivered without waiting out [`retry_delay`].
    Wait,
    /// Leave a rule another pass holds to that pass, and skip one that is no
    /// longer enabled. The scheduled evaluator runs this way.
    SkipLocked,
}

/// A rule as read before its signal query, with the database clock at that
/// moment.
#[derive(sqlx::FromRow)]
struct Unlocked {
    #[sqlx(flatten)]
    rule: Rule,
    read_at: DateTime<Utc>,
}

/// One history row, as the transition decision reads it.
#[derive(sqlx::FromRow)]
struct Reported {
    state: String,
    delivery_status: String,
    sent_at: DateTime<Utc>,
    /// seconds since `sent_at` by the database clock, so a replica whose own
    /// clock is off cannot misjudge whether a `pending` row is still in flight
    age_secs: f64,
}

/// Evaluate one rule, then deliver and record whatever changed, returning once
/// the delivery has settled. **Evaluate now** runs this way, since the operator
/// is waiting for the outcome; the scheduled pass uses [`evaluate_rule`] and
/// hands the delivery off.
async fn evaluate_one(
    state: &ControlState,
    id: Uuid,
    lock: Lock,
    kek: Option<&Kek>,
) -> ApiResult<Option<Evaluation>> {
    let Some((mut evaluation, outbox)) = evaluate_rule(state, id, lock, kek).await? else {
        return Ok(None);
    };
    if let Some(outbox) = outbox {
        let settled = settle(pool(state), &state.egress, outbox).await?;
        evaluation.notified = settled.delivery_status == "delivered";
        evaluation.notification = Some(settled);
    }
    Ok(Some(evaluation))
}

/// Evaluate one rule and record what changed, without sending anything.
///
/// `None` when the rule is gone, or, under [`Lock::SkipLocked`], when it is
/// disabled or another evaluation holds it. Otherwise the evaluation and, for a
/// transition owed to an enabled channel, the [`Outbox`] to send afterwards.
///
/// The ClickHouse read happens before any lock is taken, because it is the slow
/// part. Everything after it runs in one short transaction holding the rule's
/// row lock: deciding whether the reading is a transition, writing the rule and
/// writing its history row, `pending` when a delivery is owed. Every
/// control-plane replica runs an evaluator, and the lock is what stops two of
/// them that read the same state from reporting the same change twice: the
/// second finds the first's `pending` row. The send itself happens after the
/// commit, so a slow receiver holds neither the lock nor a connection, and a
/// database that closes idle transactions cannot kill a commit mid-delivery and
/// have the alert sent again. A reading older than the one the rule already
/// records is dropped, so a slow query cannot undo a newer result.
async fn evaluate_rule(
    state: &ControlState,
    id: Uuid,
    lock: Lock,
    kek: Option<&Kek>,
) -> ApiResult<Option<(Evaluation, Option<Outbox>)>> {
    let filter = if lock == Lock::SkipLocked {
        " and enabled"
    } else {
        ""
    };
    let Some(Unlocked {
        rule: unlocked,
        read_at,
    }) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {}, clock_timestamp() as read_at from alert_rules where id=$1{filter}",
        rule_columns()
    )))
    .bind(id)
    .fetch_optional(pool(state))
    .await
    .map_err(rolter_store::postgres::store_err)?
    else {
        return Ok(None);
    };
    let reading = read_signal(state, &unlocked).await;

    let mut tx = pool(state)
        .begin()
        .await
        .map_err(rolter_store::postgres::store_err)?;
    // the channel is locked before the rule: deleting a channel locks it and
    // then, through on delete set null, every rule naming it, so taking the
    // two in that same order makes a delete wait for an in-flight delivery
    // instead of deadlocking with it. key share still lets the channel be
    // edited meanwhile
    let channel: Option<ChannelTarget> = match unlocked.channel_id {
        Some(channel_id) => sqlx::query_as(
            "select id, endpoint, enabled, secret_ciphertext, secret_nonce, org_id from alert_channels where id=$1 for key share",
        )
        .bind(channel_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(rolter_store::postgres::store_err)?,
        None => None,
    };
    let locking = match lock {
        Lock::Wait => "for update",
        Lock::SkipLocked => "for update skip locked",
    };
    let Some(rule): Option<Rule> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {} from alert_rules where id=$1{filter} {locking}",
        rule_columns()
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(rolter_store::postgres::store_err)?
    else {
        return Ok(None);
    };
    // an edit landed between the read and the lock, so the reading measures a
    // query the rule no longer asks for, or the channel locked above is no
    // longer the rule's; the next pass starts from the edited rule. a channel
    // deleted meanwhile has already cleared the rule's channel_id, which
    // matches the missing channel and is evaluated as no channel
    let edited = rule.signal != unlocked.signal
        || rule.window_secs != unlocked.window_secs
        || rule.scope_id != unlocked.scope_id
        || rule.channel_id != channel.as_ref().map(|c| c.id);
    // another evaluation started after this one and has already recorded its
    // reading, which this older one must not overwrite
    let superseded = rule.last_evaluated_at.is_some_and(|at| at > read_at);
    if edited || superseded {
        return Ok(Some((
            Evaluation {
                rule,
                notified: false,
                notification: None,
            },
            None,
        )));
    }

    // the API only lets a rule name a channel of its own org or a deployment
    // one, and this holds the line if a row got past it: a channel of another
    // org is treated as no channel, so a scoped rule's numbers never reach it
    let channel = channel.filter(|channel| channel_reaches(channel.org_id, rule.org_id));
    let value = match reading {
        Ok(value) => value,
        Err(reason) => {
            // last_value and last_evaluated_at stay as the last successful
            // reading left them, so the card still says what was last known
            sqlx::query(
                "update alert_rules set state='error', last_error=$2, last_error_code=$3, last_error_params=$4, updated_at=now() where id=$1",
            )
            .bind(id)
            .bind(&reason.message)
            .bind(reason.code)
            .bind(&reason.params)
            .execute(&mut *tx)
            .await
            .map_err(rolter_store::postgres::store_err)?;
            tx.commit()
                .await
                .map_err(rolter_store::postgres::store_err)?;
            // `read_signal` words the reason for the rule card, and never with
            // the ClickHouse endpoint in it, so the caller may read it too
            return Err(ApiError::CodedInternal {
                code: reason.code,
                message: reason.message,
            });
        }
    };
    let history: Vec<Reported> = sqlx::query_as(
        "select state, delivery_status, sent_at, \
                extract(epoch from clock_timestamp() - sent_at)::float8 as age_secs \
         from alert_notification_history where rule_id=$1 order by sent_at desc limit $2",
    )
    .bind(id)
    .bind(HISTORY_WINDOW)
    .fetch_all(&mut *tx)
    .await
    .map_err(rolter_store::postgres::store_err)?;
    let next_state = judge(&rule, value, held_state(&rule.state, &history));
    // the time the reading was taken rather than now(), so a later reading
    // can tell this one is older
    let rule: Rule = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update alert_rules set state=$2, last_value=$3, last_evaluated_at=$4, last_error=null, last_error_code=null, last_error_params=null, updated_at=now() where id=$1 returning {}",
        rule_columns()
    )))
    .bind(id)
    .bind(next_state)
    .bind(value)
    .bind(read_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(rolter_store::postgres::store_err)?;
    let redeliver = match (&channel, lock) {
        (Some(channel), Lock::Wait) if channel.enabled => Redeliver::Now,
        (Some(channel), Lock::SkipLocked) if channel.enabled => Redeliver::After(read_at),
        _ => Redeliver::Never,
    };
    let mut outbox = None;
    let notification = match transition(&history, next_state, redeliver) {
        Some(change) => {
            let no_data = value.is_none() && rule.no_data != "ignore";
            let currency = state.currency.base_code();
            let (notification, owed) = record(
                &mut tx,
                &rule,
                channel.as_ref(),
                (change, no_data),
                &currency,
                kek,
            )
            .await?;
            outbox = owed;
            Some(notification)
        }
        None => None,
    };
    tx.commit()
        .await
        .map_err(rolter_store::postgres::store_err)?;
    Ok(Some((
        Evaluation {
            rule,
            notified: false,
            notification,
        },
        outbox,
    )))
}

/// The state a rule keeps when a window has no data and its policy is `ignore`.
///
/// A rule in `error` has lost its state, so it is read back from the newest
/// history row, the same record [`transition`] decides against.
fn held_state(state: &str, history: &[Reported]) -> &'static str {
    match state {
        "firing" => "firing",
        "ok" => "ok",
        "unknown" => "unknown",
        _ => match history.first().map(|r| r.state.as_str()) {
            Some("firing") => "firing",
            Some("resolved") => "ok",
            _ => "unknown",
        },
    }
}

/// The state a reading puts `rule` in. `value` is `None` for a window with no
/// data, which the rule's `no_data` policy settles; `held` is what `ignore`
/// keeps.
///
/// `above` fires at `value >= threshold` and `below` at `value <= threshold`,
/// both inclusive, so a rule that reads exactly its threshold fires either way.
fn judge(rule: &Rule, value: Option<f64>, held: &'static str) -> &'static str {
    let fires = match value {
        Some(value) if rule.comparison == "below" => value <= rule.threshold,
        Some(value) => value >= rule.threshold,
        None => {
            return match rule.no_data.as_str() {
                "fire" => "firing",
                "ok" => "ok",
                _ => held,
            }
        }
    };
    if fires {
        "firing"
    } else {
        "ok"
    }
}

/// Whether a state that holds is sent again when no attempt at it got
/// through.
#[derive(Clone, Copy)]
enum Redeliver {
    /// The rule has no enabled channel, so there is nothing to retry with.
    Never,
    /// Retry now. **Evaluate now** is an operator asking for it.
    Now,
    /// Retry once [`retry_delay`] has passed since the last attempt, measured
    /// at this moment. The scheduled pass runs this way.
    After(DateTime<Utc>),
}

/// What a reading reports, given the rule's newest history rows, newest first:
/// the state to record and deliver, or `None`.
///
/// Decided against the history rather than the rule's `state` column, which
/// also holds `unknown` and `error`. A rule whose evaluation failed between two
/// firing readings is still the same alert and is not reported again, and one
/// that recovered while its evaluation was failing still owes a `resolved`. A
/// rule with no history that reads high reports `firing`, so a rule enabled
/// while its condition already holds says so on its first pass.
///
/// A state that holds is sent again only while no attempt at it was
/// `delivered`: a failed delivery, or one skipped because the channel was
/// disabled or missing, is retried once the rule has an enabled channel. The
/// scheduled pass spaces the retries out (see [`retry_delay`]), so a dead
/// endpoint costs a handful of history rows an hour rather than one a minute.
fn transition(
    history: &[Reported],
    next_state: &str,
    redeliver: Redeliver,
) -> Option<&'static str> {
    let holding = match (history.first().map(|r| r.state.as_str()), next_state) {
        (Some("firing"), "ok") => return Some("resolved"),
        (Some("firing"), _) => "firing",
        (_, "firing") => return Some("firing"),
        (Some("resolved"), _) => "resolved",
        _ => return None,
    };
    let mut unreported = history
        .iter()
        .take_while(|r| r.state == holding && r.delivery_status != "delivered");
    let last = unreported.next()?;
    // a delivery that was committed to and has not settled is on its way from
    // this replica or another, and sending it again now would duplicate it
    if last.delivery_status == "pending" && last.age_secs < PENDING_GRACE_SECS {
        return None;
    }
    match redeliver {
        Redeliver::Never => None,
        Redeliver::Now => Some(holding),
        Redeliver::After(now) => {
            let failures = usize::from(last.delivery_status == "failed")
                + unreported.filter(|r| r.delivery_status == "failed").count();
            (now - last.sent_at >= retry_delay(failures)).then_some(holding)
        }
    }
}

/// How long after the last attempt a state that was not delivered is retried,
/// given how many attempts at it failed.
///
/// The pass after the first failure, then two, four, eight, sixteen and
/// thirty-two passes on, then hourly. Each delay is a minute under that
/// spacing because an attempt is recorded up to 40 seconds into its pass, so
/// the pass that many minutes later reads a little under it.
fn retry_delay(failures: usize) -> chrono::Duration {
    let passes = 1_i64 << failures.saturating_sub(1).min(6);
    chrono::Duration::minutes((passes - 1).min(59))
}

/// Run the rule's signal query, bounded by [`QUERY_TIMEOUT`].
///
/// The error is what the rule's `last_error` shows, so it is kept short and
/// never carries the ClickHouse endpoint (see [`describe_query_error`]).
///
/// `Ok(None)` is a window with no data, for a signal that can lack it (see
/// [`can_lack_data`]); any other empty window reads `0`.
async fn read_signal(state: &ControlState, rule: &Rule) -> Result<Option<f64>, SignalError> {
    let Some(ch) = state.clickhouse.as_ref() else {
        return Err(SignalError::new(
            error_code::CLICKHOUSE_NOT_CONFIGURED,
            "alert evaluation requires CLICKHOUSE_URL",
        ));
    };
    // a scoped rule whose team, project, key... was deleted would read an empty
    // window, which for `below request_volume` is a standing false alarm
    let scope = match (rule.scope_type.as_deref(), rule.scope_id) {
        (Some(scope_type), Some(scope_id)) => {
            if ScopeChain::from_scope(pool(state), scope_type, scope_id)
                .await
                .is_err()
            {
                return Err(SignalError::new(
                    error_code::SCOPE_GONE,
                    "the scope this rule watches no longer exists",
                ));
            }
            Some((scope_type, scope_id))
        }
        _ => None,
    };
    read_signal_from(ch, &rule.signal, rule.window_secs, scope).await
}

/// [`read_signal`] against a given ClickHouse client.
async fn read_signal_from(
    ch: &crate::analytics::ClickHouseClient,
    signal: &str,
    window_secs: i32,
    scope: Option<(&str, Uuid)>,
) -> Result<Option<f64>, SignalError> {
    let column = scope.and_then(|(scope_type, _)| scope_column(scope_type));
    // the scope id is bound as a query parameter, never spliced into the sql
    let params: Vec<(String, String)> = scope
        .map(|(_, id)| vec![("param_scope_id".to_string(), id.to_string())])
        .unwrap_or_default();
    let sql = metric_sql_scoped(signal, window_secs, column).map_err(|_| SignalError {
        code: error_code::UNKNOWN_SIGNAL,
        params: Some(serde_json::json!({ "signal": signal })),
        message: format!("unknown signal '{signal}'"),
    })?;
    let rows = match tokio::time::timeout(QUERY_TIMEOUT, ch.query(&sql, &params)).await {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => {
            tracing::warn!(
                error = %rolter_core::redact::redact_urls_in_text(&format!("{error:#}")),
                signal,
                "alert signal query failed"
            );
            return Err(describe_query_error(&error));
        }
        Err(_) => {
            return Err(SignalError {
                code: error_code::QUERY_TIMEOUT,
                params: Some(serde_json::json!({ "seconds": QUERY_TIMEOUT.as_secs() })),
                message: format!(
                    "analytics query timed out after {}s",
                    QUERY_TIMEOUT.as_secs()
                ),
            })
        }
    };
    let row = rows.first();
    let samples = row
        .and_then(|row| row.get("samples"))
        .and_then(serde_json::Value::as_f64);
    let value = row
        .and_then(|row| row.get("value"))
        .and_then(serde_json::Value::as_f64);
    Ok(reading(signal, value, samples))
}

/// A signal's reading from the row its query returned: the value and how many
/// logged requests it was computed from.
fn reading(signal: &str, value: Option<f64>, samples: Option<f64>) -> Option<f64> {
    if can_lack_data(signal) && (value.is_none() || samples.is_some_and(|n| n < 1.0)) {
        return None;
    }
    Some(value.unwrap_or(0.0))
}

/// A bounded description of a failed signal query that is safe to store.
///
/// A transport error's message carries the request URL, and `CLICKHOUSE_URL`
/// may hold a password, so those are reduced to their class. ClickHouse error
/// responses may expose SQL or internal schema details, so their text stays in
/// the URL-redacted server log rather than reaching API clients.
fn describe_query_error(error: &anyhow::Error) -> SignalError {
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        let (code, class) = if error.is_timeout() {
            (error_code::CLICKHOUSE_TIMEOUT, "analytics query timed out")
        } else if error.is_connect() {
            (
                error_code::CLICKHOUSE_UNREACHABLE,
                "could not connect to ClickHouse",
            )
        } else if error.is_decode() {
            (
                error_code::CLICKHOUSE_BAD_RESPONSE,
                "ClickHouse returned a response that is not JSON",
            )
        } else {
            (error_code::QUERY_FAILED, "analytics query failed")
        };
        return SignalError::new(code, class);
    }
    SignalError::new(error_code::CLICKHOUSE_ERROR, "analytics query failed")
}

#[cfg(test)]
fn metric_sql(signal: &str, window_secs: i32) -> ApiResult<String> {
    metric_sql_scoped(signal, window_secs, None)
}

/// [`metric_sql`] narrowed to the rows whose `column` equals the bound
/// `scope_id` parameter, for a rule that watches one scope rather than the
/// deployment (#1829).
fn metric_sql_scoped(signal: &str, window_secs: i32, column: Option<&str>) -> ApiResult<String> {
    let window = window_secs.clamp(60, 86_400);
    let expression = match signal {
        // a ratio of weighted counts: each kept row stands for `sample_weight`
        // real requests, so a rate change inside the window cannot skew it
        // server-side failures: 5xx plus status 0, a request that never got an
        // answer (#3294). 4xx stays out, it is the caller's own mistake and a
        // rule on it would page on a client bug
        "error_rate" => "if(sum(sample_weight) = 0, 0, sumIf(sample_weight, status = 0 or status >= 500) / sum(sample_weight))".to_string(),
        // weighted by the row's sample weight like the other signals (#3355)
        "p95_latency_ms" => format!("if(count() = 0, 0, {})", crate::analytics::P95_LATENCY),
        "spend_velocity" => format!("sum(cost_usd * sample_weight) * 3600 / {window}"),
        "request_volume" => "sum(sample_weight)".to_string(),
        "provider_health_flaps" => "countIf(outcome != 'ok')".to_string(),
        _ => return Err(invalid_field("signal", "unknown alert signal")),
    };
    let table = if signal == "provider_health_flaps" {
        "provider_health_events"
    } else {
        "request_logs"
    };
    // the column comes from `scope_column`'s fixed list, the value is a bound
    // parameter
    let scope = column
        .map(|column| format!(" and {column} = {{scope_id:String}}"))
        .unwrap_or_default();
    // samples stays a plain row count: it only says whether the window held
    // any logged row, which the weights must not inflate. provider_health_events
    // is not sampled and has no weight column
    // samples as a float: ClickHouse quotes a 64-bit integer in JSON output
    Ok(format!("select toFloat64({expression}) as value, toFloat64(count()) as samples from {table} where ts >= now64(3) - interval {window} second{scope} format JSON"))
}

/// The channel columns delivery needs, secret included. Never serialized.
#[derive(Clone, sqlx::FromRow)]
struct ChannelTarget {
    id: Uuid,
    endpoint: String,
    enabled: bool,
    secret_ciphertext: Option<Vec<u8>>,
    secret_nonce: Option<Vec<u8>>,
    /// owning org; null for a deployment channel
    org_id: Option<Uuid>,
}

/// One history row's `delivery_status`, `detail` and its stable code.
struct Outcome {
    status: &'static str,
    code: &'static str,
    params: Option<serde_json::Value>,
    detail: String,
}

impl Outcome {
    fn new(status: &'static str, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status,
            code,
            params: None,
            detail: detail.into(),
        }
    }

    fn delivered(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new("delivered", code, detail)
    }

    fn failed(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new("failed", code, detail)
    }

    fn skipped(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new("skipped", code, detail)
    }

    /// The numbers the code interpolates
    fn with(mut self, params: serde_json::Value) -> Self {
        self.params = Some(params);
        self
    }

    /// An HTTP outcome: the status is both in the text and a parameter
    fn http(status: u16, ok: bool) -> Self {
        let outcome = if ok {
            Self::delivered(detail_code::HTTP_OK, format!("HTTP {status}"))
        } else {
            Self::failed(detail_code::HTTP_ERROR, format!("HTTP {status}"))
        };
        outcome.with(serde_json::json!({ "status": status }))
    }
}

/// A delivery an evaluation committed to and has not sent yet: everything the
/// send needs, so it can run after the transaction and the rule's row lock are
/// gone. The `pending` history row it settles is already in the database.
struct Outbox {
    /// the `pending` history row to settle
    id: Uuid,
    rule_id: Uuid,
    change: &'static str,
    no_data: bool,
    channel: ChannelTarget,
    body: serde_json::Value,
    /// held so the secret is unsealed at send time, not at commit time
    kek: Option<Kek>,
}

/// Write the history row for `change`. A channel that cannot be sent to has its
/// outcome (`skipped`) written final; one that can gets a `pending` row and the
/// [`Outbox`] that settles it once the transaction has committed.
async fn record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &Rule,
    channel: Option<&ChannelTarget>,
    (change, no_data): (&'static str, bool),
    currency: &str,
    kek: Option<&Kek>,
) -> ApiResult<(Notification, Option<Outbox>)> {
    let id = Uuid::new_v4();
    let (outcome, outbox) = match channel {
        None => (
            Outcome::skipped(detail_code::NO_CHANNEL, "no channel configured"),
            None,
        ),
        Some(channel) if !channel.enabled => (
            Outcome::skipped(detail_code::CHANNEL_DISABLED, "channel disabled"),
            None,
        ),
        Some(channel) => (
            Outcome::new(
                "pending",
                detail_code::DELIVERY_PENDING,
                "delivery in progress",
            ),
            Some(Outbox {
                id,
                rule_id: rule.id,
                change,
                no_data,
                channel: channel.clone(),
                body: payload(id, rule, change, no_data, currency),
                kek: kek.cloned(),
            }),
        ),
    };
    let outcome = note_no_data(outcome, no_data);
    // clock_timestamp rather than the column's now() default: now() is when the
    // transaction began, and a transaction that waited for the row lock began
    // before the one it waited for, which would order the history backwards
    let notification: Notification = sqlx::query_as(
        "insert into alert_notification_history (id, rule_id, channel_id, state, delivery_status, detail, detail_code, detail_params, value, threshold, sent_at) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, clock_timestamp()) \
         returning id, rule_id, channel_id, state, delivery_status, detail, detail_code, detail_params, value, threshold, sent_at",
    )
    .bind(id)
    .bind(rule.id)
    .bind(channel.map(|c| c.id))
    .bind(change)
    .bind(outcome.status)
    .bind(&outcome.detail)
    .bind(outcome.code)
    .bind(&outcome.params)
    .bind(reading_of(rule, no_data))
    .bind(rule.threshold)
    .fetch_one(&mut **tx)
    .await
    .map_err(rolter_store::postgres::store_err)?;
    Ok((notification, outbox))
}

/// The history row is the record of why a rule fired, and an empty window is
/// not evident from the reading, so an outcome on one says so.
fn note_no_data(outcome: Outcome, no_data: bool) -> Outcome {
    if !no_data {
        return outcome;
    }
    let mut params = match outcome.params {
        Some(serde_json::Value::Object(params)) => params,
        _ => serde_json::Map::new(),
    };
    params.insert("no_data".into(), true.into());
    Outcome {
        detail: format!("{}; {NO_DATA_DETAIL}", outcome.detail),
        params: Some(serde_json::Value::Object(params)),
        ..outcome
    }
}

/// Send `outbox` and write the outcome over its `pending` row.
///
/// Runs outside any transaction. A failed write leaves the row `pending`, which
/// [`transition`] reads as lost once it is older than [`PENDING_GRACE_SECS`].
async fn settle(
    pool: &sqlx::PgPool,
    egress: &Arc<EgressPolicy>,
    outbox: Outbox,
) -> ApiResult<Notification> {
    let outcome = deliver(egress, &outbox.channel, &outbox.body, outbox.kek.as_ref()).await;
    let outcome = note_no_data(outcome, outbox.no_data);
    if outcome.status == "failed" {
        tracing::warn!(rule = %outbox.rule_id, change = outbox.change, detail = %outcome.detail, "alert delivery failed");
    }
    sqlx::query_as(
        "update alert_notification_history set delivery_status=$2, detail=$3, detail_code=$4, detail_params=$5 \
         where id=$1 \
         returning id, rule_id, channel_id, state, delivery_status, detail, detail_code, detail_params, value, threshold, sent_at",
    )
    .bind(outbox.id)
    .bind(outcome.status)
    .bind(&outcome.detail)
    .bind(outcome.code)
    .bind(&outcome.params)
    .fetch_one(pool)
    .await
    .map_err(rolter_store::postgres::store_err)
    .map_err(ApiError::from)
}

/// The reading a transition reports: the rule's last value, or `None` for a
/// window with no data, where a made-up `0` would mislead.
fn reading_of(rule: &Rule, no_data: bool) -> Option<f64> {
    if no_data {
        None
    } else {
        Some(rule.last_value.unwrap_or_default())
    }
}

/// The machine-readable unit of a signal's `value` and `threshold`: `fraction`
/// (0-1), `ms`, `currency_per_hour` (in the payload's `currency`) or `count`
/// (over the rule's window).
fn signal_unit(signal: &str) -> &'static str {
    match signal {
        "error_rate" => "fraction",
        "p95_latency_ms" => "ms",
        "spend_velocity" => "currency_per_hour",
        _ => "count",
    }
}

/// A number with at most `decimals` places and no trailing zeros.
fn plain(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// A reading or threshold in its signal's unit, for a person to read.
fn with_unit(signal: &str, value: f64, currency: &str) -> String {
    match signal {
        "error_rate" => format!("{}%", plain(value * 100.0, 1)),
        "p95_latency_ms" => format!("{} ms", plain(value, if value < 10.0 { 1 } else { 0 })),
        "spend_velocity" => format!("{currency} {} per hour", plain(value, 2)),
        "provider_health_flaps" => format!("{} failed health checks", plain(value, 0)),
        _ => format!("{} requests", plain(value, 0)),
    }
}

/// A window as a duration rather than a bare second count.
fn window_text(secs: i32) -> String {
    let (n, unit) = if secs >= 3600 && secs % 3600 == 0 {
        (secs / 3600, "hour")
    } else if secs >= 60 && secs % 60 == 0 {
        (secs / 60, "minute")
    } else {
        (secs, "second")
    };
    if n == 1 {
        unit.to_string()
    } else {
        format!("{n} {unit}s")
    }
}

/// The JSON body a channel receives.
///
/// `id` is the history row's id, so a receiver can match a request to the row
/// the **History** screen shows. `text` is a one-line summary for a relay or a
/// chat tool to display as-is, with the reading and the threshold in the
/// signal's unit. `value` and `threshold` stay in the API's unit, which `unit`
/// names (and `currency` completes for `currency_per_hour`), so a relay can
/// format them without its own copy of the signal table.
///
/// `no_data` marks a transition caused by a window with no data: `value` is
/// then `null` and `text` says so, rather than reporting a made-up `0`.
fn payload(
    id: Uuid,
    rule: &Rule,
    change: &str,
    no_data: bool,
    currency: &str,
) -> serde_json::Value {
    let value = reading_of(rule, no_data);
    let verb = if change == "firing" {
        "is firing"
    } else {
        "resolved"
    };
    let window = window_text(rule.window_secs);
    let text = match value {
        Some(value) => {
            // an above rule reads as it always has
            let below = if rule.comparison == "below" {
                " (below)"
            } else {
                ""
            };
            format!(
                "{} {verb}: {} is {}, threshold {}{below}, over the last {window}",
                rule.name,
                rule.signal,
                with_unit(&rule.signal, value, currency),
                with_unit(&rule.signal, rule.threshold, currency),
            )
        }
        None => format!(
            "{} {verb}: {NO_DATA_DETAIL} for {}, over the last {window}",
            rule.name, rule.signal
        ),
    };
    let unit = signal_unit(&rule.signal);
    serde_json::json!({
        "id": id,
        "state": change,
        "rule": { "id": rule.id, "name": rule.name },
        "signal": rule.signal,
        "value": value,
        "no_data": no_data,
        "comparison": rule.comparison,
        "threshold": rule.threshold,
        "unit": unit,
        "currency": (unit == "currency_per_hour").then_some(currency),
        "window_secs": rule.window_secs,
        "evaluated_at": rule.last_evaluated_at,
        "text": text,
    })
}

/// POST one transition to an enabled channel.
///
/// Infallible on purpose: every way delivery can go wrong is an outcome the
/// history records, never an evaluation error, so a dead endpoint cannot stop
/// the rule's state from moving.
async fn deliver(
    egress: &Arc<EgressPolicy>,
    channel: &ChannelTarget,
    payload: &serde_json::Value,
    kek: Option<&Kek>,
) -> Outcome {
    // checked when the channel was saved, but the policy may have been
    // tightened since; a stored row is not a standing permission to egress.
    // the log gets the reason and the channel, never the endpoint, whose query
    // string can carry a token
    if let Some(reason) = egress.url_deny_reason(&channel.endpoint) {
        tracing::warn!(channel = %channel.id, reason, "alert delivery refused by the egress policy");
        return Outcome::failed(
            detail_code::EGRESS_DENIED,
            "endpoint denied by the egress policy",
        );
    }
    let authorization = match (&channel.secret_ciphertext, &channel.secret_nonce) {
        (Some(ciphertext), Some(nonce)) => {
            match kek.and_then(|kek| kek.decrypt(ciphertext, nonce).ok()) {
                Some(secret) => match bearer(&secret) {
                    Some(value) => Some(value),
                    // refused at save time; a secret stored before that check
                    // existed would otherwise fail as a bare "delivery failed"
                    None => {
                        return Outcome::failed(
                            detail_code::SECRET_INVALID,
                            "channel secret is not a valid header value",
                        )
                    }
                },
                // sending without the credential would hand an unauthenticated
                // alert to a receiver that may well accept it
                None => {
                    return Outcome::failed(
                        detail_code::SECRET_UNSEALABLE,
                        format!("channel secret could not be unsealed; check {KEK_ENV}"),
                    )
                }
            }
        }
        _ => None,
    };
    let Some(client) = delivery_client(egress) else {
        return Outcome::failed(
            detail_code::CLIENT_UNAVAILABLE,
            "webhook client unavailable",
        );
    };
    let mut request = client.post(&channel.endpoint).json(payload);
    if let Some(value) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            Outcome::http(response.status().as_u16(), true)
        }
        // the status alone, never the body: a receiver's error body can echo
        // the credential it just rejected
        Ok(response) => Outcome::http(response.status().as_u16(), false),
        Err(error) if error.is_timeout() => Outcome::failed(
            detail_code::TIMEOUT,
            format!("timed out after {}s", DELIVERY_TIMEOUT.as_secs()),
        )
        .with(serde_json::json!({ "seconds": DELIVERY_TIMEOUT.as_secs() })),
        Err(error) if error.is_connect() => Outcome::failed(
            detail_code::CONNECT_FAILED,
            "could not connect to the endpoint",
        ),
        Err(_) => Outcome::failed(detail_code::DELIVERY_FAILED, "delivery failed"),
    }
}

/// The client webhooks are sent with.
///
/// It resolves through the egress policy, so a hostname that answers with a
/// denied address is refused at connect time, and it does not follow
/// redirects: a `3xx` is how an endpoint that passed the egress check hands the
/// request to one that would not have, so it is recorded as a failure instead.
/// Bounded end to end, since the evaluator waits on it while holding the
/// rule's row lock.
fn delivery_client(egress: &Arc<EgressPolicy>) -> Option<reqwest::Client> {
    crate::egress_client::builder(egress)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DELIVERY_TIMEOUT)
        .user_agent(concat!("rolter/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
}

/// Record an alert change in the audit log of the org that owns the row, or in
/// the deployment's alone (`org: None`) for a deployment-wide channel or rule,
/// so an org admin's changes show in their own org's log and export (#1829).
async fn audit(
    state: &ControlState,
    principal: &Principal,
    org: Option<Uuid>,
    action: &str,
    target: Uuid,
    detail: serde_json::Value,
) {
    let actor = principal.account_id();
    if let Err(error) = AuditLogRepo(pool(state))
        .create(
            org,
            actor,
            action,
            Some("alerting"),
            Some(target),
            Some(detail),
        )
        .await
    {
        tracing::warn!(error = %error, action, "failed to write alert audit log");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_limit_counts_characters_of_the_trimmed_value() {
        // 128 cyrillic characters are 256 bytes and still fit
        assert!(validate_name(&"я".repeat(128), "rule name").is_ok());
        assert!(validate_name(&"я".repeat(129), "rule name").is_err());
        // surrounding whitespace is trimmed before storing, so it does not count
        assert!(validate_name(&format!("  {}  ", "a".repeat(128)), "rule name").is_ok());
        assert!(validate_name("   ", "rule name").is_err());
        assert!(validate_name("a\u{7}b", "rule name").is_err());
    }

    fn rule(signal: &str, threshold: f64, window_secs: i32) -> RuleInput {
        RuleInput {
            scope_type: None,
            scope_id: None,
            name: "x".into(),
            signal: signal.into(),
            threshold,
            comparison: None,
            no_data: None,
            window_secs,
            channel_id: None,
            enabled: false,
        }
    }

    fn stored(comparison: &str, no_data: &str) -> Rule {
        Rule {
            org_id: None,
            scope_type: None,
            scope_id: None,
            id: Uuid::nil(),
            name: "r".into(),
            signal: "error_rate".into(),
            threshold: 0.5,
            comparison: comparison.into(),
            no_data: no_data.into(),
            window_secs: 300,
            channel_id: None,
            enabled: true,
            state: "ok".into(),
            last_value: None,
            last_evaluated_at: None,
            last_error: None,
            last_error_code: None,
            last_error_params: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_rule_body_without_the_new_fields_keeps_todays_behaviour() {
        let input: RuleInput = serde_json::from_value(serde_json::json!({
            "name": "x", "signal": "error_rate", "threshold": 0.5, "window_secs": 300
        }))
        .expect("an old-style body still parses");
        assert_eq!(
            (input.comparison().as_str(), input.no_data().as_str()),
            ("above", "ignore")
        );
        assert!(validate_rule(&input).is_ok());
    }

    #[test]
    fn rules_reject_unknown_comparison_and_no_data_values() {
        let mut input = rule("error_rate", 0.5, 60);
        input.comparison = Some("equal".into());
        assert!(validate_rule(&input).is_err());
        let mut input = rule("error_rate", 0.5, 60);
        input.no_data = Some("page".into());
        assert!(validate_rule(&input).is_err());
    }

    #[test]
    fn no_data_policy_is_only_accepted_where_a_window_can_lack_data() {
        for signal in SIGNALS {
            let mut input = rule(signal, 0.5, 60);
            input.no_data = Some("fire".into());
            assert_eq!(
                validate_rule(&input).is_ok(),
                can_lack_data(signal),
                "{signal}"
            );
            input.comparison = Some("below".into());
            input.no_data = Some("ignore".into());
            assert!(validate_rule(&input).is_ok(), "{signal}");
        }
    }

    #[test]
    fn above_fires_at_or_over_and_below_at_or_under_the_threshold() {
        let above = stored("above", "ignore");
        assert_eq!(judge(&above, Some(0.5), "ok"), "firing");
        assert_eq!(judge(&above, Some(0.9), "ok"), "firing");
        assert_eq!(judge(&above, Some(0.1), "firing"), "ok");
        let below = stored("below", "ignore");
        assert_eq!(judge(&below, Some(0.5), "ok"), "firing");
        assert_eq!(judge(&below, Some(0.1), "ok"), "firing");
        assert_eq!(judge(&below, Some(0.9), "firing"), "ok");
    }

    #[test]
    fn a_window_with_no_data_follows_the_rules_policy() {
        for comparison in ["above", "below"] {
            let ignore = stored(comparison, "ignore");
            assert_eq!(judge(&ignore, None, "ok"), "ok");
            assert_eq!(judge(&ignore, None, "firing"), "firing");
            assert_eq!(judge(&ignore, None, "unknown"), "unknown");
            assert_eq!(judge(&stored(comparison, "fire"), None, "ok"), "firing");
            assert_eq!(judge(&stored(comparison, "ok"), None, "firing"), "ok");
        }
    }

    #[test]
    fn an_empty_window_reads_as_no_data_only_for_signals_that_can_lack_it() {
        assert_eq!(reading("error_rate", Some(0.0), Some(0.0)), None);
        assert_eq!(reading("p95_latency_ms", Some(0.0), Some(0.0)), None);
        assert_eq!(reading("error_rate", None, None), None);
        assert_eq!(reading("error_rate", Some(0.0), Some(40.0)), Some(0.0));
        assert_eq!(reading("request_volume", Some(0.0), Some(0.0)), Some(0.0));
        assert_eq!(reading("spend_velocity", None, None), Some(0.0));
    }

    #[test]
    fn a_rule_in_error_holds_the_state_its_history_last_reported() {
        let row = |state: &str| Reported {
            state: state.into(),
            delivery_status: "delivered".into(),
            sent_at: Utc::now(),
            age_secs: 0.0,
        };
        assert_eq!(held_state("error", &[row("firing")]), "firing");
        assert_eq!(held_state("error", &[row("resolved")]), "ok");
        assert_eq!(held_state("error", &[]), "unknown");
        assert_eq!(held_state("ok", &[row("firing")]), "ok");
    }

    #[test]
    fn query_errors_do_not_expose_internal_exception_details() {
        let error = anyhow::anyhow!("DB::Exception: unknown table private_schema.requests");
        let described = describe_query_error(&error);
        assert_eq!(described.message, "analytics query failed");
        assert_eq!(described.code, error_code::CLICKHOUSE_ERROR);
    }

    #[test]
    fn a_no_data_payload_says_so_and_carries_no_made_up_value() {
        let mut rule = stored("above", "fire");
        rule.state = "firing".into();
        let body = payload(Uuid::nil(), &rule, "firing", true, "USD");
        assert_eq!(body["no_data"], true);
        assert!(body["value"].is_null());
        assert_eq!(
            body["text"],
            "r is firing: no data in window for error_rate, over the last 5 minutes"
        );
        rule.last_value = Some(0.7);
        let body = payload(Uuid::nil(), &rule, "firing", false, "USD");
        assert_eq!(body["no_data"], false);
        assert_eq!(body["value"], 0.7);
        assert_eq!(body["comparison"], "above");
    }

    #[test]
    fn readings_are_worded_in_their_signals_unit() {
        assert_eq!(with_unit("error_rate", 0.073, "EUR"), "7.3%");
        assert_eq!(with_unit("p95_latency_ms", 840.0, "EUR"), "840 ms");
        assert_eq!(with_unit("p95_latency_ms", 2.5, "EUR"), "2.5 ms");
        assert_eq!(
            with_unit("spend_velocity", 12.5, "EUR"),
            "EUR 12.5 per hour"
        );
        assert_eq!(with_unit("request_volume", 340.0, "EUR"), "340 requests");
        assert_eq!(
            with_unit("provider_health_flaps", 3.0, "EUR"),
            "3 failed health checks"
        );
        assert_eq!(window_text(300), "5 minutes");
        assert_eq!(window_text(60), "minute");
        assert_eq!(window_text(7200), "2 hours");
        assert_eq!(window_text(90), "90 seconds");
    }

    #[test]
    fn a_spend_payload_names_its_unit_and_currency() {
        let mut rule = stored("above", "ignore");
        rule.signal = "spend_velocity".into();
        rule.threshold = 10.0;
        rule.last_value = Some(12.5);
        let body = payload(Uuid::nil(), &rule, "firing", false, "EUR");
        assert_eq!(body["unit"], "currency_per_hour");
        assert_eq!(body["currency"], "EUR");
        assert_eq!(
            body["text"],
            "r is firing: spend_velocity is EUR 12.5 per hour, threshold EUR 10 per hour, over the last 5 minutes"
        );
    }

    #[test]
    fn a_history_cursor_round_trips_and_a_bad_one_is_refused() {
        let id = Uuid::new_v4();
        let at = Utc::now();
        let raw = format!(
            "{},{id}",
            at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
        );
        let (parsed_at, parsed_id) = parse_cursor(&raw).expect("a cursor parses");
        assert_eq!(parsed_id, id);
        assert_eq!(parsed_at.timestamp_micros(), at.timestamp_micros());
        assert!(parse_cursor("nonsense").is_err());
        assert!(parse_cursor(&format!("yesterday,{id}")).is_err());
        assert!(one_of("state", Some("paused".into()), HISTORY_STATES).is_err());
        assert!(one_of("state", Some("firing".into()), HISTORY_STATES).is_ok());
    }

    fn channel(endpoint: &str, managed_secret: Option<String>) -> ChannelInput {
        ChannelInput {
            org_id: None,
            name: "ops".into(),
            endpoint: endpoint.into(),
            enabled: false,
            managed_secret,
        }
    }

    /// #1949: a hostname that resolves only to an address the policy denies
    /// passes the literal check, so only the connect-time resolver stops it.
    #[tokio::test]
    async fn a_name_resolving_to_a_denied_address_is_never_contacted() {
        let listener = crate::egress_client::testing::Counter::start().await;
        let channel = ChannelTarget {
            org_id: None,
            id: Uuid::new_v4(),
            endpoint: listener.url("/hook"),
            enabled: true,
            secret_ciphertext: None,
            secret_nonce: None,
        };
        let policy = crate::egress_client::testing::deny_loopback();
        // the write-time and pre-send literal checks cannot see a hostname
        assert!(policy.url_deny_reason(&channel.endpoint).is_none());
        let outcome = deliver(&policy, &channel, &serde_json::json!({}), None).await;
        assert_eq!(outcome.status, "failed");
        assert_eq!(outcome.detail, "could not connect to the endpoint");
        assert_eq!(listener.accepted(), 0);
    }

    fn egress() -> EgressPolicy {
        EgressPolicy::default()
    }

    /// Validate `input` and run the save-time egress check, as a create does.
    fn save(input: &ChannelInput, egress: &EgressPolicy) -> ApiResult<reqwest::Url> {
        let endpoint = validate_channel(input)?;
        check_egress(egress, &endpoint)?;
        Ok(endpoint)
    }

    #[test]
    fn rules_reject_negative_threshold() {
        assert!(validate_rule(&rule("error_rate", -1.0, 60)).is_err());
    }

    #[test]
    fn rules_reject_non_finite_threshold() {
        assert!(validate_rule(&rule("error_rate", f64::NAN, 60)).is_err());
    }

    #[test]
    fn rules_reject_out_of_bounds_window_secs() {
        assert!(validate_rule(&rule("error_rate", 0.5, 30)).is_err());
        assert!(validate_rule(&rule("error_rate", 0.5, 86_401)).is_err());
    }

    #[test]
    fn rules_reject_unknown_signal() {
        assert!(validate_rule(&rule("not_a_real_signal", 0.5, 60)).is_err());
    }

    #[test]
    fn rules_accept_every_supported_signal() {
        for signal in SIGNALS {
            assert!(validate_rule(&rule(signal, 0.5, 60)).is_ok());
        }
    }

    #[test]
    fn a_rule_notifies_its_own_orgs_channel_or_a_deployment_one_and_no_other() {
        let a = Some(Uuid::from_u128(1));
        let b = Some(Uuid::from_u128(2));
        // channel owner, rule owner, may notify
        for (channel, rule, expected) in [
            (None, None, true),
            (None, a, true),
            (a, a, true),
            (a, None, false),
            (a, b, false),
            (b, a, false),
        ] {
            assert_eq!(
                channel_reaches(channel, rule),
                expected,
                "{channel:?} {rule:?}"
            );
        }
    }

    #[test]
    fn a_scoped_rule_reads_only_its_scope_through_a_bound_parameter() {
        for (scope, column) in [
            ("org", "org_id"),
            ("team", "team_id"),
            ("project", "project_id"),
            ("virtual_key", "virtual_key_id"),
            ("business_unit", "business_unit_id"),
            ("customer", "customer_id"),
        ] {
            assert_eq!(scope_column(scope), Some(column));
            let sql = metric_sql_scoped("request_volume", 300, scope_column(scope)).unwrap();
            assert!(
                sql.contains(&format!("and {column} = {{scope_id:String}}")),
                "{sql}"
            );
            // the id is never spliced in
            assert!(!sql.contains("'"), "{sql}");
        }
        assert!(scope_column("deployment").is_none());
        assert!(!metric_sql("request_volume", 300)
            .unwrap()
            .contains("scope_id"));
        // the health stream is keyed by org alone
        let sql = metric_sql_scoped("provider_health_flaps", 300, Some("org_id")).unwrap();
        assert!(sql.contains("provider_health_events") && sql.contains("org_id = {scope_id"));
    }

    #[test]
    fn the_error_rate_counts_server_failures_and_requests_without_a_status() {
        let sql = metric_sql("error_rate", 300).unwrap();
        // status 0 never got an answer; a 4xx is the caller's own mistake
        assert!(sql.contains("status = 0 or status >= 500"), "{sql}");
        assert!(!sql.contains("status >= 400"), "{sql}");
    }

    #[test]
    fn metric_sql_uses_expected_tables() {
        assert!(metric_sql("error_rate", 60)
            .unwrap()
            .contains("request_logs"));
        assert!(metric_sql("provider_health_flaps", 60)
            .unwrap()
            .contains("provider_health_events"));
        assert!(metric_sql("not_a_real_signal", 60).is_err());
    }

    #[test]
    fn request_log_signals_scale_by_the_row_weight() {
        for signal in ["request_volume", "spend_velocity", "error_rate"] {
            let sql = metric_sql(signal, 60).unwrap();
            assert!(sql.contains("sample_weight"), "{signal}: {sql}");
        }
        // the ratio is of weighted counts on both sides
        let sql = metric_sql("error_rate", 60).unwrap();
        assert!(
            !sql.contains("countIf(status") && !sql.contains("/ count()"),
            "{sql}"
        );
        // the unsampled health table has no weight to apply
        let sql = metric_sql("provider_health_flaps", 60).unwrap();
        assert!(!sql.contains("sample_weight"), "{sql}");
    }

    #[test]
    fn channels_accept_exact_http_endpoints() {
        assert!(save(&channel("https://hooks.example/rolter", None), &egress()).is_ok());
    }

    #[test]
    fn channels_reject_empty_managed_secret() {
        assert!(save(
            &channel("https://hooks.example/rolter", Some(String::new())),
            &egress()
        )
        .is_err());
    }

    #[test]
    fn channels_reject_endpoints_with_userinfo() {
        assert!(save(
            &channel("https://user:pass@hooks.example/rolter", None),
            &egress()
        )
        .is_err());
    }

    #[test]
    fn channels_reject_non_http_schemes() {
        assert!(save(&channel("ftp://hooks.example/rolter", None), &egress()).is_err());
    }

    #[test]
    fn channels_go_through_the_egress_policy() {
        // cloud instance metadata is denied by default, in every spelling the
        // http client would dial it by (#1953)
        for endpoint in [
            "http://169.254.169.254/latest/meta-data/",
            "http://2852039166/latest/meta-data/",
            "http://0xa9fea9fe/",
            "http://169.254.43518/",
            "http://0251.0376.0251.0376/",
            "http://169.254.169.254./",
            "http://[::ffff:169.254.169.254]/",
        ] {
            assert!(
                save(&channel(endpoint, None), &egress()).is_err(),
                "{endpoint} was saved"
            );
        }
        // and loopback in every spelling once the operator denies it
        let no_loopback = EgressPolicy {
            block_loopback: true,
            ..EgressPolicy::default()
        };
        for endpoint in ["http://2130706433/", "http://0.0.0.0:4001/", "http://[::]/"] {
            assert!(
                save(&channel(endpoint, None), &no_loopback).is_err(),
                "{endpoint} was saved"
            );
        }
        // and an operator can deny private ranges too
        let strict = EgressPolicy {
            block_private: true,
            ..EgressPolicy::default()
        };
        assert!(save(&channel("http://10.0.0.5/hook", None), &strict).is_err());
        assert!(save(&channel("http://10.0.0.5/hook", None), &egress()).is_ok());
    }

    #[test]
    fn channels_store_the_endpoint_as_the_client_parses_it() {
        let endpoint = save(&channel("HTTPS://Alerts.Example.COM", None), &egress()).unwrap();
        assert_eq!(endpoint.as_str(), "https://alerts.example.com/");
    }

    #[test]
    fn channels_reject_a_secret_that_is_not_a_header_value() {
        // the case index, not the value, in the message: an assertion that
        // prints a secret is the habit this module exists to avoid
        for (case, value) in ["tok\n", "tok\r\nX-Injected: 1", "a\u{7f}b"]
            .into_iter()
            .enumerate()
        {
            assert!(
                save(
                    &channel("https://hooks.example/rolter", Some(value.into())),
                    &egress()
                )
                .is_err(),
                "case {case} was accepted"
            );
        }
        assert!(save(
            &channel("https://hooks.example/rolter", Some("tok-123".into())),
            &egress()
        )
        .is_ok());
    }

    /// A fixed "now", so a row written zero minutes ago is not a hair after it.
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn row(state: &str, delivery_status: &str, minutes_ago: i64) -> Reported {
        Reported {
            state: state.into(),
            delivery_status: delivery_status.into(),
            sent_at: now() - chrono::Duration::minutes(minutes_ago),
            age_secs: (minutes_ago * 60) as f64,
        }
    }

    #[test]
    fn a_delivery_still_in_flight_is_not_sent_again_until_it_is_presumed_lost() {
        for redeliver in [Redeliver::Now, Redeliver::After(now())] {
            // committed a minute ago: some replica is sending it
            assert_eq!(
                transition(&[row("firing", "pending", 1)], "firing", redeliver),
                None
            );
            // past the grace the process that owed it is gone
            assert_eq!(
                transition(&[row("firing", "pending", 10)], "firing", redeliver),
                Some("firing")
            );
        }
        // a recovery is a new transition, never held back by a pending firing
        assert_eq!(
            transition(&[row("firing", "pending", 0)], "ok", Redeliver::Now),
            Some("resolved")
        );
    }

    #[test]
    fn a_transition_is_decided_against_the_last_reported_state() {
        let now = Redeliver::After(now());
        // nothing reported yet: a high reading fires, a low one is quiet
        assert_eq!(transition(&[], "firing", now), Some("firing"));
        assert_eq!(transition(&[], "ok", now), None);
        // after a resolve the next high reading fires again
        let resolved = [row("resolved", "delivered", 5)];
        assert_eq!(transition(&resolved, "firing", now), Some("firing"));
        assert_eq!(transition(&resolved, "ok", now), None);
        // a firing alert is reported once, and resolved once
        let firing = [row("firing", "delivered", 5)];
        assert_eq!(transition(&firing, "firing", now), None);
        assert_eq!(transition(&firing, "ok", now), Some("resolved"));
        // a firing that never got through still resolves when the reading drops
        let failed = [row("firing", "failed", 0)];
        assert_eq!(
            transition(&failed, "ok", Redeliver::Never),
            Some("resolved")
        );
    }

    #[test]
    fn a_state_that_was_not_delivered_is_retried_with_a_growing_delay() {
        let at = Redeliver::After(now());
        // the first failure is retried on the next pass, however soon
        assert_eq!(
            transition(&[row("firing", "failed", 0)], "firing", at),
            Some("firing")
        );
        // the second waits a pass
        let twice = [row("firing", "failed", 0), row("firing", "failed", 1)];
        assert_eq!(transition(&twice, "firing", at), None);
        let twice = [row("firing", "failed", 1), row("firing", "failed", 2)];
        assert_eq!(transition(&twice, "firing", at), Some("firing"));
        // a resolve that failed is retried the same way
        let resolve = [row("resolved", "failed", 0), row("firing", "delivered", 9)];
        assert_eq!(transition(&resolve, "ok", at), Some("resolved"));
        // one delivery ends the retries, and so does having no channel to use
        let delivered = [row("firing", "delivered", 0), row("firing", "failed", 1)];
        assert_eq!(transition(&delivered, "firing", at), None);
        assert_eq!(
            transition(&[row("firing", "failed", 90)], "firing", Redeliver::Never),
            None
        );
        // a transition skipped while the channel was off goes out once it is on
        assert_eq!(
            transition(&[row("firing", "skipped", 0)], "firing", at),
            Some("firing")
        );
        // evaluate now does not wait out the delay
        let many: Vec<Reported> = (0..10).map(|m| row("firing", "failed", m)).collect();
        assert_eq!(transition(&many, "firing", at), None);
        assert_eq!(transition(&many, "firing", Redeliver::Now), Some("firing"));
    }

    #[test]
    fn the_retry_delay_doubles_up_to_an_hour() {
        let minutes: Vec<i64> = (0..10).map(|f| retry_delay(f).num_minutes()).collect();
        assert_eq!(minutes, [0, 0, 1, 3, 7, 15, 31, 59, 59, 59]);
    }

    #[test]
    fn the_payload_carries_the_rule_the_reading_and_a_summary() {
        let rule = Rule {
            org_id: None,
            scope_type: None,
            scope_id: None,
            id: Uuid::nil(),
            name: "high error rate".into(),
            signal: "error_rate".into(),
            threshold: 0.05,
            comparison: "above".into(),
            no_data: "ignore".into(),
            window_secs: 300,
            channel_id: None,
            enabled: true,
            state: "firing".into(),
            last_value: Some(0.07),
            last_evaluated_at: None,
            last_error: None,
            last_error_code: None,
            last_error_params: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let body = payload(Uuid::nil(), &rule, "firing", false, "USD");
        assert_eq!(body["state"], "firing");
        assert_eq!(body["rule"]["name"], "high error rate");
        assert_eq!(body["signal"], "error_rate");
        assert_eq!(body["value"], 0.07);
        assert_eq!(body["threshold"], 0.05);
        assert_eq!(body["window_secs"], 300);
        assert_eq!(
            body["text"],
            "high error rate is firing: error_rate is 7%, threshold 5%, over the last 5 minutes"
        );
        assert_eq!(body["unit"], "fraction");
        assert!(body["currency"].is_null());
        let resolved = payload(Uuid::nil(), &rule, "resolved", false, "USD");
        assert!(resolved["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("high error rate resolved")));
    }

    /// Evaluation, delivery and history against a real schema, with a stub
    /// that answers as ClickHouse on `/` and as the webhook receiver on
    /// `/hook`.
    mod delivery {
        use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
        use std::sync::Arc;

        use axum::body::{to_bytes, Body};
        use axum::http::{HeaderMap, Request};
        use axum::response::{IntoResponse, Response};
        use rolter_store::postgres::test_schema::TestSchema;
        use serde_json::{json, Value};
        use tower::ServiceExt;

        use super::*;

        /// The webhook bearer secret, generated once per process rather than
        /// written out.
        fn bearer_secret() -> &'static str {
            static SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
            SECRET.get_or_init(|| format!("bearer-{}", uuid::Uuid::new_v4()))
        }

        /// One request the webhook receiver saw: its `Authorization` header and
        /// its JSON body.
        type Hook = (Option<String>, Value);

        #[derive(Clone)]
        struct Stub {
            /// The signal value the fake ClickHouse answers with, as `f64` bits.
            value: Arc<AtomicU64>,
            /// When set, the window has no requests: ClickHouse answers with a
            /// sample count of zero, as it does for an aggregate over no rows.
            empty: Arc<AtomicBool>,
            /// When non-zero, ClickHouse answers with this status instead.
            clickhouse_status: Arc<AtomicU16>,
            /// The status the webhook receiver answers with.
            hook_status: Arc<AtomicU16>,
            /// Every webhook request: its `Authorization` header and body.
            hooks: Arc<parking_lot::Mutex<Vec<Hook>>>,
            /// When set, the receiver holds each request until [`Self::release`]
            /// is notified, so a test can act while a delivery is in flight.
            holding: Arc<AtomicBool>,
            release: Arc<tokio::sync::Notify>,
            url: String,
        }

        impl Stub {
            async fn start() -> Self {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind a local port");
                let url = format!("http://{}", listener.local_addr().expect("a local address"));
                let stub = Self {
                    value: Arc::new(AtomicU64::new(0f64.to_bits())),
                    empty: Arc::default(),
                    clickhouse_status: Arc::new(AtomicU16::new(0)),
                    hook_status: Arc::new(AtomicU16::new(200)),
                    hooks: Arc::default(),
                    holding: Arc::default(),
                    release: Arc::default(),
                    url,
                };
                let app = Router::new()
                    .route("/", post(clickhouse))
                    .route("/hook", post(hook))
                    .route("/hold", post(hold))
                    .with_state(stub.clone());
                tokio::spawn(async move { axum::serve(listener, app).await });
                stub
            }

            fn read(&self, value: f64) {
                self.value.store(value.to_bits(), Ordering::SeqCst);
            }

            fn empty_window(&self, empty: bool) {
                self.empty.store(empty, Ordering::SeqCst);
            }

            fn hook_url(&self) -> String {
                format!("{}/hook", self.url)
            }

            fn hooks(&self) -> Vec<Hook> {
                self.hooks.lock().clone()
            }

            /// An endpoint that takes the request and never answers until
            /// [`Self::release`] is notified, like a receiver that black-holes
            fn hold_url(&self) -> String {
                format!("{}/hold", self.url)
            }
        }

        async fn hold(State(stub): State<Stub>, Json(body): Json<Value>) -> Response {
            stub.hooks.lock().push((None, body));
            stub.release.notified().await;
            StatusCode::OK.into_response()
        }

        async fn clickhouse(State(stub): State<Stub>) -> Response {
            let status = stub.clickhouse_status.load(Ordering::SeqCst);
            if status != 0 {
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    "Code: 60. DB::Exception: Table default.request_logs does not exist",
                )
                    .into_response();
            }
            let value = f64::from_bits(stub.value.load(Ordering::SeqCst));
            let samples = if stub.empty.load(Ordering::SeqCst) {
                0.0
            } else {
                25.0
            };
            Json(json!({ "data": [{ "value": value, "samples": samples }] })).into_response()
        }

        async fn hook(
            State(stub): State<Stub>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Response {
            let authorization = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            stub.hooks.lock().push((authorization, body));
            if stub.holding.load(Ordering::SeqCst) {
                stub.release.notified().await;
            }
            let status = StatusCode::from_u16(stub.hook_status.load(Ordering::SeqCst))
                .unwrap_or(StatusCode::OK);
            // a receiver that echoes what it was sent, which the history must not keep
            (status, format!("rejected {}", bearer_secret())).into_response()
        }

        /// A schema of its own per test; the guard drops it when the test ends.
        async fn scratch() -> Option<TestSchema> {
            let url = rolter_store::postgres::test_database::url()
                .await
                .or_else(|| {
                    eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
                    None
                })?;
            Some(TestSchema::migrated(&url).await)
        }

        fn state_with(db: &TestSchema, stub: &Stub) -> ControlState {
            let mut state = crate::test_state(db.pool().clone(), None, None);
            state.clickhouse = Some(crate::analytics::ClickHouseClient::new(&stub.url));
            state
        }

        fn kek() -> Kek {
            Kek::from_secret("alerting-test-kek")
        }

        async fn add_channel(db: &TestSchema, endpoint: &str, enabled: bool, secret: bool) -> Uuid {
            let sealed = secret.then(|| {
                kek()
                    .encrypt(bearer_secret())
                    .expect("seal the test secret")
            });
            sqlx::query_scalar(
                "insert into alert_channels (name, kind, endpoint, enabled, secret_ciphertext, secret_nonce) \
                 values ($1, 'webhook', $2, $3, $4, $5) returning id",
            )
            .bind(format!("channel-{}", Uuid::new_v4()))
            .bind(endpoint)
            .bind(enabled)
            .bind(sealed.as_ref().map(|(c, _)| c.clone()))
            .bind(sealed.as_ref().map(|(_, n)| n.clone()))
            .fetch_one(db.pool())
            .await
            .expect("insert a channel")
        }

        async fn add_rule(db: &TestSchema, channel_id: Option<Uuid>) -> Uuid {
            sqlx::query_scalar(
                "insert into alert_rules (name, signal, threshold, window_secs, channel_id, enabled) \
                 values ($1, 'error_rate', 0.5, 300, $2, true) returning id",
            )
            .bind(format!("rule-{}", Uuid::new_v4()))
            .bind(channel_id)
            .fetch_one(db.pool())
            .await
            .expect("insert a rule")
        }

        async fn add_rule_with(
            db: &TestSchema,
            signal: &str,
            comparison: &str,
            no_data: &str,
            threshold: f64,
        ) -> Uuid {
            sqlx::query_scalar(
                "insert into alert_rules (name, signal, threshold, comparison, no_data, window_secs, enabled) \
                 values ($1, $2, $3, $4, $5, 300, true) returning id",
            )
            .bind(format!("rule-{}", Uuid::new_v4()))
            .bind(signal)
            .bind(threshold)
            .bind(comparison)
            .bind(no_data)
            .fetch_one(db.pool())
            .await
            .expect("insert a rule")
        }

        async fn history(db: &TestSchema, rule_id: Uuid) -> Vec<(String, String, Option<String>)> {
            sqlx::query_as(
                "select state, delivery_status, detail from alert_notification_history \
                 where rule_id = $1 order by sent_at",
            )
            .bind(rule_id)
            .fetch_all(db.pool())
            .await
            .expect("read the history")
        }

        async fn rule_row(db: &TestSchema, id: Uuid) -> Rule {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {} from alert_rules where id=$1",
                rule_columns()
            )))
            .bind(id)
            .fetch_one(db.pool())
            .await
            .expect("read the rule")
        }

        async fn run(state: &ControlState, id: Uuid, kek: Option<&Kek>) -> Evaluation {
            evaluate_one(state, id, Lock::Wait, kek)
                .await
                .expect("the evaluation succeeds")
                .expect("the rule exists")
        }

        /// The API refuses a rule that names another org's channel, and the
        /// evaluator holds the line if a row got past it (#1829): the channel
        /// of another org is treated as no channel, so nothing is delivered
        /// and the transition is recorded as skipped.
        #[tokio::test]
        async fn a_channel_of_another_org_is_never_delivered_to_even_if_a_rule_names_it() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let mut orgs = Vec::new();
            for slug in ["mine", "theirs"] {
                let org: Uuid = sqlx::query_scalar(
                    "insert into orgs (name, slug) values ($1, $1) returning id",
                )
                .bind(slug)
                .fetch_one(db.pool())
                .await
                .expect("insert an org");
                orgs.push(org);
            }
            let theirs = add_channel(&db, &stub.hook_url(), true, false).await;
            sqlx::query("update alert_channels set org_id = $2 where id = $1")
                .bind(theirs)
                .bind(orgs[1])
                .execute(db.pool())
                .await
                .expect("give the channel an owner");
            // the rule belongs to the other org, and the channel is put on it by
            // SQL, the way the API would never allow
            let id = add_rule(&db, Some(theirs)).await;
            sqlx::query(
                "update alert_rules set org_id = $2, scope_type = 'org', scope_id = $2 where id = $1",
            )
            .bind(id)
            .bind(orgs[0])
            .execute(db.pool())
            .await
            .expect("scope the rule to the first org");

            stub.read(0.9);
            let evaluation = run(&state, id, None).await;
            assert_eq!(evaluation.rule.state, "firing");
            assert!(!evaluation.notified);
            assert!(
                stub.hooks().is_empty(),
                "nothing reached the other org's channel"
            );
            let rows = history(&db, id).await;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, "skipped");
        }

        /// #3361: the rules read for a scoped caller are chosen by their reach in
        /// SQL, so another org's rules are never loaded, however many there are.
        #[tokio::test]
        async fn only_the_rules_inside_the_reach_are_read() {
            let Some(db) = scratch().await else { return };
            let pool = db.pool();
            let mut orgs = Vec::new();
            for slug in ["mine", "theirs"] {
                let org: Uuid = sqlx::query_scalar(
                    "insert into orgs (name, slug) values ($1, $1) returning id",
                )
                .bind(slug)
                .fetch_one(pool)
                .await
                .expect("insert an org");
                let team: Uuid = sqlx::query_scalar(
                    "insert into teams (org_id, name) values ($1, 'core') returning id",
                )
                .bind(org)
                .fetch_one(pool)
                .await
                .expect("insert a team");
                let project: Uuid = sqlx::query_scalar(
                    "insert into projects (team_id, name) values ($1, 'app') returning id",
                )
                .bind(team)
                .fetch_one(pool)
                .await
                .expect("insert a project");
                orgs.push((org, team, project));
            }
            let add = |name: String, org: Uuid, scope: (&'static str, Uuid)| {
                sqlx::query(
                    "insert into alert_rules (name, signal, threshold, window_secs, org_id, scope_type, scope_id) \
                     values ($1, 'error_rate', 0.5, 300, $2, $3, $4)",
                )
                .bind(name)
                .bind(org)
                .bind(scope.0)
                .bind(scope.1)
                .execute(pool)
            };
            let (org_a, team_a, project_a) = orgs[0];
            let (org_b, team_b, project_b) = orgs[1];
            add("a org".into(), org_a, ("org", org_a)).await.unwrap();
            add("a team".into(), org_a, ("team", team_a)).await.unwrap();
            add("a project".into(), org_a, ("project", project_a))
                .await
                .unwrap();
            for i in 0..25 {
                add(format!("b project {i}"), org_b, ("project", project_b))
                    .await
                    .unwrap();
            }
            add("b team".into(), org_b, ("team", team_b)).await.unwrap();
            // a deployment rule belongs to nobody's reach
            add_rule(&db, None).await;

            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let names = |rules: Vec<Rule>| -> Vec<String> {
                rules.into_iter().map(|rule| rule.name).collect()
            };
            // an org admin reads their org's rules and nothing else
            let rules = rules_in_reach(&state, &[ScopeChain::org(org_a)])
                .await
                .unwrap();
            assert_eq!(names(rules), ["a org", "a project", "a team"]);
            // a team admin reads the team's, and the projects inside it
            let team = ScopeChain {
                org: Some(org_a),
                team: Some(team_a),
                project: None,
            };
            let rules = rules_in_reach(&state, &[team]).await.unwrap();
            assert_eq!(names(rules), ["a project", "a team"]);
            // a project admin reads that project's alone
            let project = ScopeChain {
                org: Some(org_a),
                team: Some(team_a),
                project: Some(project_a),
            };
            let rules = rules_in_reach(&state, &[project]).await.unwrap();
            assert_eq!(names(rules), ["a project"]);
            // and no reach reads nothing
            assert!(rules_in_reach(&state, &[]).await.unwrap().is_empty());
        }

        #[tokio::test]
        async fn a_transition_is_posted_with_the_secret_and_recorded_delivered() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;
            let kek = kek();

            stub.read(0.9);
            let first = run(&state, id, Some(&kek)).await;
            assert!(first.notified);
            assert_eq!(first.rule.state, "firing");
            let notification = first.notification.expect("a history row");
            assert_eq!(notification.delivery_status, "delivered");
            assert_eq!(notification.detail.as_deref(), Some("HTTP 200"));
            assert_eq!(notification.channel_id, Some(channel));
            assert_eq!(notification.detail_code.as_deref(), Some("http_ok"));
            assert_eq!(notification.detail_params, Some(json!({"status": 200})));
            assert_eq!(notification.value, Some(0.9));
            assert_eq!(notification.threshold, Some(0.5));

            let hooks = stub.hooks();
            assert_eq!(hooks.len(), 1);
            let (authorization, body) = &hooks[0];
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {}", bearer_secret()).as_str())
            );
            assert_eq!(body["id"], json!(notification.id));
            assert_eq!(body["state"], "firing");
            assert_eq!(body["rule"]["id"], json!(id));
            assert_eq!(body["signal"], "error_rate");
            assert_eq!(body["value"], 0.9);
            assert_eq!(body["threshold"], 0.5);
            assert_eq!(body["window_secs"], 300);

            // still firing: nothing new is sent or recorded
            let again = run(&state, id, Some(&kek)).await;
            assert!(!again.notified);
            assert!(again.notification.is_none());
            assert_eq!(stub.hooks().len(), 1);

            stub.read(0.1);
            let resolved = run(&state, id, Some(&kek)).await;
            assert!(resolved.notified);
            assert_eq!(resolved.rule.state, "ok");
            assert_eq!(stub.hooks()[1].1["state"], "resolved");
            assert_eq!(
                history(&db, id).await,
                vec![
                    ("firing".into(), "delivered".into(), Some("HTTP 200".into())),
                    (
                        "resolved".into(),
                        "delivered".into(),
                        Some("HTTP 200".into())
                    ),
                ]
            );
        }

        async fn status_of(db: &TestSchema, rule_id: Uuid) -> Vec<String> {
            history(db, rule_id)
                .await
                .into_iter()
                .map(|(_, status, _)| status)
                .collect()
        }

        /// Wait for `rule_id`'s history to read `expected`, or fail
        async fn settles_to(db: &TestSchema, rule_id: Uuid, expected: &[&str]) {
            for _ in 0..100 {
                if status_of(db, rule_id).await == expected {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!(
                "history of {rule_id} is {:?}, wanted {expected:?}",
                status_of(db, rule_id).await
            );
        }

        /// #1967: a receiver that black-holes its connection used to hold the
        /// pass on that rule for the whole delivery timeout, and every rule
        /// after it waited too.
        #[tokio::test]
        async fn a_slow_receiver_does_not_hold_up_the_next_rule_in_the_pass() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let slow_channel = add_channel(&db, &stub.hold_url(), true, false).await;
            let fast_channel = add_channel(&db, &stub.hook_url(), true, false).await;
            // evaluated in name order, so the slow rule goes first
            let slow = add_rule(&db, Some(slow_channel)).await;
            let fast = add_rule(&db, Some(fast_channel)).await;
            for (id, name) in [(slow, "a-slow"), (fast, "b-fast")] {
                sqlx::query("update alert_rules set name=$2 where id=$1")
                    .bind(id)
                    .bind(name)
                    .execute(db.pool())
                    .await
                    .expect("name the rule");
            }

            stub.read(0.9);
            tokio::time::timeout(std::time::Duration::from_secs(5), evaluate_enabled(&state))
                .await
                .expect("the pass must not wait on the slow receiver")
                .expect("the pass succeeds");

            settles_to(&db, fast, &["delivered"]).await;
            // both were evaluated; the slow one is still being sent
            assert_eq!(rule_row(&db, slow).await.state, "firing");
            assert_eq!(rule_row(&db, fast).await.state, "firing");
            assert_eq!(status_of(&db, slow).await, ["pending"]);

            // a later pass finds the delivery in flight and does not send again
            evaluate_enabled(&state).await.expect("the second pass");
            assert_eq!(status_of(&db, slow).await, ["pending"]);
            assert_eq!(
                stub.hooks()
                    .iter()
                    .filter(|(_, body)| body["rule"]["name"] == "a-slow")
                    .count(),
                1,
                "the transition was sent twice"
            );

            // and the slow one settles once its receiver answers
            stub.release.notify_one();
            settles_to(&db, slow, &["delivered"]).await;
        }

        /// #1967: the transaction used to stay open for the whole delivery, so a
        /// database that closes idle transactions killed the commit, wrote no
        /// history and sent the same alert again on the next pass.
        #[tokio::test]
        async fn a_session_that_closes_idle_transactions_early_does_not_resend() {
            let Some(db) = scratch().await else { return };
            let url = rolter_store::postgres::test_database::url()
                .await
                .expect("configured, or scratch() returned above");
            let stub = Stub::start().await;
            // every connection of this pool is killed 200 ms into an idle
            // transaction, well inside the 700 ms the receiver takes
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&format!(
                    "{}%20-c%20idle_in_transaction_session_timeout%3D200",
                    rolter_store::postgres::test_schema::with_search_path(&url, db.schema())
                ))
                .await
                .expect("connect with a short idle-in-transaction timeout");
            let mut state = state_with(&db, &stub);
            state.pool = Some(pool);
            let channel = add_channel(&db, &stub.hold_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            let release = stub.release.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                release.notify_one();
            });
            let evaluation = run(&state, id, None).await;
            assert!(evaluation.notified);
            assert_eq!(evaluation.rule.state, "firing");
            assert_eq!(status_of(&db, id).await, ["delivered"]);

            // the next pass has nothing to send
            let again = run(&state, id, None).await;
            assert!(again.notification.is_none());
            assert_eq!(stub.hooks().len(), 1);
        }

        #[tokio::test]
        async fn a_rule_saved_before_the_new_fields_evaluates_exactly_as_before() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            // the original insert, which names neither column
            let id = add_rule(&db, None).await;
            let row = rule_row(&db, id).await;
            assert_eq!(
                (row.comparison.as_str(), row.no_data.as_str()),
                ("above", "ignore")
            );

            stub.read(0.9);
            assert_eq!(run(&state, id, None).await.rule.state, "firing");
            // an empty window with the default policy leaves the state alone
            stub.read(0.0);
            stub.empty_window(true);
            let held = run(&state, id, None).await;
            assert_eq!(held.rule.state, "firing");
            assert!(held.notification.is_none());
            assert_eq!(held.rule.last_value, None);
            stub.empty_window(false);
            assert_eq!(run(&state, id, None).await.rule.state, "ok");
        }

        #[tokio::test]
        async fn a_zero_request_window_fires_a_no_data_rule_and_resolves_when_traffic_returns() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule_with(&db, "error_rate", "above", "fire", 0.5).await;
            sqlx::query("update alert_rules set channel_id=$2 where id=$1")
                .bind(id)
                .bind(channel)
                .execute(db.pool())
                .await
                .expect("attach the channel");

            // the query reads 0, which a rule that only compared values would
            // call healthy
            stub.read(0.0);
            stub.empty_window(true);
            let fired = run(&state, id, None).await;
            assert_eq!(fired.rule.state, "firing");
            assert_eq!(fired.rule.last_value, None);
            let notification = fired.notification.expect("a history row");
            assert_eq!(notification.state, "firing");
            assert_eq!(
                notification.detail.as_deref(),
                Some("HTTP 200; no data in window")
            );
            let hooks = stub.hooks();
            assert_eq!(hooks.len(), 1);
            assert_eq!(hooks[0].1["no_data"], true);
            assert!(hooks[0].1["value"].is_null());

            // traffic comes back healthy
            stub.empty_window(false);
            stub.read(0.01);
            let resolved = run(&state, id, None).await;
            assert_eq!(resolved.rule.state, "ok");
            assert_eq!(stub.hooks()[1].1["state"], "resolved");
            assert_eq!(stub.hooks()[1].1["no_data"], false);
        }

        #[tokio::test]
        async fn no_data_ok_resolves_a_firing_rule() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let id = add_rule_with(&db, "p95_latency_ms", "above", "ok", 100.0).await;
            stub.read(900.0);
            assert_eq!(run(&state, id, None).await.rule.state, "firing");
            stub.empty_window(true);
            let resolved = run(&state, id, None).await;
            assert_eq!(resolved.rule.state, "ok");
            assert_eq!(
                resolved.notification.expect("a resolved row").state,
                "resolved"
            );
        }

        #[tokio::test]
        async fn a_below_rule_on_request_volume_catches_stopped_traffic() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let id = add_rule_with(&db, "request_volume", "below", "ignore", 0.0).await;
            stub.read(340.0);
            assert_eq!(run(&state, id, None).await.rule.state, "ok");
            // no requests is a count of 0, a real reading for this signal
            stub.read(0.0);
            stub.empty_window(true);
            assert_eq!(run(&state, id, None).await.rule.state, "firing");
        }

        #[tokio::test]
        async fn the_rule_api_round_trips_comparison_and_no_data() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let id = add_rule(&db, None).await;
            let (status, body) = put(
                &state,
                format!("/api/v1/alert-rules/{id}"),
                json!({"name": "stopped", "signal": "error_rate", "threshold": 0.5,
                       "window_secs": 300, "comparison": "below", "no_data": "fire"}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["comparison"], "below");
            assert_eq!(body["no_data"], "fire");
            // an omitted field keeps the stored value, so an editor that
            // predates the fields cannot reset them on an unrelated edit
            let (status, body) = put(
                &state,
                format!("/api/v1/alert-rules/{id}"),
                json!({"name": "renamed", "signal": "error_rate", "threshold": 0.4,
                       "window_secs": 300}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["name"], "renamed");
            assert_eq!(body["comparison"], "below");
            assert_eq!(body["no_data"], "fire");
            // moved to a signal that cannot lack data, a kept `fire` would
            // never apply, so the omitted policy is written `ignore`
            let (status, body) = put(
                &state,
                format!("/api/v1/alert-rules/{id}"),
                json!({"name": "renamed", "signal": "request_volume", "threshold": 1,
                       "window_secs": 300}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["comparison"], "below");
            assert_eq!(body["no_data"], "ignore");
            let (status, _) = put(
                &state,
                format!("/api/v1/alert-rules/{id}"),
                json!({"name": "stopped", "signal": "request_volume", "threshold": 1,
                       "window_secs": 300, "no_data": "fire"}),
            )
            .await;
            assert!(status.is_client_error(), "{status}");
        }

        async fn scheduled(
            state: &ControlState,
            id: Uuid,
            kek: Option<&Kek>,
        ) -> Option<Evaluation> {
            evaluate_one(state, id, Lock::SkipLocked, kek)
                .await
                .expect("the evaluation succeeds")
        }

        /// Move a rule's history back in time, as if the passes that wrote it
        /// ran that long ago.
        async fn age_history(db: &TestSchema, rule_id: Uuid, minutes: i32) {
            sqlx::query(
                "update alert_notification_history \
                 set sent_at = sent_at - make_interval(mins => $2) where rule_id = $1",
            )
            .bind(rule_id)
            .bind(minutes)
            .execute(db.pool())
            .await
            .expect("age the history");
        }

        async fn put(state: &ControlState, uri: String, body: Value) -> (StatusCode, Value) {
            let response = router()
                .with_state(state.clone())
                .oneshot(
                    Request::put(uri)
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            let status = response.status();
            let body = to_bytes(response.into_body(), 1 << 16)
                .await
                .expect("a body");
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }

        #[tokio::test]
        async fn a_rejected_delivery_is_recorded_failed_and_retried_until_it_gets_through() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;
            let kek = kek();
            stub.hook_status.store(500, Ordering::SeqCst);

            stub.read(0.9);
            let evaluation = run(&state, id, Some(&kek)).await;
            assert!(!evaluation.notified);
            // the evaluation itself succeeded, so the rule's state still moves
            assert_eq!(evaluation.rule.state, "firing");
            assert_eq!(evaluation.rule.last_error, None);
            assert_eq!(
                history(&db, id).await,
                vec![("firing".into(), "failed".into(), Some("HTTP 500".into()))]
            );

            // the next pass tries again, whatever the time
            scheduled(&state, id, Some(&kek)).await;
            assert_eq!(stub.hooks().len(), 2);
            // after a second failure the pass a moment later waits
            scheduled(&state, id, Some(&kek)).await;
            assert_eq!(stub.hooks().len(), 2);
            assert_eq!(history(&db, id).await.len(), 2);

            // two minutes on, the receiver is back
            age_history(&db, id, 2).await;
            stub.hook_status.store(200, Ordering::SeqCst);
            let retried = scheduled(&state, id, Some(&kek)).await.expect("evaluated");
            assert!(retried.notified);
            let hooks = stub.hooks();
            assert_eq!(hooks.len(), 3);
            assert_eq!(hooks[2].1["state"], "firing");
            assert_eq!(
                hooks[2].0.as_deref(),
                Some(format!("Bearer {}", bearer_secret()).as_str())
            );

            // delivered once: neither the schedule nor evaluate now sends it again
            age_history(&db, id, 120).await;
            scheduled(&state, id, Some(&kek)).await;
            run(&state, id, Some(&kek)).await;
            assert_eq!(stub.hooks().len(), 3);
            let rows: Vec<String> = history(&db, id)
                .await
                .into_iter()
                .map(|(_, status, _)| status)
                .collect();
            assert_eq!(rows, ["failed", "failed", "delivered"]);
        }

        #[tokio::test]
        async fn a_transition_skipped_while_the_channel_was_off_goes_out_once_it_is_on() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), false, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            scheduled(&state, id, None).await;
            // still off: nothing to retry with, so nothing new is written
            scheduled(&state, id, None).await;
            assert_eq!(history(&db, id).await.len(), 1);

            sqlx::query("update alert_channels set enabled=true where id=$1")
                .bind(channel)
                .execute(db.pool())
                .await
                .expect("enable the channel");
            let evaluation = scheduled(&state, id, None).await.expect("evaluated");
            assert!(evaluation.notified);
            assert_eq!(stub.hooks().len(), 1);
            assert_eq!(
                history(&db, id).await,
                vec![
                    (
                        "firing".into(),
                        "skipped".into(),
                        Some("channel disabled".into())
                    ),
                    ("firing".into(), "delivered".into(), Some("HTTP 200".into())),
                ]
            );
        }

        #[tokio::test]
        async fn an_older_reading_does_not_overwrite_a_newer_one() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;
            // another replica took a reading after this one started and has
            // already recorded it
            sqlx::query(
                "update alert_rules set state='ok', last_value=0.1, \
                 last_evaluated_at = clock_timestamp() + interval '1 hour' where id=$1",
            )
            .bind(id)
            .execute(db.pool())
            .await
            .expect("record a newer reading");

            stub.read(0.9);
            let evaluation = run(&state, id, None).await;
            assert!(evaluation.notification.is_none());
            assert_eq!(evaluation.rule.state, "ok");
            assert_eq!(evaluation.rule.last_value, Some(0.1));
            assert!(stub.hooks().is_empty());
            assert!(history(&db, id).await.is_empty());
        }

        /// The delivery runs after the transaction, so a channel delete no
        /// longer queues behind it (#1967); it must also still not deadlock
        /// with the evaluation, and the delivery still settles its row.
        #[tokio::test]
        async fn deleting_a_channel_during_a_delivery_neither_waits_nor_deadlocks() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;
            stub.read(0.9);
            stub.holding.store(true, Ordering::SeqCst);

            let evaluation = tokio::spawn({
                let state = state.clone();
                async move { evaluate_one(&state, id, Lock::Wait, None).await }
            });
            // the receiver has the request and is holding it
            tokio::time::timeout(Duration::from_secs(10), async {
                while stub.hooks().is_empty() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the delivery reaches the receiver");

            tokio::time::timeout(
                Duration::from_secs(5),
                sqlx::query("delete from alert_channels where id=$1")
                    .bind(channel)
                    .execute(db.pool()),
            )
            .await
            .expect("the delete does not wait for the delivery")
            .expect("the delete is not a deadlock victim");
            stub.release.notify_one();

            let evaluation = evaluation
                .await
                .expect("the evaluation task")
                .expect("the evaluation is not a deadlock victim")
                .expect("the rule exists");
            assert!(evaluation.notified);
            assert_eq!(
                history(&db, id).await,
                vec![("firing".into(), "delivered".into(), Some("HTTP 200".into()))]
            );
            assert_eq!(rule_row(&db, id).await.channel_id, None);
        }

        #[tokio::test]
        async fn a_repointed_channel_drops_its_secret_unless_given_a_new_one() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let uri = format!("/api/v1/alert-channels/{channel}");

            // another path on the same origin keeps it
            let (status, body) = put(
                &state,
                uri.clone(),
                json!({"name": "ops", "endpoint": format!("{}/other", stub.url), "enabled": true}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["secret_configured"], true);

            // another origin does not
            let (status, body) = put(
                &state,
                uri,
                json!({"name": "ops", "endpoint": "HTTPS://Alerts.Example.com/rolter", "enabled": true}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["secret_configured"], false);
            // stored as the client parses it
            assert_eq!(body["endpoint"], "https://alerts.example.com/rolter");
            let detail: Value = sqlx::query_scalar(
                "select detail from audit_log where action = 'alert.channel.update' \
                 and target_id = $1 order by at desc limit 1",
            )
            .bind(channel)
            .fetch_one(db.pool())
            .await
            .expect("an audit entry");
            assert_eq!(detail["endpoint_changed"], true);
            assert_eq!(detail["secret_cleared"], true);
            assert!(!detail.to_string().contains("alerts.example.com"));
        }

        #[tokio::test]
        async fn a_channel_the_policy_now_denies_can_still_be_switched_off() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            // saved while loopback was allowed
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            state.egress = Arc::new(EgressPolicy {
                block_loopback: true,
                ..EgressPolicy::default()
            });
            let uri = format!("/api/v1/alert-channels/{channel}");
            let endpoint = stub.hook_url();

            let (status, body) = put(
                &state,
                uri.clone(),
                json!({"name": "ops", "endpoint": endpoint, "enabled": false}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["enabled"], false);
            // but not switched back on, or pointed at another denied address
            let (status, _) = put(
                &state,
                uri.clone(),
                json!({"name": "ops", "endpoint": endpoint, "enabled": true}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let (status, _) = put(
                &state,
                uri,
                json!({"name": "ops", "endpoint": "http://2130706433/hook", "enabled": false}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn an_unreachable_endpoint_is_recorded_failed() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            // a port that was listening a moment ago and no longer is
            let closed = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
                listener.local_addr().expect("an address")
            };
            let channel = add_channel(&db, &format!("http://{closed}/hook"), true, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            run(&state, id, None).await;
            assert_eq!(
                history(&db, id).await,
                vec![(
                    "firing".into(),
                    "failed".into(),
                    Some("could not connect to the endpoint".into())
                )]
            );
        }

        #[tokio::test]
        async fn an_endpoint_the_egress_policy_denies_is_never_contacted() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            // saved while loopback was allowed, delivered after it was denied
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;
            state.egress = Arc::new(EgressPolicy {
                block_loopback: true,
                ..EgressPolicy::default()
            });

            stub.read(0.9);
            run(&state, id, None).await;
            assert!(stub.hooks().is_empty());
            assert_eq!(
                history(&db, id).await,
                vec![(
                    "firing".into(),
                    "failed".into(),
                    Some("endpoint denied by the egress policy".into())
                )]
            );
        }

        #[tokio::test]
        async fn a_secret_that_cannot_be_unsealed_is_not_sent_without_it() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            // no key at all, then the wrong one
            run(&state, id, None).await;
            stub.read(0.1);
            run(&state, id, Some(&Kek::from_secret("not-the-key"))).await;
            assert!(stub.hooks().is_empty());
            let rows = history(&db, id).await;
            assert_eq!(rows.len(), 2);
            for (_, status, detail) in rows {
                assert_eq!(status, "failed");
                assert!(detail.is_some_and(|d| d.contains(KEK_ENV)));
            }
        }

        #[tokio::test]
        async fn transitions_without_a_live_channel_are_recorded_skipped() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let no_channel = add_rule(&db, None).await;
            let disabled = add_channel(&db, &stub.hook_url(), false, false).await;
            let to_disabled = add_rule(&db, Some(disabled)).await;

            stub.read(0.9);
            run(&state, no_channel, None).await;
            run(&state, to_disabled, None).await;
            assert!(stub.hooks().is_empty());
            assert_eq!(
                history(&db, no_channel).await,
                vec![(
                    "firing".into(),
                    "skipped".into(),
                    Some("no channel configured".into())
                )]
            );
            assert_eq!(
                history(&db, to_disabled).await,
                vec![(
                    "firing".into(),
                    "skipped".into(),
                    Some("channel disabled".into())
                )]
            );
        }

        #[tokio::test]
        async fn a_failed_evaluation_sets_error_and_the_alert_survives_it() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            run(&state, id, None).await;
            assert_eq!(stub.hooks().len(), 1);

            // clickhouse answers with an error: the rule says so, and keeps
            // the last value it read
            stub.clickhouse_status.store(500, Ordering::SeqCst);
            let error = evaluate_one(&state, id, Lock::Wait, None).await;
            assert!(error.is_err());
            let errored = rule_row(&db, id).await;
            assert_eq!(errored.state, "error");
            assert_eq!(errored.last_value, Some(0.9));
            let last_error = errored.last_error.expect("last_error is set");
            assert_eq!(last_error, "analytics query failed");
            assert_eq!(errored.last_error_code.as_deref(), Some("clickhouse_error"));

            // no clickhouse configured at all
            state.clickhouse = None;
            assert!(evaluate_one(&state, id, Lock::Wait, None).await.is_err());
            let errored = rule_row(&db, id).await;
            assert_eq!(
                errored.last_error.as_deref(),
                Some("alert evaluation requires CLICKHOUSE_URL")
            );
            assert_eq!(
                errored.last_error_code.as_deref(),
                Some("clickhouse_not_configured")
            );

            // back, and still high: the alert was already reported
            state.clickhouse = Some(crate::analytics::ClickHouseClient::new(&stub.url));
            stub.clickhouse_status.store(0, Ordering::SeqCst);
            let recovered = run(&state, id, None).await;
            assert_eq!(recovered.rule.state, "firing");
            assert_eq!(recovered.rule.last_error, None);
            assert_eq!(recovered.rule.last_error_code, None);
            assert!(recovered.notification.is_none());
            assert_eq!(stub.hooks().len(), 1);

            // an error between firing and a low reading still resolves
            stub.clickhouse_status.store(500, Ordering::SeqCst);
            let _ = evaluate_one(&state, id, Lock::Wait, None).await;
            stub.clickhouse_status.store(0, Ordering::SeqCst);
            stub.read(0.1);
            let resolved = run(&state, id, None).await;
            assert!(resolved.notified);
            assert_eq!(stub.hooks()[1].1["state"], "resolved");
            // error passes write no history of their own
            assert_eq!(history(&db, id).await.len(), 2);
        }

        #[tokio::test]
        async fn a_stalled_clickhouse_is_an_evaluation_error_not_a_hang() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            let id = add_rule(&db, None).await;
            let stalled = crate::analytics::testing::Stalled::start().await;
            state.clickhouse = Some(crate::analytics::ClickHouseClient::with_timeouts(
                &stalled.url,
                Duration::from_secs(1),
                Duration::from_millis(300),
            ));

            let outcome = tokio::time::timeout(
                Duration::from_secs(10),
                evaluate_one(&state, id, Lock::Wait, None),
            )
            .await
            .expect("a stalled signal read must not hang the pass");
            assert!(outcome.is_err());
            let errored = rule_row(&db, id).await;
            assert_eq!(errored.state, "error");
            let last_error = errored.last_error.expect("last_error is set");
            assert_eq!(last_error, "analytics query timed out");
        }

        #[tokio::test]
        async fn a_rule_another_pass_holds_is_left_to_that_pass() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let id = add_rule(&db, None).await;
            stub.read(0.9);

            // a second replica mid-evaluation holds the row
            let mut other = db.pool().begin().await.expect("begin");
            sqlx::query("select id from alert_rules where id=$1 for update")
                .bind(id)
                .execute(&mut *other)
                .await
                .expect("lock the rule");
            let skipped = evaluate_one(&state, id, Lock::SkipLocked, None)
                .await
                .expect("a held rule is not an error");
            assert!(skipped.is_none());
            other.rollback().await.expect("release the rule");
            assert!(history(&db, id).await.is_empty());

            let evaluated = evaluate_one(&state, id, Lock::SkipLocked, None)
                .await
                .expect("the evaluation succeeds");
            assert!(evaluated.is_some());
            assert_eq!(history(&db, id).await.len(), 1);

            // the scheduled pass leaves a disabled rule alone
            sqlx::query("update alert_rules set enabled=false where id=$1")
                .bind(id)
                .execute(db.pool())
                .await
                .expect("disable the rule");
            assert!(evaluate_one(&state, id, Lock::SkipLocked, None)
                .await
                .expect("a disabled rule is not an error")
                .is_none());
        }

        #[tokio::test]
        async fn evaluate_now_reports_a_failure_and_records_it_on_the_rule() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            state.clickhouse = None;
            let id = add_rule(&db, None).await;
            let app = router().with_state(state);

            let response = app
                .oneshot(
                    Request::post(format!("/api/v1/alert-rules/{id}/evaluate"))
                        .body(Body::empty())
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = to_bytes(response.into_body(), 1 << 16)
                .await
                .expect("a body");
            let body: Value = serde_json::from_slice(&body).expect("json");
            assert!(body["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("CLICKHOUSE_URL")));
            assert_eq!(body["error"]["code"], "clickhouse_not_configured");
            assert_eq!(rule_row(&db, id).await.state, "error");
        }

        async fn get(state: &ControlState, uri: &str) -> (StatusCode, Value) {
            let response = router()
                .with_state(state.clone())
                .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
                .await
                .expect("the router answers");
            let status = response.status();
            let body = to_bytes(response.into_body(), 1 << 20)
                .await
                .expect("a body");
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }

        fn ids(page: &Value) -> Vec<String> {
            page.as_array()
                .expect("an array")
                .iter()
                .map(|row| row["id"].as_str().expect("an id").to_string())
                .collect()
        }

        #[tokio::test]
        async fn the_history_pages_by_keyset_and_filters_by_state_and_delivery() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let rule = add_rule(&db, None).await;
            let other = add_rule(&db, None).await;
            // five rows share one sent_at, so the cursor has to tie-break on id
            let mut all = Vec::new();
            for (n, (rule_id, change, status)) in [
                (rule, "firing", "failed"),
                (rule, "firing", "delivered"),
                (rule, "resolved", "delivered"),
                (other, "firing", "failed"),
                (rule, "firing", "skipped"),
            ]
            .into_iter()
            .enumerate()
            {
                let id: Uuid = sqlx::query_scalar(
                    "insert into alert_notification_history (rule_id, state, delivery_status, sent_at) \
                     values ($1, $2, $3, '2026-01-01T00:00:00Z'::timestamptz + ($4::int / 3) * interval '1 minute') \
                     returning id",
                )
                .bind(rule_id)
                .bind(change)
                .bind(status)
                .bind(n as i32)
                .fetch_one(db.pool())
                .await
                .expect("insert a row");
                all.push(id.to_string());
            }

            let (status, full) = get(&state, "/api/v1/alert-notifications").await;
            assert_eq!(status, StatusCode::OK);
            let newest_first = ids(&full);
            assert_eq!(newest_first.len(), 5);

            // walk it two at a time: every row exactly once, in the same order
            let mut walked = Vec::new();
            let mut uri = "/api/v1/alert-notifications?limit=2".to_string();
            loop {
                let (_, page) = get(&state, &uri).await;
                let rows = page.as_array().expect("an array");
                walked.extend(ids(&page));
                let Some(last) = rows.last().filter(|_| rows.len() == 2) else {
                    break;
                };
                uri = format!(
                    "/api/v1/alert-notifications?limit=2&before={},{}",
                    last["sent_at"].as_str().expect("sent_at"),
                    last["id"].as_str().expect("id"),
                );
            }
            assert_eq!(walked, newest_first);

            let (_, failed) =
                get(&state, "/api/v1/alert-notifications?delivery_status=failed").await;
            assert_eq!(ids(&failed).len(), 2);
            let (_, resolved) = get(&state, "/api/v1/alert-notifications?state=resolved").await;
            assert_eq!(ids(&resolved).len(), 1);
            let (_, both) = get(
                &state,
                &format!("/api/v1/alert-notifications?rule_id={rule}&state=firing&delivery_status=failed"),
            )
            .await;
            assert_eq!(ids(&both).len(), 1);

            for bad in ["state=paused", "delivery_status=lost", "before=yesterday"] {
                let (status, body) =
                    get(&state, &format!("/api/v1/alert-notifications?{bad}")).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
                assert_eq!(body["error"]["code"], "invalid_field", "{bad}");
            }
        }
    }
}
