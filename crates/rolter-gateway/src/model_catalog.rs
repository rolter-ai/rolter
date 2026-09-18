//! What each provider says it serves, cached from the health sweep's catalogue
//! probe so `GET /v1/models` can list the models a provider actually offers
//! under `provider-slug/model` — not only the ones a configured route target
//! happens to name (#1647).
//!
//! The models were always routable: `Snapshot::resolve_pinned` turns any
//! `provider-slug/model` into a pinned single-target route, so a caller who
//! already knew the name got a `200`. Only the listing was narrower than the
//! addressing, which made a whole fleet invisible to any client that builds its
//! model picker from `/v1/models`.
//!
//! The source is the probe the prober already sends: for every provider kind
//! whose liveness endpoint is a model catalogue
//! ([`rolter_core::probe::ProbeExpectation::Catalogue`]) the body is parsed
//! instead of discarded. That adds no upstream request and no configuration —
//! but it does mean the catalogue is only populated while active health checks
//! are enabled, which the API docs state.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Most models kept per provider.
///
/// The listing is the product of the fleet and each provider's catalogue, and
/// the wide end of that product is real: an aggregator answers `/v1/models`
/// with several hundred entries, so thirty of them would bury the route ids a
/// caller is actually looking for under tens of thousands of lines. The cap is
/// per provider rather than global so no single provider can crowd the others
/// out, and it is applied to the catalogue only — a model named by a configured
/// route target is listed regardless, because that is the set `/v1/models`
/// reported before this cache existed and a listing must never shrink.
pub const MAX_MODELS_PER_PROVIDER: usize = 200;

/// Provider name -> the models it reported, sorted and deduped.
type CatalogMap = HashMap<String, Arc<Vec<String>>>;

/// Shared, cheaply-cloneable cache of per-provider model catalogues. Lives
/// beside [`crate::health::Health`] in `AppState` rather than on the snapshot,
/// for the same reason: it is observed state and must survive a config reload.
///
/// The default instance is inert and reports no models for every provider, so a
/// deployment that never probes keeps exactly the route-derived listing.
#[derive(Clone, Default)]
pub struct ModelCatalog {
    inner: Option<Arc<Mutex<CatalogMap>>>,
}

impl ModelCatalog {
    /// An enabled cache with no observations yet.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    /// Record what `provider` reported. The list is sorted, deduped and capped
    /// at [`MAX_MODELS_PER_PROVIDER`] here, once per sweep, so the listing path
    /// only clones an `Arc`.
    ///
    /// An empty catalogue is recorded as such: a provider that stops serving a
    /// model should stop listing it, and treating empty as "no observation"
    /// would pin the previous answer forever.
    pub fn record(&self, provider: &str, models: Vec<String>) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut models: Vec<String> = models;
        models.sort_unstable();
        models.dedup();
        if models.len() > MAX_MODELS_PER_PROVIDER {
            tracing::debug!(
                provider = %provider,
                reported = models.len(),
                cap = MAX_MODELS_PER_PROVIDER,
                "provider catalogue truncated for /v1/models; addressing is unaffected"
            );
            models.truncate(MAX_MODELS_PER_PROVIDER);
        }
        inner.lock().insert(provider.to_string(), Arc::new(models));
    }

    /// The cached catalogue for `provider`, or `None` when it was never probed.
    pub fn models(&self, provider: &str) -> Option<Arc<Vec<String>>> {
        self.inner.as_ref()?.lock().get(provider).cloned()
    }

    /// Drop every provider not in `live`, so a provider removed by a reload
    /// stops appearing in the listing.
    pub fn retain(&self, live: &HashSet<&str>) {
        if let Some(inner) = &self.inner {
            inner.lock().retain(|name, _| live.contains(name.as_str()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_catalog_records_nothing() {
        let catalog = ModelCatalog::default();
        catalog.record("openai-edge", vec!["gpt-4o".to_string()]);
        assert!(catalog.models("openai-edge").is_none());
    }

    #[test]
    fn records_are_sorted_and_deduped() {
        let catalog = ModelCatalog::new();
        catalog.record(
            "openai-edge",
            vec![
                "o3-mini".to_string(),
                "gpt-4o".to_string(),
                "gpt-4o".to_string(),
            ],
        );
        assert_eq!(
            catalog.models("openai-edge").as_deref(),
            Some(&vec!["gpt-4o".to_string(), "o3-mini".to_string()])
        );
    }

    #[test]
    fn a_huge_catalogue_is_capped() {
        let catalog = ModelCatalog::new();
        let reported: Vec<String> = (0..MAX_MODELS_PER_PROVIDER + 50)
            .map(|i| format!("m-{i:04}"))
            .collect();
        catalog.record("aggregator", reported);
        assert_eq!(
            catalog.models("aggregator").map(|m| m.len()),
            Some(MAX_MODELS_PER_PROVIDER)
        );
    }

    /// An empty answer is an observation, not a gap: a model the provider
    /// dropped must leave the listing.
    #[test]
    fn an_empty_catalogue_replaces_a_previous_one() {
        let catalog = ModelCatalog::new();
        catalog.record("edge", vec!["gpt-4o".to_string()]);
        catalog.record("edge", Vec::new());
        assert_eq!(catalog.models("edge").map(|m| m.len()), Some(0));
    }

    #[test]
    fn retain_drops_providers_a_reload_removed() {
        let catalog = ModelCatalog::new();
        catalog.record("kept", vec!["a".to_string()]);
        catalog.record("gone", vec!["b".to_string()]);
        catalog.retain(&HashSet::from(["kept"]));
        assert!(catalog.models("kept").is_some());
        assert!(catalog.models("gone").is_none());
    }
}
