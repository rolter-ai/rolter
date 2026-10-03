//! Saved filter presets (#1825): a signed-in account names the filter set it
//! has built on the **LLM Logs** or **Dashboard** screen, and applies it again
//! on its next visit instead of rebuilding it.
//!
//! Presets are private to their owner. Every route authenticates through
//! [`CurrentUser`] and every store call is keyed by the session's own user id,
//! so there is no way to name, list or share another account's preset; one that
//! belongs to someone else answers `404` exactly as an id that does not exist
//! does. Any role may use them, a viewer included: a preset only remembers
//! query parameters the caller could already send.
//!
//! # What a preset may hold
//!
//! `filters` is an object whose keys are allow-listed per surface and mirror
//! the query parameters the screen sends; a key outside the list, or a value
//! of the wrong shape, is refused rather than stored, so a preset can never
//! carry an arbitrary parameter back to the analytics routes:
//!
//! * `llm_logs` mirrors `GET /api/v1/analytics/invocations`: `window`,
//!   `status`, `model`, `key`, `business_unit` and `customer`.
//! * `dashboard` mirrors the summary / timeseries / by-model routes the
//!   dashboard calls: `window`, `bucket`, `model`, `key`, `business_unit` and
//!   `customer` (#2453). The four row filters take the same shapes as on
//!   `llm_logs`, because the routes read them as the invocation list does.
//!
//! `window` is a rolling window's *name* (`24h`, `7d`, ...), never a pair of
//! timestamps: a saved "last 7 days" has to mean the seven days before it is
//! applied, not before it was saved.
//!
//! # What a preset that outlived its access says
//!
//! A preset can name a virtual key, business unit or customer the caller can
//! no longer read (their membership was removed, or the row was deleted).
//! Every read returns the stored `filters` untouched, `effective_filters`
//! with those ids removed, and `unavailable` naming what was removed, so the
//! screen applies the rest and says what it dropped. A model name or a window
//! is never unavailable. Readability is decided by [`ScopeFilter`], the rule
//! the CRUD listings of those resources apply, never by a second copy of it.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::SavedView;
use rolter_store::postgres::repo::{
    AuditLogRepo, BusinessUnitRepo, CustomerRepo, SavedViewRepo, SavedViewWrite, VirtualKeyRepo,
};

use crate::auth::CurrentUser;
use crate::crud::{name_taken, pool, ApiError, ApiResult, SafeJson};
use crate::me::bad_request;
use crate::rbac::{Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// presets one account may keep per surface
pub(crate) const MAX_VIEWS_PER_SURFACE: i64 = 50;
/// longest preset name, in characters; matches `saved_views_name_shape`
const MAX_NAME_LEN: usize = 80;
/// longest model name a filter may carry
const MAX_MODEL_LEN: usize = 200;
/// most ids one `business_unit` / `customer` filter may list
const MAX_IDS: usize = 100;

/// the reporting windows the dashboard names; `ui/src/lib/time-window.ts` is
/// the other copy, and a window this list lacks is refused, not passed on
const WINDOWS: &[&str] = &["24h", "7d", "30d", "mtd", "last-month"];

const SURFACES: &[&str] = &["llm_logs", "dashboard"];

const LLM_LOGS_KEYS: &[&str] = &[
    "window",
    "status",
    "model",
    "key",
    "business_unit",
    "customer",
];
const DASHBOARD_KEYS: &[&str] = &[
    "window",
    "bucket",
    "model",
    "key",
    "business_unit",
    "customer",
];

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/me/saved-views", get(list_views).post(create_view))
        .route(
            "/api/v1/me/saved-views/{id}",
            get(get_view).patch(update_view).delete(delete_view),
        )
}

fn allowed_keys(surface: &str) -> &'static [&'static str] {
    match surface {
        "llm_logs" => LLM_LOGS_KEYS,
        _ => DASHBOARD_KEYS,
    }
}

fn check_surface(surface: &str) -> Result<(), ApiError> {
    if SURFACES.contains(&surface) {
        Ok(())
    } else {
        Err(bad_request(&format!(
            "surface must be one of: {}",
            SURFACES.join(", ")
        )))
    }
}

fn check_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(bad_request("name must not be empty"));
    }
    if name.chars().any(char::is_control) {
        return Err(bad_request("name must not contain control characters"));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(bad_request(&format!(
            "name must be at most {MAX_NAME_LEN} characters"
        )));
    }
    Ok(name.to_string())
}

fn one_of(
    field: &str,
    value: &Value,
    allowed: impl Fn(&str) -> bool,
    hint: &str,
) -> ApiResult<Value> {
    match value.as_str() {
        Some(text) if allowed(text) => Ok(Value::String(text.to_string())),
        _ => Err(bad_request(&format!(
            "filter '{field}' must be one of: {hint}"
        ))),
    }
}

fn uuid_of(field: &str, value: &Value) -> ApiResult<Uuid> {
    value
        .as_str()
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or_else(|| bad_request(&format!("filter '{field}' must hold uuid strings")))
}

/// Validate a filter set for `surface` and return it in the form that is
/// stored: only allow-listed keys, `null` values dropped, ids in canonical
/// form, id lists de-duplicated with empty ones dropped.
pub(crate) fn validate_filters(surface: &str, filters: &Value) -> ApiResult<Map<String, Value>> {
    let Some(map) = filters.as_object() else {
        return Err(bad_request("filters must be a json object"));
    };
    let allowed = allowed_keys(surface);
    let mut out = Map::new();
    for (name, value) in map {
        if !allowed.contains(&name.as_str()) {
            return Err(bad_request(&format!(
                "unknown filter '{name}' for surface {surface}; allowed: {}",
                allowed.join(", ")
            )));
        }
        // null means "not set", the same as leaving the key out
        if value.is_null() {
            continue;
        }
        let checked = match name.as_str() {
            "window" => one_of(name, value, |w| WINDOWS.contains(&w), &WINDOWS.join(", "))?,
            "status" => one_of(
                name,
                value,
                |s| crate::analytics::status_predicate(s).is_some(),
                "all, error, success",
            )?,
            "bucket" => one_of(
                name,
                value,
                |b| crate::analytics::bucket_fn(b).is_some(),
                "hour, day, week, month",
            )?,
            "model" => {
                let model = value.as_str().map(str::trim).unwrap_or_default();
                if model.is_empty()
                    || model.chars().count() > MAX_MODEL_LEN
                    || model.chars().any(char::is_control)
                {
                    return Err(bad_request(&format!(
                        "filter 'model' must be a model name of 1 to {MAX_MODEL_LEN} characters"
                    )));
                }
                Value::String(model.to_string())
            }
            "key" => Value::String(uuid_of(name, value)?.to_string()),
            // business_unit and customer
            _ => {
                let Some(items) = value.as_array() else {
                    return Err(bad_request(&format!(
                        "filter '{name}' must be a list of uuid strings"
                    )));
                };
                if items.len() > MAX_IDS {
                    return Err(bad_request(&format!(
                        "filter '{name}' may list at most {MAX_IDS} ids"
                    )));
                }
                let mut ids: Vec<String> = Vec::new();
                for item in items {
                    let id = uuid_of(name, item)?.to_string();
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                if ids.is_empty() {
                    continue;
                }
                json!(ids)
            }
        };
        out.insert(name.clone(), checked);
    }
    Ok(out)
}

/// A filter entry the caller can no longer read.
#[derive(Serialize, Debug, PartialEq)]
struct Unavailable {
    /// the filter key: `key`, `business_unit` or `customer`
    filter: &'static str,
    id: Uuid,
}

#[derive(Serialize)]
struct SavedViewResponse {
    id: Uuid,
    surface: String,
    name: String,
    /// as stored
    filters: Value,
    /// `filters` without the entries in `unavailable`: what to apply
    effective_filters: Value,
    unavailable: Vec<Unavailable>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Key,
    BusinessUnit,
    Customer,
}

impl Kind {
    fn of(filter: &str) -> Option<(&'static str, Kind)> {
        match filter {
            "key" => Some(("key", Kind::Key)),
            "business_unit" => Some(("business_unit", Kind::BusinessUnit)),
            "customer" => Some(("customer", Kind::Customer)),
            _ => None,
        }
    }
}

/// Decides, once per id per request, whether the caller can still read what a
/// preset names. The caller's memberships and grants load on the first
/// question, so a screen whose presets name nothing pays for none of it.
struct Readability<'a> {
    state: &'a ControlState,
    user: &'a rolter_store::postgres::models::User,
    scope: Option<ScopeFilter>,
    seen: HashMap<(Kind, Uuid), bool>,
}

impl<'a> Readability<'a> {
    fn new(state: &'a ControlState, user: &'a rolter_store::postgres::models::User) -> Self {
        Self {
            state,
            user,
            scope: None,
            seen: HashMap::new(),
        }
    }

    /// Whether the caller may read the resource, by the same rule its CRUD
    /// listing applies: the capability's floor at the resource's own scope.
    /// A row that no longer exists is unreadable.
    async fn can_read(&mut self, kind: Kind, id: Uuid) -> ApiResult<bool> {
        if let Some(known) = self.seen.get(&(kind, id)) {
            return Ok(*known);
        }
        let pool = pool(self.state);
        let chain = match kind {
            Kind::Key => match VirtualKeyRepo(pool).get(id).await {
                Ok(key) => ScopeChain::from_project(pool, key.project_id).await,
                Err(err) => Err(err.into()),
            },
            Kind::BusinessUnit => BusinessUnitRepo(pool)
                .get(id)
                .await
                .map(|unit| ScopeChain::org(unit.org_id))
                .map_err(ApiError::from),
            Kind::Customer => CustomerRepo(pool)
                .get(id)
                .await
                .map(|customer| ScopeChain::org(customer.org_id))
                .map_err(ApiError::from),
        };
        let chain = match chain {
            Ok(chain) => chain,
            Err(ApiError::Core(Error::NotFound(_))) => {
                self.seen.insert((kind, id), false);
                return Ok(false);
            }
            Err(other) => return Err(other),
        };
        if self.scope.is_none() {
            let principal = Principal::for_user(self.user.clone());
            self.scope =
                Some(ScopeFilter::load(self.state, &principal, cap!("virtual_key", Read)).await?);
        }
        let requirement = match kind {
            Kind::Key => cap!("virtual_key", Read),
            Kind::BusinessUnit => cap!("business_unit", Read),
            Kind::Customer => cap!("customer", Read),
        };
        let allowed = self
            .scope
            .as_ref()
            .is_some_and(|scope| scope.allows_as(chain, requirement));
        self.seen.insert((kind, id), allowed);
        Ok(allowed)
    }

    async fn present(&mut self, view: SavedView) -> ApiResult<SavedViewResponse> {
        let mut effective = Map::new();
        let mut unavailable = Vec::new();
        let stored = view.filters.as_object().cloned().unwrap_or_default();
        for (name, value) in &stored {
            let Some((filter, kind)) = Kind::of(name) else {
                effective.insert(name.clone(), value.clone());
                continue;
            };
            // `key` is one id, the others are lists; a value this build did not
            // write and cannot read is passed through untouched
            let ids: Vec<&Value> = match value {
                Value::Array(items) => items.iter().collect(),
                single => vec![single],
            };
            let mut kept = Vec::new();
            for item in ids {
                let id = item.as_str().and_then(|text| Uuid::parse_str(text).ok());
                match id {
                    Some(id) if !self.can_read(kind, id).await? => {
                        unavailable.push(Unavailable { filter, id });
                    }
                    _ => kept.push(item.clone()),
                }
            }
            match value {
                Value::Array(_) if !kept.is_empty() => {
                    effective.insert(name.clone(), Value::Array(kept));
                }
                Value::Array(_) => {}
                _ => {
                    if let Some(item) = kept.into_iter().next() {
                        effective.insert(name.clone(), item);
                    }
                }
            }
        }
        Ok(SavedViewResponse {
            id: view.id,
            surface: view.surface,
            name: view.name,
            filters: view.filters,
            effective_filters: Value::Object(effective),
            unavailable,
            created_at: view.created_at,
            updated_at: view.updated_at,
        })
    }
}

#[derive(Deserialize)]
struct ListQuery {
    surface: Option<String>,
}

/// the caller's own presets, oldest first, optionally for one surface
async fn list_views(
    current: CurrentUser,
    State(state): State<ControlState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<SavedViewResponse>>> {
    if let Some(surface) = &query.surface {
        check_surface(surface)?;
    }
    let views = SavedViewRepo(pool(&state))
        .list(current.user.id, query.surface.as_deref())
        .await?;
    let mut readable = Readability::new(&state, &current.user);
    let mut out = Vec::with_capacity(views.len());
    for view in views {
        out.push(readable.present(view).await?);
    }
    Ok(Json(out))
}

fn not_found(id: Uuid) -> ApiError {
    ApiError::Core(Error::NotFound(format!("saved view {id}")))
}

async fn get_view(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SavedViewResponse>> {
    let view = SavedViewRepo(pool(&state))
        .get(current.user.id, id)
        .await?
        .ok_or_else(|| not_found(id))?;
    Ok(Json(
        Readability::new(&state, &current.user)
            .present(view)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateView {
    surface: String,
    name: String,
    #[serde(default = "empty_object")]
    filters: Value,
}

fn empty_object() -> Value {
    json!({})
}

/// the id, surface and name only: what a person filtered by is theirs
async fn audit(state: &ControlState, user: Uuid, action: &str, view: &SavedView, extra: Value) {
    let mut detail = json!({ "surface": view.surface, "name": view.name });
    if let (Some(detail), Some(extra)) = (detail.as_object_mut(), extra.as_object()) {
        detail.extend(extra.clone());
    }
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            None,
            Some(user),
            action,
            Some("saved_view"),
            Some(view.id),
            Some(detail),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write saved view audit entry");
    }
}

fn collision(outcome: SavedViewWrite, id: Option<Uuid>) -> ApiError {
    match outcome {
        SavedViewWrite::DuplicateName => {
            name_taken("a saved view with that name already exists on this surface")
        }
        SavedViewWrite::LimitReached => ApiError::Conflict(format!(
            "at most {MAX_VIEWS_PER_SURFACE} saved views per surface; delete one first"
        )),
        _ => match id {
            Some(id) => not_found(id),
            None => ApiError::Core(Error::Store("saved view write fell through".to_string())),
        },
    }
}

async fn create_view(
    current: CurrentUser,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateView>,
) -> ApiResult<Json<SavedViewResponse>> {
    check_surface(&body.surface)?;
    let name = check_name(&body.name)?;
    let filters = Value::Object(validate_filters(&body.surface, &body.filters)?);
    let outcome = SavedViewRepo(pool(&state))
        .create(
            current.user.id,
            &body.surface,
            &name,
            &filters,
            MAX_VIEWS_PER_SURFACE,
        )
        .await?;
    let SavedViewWrite::Saved(view) = outcome else {
        return Err(collision(outcome, None));
    };
    audit(
        &state,
        current.user.id,
        "user.saved_view.create",
        &view,
        json!({}),
    )
    .await;
    Ok(Json(
        Readability::new(&state, &current.user)
            .present(view)
            .await?,
    ))
}

/// `name` renames, `filters` replaces the whole filter set; either or both may
/// be sent, and a body with neither is refused
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateView {
    name: Option<String>,
    filters: Option<Value>,
}

async fn update_view(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateView>,
) -> ApiResult<Json<SavedViewResponse>> {
    if body.name.is_none() && body.filters.is_none() {
        return Err(bad_request("send a name, filters, or both"));
    }
    let repo = SavedViewRepo(pool(&state));
    // the surface decides which filter keys are allowed, and it cannot change
    let existing = repo
        .get(current.user.id, id)
        .await?
        .ok_or_else(|| not_found(id))?;
    let name = body.name.as_deref().map(check_name).transpose()?;
    let filters = body
        .filters
        .as_ref()
        .map(|filters| validate_filters(&existing.surface, filters).map(Value::Object))
        .transpose()?;
    let outcome = repo
        .update(current.user.id, id, name.as_deref(), filters.as_ref())
        .await?;
    let SavedViewWrite::Saved(view) = outcome else {
        return Err(collision(outcome, Some(id)));
    };
    let mut changed = Vec::new();
    if name.is_some() {
        changed.push("name");
    }
    if filters.is_some() {
        changed.push("filters");
    }
    audit(
        &state,
        current.user.id,
        "user.saved_view.update",
        &view,
        json!({ "changed": changed }),
    )
    .await;
    Ok(Json(
        Readability::new(&state, &current.user)
            .present(view)
            .await?,
    ))
}

async fn delete_view(
    current: CurrentUser,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = SavedViewRepo(pool(&state));
    let view = repo
        .get(current.user.id, id)
        .await?
        .ok_or_else(|| not_found(id))?;
    if !repo.delete(current.user.id, id).await? {
        return Err(not_found(id));
    }
    audit(
        &state,
        current.user.id,
        "user.saved_view.delete",
        &view,
        json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(surface: &str, filters: Value) -> Map<String, Value> {
        validate_filters(surface, &filters).expect("valid filters")
    }

    fn refused(surface: &str, filters: Value) -> bool {
        validate_filters(surface, &filters).is_err()
    }

    #[test]
    fn llm_logs_accepts_every_filter_the_screen_sends() {
        let id = "0b9f3c1e-6f1a-4a7e-9a52-0f7d6c1a2b3c";
        let got = ok(
            "llm_logs",
            json!({
                "window": "7d", "status": "error", "model": " gpt-4o ",
                "key": id.to_uppercase(), "business_unit": [id, id], "customer": [id],
            }),
        );
        assert_eq!(got["model"], "gpt-4o");
        // ids are stored canonically and de-duplicated
        assert_eq!(got["key"], id);
        assert_eq!(got["business_unit"], json!([id]));
    }

    #[test]
    fn dashboard_takes_the_window_bucket_and_the_row_filters() {
        let id = "0b9f3c1e-6f1a-4a7e-9a52-0f7d6c1a2b3c";
        let got = ok(
            "dashboard",
            json!({
                "window": "mtd", "bucket": "day", "model": " gpt-4o ",
                "key": id.to_uppercase(), "business_unit": [id, id], "customer": [id],
            }),
        );
        assert_eq!(got.len(), 6);
        // the same normalisation llm_logs applies
        assert_eq!(got["model"], "gpt-4o");
        assert_eq!(got["key"], id);
        assert_eq!(got["business_unit"], json!([id]));
        assert_eq!(got["customer"], json!([id]));
    }

    #[test]
    fn dashboard_refuses_what_its_routes_do_not_read() {
        // status is the invocation list's alone; the rollups count errors
        assert!(refused("dashboard", json!({"status": "error"})));
        assert!(refused("dashboard", json!({"limit": 50})));
        assert!(refused(
            "dashboard",
            json!({"since": "2020-01-01T00:00:00Z"})
        ));
        // and the filters it does read keep llm_logs' shapes
        assert!(refused("dashboard", json!({"key": "not-a-uuid"})));
        assert!(refused(
            "dashboard",
            json!({"business_unit": "0b9f3c1e-6f1a-4a7e-9a52-0f7d6c1a2b3c"})
        ));
        assert!(refused("dashboard", json!({"customer": ["nope"]})));
        assert!(refused("dashboard", json!({"model": ""})));
        // and llm_logs does not take the dashboard's bucket
        assert!(refused("llm_logs", json!({"bucket": "day"})));
    }

    #[test]
    fn a_key_outside_the_allow_list_is_refused() {
        assert!(refused("llm_logs", json!({"limit": 500})));
        assert!(refused("llm_logs", json!({"cursor": "x"})));
        assert!(refused("llm_logs", json!({"request_id": "x"})));
        assert!(refused(
            "llm_logs",
            json!({"since": "2020-01-01T00:00:00Z"})
        ));
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_refused() {
        assert!(refused("llm_logs", json!({"window": "forever"})));
        assert!(refused("llm_logs", json!({"window": 7})));
        assert!(refused("llm_logs", json!({"status": "error; drop table"})));
        assert!(refused("llm_logs", json!({"key": "not-a-uuid"})));
        assert!(refused(
            "llm_logs",
            json!({"business_unit": "0b9f3c1e-6f1a-4a7e-9a52-0f7d6c1a2b3c"})
        ));
        assert!(refused("llm_logs", json!({"customer": ["nope"]})));
        assert!(refused("llm_logs", json!({"model": ""})));
        assert!(refused("dashboard", json!({"bucket": "year"})));
        assert!(validate_filters("llm_logs", &json!([])).is_err());
    }

    #[test]
    fn null_and_empty_lists_mean_not_set() {
        let got = ok("llm_logs", json!({"model": null, "customer": []}));
        assert!(got.is_empty());
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(check_name("  Errors  ").ok().as_deref(), Some("Errors"));
        assert!(check_name("   ").is_err());
        assert!(check_name(&"x".repeat(81)).is_err());
        assert!(check_name("bad\nname").is_err());
    }
}
