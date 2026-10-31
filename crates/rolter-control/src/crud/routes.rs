//! Routes: model routes, their targets and per-route tuning.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use rolter_core::config::only_restricted_dialled;
use rolter_core::failover::{all_targets_restricted, FailureClass};
use rolter_core::{AdvancedModelConfig, Error, MAX_ROUTE_RETRIES};
use rolter_store::postgres::models::{Route, RouteTarget, RouteVariant};
use rolter_store::postgres::repo::{
    RouteRepo, RouteTargetRepo, RouteTargetSpec, RouteUpdate, RouteVariantRepo, RouteVariantSpec,
    RouteVariantTargetSpec,
};

use super::{
    authorize_route, invalid_field, log_audit, name_taken, pool, publish_config_change,
    require_non_empty, require_not_config_owned, ApiError, ApiResult, Query, SafeJson,
};
use crate::access_control::caller_policy;
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

use super::provider_groups::require_providers_in_org;

use super::provider_groups::taken_in_org;

use super::provider_groups::validate_strategy;

use super::providers::require_providers_usable_from;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/routes",
            get(list_routes).post(create_route),
        )
        .route(
            "/api/v1/routes/{id}",
            put(update_route).delete(delete_route),
        )
        .route("/api/v1/routes/{id}/params", put(set_route_params))
        .route(
            "/api/v1/routes/{id}/complexity",
            get(get_route_complexity)
                .put(set_route_complexity)
                .delete(disable_route_complexity),
        )
        .route("/api/v1/routes/{id}/advanced", put(set_route_advanced))
        .route(
            "/api/v1/routes/{id}/variants",
            get(list_route_variants).put(set_route_variants),
        )
        .route(
            "/api/v1/routes/{route_id}/targets",
            get(list_route_targets).post(create_route_target),
        )
        .route("/api/v1/route-targets/{id}", delete(delete_route_target))
}

/// Refuse a route name the gateway already answers for the caller's org.
///
/// A name is taken when another route of the org has it, when it is the builtin
/// `fake-llm` every org can call, or when it has the `slug/model` shape and
/// the slug belongs to a provider or group of another org or of the bootstrap
/// file. The gateway already treats a route outside the caller's org as
/// absent, so such a route could only shadow the address for the operator's
/// own keys, which carry no org and see every row; refusing it keeps those
/// keys on the address they meant. A slug the route's own org holds is that
/// org's choice. A name with `/` is otherwise fine: `Qwen/Qwen2.5-7B` is not
/// a slug address, since a slug is lower-case. Neither refusal says who holds
/// the name (#1845).
///
/// Only the API refuses `fake-llm`. A tenant must not take the builtin away,
/// but an operator may replace it: a route of that name in the bootstrap file,
/// a `[[models.default]]` or a `rolter-seed --import` still shadows it.
async fn require_route_name_free(
    state: &ControlState,
    org_id: Option<Uuid>,
    model: &str,
) -> ApiResult<()> {
    if model == rolter_core::FAKE_LLM_MODEL {
        return Err(name_taken(format!(
            "'{model}' is the gateway's built-in model and cannot be a route name; choose another"
        )));
    }
    let routes = RouteRepo(pool(state));
    // names are per org (#1857): another org's route of the same name is no
    // conflict, but a key resolves a name per org, so its projects share one
    if routes.model_in_use(model, org_id).await? {
        return Err(taken_in_org("route name", model));
    }
    if let Some(slug) = rolter_core::slug::address_slug(model) {
        if state.config_owned.holds_slug(slug)
            || routes.name_takes_address_outside_org(model, org_id).await?
        {
            return Err(name_taken(format!(
                "route name '{model}' is a provider or provider group address in this \
                 deployment ('{slug}/…'); choose another"
            )));
        }
    }
    Ok(())
}

// --- routes + targets ---

/// What `GET /api/v1/projects/{project_id}/routes` can embed beside each route.
#[derive(Deserialize, Default)]
struct ListRoutesQuery {
    /// comma-separated extras: `targets` (the route's targets, as
    /// `GET /api/v1/routes/{id}/targets` lists them), `variants` (its weighted
    /// variants, as `GET /api/v1/routes/{id}/variants` lists them) and
    /// `complexity` (its complexity policy, as
    /// `GET /api/v1/routes/{id}/complexity` returns it).
    /// Omitted, the list is the plain routes it always was
    include: Option<String>,
}

/// One route of the list, plus whatever `?include=` asked to have embedded.
/// The extras are skipped when not asked for, so a client that sends no
/// `include` reads exactly the body it read before.
#[derive(Serialize)]
struct RouteListEntry {
    #[serde(flatten)]
    route: Route,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Vec<RouteTarget>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variants: Option<Vec<RouteVariant>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    complexity: Option<serde_json::Value>,
}

/// the complexity policy stored in a route's params, or the empty policy
fn route_complexity(route: &Route) -> serde_json::Value {
    route
        .params
        .get(rolter_balancer::complexity::POLICY_PARAM)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"tiers": []}))
}

async fn list_routes(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListRoutesQuery>,
) -> ApiResult<Json<Vec<RouteListEntry>>> {
    let mut with_targets = false;
    let mut with_variants = false;
    let mut with_complexity = false;
    for token in query.include.as_deref().unwrap_or("").split(',') {
        match token.trim() {
            "" => {}
            "targets" => with_targets = true,
            "variants" => with_variants = true,
            "complexity" => with_complexity = true,
            other => {
                return Err(invalid_field(
                    "include",
                    format!("unknown include `{other}`; expected targets, variants or complexity"),
                ))
            }
        }
    }
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    authorize(&state, &principal, chain, cap!("route", Read)).await?;
    let routes = RouteRepo(pool(&state)).list(project_id).await?;
    // an access profile may narrow which routes the caller sees (#534). With no
    // profile carrying a policy the filter is the identity, which is every
    // deployment that defines none
    let policy = caller_policy(&state, &principal).await?;
    let routes: Vec<Route> = if policy.is_unrestricted() {
        routes
    } else {
        routes
            .into_iter()
            .filter(|route| policy.permits_route(&route.model))
            .collect()
    };
    // the targets are read for the project and then dealt out to the routes the
    // caller may see, so an access profile that hides a route hides its targets
    // with it (the per-route read does not look at the profile at all)
    let mut targets: HashMap<Uuid, Vec<RouteTarget>> = HashMap::new();
    if with_targets {
        for target in RouteTargetRepo(pool(&state))
            .list_for_project(project_id)
            .await?
        {
            targets.entry(target.route_id).or_default().push(target);
        }
    }
    // the variants are dealt out the same way, for the same reason
    let mut variants: HashMap<Uuid, Vec<RouteVariant>> = HashMap::new();
    if with_variants {
        for variant in RouteVariantRepo(pool(&state))
            .list_for_project(project_id)
            .await?
        {
            variants.entry(variant.route_id).or_default().push(variant);
        }
    }
    Ok(Json(
        routes
            .into_iter()
            .map(|route| RouteListEntry {
                targets: with_targets.then(|| targets.remove(&route.id).unwrap_or_default()),
                variants: with_variants.then(|| variants.remove(&route.id).unwrap_or_default()),
                complexity: with_complexity.then(|| route_complexity(&route)),
                route,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateRoute {
    model: String,
    #[serde(default = "default_strategy")]
    strategy: String,
}

pub(super) fn default_strategy() -> String {
    "round_robin".to_string()
}

pub(super) const STRATEGIES: [&str; 14] = [
    "round_robin",
    "random",
    "power_of_two",
    "consistent_hash",
    "cache_aware",
    "weighted",
    "pipeline",
    "cheapest",
    "fastest",
    "precise_cache_aware",
    "lmcache_aware",
    "adaptive",
    "lora_aware",
    "predicted_latency",
];

async fn create_route(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateRoute>,
) -> ApiResult<Json<Route>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("route", Create)).await?;
    require_non_empty(&body.model, "model")?;
    require_not_config_owned(&state.config_owned.models, &body.model, "model")?;
    validate_strategy(&body.strategy)?;
    require_route_name_free(&state, org_id, &body.model).await?;
    // the check above is a read, so a concurrent create in another project of
    // the org can still win the name; `routes_org_model_unique` (#3459) then
    // refuses this insert, and the caller reads the same refusal
    let row = match RouteRepo(pool(&state))
        .create(project_id, &body.model, &body.strategy)
        .await
    {
        Err(Error::AlreadyExists(_)) => return Err(taken_in_org("route name", &body.model)),
        other => other?,
    };
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.create",
        "route",
        row.id,
        serde_json::json!({"model": row.model, "strategy": row.strategy}),
    )
    .await;
    Ok(Json(row))
}

/// The body of `PUT /api/v1/routes/{id}`. Every field is optional, and one
/// that is omitted (or `null`) keeps the stored value, so a client that only
/// knows about `enabled` never resets the strategy or the targets.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRoute {
    #[serde(default)]
    enabled: Option<bool>,
    /// one of [`STRATEGIES`], the same allowlist route creation applies
    #[serde(default)]
    strategy: Option<String>,
    /// the route's complete target list. Entries are matched to the stored
    /// targets by provider and upstream model: a match keeps its id and takes
    /// the new weight, a stored target the list leaves out is removed, and an
    /// entry with no match is added. Each entry passes the checks
    /// `POST /api/v1/routes/{id}/targets` applies, and the list may not be
    /// empty
    #[serde(default)]
    targets: Option<Vec<RouteTargetInput>>,
}

async fn update_route(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateRoute>,
) -> ApiResult<Json<Route>> {
    let repo = RouteRepo(pool(&state));
    let existing = repo.get(id).await?;
    let chain = ScopeChain::from_project(pool(&state), existing.project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("route", Update)).await?;
    if body.enabled.is_none() && body.strategy.is_none() && body.targets.is_none() {
        return Ok(Json(existing));
    }
    if let Some(strategy) = &body.strategy {
        validate_strategy(strategy)?;
    }
    let targets = match &body.targets {
        Some(targets) => {
            // an empty list would leave the route nothing to serve; that is
            // what disabling or deleting it is for, and the gateway would drop
            // it from the snapshot either way
            if targets.is_empty() {
                return Err(invalid_field(
                    "targets",
                    "targets must name at least one target; disable or delete the route to \
                     take it out of service",
                ));
            }
            validate_route_targets(
                &state,
                org_id,
                existing.project_id,
                targets,
                TargetPaths::InList,
            )
            .await?;
            // a route left with nothing but restricted fallbacks to dial
            // could never dial anything (#1584); a route with variants dials
            // only them, so the new pool counts only without (#3487)
            if only_restricted_dialled(
                targets.iter().map(RouteTargetInput::is_restricted),
                stored_variants_restricted(&state, id).await?,
            ) {
                return Err(invalid_field(
                    "targets",
                    all_targets_restricted(&existing.model),
                ));
            }
            Some(
                targets
                    .iter()
                    .map(RouteTargetInput::spec)
                    .collect::<Vec<_>>(),
            )
        }
        None => None,
    };
    let (row, changes) = repo
        .update(
            id,
            &RouteUpdate {
                enabled: body.enabled,
                strategy: body.strategy.as_deref(),
                targets: targets.as_deref(),
            },
        )
        .await?;
    publish_config_change(&state).await?;
    // an `enabled`-only body is what this endpoint accepted before it took
    // the strategy and targets (#2235), and it keeps its own audit action
    if body.strategy.is_none() && body.targets.is_none() {
        log_audit(
            &state,
            &principal,
            org_id,
            "route.set_enabled",
            "route",
            id,
            serde_json::json!({"enabled": row.enabled}),
        )
        .await;
        return Ok(Json(row));
    }
    let mut detail = serde_json::Map::new();
    if let Some(enabled) = body.enabled {
        detail.insert("enabled".into(), enabled.into());
    }
    if body.strategy.is_some() {
        detail.insert("strategy".into(), row.strategy.clone().into());
        detail.insert("previous_strategy".into(), existing.strategy.into());
    }
    if body.targets.is_some() {
        detail.insert(
            "targets".into(),
            serde_json::to_value(&changes).unwrap_or_default(),
        );
    }
    log_audit(
        &state,
        &principal,
        org_id,
        "route.update",
        "route",
        id,
        serde_json::Value::Object(detail),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetRouteParams {
    /// admin default inference params (json object, e.g. {"temperature": 0})
    #[serde(default)]
    params: serde_json::Value,
    /// override policy {mode, allow, deny}
    #[serde(default)]
    param_policy: serde_json::Value,
}

async fn set_route_params(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetRouteParams>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
    // both must be json objects (or null → treated as empty) so the gateway can
    // deserialize them into the param map / policy
    let params = normalize_json_object(body.params, "params")?;
    let param_policy = normalize_json_object(body.param_policy, "param_policy")?;
    let row = RouteRepo(pool(&state))
        .set_params(id, &params, &param_policy)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_params",
        "route",
        id,
        serde_json::json!({"params": params, "param_policy": param_policy}),
    )
    .await;
    Ok(Json(row))
}

/// Read the policy as a first-class control-plane resource while storing it in
/// the snapshot-compatible route params JSON.
// this read takes `route:read`, the same bar as every other attribute of the
// route it belongs to. #704 held it to the *mutation* bar instead, which made
// the policy the one part of a route a viewer could list but never see, so the
// dashboard's Complexity Router answered a reader with a permission error
// (#1666). It is configuration rather than a secret — it already travels to the
// gateway inside the route's `params` — and the PUT beside it still takes
// `route:update`, so a viewer reads it and writes nothing
async fn get_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize_route(&state, &principal, id, cap!("route", Read)).await?;
    let route = RouteRepo(pool(&state)).get(id).await?;
    Ok(Json(route_complexity(&route)))
}

/// Validate and persist one bounded policy. It travels in `params` through
/// `ConfigStore::load` and the existing atomic gateway snapshot; the gateway
/// reserves this key and removes it before any request is forwarded upstream.
async fn set_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(value): SafeJson<serde_json::Value>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
    let policy: rolter_balancer::complexity::ComplexityPolicy = serde_json::from_value(value)
        .map_err(|error| {
            ApiError::Core(Error::Config(format!("invalid complexity policy: {error}")))
        })?;
    policy
        .validate_shape()
        .map_err(|error| ApiError::Core(Error::Config(error)))?;
    let config = state.store.load().await?;
    let routes = config
        .routes
        .iter()
        .map(|route| route.model.clone())
        .collect();
    policy
        .validate_routes(&routes)
        .map_err(|error| ApiError::Core(Error::Config(error)))?;

    let existing = RouteRepo(pool(&state)).get(id).await?;
    let mut params = normalize_json_object(existing.params, "params")?;
    let Some(object) = params.as_object_mut() else {
        return Err(ApiError::Core(Error::Config(
            "params must be a json object".to_string(),
        )));
    };
    object.insert(
        rolter_balancer::complexity::POLICY_PARAM.to_string(),
        serde_json::to_value(&policy).map_err(|error| {
            ApiError::Core(Error::Config(format!("invalid complexity policy: {error}")))
        })?,
    );
    let row = RouteRepo(pool(&state))
        .set_params(id, &params, &existing.param_policy)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_complexity",
        "route",
        id,
        serde_json::json!({"complexity": policy}),
    )
    .await;
    Ok(Json(row))
}

/// Remove only the routing policy, preserving ordinary upstream parameters.
async fn disable_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
    let existing = RouteRepo(pool(&state)).get(id).await?;
    let mut params = normalize_json_object(existing.params.clone(), "params")?;
    let removed = params
        .as_object_mut()
        .and_then(|object| object.remove(rolter_balancer::complexity::POLICY_PARAM));
    // nothing to disable: answer with the route as it is, so a repeated or stray
    // disable neither bumps the config version, publishes to the gateways nor
    // leaves an audit row that reads like a routing change (#3221)
    if removed.is_none() {
        return Ok(Json(existing));
    }
    let row = RouteRepo(pool(&state))
        .set_params(id, &params, &existing.param_policy)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_complexity",
        "route",
        id,
        serde_json::json!({"complexity": null}),
    )
    .await;
    Ok(Json(row))
}

/// Coerce a json value to an object: `null` becomes `{}`, an object passes
/// through, anything else is a client error.
fn normalize_json_object(value: serde_json::Value, field: &str) -> ApiResult<serde_json::Value> {
    match value {
        serde_json::Value::Null => Ok(serde_json::json!({})),
        v @ serde_json::Value::Object(_) => Ok(v),
        _ => Err(invalid_field(
            field,
            format!("{field} must be a json object"),
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetRouteAdvanced {
    #[serde(default)]
    advanced: serde_json::Value,
}

pub(super) fn validate_advanced(advanced: &AdvancedModelConfig) -> ApiResult<()> {
    for (field, value) in [
        ("timeout_secs", advanced.limits.timeout_secs),
        ("output_tokens", advanced.limits.output_tokens),
    ] {
        if value.is_some_and(|limit| limit == 0 || limit > 10_000_000) {
            return Err(invalid_field(
                field,
                format!("{field} must be between 1 and 10000000"),
            ));
        }
    }
    // zero is not a window; served on /v1/models, so it must mean something
    if advanced.context_length == Some(0) {
        return Err(invalid_field(
            "context_length",
            "context_length must be at least 1".to_string(),
        ));
    }
    // the same rule the snapshot applies, so a cap accepted here is one the
    // gateway enforces rather than drops (#2936)
    if let Some(problem) = advanced.limits.throughput_problems().into_iter().next() {
        return Err(ApiError::Core(Error::Config(problem)));
    }
    // zero is a value here, unlike the other limits: it turns retries off for
    // the route. the ceiling is the one the gateway applies, which is also what
    // the deployment-wide `retry_max_retries` is held to
    if advanced
        .limits
        .retries
        .is_some_and(|retries| retries > MAX_ROUTE_RETRIES)
    {
        return Err(invalid_field(
            "retries",
            format!("retries must be between 0 and {MAX_ROUTE_RETRIES}"),
        ));
    }
    // the same rule the snapshot and the proxy apply, so a header accepted here
    // is one the gateway sends: a name that is not a token, a value the wire
    // cannot carry, or a credential header a route may not replace
    if let Some(problem) = advanced.header_problems().into_iter().next() {
        return Err(ApiError::Core(Error::Config(problem)));
    }
    for id in advanced
        .visibility
        .allowed_team_ids
        .iter()
        .chain(advanced.visibility.allowed_key_ids.iter())
        .chain(advanced.visibility.allowed_user_ids.iter())
    {
        Uuid::parse_str(id).map_err(|_| {
            ApiError::Core(Error::Config(format!(
                "invalid scoped access reference '{id}'"
            )))
        })?;
    }
    Ok(())
}

async fn set_route_advanced(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetRouteAdvanced>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
    let mut advanced_value = normalize_json_object(body.advanced, "advanced")?;
    // keys the gateway never read (#2924) are accepted and dropped, so an older
    // dashboard's save still succeeds and the routes API never reads one back
    // as if it were stored policy
    let dropped = rolter_core::strip_retired_advanced_keys(&mut advanced_value);
    let advanced: AdvancedModelConfig =
        serde_json::from_value(advanced_value.clone()).map_err(|err| {
            ApiError::Core(Error::Config(format!(
                "invalid advanced model configuration: {err}"
            )))
        })?;
    validate_advanced(&advanced)?;
    let row = RouteRepo(pool(&state))
        .set_advanced(id, &advanced_value)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_advanced",
        "route",
        id,
        serde_json::json!({"advanced": advanced_value, "dropped_retired_keys": dropped}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_route(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Delete)).await?;
    RouteRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.delete",
        "route",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_route_targets(
    principal: Principal,
    State(state): State<ControlState>,
    Path(route_id): Path<Uuid>,
) -> ApiResult<Json<Vec<RouteTarget>>> {
    let route = RouteRepo(pool(&state)).get(route_id).await?;
    let chain = ScopeChain::from_project(pool(&state), route.project_id).await?;
    authorize(&state, &principal, chain, cap!("route", Read)).await?;
    Ok(Json(RouteTargetRepo(pool(&state)).list(route_id).await?))
}

/// One route target as a client sends it, alone to
/// `POST /api/v1/routes/{id}/targets` or in the list `PUT /api/v1/routes/{id}`
/// takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteTargetInput {
    provider_id: Uuid,
    /// the model name sent upstream; omitted, `null` or blank forwards the
    /// route's public name
    upstream_model: Option<String>,
    #[serde(default = "default_weight")]
    weight: i32,
    /// the failure classes this target is a fallback for (#1584): `throttled`,
    /// `timeout`, `transport`, `server_error`, `refusal` or
    /// `refusal:<category>`. Omitted or empty, the target is an ordinary
    /// member of the pool
    #[serde(default)]
    takes_over: Vec<String>,
}

impl RouteTargetInput {
    /// Whether the target is a restricted fallback rather than a member of the
    /// pool.
    fn is_restricted(&self) -> bool {
        !self.takes_over.is_empty()
    }

    /// The target as the store takes it. A blank upstream model is stored as
    /// none, the way a provider group member's is: forwarding an empty model
    /// name upstream is never what was meant, and it would match no target
    /// that stores `null`.
    fn spec(&self) -> RouteTargetSpec {
        RouteTargetSpec {
            provider_id: self.provider_id,
            upstream_model: self
                .upstream_model
                .clone()
                .filter(|model| !model.trim().is_empty()),
            weight: self.weight,
            // stored as the canonical token, which is what the input already
            // is once `validate_route_targets` has parsed it
            takes_over: self.takes_over.clone(),
        }
    }
}

fn default_weight() -> i32 {
    1
}

/// How [`validate_route_targets`] names a refused field: a lone target's body
/// is the request root, while a list entry is `targets[i]`.
#[derive(Clone, Copy)]
enum TargetPaths {
    Alone,
    InList,
}

impl TargetPaths {
    fn field(self, index: usize, name: &str) -> String {
        match self {
            Self::Alone => name.to_string(),
            Self::InList => format!("targets[{index}].{name}"),
        }
    }
}

/// The checks every route target passes before it is stored, whether it is
/// added alone or as part of a route's new target list: a positive weight, a
/// provider in the route's org (an unknown or foreign one is the same 404), and
/// one the route's project may use (#1919).
async fn validate_route_targets(
    state: &ControlState,
    org_id: Option<Uuid>,
    project_id: Uuid,
    targets: &[RouteTargetInput],
    paths: TargetPaths,
) -> ApiResult<()> {
    for (index, target) in targets.iter().enumerate() {
        if target.weight <= 0 {
            return Err(invalid_field(
                paths.field(index, "weight"),
                "weight must be > 0",
            ));
        }
        validate_takes_over(&target.takes_over, &paths.field(index, "takes_over"))?;
    }
    let provider_ids: Vec<Uuid> = targets.iter().map(|t| t.provider_id).collect();
    if let Some(org_id) = org_id {
        require_providers_in_org(state, org_id, &provider_ids).await?;
    }
    // a route may use its own project's providers and org-wide ones (#1919)
    require_providers_usable_from(state, &provider_ids, Some(project_id), "this route").await
}

/// A target's `takes_over` set: every entry a failure class, none twice.
/// `field` names the set in the refusal.
fn validate_takes_over(takes_over: &[String], field: &str) -> ApiResult<()> {
    let mut classes: Vec<FailureClass> = Vec::with_capacity(takes_over.len());
    for token in takes_over {
        let class: FailureClass = token
            .parse()
            .map_err(|problem: String| invalid_field(field, problem))?;
        if classes.contains(&class) {
            return Err(invalid_field(
                field,
                format!("takes_over lists '{class}' twice"),
            ));
        }
        classes.push(class);
    }
    Ok(())
}

/// Whether each of the route's stored variants' targets is restricted, one
/// set per variant, for [`only_restricted_dialled`]: a route with variants
/// dials only them, so they decide whether a write to its pool strands it
/// (#3487).
async fn stored_variants_restricted(
    state: &ControlState,
    route_id: Uuid,
) -> ApiResult<Vec<Vec<bool>>> {
    Ok(RouteVariantRepo(pool(state))
        .list(route_id)
        .await?
        .iter()
        .map(|variant| {
            variant
                .targets
                .iter()
                .map(|target| !target.takes_over.is_empty())
                .collect()
        })
        .collect())
}

async fn create_route_target(
    principal: Principal,
    State(state): State<ControlState>,
    Path(route_id): Path<Uuid>,
    SafeJson(body): SafeJson<RouteTargetInput>,
) -> ApiResult<Json<RouteTarget>> {
    let route = RouteRepo(pool(&state)).get(route_id).await?;
    let chain = ScopeChain::from_project(pool(&state), route.project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("route", Update)).await?;
    validate_route_targets(
        &state,
        org_id,
        route.project_id,
        std::slice::from_ref(&body),
        TargetPaths::Alone,
    )
    .await?;
    // a restricted target added to a route with no ordinary one to dial
    // would leave the route nothing it may dial first (#1584, #3487)
    if body.is_restricted() {
        let existing = RouteTargetRepo(pool(&state)).list(route_id).await?;
        if only_restricted_dialled(
            existing
                .iter()
                .map(|t| !t.takes_over.is_empty())
                .chain([true]),
            stored_variants_restricted(&state, route_id).await?,
        ) {
            return Err(invalid_field(
                "takes_over",
                all_targets_restricted(&route.model),
            ));
        }
    }
    let spec = body.spec();
    let row = RouteTargetRepo(pool(&state))
        .create(
            route_id,
            spec.provider_id,
            spec.upstream_model.as_deref(),
            spec.weight,
            &spec.takes_over,
        )
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route_target.create",
        "route_target",
        row.id,
        serde_json::json!({"route_id": route_id, "provider_id": spec.provider_id}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_route_target(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let target = RouteTargetRepo(pool(&state)).get(id).await?;
    let org_id =
        authorize_route(&state, &principal, target.route_id, cap!("route", Update)).await?;
    // removing the last ordinary target to dial would leave only restricted
    // fallbacks, which could never dial anything (#1584, #3487)
    if target.takes_over.is_empty() {
        let siblings = RouteTargetRepo(pool(&state)).list(target.route_id).await?;
        if only_restricted_dialled(
            siblings
                .iter()
                .filter(|t| t.id != id)
                .map(|t| !t.takes_over.is_empty()),
            stored_variants_restricted(&state, target.route_id).await?,
        ) {
            let route = RouteRepo(pool(&state)).get(target.route_id).await?;
            return Err(ApiError::Conflict(all_targets_restricted(&route.model)));
        }
    }
    RouteTargetRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route_target.delete",
        "route_target",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// --- weighted variants (#391) ---

/// The most variants one route may carry. A split wider than this is a
/// mistake rather than an experiment, and the bound keeps one body from
/// writing an unbounded number of rows.
const MAX_VARIANTS: usize = 32;

/// The most targets one variant may carry, for the same reason.
const MAX_VARIANT_TARGETS: usize = 32;

async fn list_route_variants(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<RouteVariant>>> {
    authorize_route(&state, &principal, id, cap!("route", Read)).await?;
    Ok(Json(RouteVariantRepo(pool(&state)).list(id).await?))
}

/// The body of `PUT /api/v1/routes/{id}/variants`: the route's whole variant
/// set, in fallback order. An empty list clears the variants and puts the
/// route back on its own target pool.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetRouteVariants {
    variants: Vec<VariantInput>,
}

/// One variant as a client sends it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantInput {
    name: String,
    #[serde(default = "default_weight")]
    weight: i32,
    /// variant-scoped default inference params, layered over the route's own;
    /// omitted or `null` is none
    #[serde(default)]
    params: serde_json::Value,
    /// in routing order
    targets: Vec<VariantTargetInput>,
}

/// One target of a [`VariantInput`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantTargetInput {
    provider_id: Uuid,
    /// the model name sent upstream; omitted, `null` or blank forwards the
    /// route's public name
    upstream_model: Option<String>,
    #[serde(default = "default_weight")]
    weight: i32,
    /// the failure classes this target is a fallback for, as on a route
    /// target (#3373). Omitted or empty, it is an ordinary member of the
    /// variant
    #[serde(default)]
    takes_over: Vec<String>,
}

/// The checks a variant set passes before any provider is looked up, and the
/// set as the store takes it: unique non-blank names, a variant weight of 0
/// or more, target weights none negative and not all zero within a variant, at
/// least one target per variant, and object params. Every refusal names
/// the field it is about, `variants[i].targets[j].weight` say.
fn variant_specs(variants: Vec<VariantInput>) -> ApiResult<Vec<RouteVariantSpec>> {
    if variants.len() > MAX_VARIANTS {
        return Err(invalid_field(
            "variants",
            format!("a route takes at most {MAX_VARIANTS} variants"),
        ));
    }
    let mut names = std::collections::HashSet::new();
    let mut specs = Vec::new();
    for (i, variant) in variants.into_iter().enumerate() {
        let name = variant.name.trim().to_string();
        if name.is_empty() {
            return Err(invalid_field(
                format!("variants[{i}].name"),
                "a variant needs a name",
            ));
        }
        if !names.insert(name.clone()) {
            return Err(invalid_field(
                format!("variants[{i}].name"),
                format!("variant name '{name}' is used twice; names must be unique in a route"),
            ));
        }
        // a 0 is a fallback-only variant, and a set of only zeroes is sampled
        // uniformly by the gateway (#3341, #3427)
        if variant.weight < 0 {
            return Err(invalid_field(
                format!("variants[{i}].weight"),
                "weight must be 0 or more",
            ));
        }
        if variant.targets.is_empty() {
            return Err(invalid_field(
                format!("variants[{i}].targets"),
                "a variant needs at least one target",
            ));
        }
        if variant.targets.len() > MAX_VARIANT_TARGETS {
            return Err(invalid_field(
                format!("variants[{i}].targets"),
                format!("a variant takes at most {MAX_VARIANT_TARGETS} targets"),
            ));
        }
        for (j, target) in variant.targets.iter().enumerate() {
            if target.weight < 0 {
                return Err(invalid_field(
                    format!("variants[{i}].targets[{j}].weight"),
                    "weight must be 0 or more",
                ));
            }
            validate_takes_over(
                &target.takes_over,
                &format!("variants[{i}].targets[{j}].takes_over"),
            )?;
        }
        if variant.targets.iter().all(|t| t.weight == 0) {
            return Err(invalid_field(
                format!("variants[{i}].targets"),
                "at least one target of a variant needs a weight above 0",
            ));
        }
        let params = normalize_json_object(variant.params, &format!("variants[{i}].params"))?;
        specs.push(RouteVariantSpec {
            name,
            weight: variant.weight,
            params,
            targets: variant
                .targets
                .into_iter()
                .map(|target| RouteVariantTargetSpec {
                    provider_id: target.provider_id,
                    // blank is none, as for a route target
                    upstream_model: target.upstream_model.filter(|m| !m.trim().is_empty()),
                    weight: target.weight,
                    // the canonical tokens, which `validate_takes_over` has
                    // just parsed
                    takes_over: target.takes_over,
                })
                .collect(),
        });
    }
    Ok(specs)
}

/// Whether a route with `pool` as its classic targets and `variants` as its
/// variant set would dial only restricted targets: the variants' targets when
/// there are any, the pool otherwise (#3487).
fn leaves_only_restricted(pool: &[RouteTarget], variants: &[RouteVariantSpec]) -> bool {
    only_restricted_dialled(
        pool.iter().map(|target| !target.takes_over.is_empty()),
        variants.iter().map(|variant| {
            variant
                .targets
                .iter()
                .map(|target| !target.takes_over.is_empty())
        }),
    )
}

async fn set_route_variants(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetRouteVariants>,
) -> ApiResult<Json<Vec<RouteVariant>>> {
    let route = RouteRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(&state), route.project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("route", Update)).await?;
    let specs = variant_specs(body.variants)?;
    // a route with variants dials only them and one without dials its pool,
    // as the gateway's validation counts them: a set that leaves the route
    // nothing but restricted fallbacks to dial could never dial anything
    // (#3373, #3487)
    let pool_targets = RouteTargetRepo(pool(&state)).list(id).await?;
    if leaves_only_restricted(&pool_targets, &specs) {
        return Err(invalid_field(
            "variants",
            all_targets_restricted(&route.model),
        ));
    }
    let mut provider_ids: Vec<Uuid> = specs
        .iter()
        .flat_map(|v| v.targets.iter().map(|t| t.provider_id))
        .collect();
    provider_ids.sort_unstable();
    provider_ids.dedup();
    // every target names a provider of the route's org (an unknown or foreign
    // one is the same 404) that the route's project may use (#1919), the
    // checks a route target passes
    if let Some(org_id) = org_id {
        require_providers_in_org(&state, org_id, &provider_ids).await?;
    }
    require_providers_usable_from(&state, &provider_ids, Some(route.project_id), "this route")
        .await?;
    let (variants, changed) = RouteVariantRepo(pool(&state)).replace(id, &specs).await?;
    // an unchanged set wrote nothing, so there is nothing to publish or audit
    if !changed {
        return Ok(Json(variants));
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_variants",
        "route",
        id,
        serde_json::json!({
            "variants": variants
                .iter()
                .map(|v| serde_json::json!({
                    "name": v.name,
                    "weight": v.weight,
                    "targets": v.targets.len(),
                }))
                .collect::<Vec<_>>(),
        }),
    )
    .await;
    Ok(Json(variants))
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    fn target(weight: i32) -> VariantTargetInput {
        VariantTargetInput {
            provider_id: Uuid::nil(),
            upstream_model: None,
            weight,
            takes_over: Vec::new(),
        }
    }

    fn variant(name: &str, weight: i32) -> VariantInput {
        VariantInput {
            name: name.to_string(),
            weight,
            params: serde_json::Value::Null,
            targets: vec![target(1)],
        }
    }

    /// the field a refusal names, or `None` when the set was accepted
    fn refused_field(variants: Vec<VariantInput>) -> Option<String> {
        match variant_specs(variants) {
            Ok(_) => None,
            Err(ApiError::InvalidField { field, .. }) => Some(field),
            Err(other) => panic!("expected a field refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_set_maps_in_order_with_blank_models_as_none() {
        let mut canary = variant(" canary ", 1);
        canary.targets = vec![
            VariantTargetInput {
                provider_id: Uuid::nil(),
                upstream_model: Some("  ".into()),
                weight: 2,
                takes_over: Vec::new(),
            },
            target(0),
        ];
        let specs = variant_specs(vec![variant("stable", 9), canary]).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "stable");
        assert_eq!(specs[1].name, "canary", "the name is trimmed");
        assert_eq!(specs[1].targets[0].upstream_model, None);
        assert_eq!(specs[1].params, serde_json::json!({}));
        assert!(
            variant_specs(Vec::new()).unwrap().is_empty(),
            "empty clears"
        );
    }

    #[test]
    fn weights_may_not_be_negative_or_all_zero() {
        assert_eq!(
            refused_field(vec![variant("a", -1)]).as_deref(),
            Some("variants[0].weight")
        );
        // a 0 is a fallback-only variant, and a set of only zeroes is allowed
        // (the gateway samples it uniformly, #3427)
        assert!(variant_specs(vec![variant("a", 3), variant("b", 0)]).is_ok());
        assert!(variant_specs(vec![variant("a", 0), variant("b", 0)]).is_ok());
        let mut negative_target = variant("a", 1);
        negative_target.targets = vec![target(1), target(-3)];
        assert_eq!(
            refused_field(vec![negative_target]).as_deref(),
            Some("variants[0].targets[1].weight")
        );
        let mut idle_targets = variant("a", 1);
        idle_targets.targets = vec![target(0), target(0)];
        assert_eq!(
            refused_field(vec![idle_targets]).as_deref(),
            Some("variants[0].targets")
        );
    }

    /// #3373: a variant target's classes are checked like a route target's,
    /// and stored as sent.
    #[test]
    fn a_variant_target_takes_over_known_classes_once() {
        let mut fallback = variant("a", 1);
        let mut burst = target(1);
        burst.takes_over = vec!["throttled".into(), "refusal:violence".into()];
        fallback.targets.push(burst);
        let specs = variant_specs(vec![fallback]).unwrap();
        assert_eq!(
            specs[0].targets[1].takes_over,
            ["throttled", "refusal:violence"]
        );

        for classes in [vec!["bogus"], vec!["timeout", "timeout"]] {
            let mut bad = variant("a", 1);
            bad.targets[0].takes_over = classes.iter().map(|c| c.to_string()).collect();
            assert_eq!(
                refused_field(vec![variant("ok", 1), bad]).as_deref(),
                Some("variants[1].targets[0].takes_over"),
                "{classes:?}"
            );
        }
    }

    #[test]
    fn each_variant_needs_a_unique_name_and_a_target() {
        assert_eq!(
            refused_field(vec![variant(" ", 1)]).as_deref(),
            Some("variants[0].name")
        );
        assert_eq!(
            refused_field(vec![variant("a", 1), variant("a ", 1)]).as_deref(),
            Some("variants[1].name")
        );
        let mut empty = variant("a", 1);
        empty.targets.clear();
        assert_eq!(
            refused_field(vec![empty]).as_deref(),
            Some("variants[0].targets")
        );
        let mut params = variant("a", 1);
        params.params = serde_json::json!([1]);
        assert_eq!(
            refused_field(vec![params]).as_deref(),
            Some("variants[0].params")
        );
        let many = (0..=MAX_VARIANTS)
            .map(|i| variant(&format!("v{i}"), 1))
            .collect();
        assert_eq!(refused_field(many).as_deref(), Some("variants"));
    }
}

#[cfg(test)]
mod advanced_model_tests {
    use super::*;

    /// a 400 refusal, coded per field or not
    fn is_config_err<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(
            res,
            Err(ApiError::Core(Error::Config(_)) | ApiError::InvalidField { .. })
        )
    }

    #[test]
    fn advanced_model_validation_rejects_unsafe_values() {
        let mut advanced = AdvancedModelConfig::default();
        advanced
            .headers
            .insert("bad header".to_string(), "x".to_string());
        assert!(is_config_err(validate_advanced(&advanced)));

        advanced.headers.clear();
        advanced.limits.output_tokens = Some(0);
        assert!(is_config_err(validate_advanced(&advanced)));

        advanced.limits.output_tokens = None;
        advanced.limits.timeout_secs = Some(0);
        assert!(is_config_err(validate_advanced(&advanced)));
    }

    /// A zero cap would refuse every request to the route, and a cap past the
    /// ceiling is a typo (#2936). Unset is "no cap", and each one is a value on
    /// its own.
    #[test]
    fn advanced_model_validation_bounds_the_throughput_caps() {
        for field in ["rpm", "tpm", "concurrency"] {
            let with = |value: Option<u32>| {
                let mut advanced = AdvancedModelConfig::default();
                match field {
                    "rpm" => advanced.limits.rpm = value,
                    "tpm" => advanced.limits.tpm = value,
                    _ => advanced.limits.concurrency = value,
                }
                validate_advanced(&advanced)
            };
            assert!(with(None).is_ok(), "{field} unset");
            assert!(with(Some(1)).is_ok(), "{field} = 1");
            assert!(is_config_err(with(Some(0))), "{field} = 0");
        }
        let mut advanced = AdvancedModelConfig::default();
        advanced.limits.rpm = Some(rolter_core::MAX_ROUTE_THROUGHPUT + 1);
        assert!(is_config_err(validate_advanced(&advanced)));
        advanced.limits.rpm = Some(rolter_core::MAX_ROUTE_THROUGHPUT);
        advanced.limits.concurrency = Some(rolter_core::MAX_ROUTE_THROUGHPUT + 1);
        assert!(is_config_err(validate_advanced(&advanced)));
    }

    /// A route cannot present a credential of its own in place of the
    /// provider's, nor a header the wire cannot carry (#2924).
    #[test]
    fn advanced_model_validation_refuses_credential_and_unsendable_headers() {
        for (name, value) in [
            ("Authorization", "Bearer sk-route"),
            ("x-api-key", "sk-route"),
            ("api-key", "sk-route"),
            ("x-goog-api-key", "sk-route"),
            ("host", "elsewhere.example"),
            ("content-length", "0"),
            ("x-model-region", "a\nb"),
        ] {
            let mut advanced = AdvancedModelConfig::default();
            advanced.headers.insert(name.to_string(), value.to_string());
            assert!(
                is_config_err(validate_advanced(&advanced)),
                "{name} should be refused"
            );
        }
        let mut advanced = AdvancedModelConfig::default();
        advanced
            .headers
            .insert("x-model-region".to_string(), "eu".to_string());
        advanced.locked_headers.push("x-model-region".to_string());
        assert!(validate_advanced(&advanced).is_ok());
        advanced.locked_headers.push("not a name".to_string());
        assert!(is_config_err(validate_advanced(&advanced)));
    }

    /// Zero is a value for `retries` (it turns retries off for the route) and
    /// not for the other limits, and nothing may exceed the ceiling the gateway
    /// applies.
    #[test]
    fn advanced_model_validation_bounds_the_retry_budget() {
        let with = |retries| {
            let mut advanced = AdvancedModelConfig::default();
            advanced.limits.retries = retries;
            validate_advanced(&advanced)
        };
        assert!(with(None).is_ok());
        assert!(with(Some(0)).is_ok());
        assert!(with(Some(MAX_ROUTE_RETRIES)).is_ok());
        assert!(is_config_err(with(Some(MAX_ROUTE_RETRIES + 1))));
    }
}
