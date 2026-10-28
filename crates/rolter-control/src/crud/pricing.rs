//! Model pricing catalog: per-token rates the cost accounting reads.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rolter_core::Error;
use rolter_store::postgres::models::ModelPrice;
use rolter_store::postgres::repo::{CatalogueImport, ModelPriceRepo, NewModelPrice, UnitRates};

use super::{
    explicit_null, invalid_field, pool, require_non_empty, ApiError, ApiResult, Query, SafeJson,
};
use crate::price_catalogue::{default_catalogue, Catalogue, CataloguePrice};
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain};
use crate::rbac_matrix::{cap, superadmin_cap};
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/model-prices",
            get(list_model_prices).put(upsert_model_price),
        )
        .route("/api/v1/model-prices/{model}", delete(delete_model_price))
        .route("/api/v1/model-prices/catalogue", get(list_price_catalogue))
        .route("/api/v1/model-prices/import", post(import_price_catalogue))
}

// --- model pricing catalog ---

async fn list_model_prices(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<ModelPrice>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_price", Read),
    )
    .await?;
    Ok(Json(ModelPriceRepo(pool(&state)).list().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpsertModelPrice {
    model: String,
    input_per_mtok: String,
    output_per_mtok: String,
    cached_input_per_mtok: Option<String>,
    /// omit to leave the stored rate unchanged; `null` clears it back to the
    /// input rate; a number sets it. Tri-state, unlike the rates beside it,
    /// because a client that predates the field (the dashboard until its half
    /// landed) replaces the whole row with a body that never names it, and that
    /// must not reset a rate set since (#2876)
    #[serde(default, deserialize_with = "explicit_null")]
    cache_write_per_mtok: Option<Option<String>>,
    /// the rate for tokens written to the 1 hour prompt cache (#2891), with the
    /// same tri-state as `cache_write_per_mtok` and for the same reason; `null`
    /// clears it back to the 5 minute write rate
    #[serde(default, deserialize_with = "explicit_null")]
    cache_write_1h_per_mtok: Option<Option<String>>,
    /// price of one generated image (#2937), tri-state like the cache write
    /// rates: a client that predates the field replaces the row with a body
    /// that never names it, and that must not clear a rate set since
    #[serde(default, deserialize_with = "explicit_null")]
    image_per_unit: Option<Option<String>>,
    /// price of one minute of input audio (#2937), same tri-state
    #[serde(default, deserialize_with = "explicit_null")]
    audio_input_per_minute: Option<Option<String>>,
    /// price per million characters of text-to-speech input (#2937), same
    /// tri-state
    #[serde(default, deserialize_with = "explicit_null")]
    speech_per_mchar: Option<Option<String>>,
    /// price per million audio input tokens (#1882), same tri-state: `null`
    /// clears it back to the text input rate
    #[serde(default, deserialize_with = "explicit_null")]
    audio_input_per_mtok: Option<Option<String>>,
    /// price per million audio output tokens (#1882), same tri-state: `null`
    /// clears it back to the text output rate
    #[serde(default, deserialize_with = "explicit_null")]
    audio_output_per_mtok: Option<Option<String>>,
    /// price per million cached audio input tokens (#1882), same tri-state
    #[serde(default, deserialize_with = "explicit_null")]
    cached_audio_input_per_mtok: Option<Option<String>>,
    #[serde(default = "default_currency")]
    currency: String,
}

fn default_currency() -> String {
    "USD".to_string()
}

/// Reject a model price in a currency the deployment cannot convert.
///
/// The dashboard has always let an operator pick a currency, but until #650 it
/// was a display label the cost engine ignored — a EUR price was charged as if
/// it were USD. Now it is real, which means a code with no rate has to be
/// refused here: the alternative is storing a price nothing can convert and
/// discovering it as wrong spend later.
fn require_known_currency(state: &ControlState, currency: &str) -> ApiResult<()> {
    require_non_empty(currency, "currency")?;
    if state.currency.rate(currency).is_some() {
        return Ok(());
    }
    Err(ApiError::Core(Error::Config(format!(
        "currency '{}' has no conversion rate configured (base is '{}'); \
         add it to [currency.rates] in the bootstrap config, or price in the base currency",
        currency.trim(),
        state.currency.base
    ))))
}

/// The first value the `numeric(12, 6)` rate columns cannot hold: one that
/// rounds to a million at six places overflows, and the database error would
/// reach the client as a 500 (#2898).
const RATE_CEILING: f64 = 999_999.999_999_5;

/// A price rate must be a real number the column can store, and not below zero.
/// `parse::<f64>` accepts `NaN` and `inf`, which `numeric` would store (`NaN`
/// at least) and the snapshot loader could not read back, and a negative rate
/// would turn a request into a credit against a budget (#2889).
pub(super) fn require_rate(value: &str, field: &str) -> ApiResult<()> {
    match value.trim().parse::<f64>() {
        Ok(rate) if rate.is_finite() && (0.0..RATE_CEILING).contains(&rate) => Ok(()),
        Ok(_) => Err(invalid_field(
            field,
            format!("{field} must be a number from 0 up to 999999.999999"),
        )),
        Err(_) => Err(invalid_field(field, format!("{field} must be numeric"))),
    }
}

// the pricing catalog is a global (unscoped) resource, so its mutations are
// superadmin-only
async fn upsert_model_price(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpsertModelPrice>,
) -> ApiResult<Json<ModelPrice>> {
    authorize_superadmin(&principal, superadmin_cap!("model_price", Update))?;
    require_non_empty(&body.model, "model")?;
    require_rate(&body.input_per_mtok, "input_per_mtok")?;
    require_rate(&body.output_per_mtok, "output_per_mtok")?;
    if let Some(cached) = &body.cached_input_per_mtok {
        require_rate(cached, "cached_input_per_mtok")?;
    }
    if let Some(Some(write)) = &body.cache_write_per_mtok {
        require_rate(write, "cache_write_per_mtok")?;
    }
    if let Some(Some(write)) = &body.cache_write_1h_per_mtok {
        require_rate(write, "cache_write_1h_per_mtok")?;
    }
    for (rate, field) in [
        (&body.image_per_unit, "image_per_unit"),
        (&body.audio_input_per_minute, "audio_input_per_minute"),
        (&body.speech_per_mchar, "speech_per_mchar"),
        (&body.audio_input_per_mtok, "audio_input_per_mtok"),
        (&body.audio_output_per_mtok, "audio_output_per_mtok"),
        (
            &body.cached_audio_input_per_mtok,
            "cached_audio_input_per_mtok",
        ),
    ] {
        if let Some(Some(rate)) = rate {
            require_rate(rate, field)?;
        }
    }
    require_known_currency(&state, &body.currency)?;
    Ok(Json(
        ModelPriceRepo(pool(&state))
            .upsert(NewModelPrice {
                model: &body.model,
                input_per_mtok: &body.input_per_mtok,
                output_per_mtok: &body.output_per_mtok,
                cached_input_per_mtok: body.cached_input_per_mtok.as_deref(),
                cache_write_per_mtok: body.cache_write_per_mtok.as_ref().map(Option::as_deref),
                cache_write_1h_per_mtok: body
                    .cache_write_1h_per_mtok
                    .as_ref()
                    .map(Option::as_deref),
                units: UnitRates {
                    image_per_unit: body.image_per_unit.as_ref().map(Option::as_deref),
                    audio_input_per_minute: body
                        .audio_input_per_minute
                        .as_ref()
                        .map(Option::as_deref),
                    speech_per_mchar: body.speech_per_mchar.as_ref().map(Option::as_deref),
                    audio_input_per_mtok: body.audio_input_per_mtok.as_ref().map(Option::as_deref),
                    audio_output_per_mtok: body
                        .audio_output_per_mtok
                        .as_ref()
                        .map(Option::as_deref),
                    cached_audio_input_per_mtok: body
                        .cached_audio_input_per_mtok
                        .as_ref()
                        .map(Option::as_deref),
                },
                currency: &body.currency,
            })
            .await?,
    ))
}

async fn delete_model_price(
    principal: Principal,
    State(state): State<ControlState>,
    Path(model): Path<String>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("model_price", Delete))?;
    ModelPriceRepo(pool(&state)).delete(&model).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- price catalogue import (#966) ---

/// Most models one import names, so a request cannot hold a transaction open
/// over the whole catalogue.
const MAX_IMPORT_MODELS: usize = 500;

/// Most catalogue entries one listing returns.
const MAX_CATALOGUE_LIST: usize = 200;

fn catalogue() -> ApiResult<&'static Catalogue> {
    default_catalogue().map_err(|e| ApiError::Core(Error::Config(e)))
}

#[derive(Deserialize)]
struct CatalogueQuery {
    /// a substring of the model id; every model when omitted
    q: Option<String>,
    /// at most this many entries (default and ceiling 200)
    limit: Option<usize>,
}

#[derive(Serialize)]
struct CatalogueListing<'a> {
    /// the name stored as `source` on a row imported from it
    source: &'a str,
    licence: &'a str,
    url: &'a str,
    currency: &'a str,
    /// the day the vendored snapshot was taken; prices drift from it
    snapshot: String,
    /// how many models the whole catalogue prices
    total: usize,
    entries: Vec<&'a CataloguePrice>,
}

/// What the importer can offer: the vendored catalogue, searchable, with its
/// provenance. Read-only and offline.
async fn list_price_catalogue(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<CatalogueQuery>,
) -> ApiResult<Json<CatalogueListing<'static>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_price", Read),
    )
    .await?;
    let catalogue = catalogue()?;
    let limit = query
        .limit
        .unwrap_or(MAX_CATALOGUE_LIST)
        .min(MAX_CATALOGUE_LIST);
    Ok(Json(CatalogueListing {
        source: catalogue.source,
        licence: &catalogue.licence,
        url: &catalogue.url,
        currency: catalogue.currency,
        snapshot: catalogue.snapshot.to_string(),
        total: catalogue.model_count(),
        entries: catalogue.search(query.q.as_deref().unwrap_or(""), limit),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportPrices {
    /// catalogue model ids to import; an id the catalogue does not price is
    /// reported, not an error
    models: Vec<String>,
}

#[derive(Serialize)]
struct ImportResult {
    model: String,
    /// `created`, `updated`, `kept_override` (an operator's own price, left
    /// alone) or `not_in_catalogue`
    outcome: &'static str,
}

#[derive(Serialize)]
struct ImportReport {
    source: &'static str,
    currency: &'static str,
    snapshot: String,
    results: Vec<ImportResult>,
}

/// Import the named models from the vendored catalogue. A price an operator
/// set is never replaced; an earlier catalogue price is refreshed. Imported
/// rows are stamped with their source and the snapshot's date, and editing one
/// afterwards turns it into an operator override.
async fn import_price_catalogue(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<ImportPrices>,
) -> ApiResult<Json<ImportReport>> {
    authorize_superadmin(&principal, superadmin_cap!("model_price", Update))?;
    if body.models.is_empty() {
        return Err(invalid_field("models", "name at least one model to import"));
    }
    if body.models.len() > MAX_IMPORT_MODELS {
        return Err(invalid_field(
            "models",
            format!("import at most {MAX_IMPORT_MODELS} models at a time"),
        ));
    }
    let catalogue = catalogue()?;
    // the catalogue's currency is stated, not assumed, and has to be one the
    // deployment can convert, like a hand-entered price
    require_known_currency(&state, catalogue.currency)?;
    let mut wanted = Vec::new();
    let mut missing = Vec::new();
    for model in &body.models {
        match catalogue.get(model.trim()) {
            Some(price)
                if !wanted
                    .iter()
                    .any(|p: &CataloguePrice| p.model == price.model) =>
            {
                wanted.push(price.clone())
            }
            Some(_) => {}
            None => missing.push(model.clone()),
        }
    }
    let outcomes = ModelPriceRepo(pool(&state))
        .import_catalogue(catalogue.source, catalogue.imported_at(), &wanted)
        .await?;
    let mut results: Vec<ImportResult> = outcomes
        .into_iter()
        .map(|(model, outcome)| ImportResult {
            model,
            outcome: match outcome {
                CatalogueImport::Created => "created",
                CatalogueImport::Updated => "updated",
                CatalogueImport::KeptOverride => "kept_override",
            },
        })
        .collect();
    results.extend(missing.into_iter().map(|model| ImportResult {
        model,
        outcome: "not_in_catalogue",
    }));
    Ok(Json(ImportReport {
        source: catalogue.source,
        currency: catalogue.currency,
        snapshot: catalogue.snapshot.to_string(),
        results,
    }))
}

#[cfg(test)]
mod price_rate_tests {
    use super::*;

    #[test]
    fn a_price_rate_must_be_a_finite_non_negative_number() {
        // "-0" is zero, which is a rate: a self-hosted model costs nothing
        for good in [
            "0",
            "-0",
            "0.000000",
            "3.75",
            " 2 ",
            "1e3",
            ".5",
            "999999.999999",
            "999999.9999994",
        ] {
            assert!(require_rate(good, "input_per_mtok").is_ok(), "{good:?}");
        }
        for bad in [
            "-1",
            "-0.000001",
            "NaN",
            "nan",
            "inf",
            "-inf",
            "Infinity",
            "1e999",
            // what `numeric(12, 6)` cannot hold, which was a 500 (#2898)
            "999999.9999995",
            "1000000",
            "1e20",
            "",
            "abc",
        ] {
            assert!(
                matches!(
                    require_rate(bad, "input_per_mtok"),
                    Err(ApiError::InvalidField { field, .. }) if field == "input_per_mtok"
                ),
                "{bad:?} should be refused"
            );
        }
    }
}
