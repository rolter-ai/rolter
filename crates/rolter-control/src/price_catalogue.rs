//! Model price catalogues the importer reads (#966).
//!
//! A catalogue is a source adapter: it turns one vendor file into
//! [`CataloguePrice`]s in a stated currency, and the importer
//! (`crud::pricing`) knows nothing else about it. The first adapter is
//! LiteLLM's `model_prices_and_context_window.json` (MIT), vendored under
//! `catalogue/` and cut down by `just refresh-prices`; the choice is
//! provisional, so replacing it means writing another [`PriceSource`], not
//! touching the importer.
//!
//! The snapshot is compiled into the binary. Nothing is fetched at runtime, so
//! an air-gapped control plane imports exactly what a connected one does.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde::Deserialize;

pub use rolter_store::postgres::repo::CataloguePrice;

/// The highest rate the `numeric(12, 6)` columns hold; a catalogue value at or
/// above it is skipped rather than failing the whole import.
const RATE_CEILING: i64 = 999_999;

/// A parsed catalogue, ready to look models up in.
pub struct Catalogue {
    /// the name stored in `model_prices.source`
    pub source: &'static str,
    /// the licence the vendored data is under
    pub licence: String,
    /// where the data comes from
    pub url: String,
    /// stated by the adapter, since the vendor file may carry no currency
    pub currency: &'static str,
    /// the day the snapshot was taken
    pub snapshot: NaiveDate,
    prices: BTreeMap<String, CataloguePrice>,
}

impl Catalogue {
    pub fn model_count(&self) -> usize {
        self.prices.len()
    }

    pub fn get(&self, model: &str) -> Option<&CataloguePrice> {
        self.prices.get(model)
    }

    /// Models whose id contains `needle` (case-insensitive), in id order, at
    /// most `limit`; every model when the needle is empty.
    pub fn search(&self, needle: &str, limit: usize) -> Vec<&CataloguePrice> {
        let needle = needle.trim().to_lowercase();
        self.prices
            .values()
            .filter(|price| needle.is_empty() || price.model.to_lowercase().contains(&needle))
            .take(limit)
            .collect()
    }

    /// The instant stamped on a row imported from this snapshot.
    pub fn imported_at(&self) -> DateTime<Utc> {
        self.snapshot
            .and_hms_opt(0, 0, 0)
            .map(|day| day.and_utc())
            .unwrap_or_default()
    }
}

/// Something a price catalogue can be read from.
pub trait PriceSource {
    fn load(&self) -> Result<Catalogue, String>;
}

/// LiteLLM's price file, as `scripts/refresh-price-catalogue.ts` vendors it.
pub struct LiteLlm;

const LITELLM_SNAPSHOT: &str = include_str!("../catalogue/litellm-prices.json");

#[derive(Deserialize)]
struct Snapshot {
    #[serde(rename = "_meta")]
    meta: Meta,
    models: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Meta {
    url: String,
    licence: String,
    fetched_at: NaiveDate,
}

/// The fields of a LiteLLM entry the importer reads, per token.
#[derive(Deserialize)]
struct Entry {
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
}

/// A per-token cost as a per-million-token rate with the six places the
/// column stores. `None` for a value that is negative, not finite or too big.
fn per_mtok(per_token: f64) -> Option<String> {
    let rate = (Decimal::from_f64(per_token)? * Decimal::from(1_000_000)).round_dp(6);
    (!rate.is_sign_negative() && rate < Decimal::from(RATE_CEILING))
        .then(|| rate.normalize().to_string())
}

impl PriceSource for LiteLlm {
    fn load(&self) -> Result<Catalogue, String> {
        let snapshot: Snapshot = serde_json::from_str(LITELLM_SNAPSHOT)
            .map_err(|e| format!("the vendored LiteLLM snapshot does not parse: {e}"))?;
        let mut prices = BTreeMap::new();
        for (model, entry) in snapshot.models {
            let Some(input) = entry.input_cost_per_token.and_then(per_mtok) else {
                continue;
            };
            prices.insert(
                model.clone(),
                CataloguePrice {
                    model,
                    input_per_mtok: input,
                    // an embedding model states no output cost: it generates none
                    output_per_mtok: entry
                        .output_cost_per_token
                        .and_then(per_mtok)
                        .unwrap_or_else(|| "0".to_string()),
                    cached_input_per_mtok: entry.cache_read_input_token_cost.and_then(per_mtok),
                    cache_write_per_mtok: entry.cache_creation_input_token_cost.and_then(per_mtok),
                    // the file has no currency field: USD is the vendor's
                    // convention, stated here rather than assumed downstream
                    currency: "USD".to_string(),
                },
            );
        }
        Ok(Catalogue {
            source: "litellm",
            licence: snapshot.meta.licence,
            url: snapshot.meta.url,
            currency: "USD",
            snapshot: snapshot.meta.fetched_at,
            prices,
        })
    }
}

/// The catalogue the importer uses, parsed once.
pub fn default_catalogue() -> Result<&'static Catalogue, String> {
    static CATALOGUE: OnceLock<Result<Catalogue, String>> = OnceLock::new();
    CATALOGUE
        .get_or_init(|| LiteLlm.load())
        .as_ref()
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_token_costs_become_per_million_rates() {
        assert_eq!(per_mtok(2.5e-06).as_deref(), Some("2.5"));
        assert_eq!(per_mtok(0.0).as_deref(), Some("0"));
        assert_eq!(per_mtok(-1e-06), None);
        assert_eq!(per_mtok(f64::NAN), None);
        assert_eq!(
            per_mtok(1.0),
            None,
            "a million per million tokens overflows the column"
        );
    }

    #[test]
    fn the_vendored_snapshot_parses_and_states_usd() {
        let catalogue = default_catalogue().expect("the snapshot parses");
        assert!(
            catalogue.model_count() > 1000,
            "{} models",
            catalogue.model_count()
        );
        assert_eq!(catalogue.currency, "USD");
        assert_eq!(catalogue.source, "litellm");
        assert!(catalogue.licence.starts_with("MIT"));
        let gpt = catalogue.get("gpt-4o").expect("gpt-4o is priced");
        assert_eq!(gpt.currency, "USD");
        assert_eq!(gpt.input_per_mtok, "2.5");
        assert_eq!(gpt.output_per_mtok, "10");
        assert_eq!(gpt.cached_input_per_mtok.as_deref(), Some("1.25"));
    }

    #[test]
    fn search_is_case_insensitive_and_bounded() {
        let catalogue = default_catalogue().expect("the snapshot parses");
        let hits = catalogue.search("GPT-4O", 5);
        assert!(!hits.is_empty() && hits.len() <= 5);
        assert!(hits
            .iter()
            .all(|p| p.model.to_lowercase().contains("gpt-4o")));
        assert_eq!(catalogue.search("", 3).len(), 3);
        assert!(catalogue.search("no-such-model-anywhere", 3).is_empty());
    }
}
