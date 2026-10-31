//! The savings advisor (#1581): invocation history re-priced against the
//! cheaper models already configured in the same route's pool.
//!
//! `GET /api/v1/analytics/savings` answers three questions for a window and a
//! scope (org, project, route, virtual key):
//!
//! * **Where the money went**: spend by route, by upstream model and by
//!   caller, with the part spent on requests that took more than one upstream
//!   attempt (retries and failovers), on requests that failed, and on response
//!   cache misses called out separately.
//! * **What was substitutable**: requests whose recorded shape (a successful,
//!   token-billed model call under the prompt and output size thresholds) makes
//!   them candidates for another model in the same route's pool.
//! * **The projected delta**: what those requests would have cost at each
//!   candidate's price, as a range.
//!
//! It re-prices and claims nothing else. Whether a cheaper model answers as
//! well is a question only a test against the operator's own traffic can
//! settle, so the report is a shortlist to test and never a switch, and its
//! `limits` say so in the response itself. A request whose price is unknown is
//! excluded and counted, never priced at zero, and so is a candidate whose
//! price is unknown.
//!
//! The rows are scoped exactly as every other analytics read (#1820): the
//! caller's row visibility is bound into the query, and the scope parameters
//! narrow inside it, never past it. Candidates come only from the configured
//! pool of a route the visible rows were served by, so the report can name no
//! model the caller's own traffic could not already reach.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rolter_core::{CurrencyConverter, GatewayConfig, ModelPriceConfig, StaticRates};
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::analytics::{
    client_or_503, query_failed, row_filter_params, window_params, with_access, WindowQuery, COST,
    ERROR_STATUS, REQUESTS, ROW_FILTERS, UNPRICED, WHERE_WINDOW,
};
use crate::analytics_access::{AnalyticsAccess, SavingsAccess, ROW_VISIBLE};
use crate::request_log_columns::Columns;
use crate::time_bounds::{Query, TimeBounds};
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/analytics/savings", get(savings))
}

/// The prompt size above which a request is not counted as substitutable,
/// unless the caller sets `max_prompt_tokens`.
const DEFAULT_MAX_PROMPT_TOKENS: u32 = 4_000;
/// The output size above which a request is not counted as substitutable,
/// unless the caller sets `max_completion_tokens`.
const DEFAULT_MAX_COMPLETION_TOKENS: u32 = 1_000;
/// How much longer a candidate's answers are assumed to run in the
/// conservative end of the range, unless the caller sets `output_margin`.
const DEFAULT_OUTPUT_MARGIN: f64 = 0.25;
/// The largest `output_margin` honoured; a larger one is clamped to it.
const MAX_OUTPUT_MARGIN: f64 = 4.0;
/// How many callers the spend breakdown names, most expensive first.
const CALLER_LIMIT: u32 = 50;

/// What the report says about itself, on every answer.
const LIMITS: &[&str] = &[
    "This report re-prices recorded traffic at another model's price. It does not evaluate \
     whether that model answers as well, so it is a shortlist to test, never a recommendation \
     to switch.",
    "Only models already configured in the same route's pool are considered.",
    "A request or a candidate whose price is unknown is excluded and counted, never priced at \
     zero.",
    "Counts and costs are scaled by the request log's sampling weight, so under sampling they \
     are estimates.",
];

/// What the report adds to its `limits` while the request log cannot say what
/// a request asked for: the table lacks `clickhouse/031_request_shape.sql`, so
/// no row carries the shape flags the `uses_tools`, `image_input`,
/// `audio_input`, `structured_output` and `multi_turn` exclusions read (#3317).
const NO_SHAPE_LIMIT: &str = "The request log does not record whether a request carried tools, \
     images, audio, several turns or a structured-output schema (clickhouse/031_request_shape.sql \
     is not applied), so substitutability is judged on the outcome, the token counts and the size \
     thresholds alone.";

/// The request-shape columns `clickhouse/031_request_shape.sql` adds, each a
/// reason a request is not substitutable when set (#3317).
const SHAPE_COLUMNS: [&str; 5] = [
    "has_tools",
    "has_image_input",
    "has_audio_input",
    "has_response_schema",
    "turns",
];

/// Query string of `GET /api/v1/analytics/savings`.
#[derive(Debug, Deserialize)]
pub(crate) struct SavingsQuery {
    since: Option<String>,
    until: Option<String>,
    /// exact org id; empty or omitted means every org the caller may read
    org: Option<String>,
    /// exact project id; empty or omitted means every project the caller may
    /// read
    project: Option<String>,
    /// exact route (its public model name); empty or omitted means every route
    route: Option<String>,
    /// exact virtual key id; empty or omitted means every key
    key: Option<String>,
    max_prompt_tokens: Option<u32>,
    max_completion_tokens: Option<u32>,
    output_margin: Option<f64>,
}

impl TimeBounds for SavingsQuery {
    fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
        (self.since.as_deref(), self.until.as_deref())
    }
}

/// The substitutability thresholds and the range's margin a report was built
/// with, after defaults and clamping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Thresholds {
    max_prompt_tokens: u32,
    max_completion_tokens: u32,
    output_margin: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_prompt_tokens: DEFAULT_MAX_PROMPT_TOKENS,
            max_completion_tokens: DEFAULT_MAX_COMPLETION_TOKENS,
            output_margin: DEFAULT_OUTPUT_MARGIN,
        }
    }
}

impl SavingsQuery {
    fn thresholds(&self) -> Thresholds {
        let margin = self
            .output_margin
            .filter(|m| m.is_finite())
            .unwrap_or(DEFAULT_OUTPUT_MARGIN)
            .clamp(0.0, MAX_OUTPUT_MARGIN);
        Thresholds {
            max_prompt_tokens: self
                .max_prompt_tokens
                .unwrap_or(DEFAULT_MAX_PROMPT_TOKENS)
                .max(1),
            max_completion_tokens: self
                .max_completion_tokens
                .unwrap_or(DEFAULT_MAX_COMPLETION_TOKENS),
            output_margin: margin,
        }
    }

    /// The window, the scope filters and the thresholds as `param_*`
    /// bindings. Every one is bound, an absent filter as the empty string the
    /// predicate short-circuits on, so the SQL never changes shape.
    fn params(&self, thresholds: Thresholds) -> Vec<(String, String)> {
        let mut params = window_params(&WindowQuery {
            since: self.since.clone(),
            until: self.until.clone(),
            bucket: None,
        });
        params.extend(row_filter_params(
            self.route.as_deref(),
            self.key.as_deref(),
            None,
            None,
            self.project.as_deref(),
        ));
        params.push((
            "param_org".to_string(),
            self.org.clone().unwrap_or_default(),
        ));
        params.push((
            "param_max_prompt_tokens".to_string(),
            thresholds.max_prompt_tokens.to_string(),
        ));
        params.push((
            "param_max_completion_tokens".to_string(),
            thresholds.max_completion_tokens.to_string(),
        ));
        params
    }
}

/// The org filter, which the dashboard's row filters do not carry.
const ORG_FILTER: &str = "({org:String} = '' or org_id = {org:String})";

/// Every reason the advisor can file a request under when it leaves it out of
/// the re-pricing (#3423), in the order the dashboard lists them: the ones
/// [`reason_expr`] gives a request in the query, then the two [`analyse`] adds
/// once it has the route's pool.
///
/// The dashboard words each one (`pages.savings.reasons.<reason>` in every
/// catalog) and has no way to learn a new one from the report, so this list is
/// published as `savings-reasons.json` at the crate root and a dashboard test
/// fails while a catalog lacks one of them. A test below keeps the list equal
/// to what `reason_expr` and `analyse` can emit, so a reason added to either
/// without it fails here, in the rust suite, before it reaches the screen.
/// `eligible` is not one: it is a request that is re-priced, not left out.
/// Read by the tests that publish and check the list, so it exists only for them.
#[cfg(test)]
pub(crate) const EXCLUSION_REASONS: &[&str] = &[
    "unpriced",
    "failed",
    "cache_hit",
    "usage_unknown",
    "uses_tools",
    "image_input",
    "audio_input",
    "structured_output",
    "multi_turn",
    "not_a_model_call",
    "no_token_usage",
    "prompt_too_large",
    "output_too_large",
    "route_not_configured",
    "no_cheaper_candidate",
];

/// Why a request is or is not counted as substitutable, first match wins. The
/// order puts the money question first: an unpriced row is unpriced whatever
/// else is true of it.
///
/// The token columns are qualified because [`groups_sql`] also selects their
/// sums under the same names, and ClickHouse resolves a bare name to a select
/// alias before a column: unqualified, the grouping key would hold aggregates.
///
/// The shape reasons (#3317) come after the outcome and before the sizes: a
/// request that needed tools, an image, audio, a schema or a conversation
/// needs a model that does those, whatever its token counts. A table without
/// the shape columns reads them as `0`, so no request is excluded for them.
fn reason_expr(columns: &Columns) -> String {
    let lifecycle = columns.expr("lifecycle_operation");
    let has_tools = columns.expr("has_tools");
    let has_image_input = columns.expr("has_image_input");
    let has_audio_input = columns.expr("has_audio_input");
    let has_response_schema = columns.expr("has_response_schema");
    let turns = columns.expr("turns");
    format!(
        "multiIf(unpriced = 1, 'unpriced', \
                 {ERROR_STATUS}, 'failed', \
                 cache_hit = 1, 'cache_hit', \
                 usage_unknown = 1, 'usage_unknown', \
                 {lifecycle} != '', 'not_a_model_call', \
                 {has_tools} = 1, 'uses_tools', \
                 {has_image_input} = 1, 'image_input', \
                 {has_audio_input} = 1, 'audio_input', \
                 {has_response_schema} = 1, 'structured_output', \
                 {turns} > 1, 'multi_turn', \
                 request_logs.prompt_tokens = 0, 'no_token_usage', \
                 request_logs.prompt_tokens > {{max_prompt_tokens:UInt32}}, 'prompt_too_large', \
                 request_logs.completion_tokens > {{max_completion_tokens:UInt32}}, 'output_too_large', \
                 'eligible')"
    )
}

/// One row per route, upstream model and reason, with the weighted token
/// totals the re-pricing needs. The cardinality is bounded by the configured
/// routes and targets, not by the traffic.
fn groups_sql(columns: &Columns) -> String {
    let reason = reason_expr(columns);
    let attempts = columns.expr("attempts");
    let write_1h = columns.expr("cache_write_1h_tokens");
    format!(
        "select model as route, target, {reason} as reason, \
                {REQUESTS} as requests, \
                {COST} as cost_usd, \
                sum(prompt_tokens * sample_weight) as prompt_tokens, \
                sum(completion_tokens * sample_weight) as completion_tokens, \
                sum(cache_read_tokens * sample_weight) as cache_read_tokens, \
                sum(cache_write_tokens * sample_weight) as cache_write_tokens, \
                sum({write_1h} * sample_weight) as cache_write_1h_tokens, \
                round(sumIf(sample_weight, {attempts} > 1)) as multi_attempt_requests, \
                round(sumIf(request_logs.cost_usd * sample_weight, {attempts} > 1), 6) as multi_attempt_cost_usd \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} and {ORG_FILTER} \
         group by route, target, reason format JSON"
    )
}

/// Spend per caller: the virtual key and the project it belongs to.
fn callers_sql() -> String {
    format!(
        "select project_id, virtual_key_id, \
                {REQUESTS} as requests, \
                {COST} as cost_usd, \
                {UNPRICED} as unpriced_requests \
         from request_logs where {WHERE_WINDOW} and {ROW_VISIBLE} and {ROW_FILTERS} and {ORG_FILTER} \
         group by project_id, virtual_key_id order by cost_usd desc limit {CALLER_LIMIT} format JSON"
    )
}

async fn savings(
    SavingsAccess(access): SavingsAccess,
    State(state): State<ControlState>,
    Query(q): Query<SavingsQuery>,
) -> Response {
    report(&access, &state, &q).await
}

async fn report(access: &AnalyticsAccess, state: &ControlState, q: &SavingsQuery) -> Response {
    let ch = match client_or_503(state) {
        Ok(ch) => ch,
        Err(response) => return response,
    };
    let config = match state.store.load().await {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(%err, "savings advisor could not load the route configuration");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"message": "failed to load the route configuration"}})),
            )
                .into_response();
        }
    };
    let columns = match ch.request_log_columns().await {
        Ok(columns) => columns,
        Err(err) => return query_failed("savings query failed", &err),
    };
    let thresholds = q.thresholds();
    let params = with_access(q.params(thresholds), access);
    let (groups_sql, callers_sql) = (groups_sql(&columns), callers_sql());
    let (groups, callers) = tokio::join!(
        ch.query(&groups_sql, &params),
        ch.query(&callers_sql, &params),
    );
    let groups = match groups {
        Ok(rows) => rows,
        Err(err) => {
            ch.forget_request_log_columns();
            return query_failed("savings query failed", &err);
        }
    };
    let callers = match callers {
        Ok(rows) => rows,
        Err(err) => return query_failed("savings query failed", &err),
    };
    let groups: Vec<Group> = groups.iter().map(Group::from_row).collect();
    let mut report = analyse(&groups, &callers, &config, thresholds);
    name_callers(state, &mut report).await;
    if !SHAPE_COLUMNS.iter().all(|column| columns.has(column)) {
        if let Some(limits) = report["limits"].as_array_mut() {
            limits.push(json!(NO_SHAPE_LIMIT));
        }
    }
    Json(report).into_response()
}

/// Give each `by_caller` row the name of its virtual key as `key_name` (#3499),
/// from one store read over every key the rows hold, whichever project each is
/// in. The dashboard names a caller from the lists its pickers load, which hold
/// one project's keys, so a key of any other project read as an id.
///
/// The rows are already the caller's own traffic, so a name is only ever that
/// of a key whose requests the caller can read. It is the name the key was
/// given and never its prefix, which a role that reads the report but not the
/// keys has no business seeing. A key with no name, one that is gone, an id
/// that is not a uuid, a deployment without a store, or a store that cannot
/// answer leaves `key_name` `null`, and the report is still served.
#[cfg(feature = "postgres")]
async fn name_callers(state: &ControlState, report: &mut Value) {
    let Some(rows) = report["spend"]["by_caller"].as_array_mut() else {
        return;
    };
    let ids: Vec<uuid::Uuid> = rows
        .iter()
        .filter_map(|row| row["virtual_key_id"].as_str()?.parse().ok())
        .collect();
    let names = match state.pool.as_ref() {
        Some(pool) if !ids.is_empty() => {
            match rolter_store::postgres::repo::VirtualKeyRepo(pool)
                .given_names(&ids)
                .await
            {
                Ok(names) => names,
                Err(err) => {
                    tracing::warn!(%err, "savings advisor could not name its callers' keys");
                    Default::default()
                }
            }
        }
        _ => Default::default(),
    };
    for row in rows {
        let name = row["virtual_key_id"]
            .as_str()
            .and_then(|id| id.parse::<uuid::Uuid>().ok())
            .and_then(|id| names.get(&id).cloned());
        row["key_name"] = json!(name);
    }
}

/// Without a store there are no key names to read, so every `key_name` is
/// `null`.
#[cfg(not(feature = "postgres"))]
async fn name_callers(_state: &ControlState, report: &mut Value) {
    if let Some(rows) = report["spend"]["by_caller"].as_array_mut() {
        for row in rows {
            row["key_name"] = Value::Null;
        }
    }
}

/// Token totals over a set of requests, already scaled by the sampling weight
/// (hence fractional).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Tokens {
    prompt: f64,
    completion: f64,
    cache_read: f64,
    cache_write: f64,
    cache_write_1h: f64,
}

impl Tokens {
    fn add(&mut self, other: &Tokens) {
        self.prompt += other.prompt;
        self.completion += other.completion;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cache_write_1h += other.cache_write_1h;
    }
}

/// One row of [`groups_sql`].
#[derive(Debug, Clone, Default, PartialEq)]
struct Group {
    route: String,
    target: String,
    reason: String,
    requests: f64,
    cost: f64,
    tokens: Tokens,
    multi_attempt_requests: f64,
    multi_attempt_cost: f64,
}

/// A ClickHouse number, which arrives as a JSON number or, for a 64-bit
/// integer, as a quoted string.
fn number(row: &Value, key: &str) -> f64 {
    match row.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn text(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl Group {
    fn from_row(row: &Value) -> Self {
        Self {
            route: text(row, "route"),
            target: text(row, "target"),
            reason: text(row, "reason"),
            requests: number(row, "requests"),
            cost: number(row, "cost_usd"),
            tokens: Tokens {
                prompt: number(row, "prompt_tokens"),
                completion: number(row, "completion_tokens"),
                cache_read: number(row, "cache_read_tokens"),
                cache_write: number(row, "cache_write_tokens"),
                cache_write_1h: number(row, "cache_write_1h_tokens"),
            },
            multi_attempt_requests: number(row, "multi_attempt_requests"),
            multi_attempt_cost: number(row, "multi_attempt_cost_usd"),
        }
    }
}

fn decimal(value: f64) -> Decimal {
    Decimal::from_f64(value).unwrap_or_default()
}

/// A money figure as the JSON number the other analytics reads return,
/// rounded to the six places `cost_usd` carries.
fn money(value: Decimal) -> f64 {
    value.round_dp(6).to_f64().unwrap_or(0.0)
}

fn round6(value: f64) -> f64 {
    money(decimal(value))
}

/// What `tokens` cost at `price`, split the way the gateway prices a request:
/// cache reads at the cached rate, cache writes at the write rates and the
/// rest of the prompt at the input rate, each falling back to the input rate
/// when the price sets none. The audio split is not in the request log, so
/// every token is priced as text.
fn reprice(price: &ModelPriceConfig, tokens: &Tokens) -> Decimal {
    let prompt = tokens.prompt.max(0.0);
    let cached = tokens.cache_read.clamp(0.0, prompt);
    let written = tokens.cache_write.clamp(0.0, prompt - cached);
    let written_1h = tokens.cache_write_1h.clamp(0.0, written);
    let written_5m = written - written_1h;
    let fresh = prompt - cached - written;
    let cached_rate = price.cached_input_per_mtok.unwrap_or(price.input_per_mtok);
    let write_rate = price.cache_write_per_mtok.unwrap_or(price.input_per_mtok);
    let write_1h_rate = price.cache_write_1h_per_mtok.unwrap_or(write_rate);
    let per_mtok = decimal(fresh) * price.input_per_mtok
        + decimal(cached) * cached_rate
        + decimal(written_5m) * write_rate
        + decimal(written_1h) * write_1h_rate
        + decimal(tokens.completion.max(0.0)) * price.output_per_mtok;
    per_mtok / Decimal::from(1_000_000)
}

/// The projected cost of `tokens` at `price` as `(low, high)`.
///
/// The low end assumes the candidate sees the same token counts and the same
/// prompt-cache reuse. The high end assumes its answers run `output_margin`
/// longer and that its prompt cache starts cold (a provider's cache does not
/// carry over to another model), taking whichever of cold and warm costs more
/// since a cache write can cost more than a fresh input token.
fn projection(price: &ModelPriceConfig, tokens: &Tokens, output_margin: f64) -> (Decimal, Decimal) {
    let low = reprice(price, tokens);
    let longer = Tokens {
        completion: tokens.completion * (1.0 + output_margin),
        ..*tokens
    };
    let cold = Tokens {
        cache_read: 0.0,
        cache_write: 0.0,
        cache_write_1h: 0.0,
        ..longer
    };
    let high = reprice(price, &longer).max(reprice(price, &cold)).max(low);
    (low, high)
}

/// The price table in the deployment's base currency, which is what
/// `cost_usd` is recorded in. A price that cannot be converted is left out,
/// so it reads as unknown rather than as a wrong number.
fn base_prices(config: &GatewayConfig) -> HashMap<&str, ModelPriceConfig> {
    let fx = StaticRates::new(config.currency.clone());
    let base = config.currency.base.clone();
    config
        .model_prices
        .iter()
        .filter_map(|price| {
            let factor = fx.convert(Decimal::ONE, &price.currency, &base)?;
            let scale = |rate: Option<Decimal>| rate.map(|rate| rate * factor);
            Some((
                price.model.as_str(),
                ModelPriceConfig {
                    input_per_mtok: price.input_per_mtok * factor,
                    output_per_mtok: price.output_per_mtok * factor,
                    cached_input_per_mtok: scale(price.cached_input_per_mtok),
                    cache_write_per_mtok: scale(price.cache_write_per_mtok),
                    cache_write_1h_per_mtok: scale(price.cache_write_1h_per_mtok),
                    currency: base.clone(),
                    ..price.clone()
                },
            ))
        })
        .collect()
}

/// A slice of spend: how many requests and what they cost.
#[derive(Debug, Clone, Copy, Default)]
struct Spend {
    requests: f64,
    cost: f64,
    unpriced_requests: f64,
}

impl Spend {
    fn add(&mut self, group: &Group) {
        self.requests += group.requests;
        self.cost += group.cost;
        if group.reason == "unpriced" {
            self.unpriced_requests += group.requests;
        }
    }
}

fn breakdown(name: &str, spend: BTreeMap<String, Spend>) -> Vec<Value> {
    let mut rows: Vec<(String, Spend)> = spend.into_iter().collect();
    rows.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then_with(|| a.0.cmp(&b.0)));
    rows.into_iter()
        .map(|(key, spend)| {
            json!({
                name: key,
                "requests": spend.requests.round(),
                "cost_usd": round6(spend.cost),
                "unpriced_requests": spend.unpriced_requests.round(),
            })
        })
        .collect()
}

/// Build the report from the grouped rows, the caller rows and the live
/// configuration. Pure, so every rule can be tested over fixture rows without
/// a ClickHouse.
fn analyse(
    groups: &[Group],
    callers: &[Value],
    config: &GatewayConfig,
    thresholds: Thresholds,
) -> Value {
    let prices = base_prices(config);
    let routes: HashMap<&str, &rolter_core::ModelRoute> = config
        .routes
        .iter()
        .map(|route| (route.model.as_str(), route))
        .collect();

    let mut total = Spend::default();
    let mut by_route: BTreeMap<String, Spend> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Spend> = BTreeMap::new();
    let mut multi_attempt = Spend::default();
    let mut failed = Spend::default();
    let mut cache_hits = 0.0;
    let mut cache_misses = Spend::default();
    let mut excluded: BTreeMap<String, f64> = BTreeMap::new();
    // the eligible traffic of each (route, upstream model)
    let mut eligible: BTreeMap<(String, String), (f64, f64, Tokens)> = BTreeMap::new();

    for group in groups {
        total.add(group);
        by_route.entry(group.route.clone()).or_default().add(group);
        by_model.entry(group.target.clone()).or_default().add(group);
        multi_attempt.requests += group.multi_attempt_requests;
        multi_attempt.cost += group.multi_attempt_cost;
        if group.reason == "failed" {
            failed.add(group);
        }
        let caches = config.cache.enabled
            && routes
                .get(group.route.as_str())
                .is_some_and(|route| route.cache_enabled());
        if group.reason == "cache_hit" {
            cache_hits += group.requests;
        } else if caches {
            cache_misses.add(group);
        }
        if group.reason == "eligible" {
            let entry = eligible
                .entry((group.route.clone(), group.target.clone()))
                .or_default();
            entry.0 += group.requests;
            entry.1 += group.cost;
            entry.2.add(&group.tokens);
        } else {
            *excluded.entry(group.reason.clone()).or_default() += group.requests;
        }
    }

    let mut candidates = Vec::new();
    let mut unknown_candidate_prices: BTreeSet<String> = BTreeSet::new();
    let mut substitutable = Spend::default();
    let (mut savings_low, mut savings_high) = (Decimal::ZERO, Decimal::ZERO);
    for ((route_name, target), (requests, cost, tokens)) in &eligible {
        let Some(route) = routes.get(route_name.as_str()) else {
            *excluded.entry("route_not_configured".into()).or_default() += requests;
            continue;
        };
        let pool: BTreeSet<&str> = route
            .every_target()
            .map(|t| t.model.as_deref().unwrap_or(route.model.as_str()))
            .filter(|model| model != target)
            .collect();
        let current = decimal(*cost);
        let mut options: Vec<(Decimal, Decimal, &str)> = Vec::new();
        for model in pool {
            let Some(price) = prices.get(model) else {
                unknown_candidate_prices.insert(model.to_string());
                continue;
            };
            let (low, high) = projection(price, tokens, thresholds.output_margin);
            // a candidate that costs no less even at its best is not one
            if low < current {
                options.push((low, high, model));
            }
        }
        if options.is_empty() {
            *excluded.entry("no_cheaper_candidate".into()).or_default() += requests;
            continue;
        }
        // the conservative saving decides the order, so the first option is
        // the one that saves most even at the high end of its range
        options.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.cmp(b.2)));
        let (best_low, best_high, _) = options[0];
        substitutable.requests += requests;
        substitutable.cost += cost;
        savings_low += current - best_high;
        savings_high += current - best_low;
        let options: Vec<Value> = options
            .into_iter()
            .map(|(low, high, model)| {
                json!({
                    "model": model,
                    "projected_cost_usd": {"low": money(low), "high": money(high)},
                    "savings_usd": {"low": money(current - high), "high": money(current - low)},
                })
            })
            .collect();
        candidates.push(json!({
            "route": route_name,
            "current_model": target,
            "requests": requests.round(),
            "current_cost_usd": round6(*cost),
            "candidates": options,
        }));
    }
    // the biggest conservative saving first
    candidates.sort_by(|a, b| {
        let saving = |v: &Value| number(&v["candidates"][0]["savings_usd"], "low");
        saving(b).total_cmp(&saving(a))
    });

    let callers: Vec<Value> = callers
        .iter()
        .map(|row| {
            json!({
                "project_id": text(row, "project_id"),
                "virtual_key_id": text(row, "virtual_key_id"),
                "requests": number(row, "requests").round(),
                "cost_usd": round6(number(row, "cost_usd")),
                "unpriced_requests": number(row, "unpriced_requests").round(),
            })
        })
        .collect();

    json!({
        "currency": config.currency.base_code(),
        "limits": LIMITS,
        "assumptions": {
            "max_prompt_tokens": thresholds.max_prompt_tokens,
            "max_completion_tokens": thresholds.max_completion_tokens,
            "output_margin": thresholds.output_margin,
        },
        "spend": {
            "requests": total.requests.round(),
            "cost_usd": round6(total.cost),
            "unpriced_requests": total.unpriced_requests.round(),
            "multi_attempt": {
                "requests": multi_attempt.requests.round(),
                "cost_usd": round6(multi_attempt.cost),
            },
            "failed": {
                "requests": failed.requests.round(),
                "cost_usd": round6(failed.cost),
            },
            "cache": {
                "hits": cache_hits.round(),
                "misses": cache_misses.requests.round(),
                "miss_cost_usd": round6(cache_misses.cost),
            },
            "by_route": breakdown("route", by_route),
            "by_model": breakdown("model", by_model),
            "by_caller": callers,
        },
        "substitutable": {
            "requests": substitutable.requests.round(),
            "current_cost_usd": round6(substitutable.cost),
            "projected_savings_usd": {"low": money(savings_low), "high": money(savings_high)},
        },
        "candidates": candidates,
        "excluded": {
            "requests": excluded.iter().map(|(reason, n)| (reason.clone(), json!(n.round()))).collect::<serde_json::Map<_, _>>(),
            "unknown_candidate_prices": unknown_candidate_prices,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::ModelRoute;
    use std::sync::Arc;

    fn price(model: &str, input: i64, output: i64) -> ModelPriceConfig {
        ModelPriceConfig {
            model: model.to_string(),
            input_per_mtok: Decimal::from(input),
            output_per_mtok: Decimal::from(output),
            ..Default::default()
        }
    }

    /// A route over `targets`, through serde as the store hands it over.
    fn route(model: &str, targets: &[&str]) -> ModelRoute {
        let targets: Vec<Value> = targets
            .iter()
            .map(|t| json!({"provider": "p", "model": t}))
            .collect();
        serde_json::from_value(json!({"model": model, "targets": targets})).expect("a route")
    }

    /// `chat` balances over a large model and a small one, `embed` has one
    /// target, and `unpriced-pool` offers a candidate nobody priced.
    fn config() -> GatewayConfig {
        GatewayConfig {
            routes: vec![
                route("chat", &["large", "small"]),
                route("embed", &["embedder"]),
                route("unpriced-pool", &["large", "mystery"]),
            ],
            model_prices: vec![
                price("chat", 10, 30),
                price("large", 10, 30),
                price("small", 1, 3),
                price("embedder", 1, 0),
            ],
            ..Default::default()
        }
    }

    /// 1000 eligible requests of 1000 prompt and 100 output tokens each,
    /// recorded at the large model's price: $10 + $3 = $13.
    fn eligible(route: &str, target: &str) -> Group {
        Group {
            route: route.to_string(),
            target: target.to_string(),
            reason: "eligible".to_string(),
            requests: 1000.0,
            cost: 13.0,
            tokens: Tokens {
                prompt: 1_000_000.0,
                completion: 100_000.0,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn excluded(route: &str, reason: &str, requests: f64, cost: f64) -> Group {
        Group {
            route: route.to_string(),
            target: "large".to_string(),
            reason: reason.to_string(),
            requests,
            cost,
            ..Default::default()
        }
    }

    #[test]
    fn eligible_traffic_is_repriced_at_the_cheaper_target_of_its_own_pool() {
        let report = analyse(
            &[eligible("chat", "large")],
            &[],
            &config(),
            Thresholds::default(),
        );
        let entry = &report["candidates"][0];
        assert_eq!(entry["route"], "chat");
        assert_eq!(entry["current_model"], "large");
        assert_eq!(entry["current_cost_usd"], 13.0);
        let small = &entry["candidates"][0];
        assert_eq!(small["model"], "small");
        // same tokens: $1 + $0.30; 25% longer answers: $1 + $0.375
        assert_eq!(small["projected_cost_usd"]["low"], 1.3);
        assert_eq!(small["projected_cost_usd"]["high"], 1.375);
        assert_eq!(small["savings_usd"]["low"], 11.625);
        assert_eq!(small["savings_usd"]["high"], 11.7);
        // the current model itself is never offered back
        assert_eq!(entry["candidates"].as_array().map(Vec::len), Some(1));
        assert_eq!(report["substitutable"]["requests"], 1000.0);
        assert_eq!(
            report["substitutable"]["projected_savings_usd"],
            json!({"low": 11.625, "high": 11.7})
        );
    }

    #[test]
    fn the_report_states_that_it_reprices_and_never_judges_quality() {
        let report = analyse(&[], &[], &config(), Thresholds::default());
        let limits = report["limits"].as_array().expect("limits are listed");
        assert!(limits
            .iter()
            .filter_map(Value::as_str)
            .any(|l| l.contains("does not evaluate") && l.contains("never a recommendation")));
    }

    /// The quoted words of `text`, each between a pair of single quotes.
    fn quoted(text: &str) -> Vec<&str> {
        text.split('\'').skip(1).step_by(2).collect()
    }

    /// #3423: the published list is exactly what the advisor can emit.
    #[test]
    fn the_published_reasons_are_every_reason_the_report_can_name() {
        // the query's: every word `reason_expr` can return, whatever the table has
        let mut emitted: BTreeSet<&str> = BTreeSet::new();
        let sql = reason_expr(&Columns::default());
        let sql_reasons: Vec<&str> = quoted(&sql)
            .into_iter()
            .filter(|word| !word.is_empty() && *word != "eligible")
            .collect();
        assert!(sql_reasons.len() > 5, "{sql}");
        emitted.extend(sql_reasons);
        // `analyse`'s own: the reasons it adds, read off its source so that a
        // new `excluded.entry("...".into())` cannot be forgotten
        let source = include_str!("savings.rs");
        let code = &source[..source
            .find("#[cfg(test)]\nmod tests")
            .expect("a test module")];
        for part in code.split(".entry(\"").skip(1) {
            let word = part.split('"').next().unwrap_or_default();
            emitted.insert(word);
        }
        let published: BTreeSet<&str> = EXCLUSION_REASONS.iter().copied().collect();
        assert_eq!(
            published.len(),
            EXCLUSION_REASONS.len(),
            "a reason is listed twice"
        );
        assert_eq!(
            published, emitted,
            "EXCLUSION_REASONS (and savings-reasons.json, `just gen-savings-reasons`) \
             must list every reason reason_expr and analyse can emit, and no other"
        );
    }

    /// `savings-reasons.json` at the crate root: the published list, checked in
    /// so the dashboard's catalog test can read it without a running control plane
    const REASONS_ARTIFACT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/savings-reasons.json");

    /// set to rewrite the artifact instead of comparing against it; `ROLTER_TEST_`
    /// marks it test-only, like the capability matrix's
    const UPDATE_REASONS: &str = "ROLTER_TEST_UPDATE_SAVINGS_REASONS";

    #[test]
    fn the_checked_in_reasons_are_the_published_list() {
        let mut rendered = serde_json::to_string_pretty(&json!({
            "$comment": "written by the rolter-control test suite from EXCLUSION_REASONS in \
                         src/savings.rs (`just gen-savings-reasons`) - do not edit by hand",
            "reasons": EXCLUSION_REASONS,
        }))
        .expect("the list serializes");
        rendered.push('\n');
        if std::env::var_os(UPDATE_REASONS).is_some() {
            std::fs::write(REASONS_ARTIFACT, &rendered).expect("write savings-reasons.json");
            return;
        }
        let checked_in = std::fs::read_to_string(REASONS_ARTIFACT).unwrap_or_default();
        assert!(
            checked_in == rendered,
            "crates/rolter-control/savings-reasons.json is out of date with EXCLUSION_REASONS; \
             run `just gen-savings-reasons` and commit it"
        );
    }

    /// #3317: a request that needed tools, an image, audio, a schema or a
    /// conversation is excluded under its own reason, ahead of the sizes, and
    /// a table without the shape columns excludes nothing for them.
    #[test]
    fn the_request_shape_excludes_under_its_own_reasons() {
        let sql = reason_expr(&Columns::default());
        for (predicate, reason) in [
            ("has_tools = 1, 'uses_tools'", "uses_tools"),
            ("has_image_input = 1, 'image_input'", "image_input"),
            ("has_audio_input = 1, 'audio_input'", "audio_input"),
            (
                "has_response_schema = 1, 'structured_output'",
                "structured_output",
            ),
            ("turns > 1, 'multi_turn'", "multi_turn"),
        ] {
            assert!(sql.contains(predicate), "{reason}: {sql}");
            assert!(
                sql.find(predicate) < sql.find("'prompt_too_large'"),
                "{reason} must be judged before the sizes"
            );
        }
        let old = reason_expr(&Columns::lacking(&SHAPE_COLUMNS));
        assert!(!old.contains("has_tools"), "{old}");
        assert!(old.contains("toUInt8(0) = 1, 'uses_tools'"), "{old}");
        assert!(old.contains("toUInt16(0) > 1, 'multi_turn'"), "{old}");
    }

    /// #3419: a request that never got a status (0) is failed, by the one
    /// definition of an error the dashboard uses, and is judged ahead of the rest.
    #[test]
    fn a_failed_request_is_the_dashboards_error_including_status_zero() {
        let sql = reason_expr(&Columns::default());
        assert!(sql.contains(&format!("{ERROR_STATUS}, 'failed'")), "{sql}");
        assert!(!sql.contains("status >= 400, 'failed'"), "{sql}");
        assert!(sql.find("'failed'") < sql.find("'cache_hit'"), "{sql}");
    }

    #[test]
    fn a_shape_reason_is_counted_as_excluded() {
        let report = analyse(
            &[
                eligible("chat", "large"),
                excluded("chat", "uses_tools", 7.0, 0.5),
                excluded("chat", "multi_turn", 3.0, 0.25),
            ],
            &[],
            &config(),
            Thresholds::default(),
        );
        assert_eq!(report["excluded"]["requests"]["uses_tools"], 7.0);
        assert_eq!(report["excluded"]["requests"]["multi_turn"], 3.0);
        // the report no longer says the shape is unrecorded unless it is
        let limits = report["limits"].as_array().expect("limits are listed");
        assert!(!limits.contains(&json!(NO_SHAPE_LIMIT)));
    }

    #[test]
    fn an_empty_window_is_an_empty_report_not_an_error() {
        let report = analyse(&[], &[], &config(), Thresholds::default());
        assert_eq!(report["spend"]["requests"], 0.0);
        assert_eq!(report["spend"]["cost_usd"], 0.0);
        assert_eq!(report["spend"]["by_route"], json!([]));
        assert_eq!(report["candidates"], json!([]));
        assert_eq!(
            report["substitutable"]["projected_savings_usd"],
            json!({"low": 0.0, "high": 0.0})
        );
        assert_eq!(report["excluded"]["requests"], json!({}));
    }

    #[test]
    fn an_unpriced_request_is_excluded_and_counted_never_priced_at_zero() {
        let report = analyse(
            &[
                eligible("chat", "large"),
                excluded("chat", "unpriced", 40.0, 0.0),
            ],
            &[],
            &config(),
            Thresholds::default(),
        );
        assert_eq!(report["spend"]["unpriced_requests"], 40.0);
        assert_eq!(report["excluded"]["requests"]["unpriced"], 40.0);
        // only the priced thousand are re-priced
        assert_eq!(report["substitutable"]["requests"], 1000.0);
        assert_eq!(report["substitutable"]["current_cost_usd"], 13.0);
        assert_eq!(report["spend"]["by_route"][0]["unpriced_requests"], 40.0);
    }

    #[test]
    fn a_candidate_with_no_price_is_named_and_never_offered() {
        let report = analyse(
            &[eligible("unpriced-pool", "large")],
            &[],
            &config(),
            Thresholds::default(),
        );
        assert_eq!(report["candidates"], json!([]));
        assert_eq!(
            report["excluded"]["unknown_candidate_prices"],
            json!(["mystery"])
        );
        assert_eq!(
            report["excluded"]["requests"]["no_cheaper_candidate"],
            1000.0
        );
    }

    #[test]
    fn a_route_with_no_cheaper_target_offers_nothing() {
        let report = analyse(
            &[eligible("embed", "embedder")],
            &[],
            &config(),
            Thresholds::default(),
        );
        assert_eq!(report["candidates"], json!([]));
        assert_eq!(
            report["excluded"]["requests"]["no_cheaper_candidate"],
            1000.0
        );
    }

    #[test]
    fn traffic_of_a_route_no_longer_configured_is_counted_not_repriced() {
        let report = analyse(
            &[eligible("deleted-route", "large")],
            &[],
            &config(),
            Thresholds::default(),
        );
        assert_eq!(report["candidates"], json!([]));
        assert_eq!(
            report["excluded"]["requests"]["route_not_configured"],
            1000.0
        );
        // its spend still shows where the money went
        assert_eq!(report["spend"]["by_route"][0]["route"], "deleted-route");
    }

    #[test]
    fn retries_failures_and_cache_misses_are_called_out() {
        let mut config = config();
        config.cache.enabled = true;
        config.routes[0].cache = Some(serde_json::from_value(json!({})).expect("a cache opt-in"));
        let mut retried = eligible("chat", "large");
        retried.multi_attempt_requests = 10.0;
        retried.multi_attempt_cost = 0.5;
        let report = analyse(
            &[
                retried,
                excluded("chat", "failed", 5.0, 0.25),
                excluded("chat", "cache_hit", 20.0, 0.0),
                excluded("embed", "output_too_large", 3.0, 1.0),
            ],
            &[],
            &config,
            Thresholds::default(),
        );
        let spend = &report["spend"];
        assert_eq!(
            spend["multi_attempt"],
            json!({"requests": 10.0, "cost_usd": 0.5})
        );
        assert_eq!(spend["failed"], json!({"requests": 5.0, "cost_usd": 0.25}));
        // `embed` does not cache, so its traffic is no cache miss
        assert_eq!(
            spend["cache"],
            json!({"hits": 20.0, "misses": 1005.0, "miss_cost_usd": 13.25})
        );
        assert_eq!(report["excluded"]["requests"]["failed"], 5.0);
        assert_eq!(report["excluded"]["requests"]["cache_hit"], 20.0);
        assert_eq!(report["excluded"]["requests"]["output_too_large"], 3.0);
        assert_eq!(spend["by_route"][0]["route"], "chat");
        assert_eq!(spend["by_route"][0]["requests"], 1025.0);
    }

    #[test]
    fn the_cold_cache_end_of_the_range_prices_every_prompt_token_fresh() {
        let mut cached = price("small", 1, 3);
        cached.cached_input_per_mtok = Some(Decimal::new(1, 1));
        let tokens = Tokens {
            prompt: 1_000_000.0,
            cache_read: 1_000_000.0,
            ..Default::default()
        };
        let (low, high) = projection(&cached, &tokens, 0.0);
        assert_eq!(money(low), 0.1);
        assert_eq!(money(high), 1.0);
    }

    #[test]
    fn a_price_in_another_currency_is_converted_and_an_unconvertible_one_is_unknown() {
        let mut config = config();
        config
            .currency
            .rates
            .insert("EUR".to_string(), Decimal::from(2));
        config.model_prices.retain(|p| p.model != "small");
        config.model_prices.push(ModelPriceConfig {
            currency: "EUR".to_string(),
            ..price("small", 1, 3)
        });
        let prices = base_prices(&config);
        assert_eq!(prices["small"].input_per_mtok, Decimal::from(2));
        config.model_prices.last_mut().expect("pushed").currency = "XYZ".to_string();
        assert!(!base_prices(&config).contains_key("small"));
    }

    #[test]
    fn thresholds_default_and_clamp() {
        let query = |margin: Option<f64>| SavingsQuery {
            since: None,
            until: None,
            org: None,
            project: None,
            route: None,
            key: None,
            max_prompt_tokens: Some(0),
            max_completion_tokens: None,
            output_margin: margin,
        };
        let t = query(Some(99.0)).thresholds();
        assert_eq!(t.output_margin, MAX_OUTPUT_MARGIN);
        assert_eq!(t.max_prompt_tokens, 1);
        assert_eq!(t.max_completion_tokens, DEFAULT_MAX_COMPLETION_TOKENS);
        assert_eq!(query(Some(-1.0)).thresholds().output_margin, 0.0);
        assert_eq!(
            query(None).thresholds().output_margin,
            DEFAULT_OUTPUT_MARGIN
        );
    }

    #[test]
    fn every_query_binds_the_callers_scope_and_every_filter() {
        let columns = Columns::default();
        for sql in [groups_sql(&columns), callers_sql()] {
            assert!(sql.contains(ROW_VISIBLE), "{sql}");
            assert!(sql.contains(ROW_FILTERS), "{sql}");
            assert!(sql.contains(ORG_FILTER), "{sql}");
            assert!(sql.contains(WHERE_WINDOW), "{sql}");
        }
        // a table without the hand-applied columns reads their defaults
        let old = Columns::lacking(&["attempts", "lifecycle_operation", "cache_write_1h_tokens"]);
        let sql = groups_sql(&old);
        assert!(!sql.contains("attempts >"), "{sql}");
        assert!(sql.contains("toUInt8(0) > 1"), "{sql}");
    }

    /// A ClickHouse stand-in that answers the column probe with nothing and
    /// every read with the fixture rows the caller's bound org ids reach, the
    /// way `ROW_VISIBLE` would: an unrestricted caller sees every row.
    async fn stub_clickhouse(groups: Vec<(&'static str, Value)>) -> String {
        let groups = Arc::new(groups);
        let app = axum::Router::new().fallback(axum::routing::post(
            move |axum::extract::RawQuery(q): axum::extract::RawQuery, body: String| {
                let groups = groups.clone();
                async move {
                    let q = q.unwrap_or_default();
                    if !body.contains("from request_logs where") {
                        return Json(json!({"data": []}));
                    }
                    let unrestricted = q.contains("param_unrestricted=1");
                    let rows: Vec<Value> = if body.contains("as reason") {
                        groups
                            .iter()
                            .filter(|(org, _)| unrestricted || q.contains(org))
                            .map(|(_, row)| row.clone())
                            .collect()
                    } else {
                        Vec::new()
                    };
                    Json(json!({"data": rows}))
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let url = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(async move { axum::serve(listener, app).await });
        url
    }

    fn row(route: &str, target: &str, cost: f64) -> Value {
        json!({
            "route": route, "target": target, "reason": "eligible",
            "requests": 1000, "cost_usd": cost,
            "prompt_tokens": 1_000_000, "completion_tokens": 100_000,
            "cache_read_tokens": 0, "cache_write_tokens": 0, "cache_write_1h_tokens": 0,
            "multi_attempt_requests": 0, "multi_attempt_cost_usd": 0,
        })
    }

    fn state_with(url: &str) -> ControlState {
        let mut state = crate::tests::state_with_token(None);
        state.store = Arc::new(rolter_store::InMemoryConfigStore::new(config()));
        state.clickhouse = Some(crate::analytics::ClickHouseClient::new(url));
        state
    }

    async fn body(response: Response) -> Value {
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("a body");
        serde_json::from_slice(&bytes).expect("json")
    }

    fn query() -> SavingsQuery {
        serde_json::from_value(json!({})).expect("an empty query")
    }

    #[tokio::test]
    async fn without_a_store_every_caller_key_name_is_null() {
        let state = crate::tests::state_with_token(None);
        let mut report = json!({"spend": {"by_caller": [
            {"project_id": "p", "virtual_key_id": "8d7c2a1e-0000-4000-8000-000000000001"},
        ]}});
        name_callers(&state, &mut report).await;
        assert!(
            report["spend"]["by_caller"][0]["key_name"].is_null(),
            "{report}"
        );
    }

    /// #3499: the callers of two projects are named from one read, whichever
    /// project each key is in; a key that is gone, or an id that is not a
    /// key's, stays `null`.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn every_callers_key_is_named_whichever_project_it_is_in() {
        use rolter_store::postgres::test_schema::TestSchema;
        let Some(url) = rolter_store::postgres::test_database::url().await else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let db = TestSchema::migrated(&url).await;
        let pool = db.pool().clone();
        let org: uuid::Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team: uuid::Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org)
                .fetch_one(&pool)
                .await
                .unwrap();
        let mut keys = Vec::new();
        for (project, key, prefix) in [("alpha", "ci-deploy", "pfx-a"), ("beta", "batch", "pfx-b")]
        {
            let project: uuid::Uuid = sqlx::query_scalar(
                "insert into projects (team_id, name) values ($1, $2) returning id",
            )
            .bind(team)
            .bind(project)
            .fetch_one(&pool)
            .await
            .unwrap();
            let id: uuid::Uuid = sqlx::query_scalar(
                "insert into virtual_keys (project_id, key_hash, key_prefix, name) \
                 values ($1, $2, $3, $4) returning id",
            )
            .bind(project)
            .bind(format!("hash-{key}"))
            .bind(prefix)
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap();
            keys.push((project, id));
        }
        // a key with no name: its prefix is never handed out in its place
        let unnamed: uuid::Uuid = sqlx::query_scalar(
            "insert into virtual_keys (project_id, key_hash, key_prefix) \
             values ($1, 'hash-unnamed', 'pfx-secret') returning id",
        )
        .bind(keys[1].0)
        .fetch_one(&pool)
        .await
        .unwrap();
        let state = crate::test_state(pool, None, None);
        let gone = uuid::Uuid::new_v4();
        let mut report = json!({"spend": {"by_caller": [
            {"project_id": keys[0].0.to_string(), "virtual_key_id": keys[0].1.to_string()},
            {"project_id": keys[1].0.to_string(), "virtual_key_id": keys[1].1.to_string()},
            {"project_id": keys[1].0.to_string(), "virtual_key_id": gone.to_string()},
            {"project_id": "", "virtual_key_id": "from-the-config-file"},
            {"project_id": keys[1].0.to_string(), "virtual_key_id": unnamed.to_string()},
        ]}});
        name_callers(&state, &mut report).await;
        let names: Vec<Value> = report["spend"]["by_caller"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["key_name"].clone())
            .collect();
        assert_eq!(
            names,
            vec![
                json!("ci-deploy"),
                json!("batch"),
                Value::Null,
                Value::Null,
                Value::Null
            ]
        );
    }

    #[tokio::test]
    async fn the_endpoint_reports_over_fixture_rows() {
        let url = stub_clickhouse(vec![("org-a", row("chat", "large", 13.0))]).await;
        let state = state_with(&url);
        let report = body(report(&AnalyticsAccess::unrestricted(), &state, &query()).await).await;
        assert_eq!(report["candidates"][0]["candidates"][0]["model"], "small");
        assert_eq!(report["spend"]["cost_usd"], 13.0);
    }

    #[tokio::test]
    async fn rows_outside_the_callers_scope_never_reach_the_report() {
        let url = stub_clickhouse(vec![
            ("org-a", row("chat", "large", 13.0)),
            ("org-b", row("chat", "large", 26.0)),
        ])
        .await;
        let state = state_with(&url);
        let scoped = AnalyticsAccess::member_of_orgs(&["org-a"]);
        let report = body(report(&scoped, &state, &query()).await).await;
        assert_eq!(report["spend"]["cost_usd"], 13.0);
        assert_eq!(report["candidates"].as_array().map(Vec::len), Some(1));
        assert_eq!(report["candidates"][0]["current_cost_usd"], 13.0);
    }

    #[tokio::test]
    async fn without_clickhouse_the_endpoint_says_analytics_is_unavailable() {
        let state = crate::tests::state_with_token(None);
        let response = report(&AnalyticsAccess::unrestricted(), &state, &query()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
