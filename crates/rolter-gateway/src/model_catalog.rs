//! What each provider says it serves, cached from the provider's own model
//! catalogue so `GET /v1/models` can list the models a provider actually offers
//! under `provider-slug/model` — not only the ones a configured route target
//! happens to name (#1647).
//!
//! The models were always routable: `Snapshot::resolve_pinned` turns any
//! `provider-slug/model` into a pinned single-target route, so a caller who
//! already knew the name got a `200`. Only the listing was narrower than the
//! addressing, which made a whole fleet invisible to any client that builds its
//! model picker from `/v1/models`.
//!
//! Two writers fill the cache, and they agree because both read the same free
//! probe URL ([`rolter_core::probe::probe_request`]): the health sweep, which
//! already fetches the catalogue every interval while active health checks are
//! on, and [`spawn_refresher`], an independent and slow loop that runs whether
//! or not they are (#1729). The refresher is what lets a deployment that never
//! enabled `[health]` list a complete fleet.

// spec-ref: openai-models
use parking_lot::Mutex;
use rolter_core::probe::{probe_expectation, ProbeExpectation};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// Longest a single catalogue fetch may take, headers and body together.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the refresher looks for providers that are due, bounded by the
/// configured interval. A provider with no catalogue yet is due immediately, so
/// a provider added by a reload is listed within one tick instead of one
/// interval.
const REFRESH_TICK: Duration = Duration::from_secs(30);

/// Spawn the catalogue refresher. It is always spawned and reads
/// `health.catalogue_interval_secs` off the live snapshot each tick, so a
/// hot-reload can retune or switch it off (`0`) without a restart.
pub fn spawn_refresher(state: crate::state::AppState) {
    tokio::spawn(run_refresher(state));
}

async fn run_refresher(state: crate::state::AppState) {
    let mut last_attempt: HashMap<String, Instant> = HashMap::new();
    loop {
        let (interval, concurrency) = {
            let snap = state.snapshot.load();
            (
                snap.health.catalogue_interval_secs,
                snap.health.probe_concurrency.max(1),
            )
        };
        let tick = if interval == 0 {
            REFRESH_TICK
        } else {
            REFRESH_TICK.min(Duration::from_secs(interval))
        };
        if interval > 0 {
            refresh_due(
                &state,
                Duration::from_secs(interval),
                concurrency,
                &mut last_attempt,
            )
            .await;
        }
        tokio::time::sleep(tick).await;
    }
}

/// Refresh every provider whose catalogue is missing or older than `interval`,
/// bounded to `concurrency` fetches in flight. A failed fetch keeps the
/// previous catalogue: an upstream that blips should not empty the listing.
pub(crate) async fn refresh_due(
    state: &crate::state::AppState,
    interval: Duration,
    concurrency: usize,
    last_attempt: &mut HashMap<String, Instant>,
) {
    let due: Vec<(String, crate::health::ProbePlan, reqwest::Client)> = {
        let snap = state.snapshot.load();
        // a provider a reload removed must stop showing up in /v1/models
        let live: HashSet<&str> = snap.providers.keys().map(String::as_str).collect();
        state.model_catalog.retain(&live);
        last_attempt.retain(|name, _| live.contains(name.as_str()));
        snap.providers
            .values()
            .filter(|p| {
                // the catalogue is the free probe's body, so it is only read
                // for kinds whose default probe URL is a model list
                probe_expectation(p.kind, "/") == ProbeExpectation::Catalogue
                    && last_attempt
                        .get(&p.name)
                        .is_none_or(|at| at.elapsed() >= interval)
            })
            .filter_map(|p| {
                // the default path, not `health.path`: an operator's override
                // points the liveness check somewhere that is not a catalogue
                let (plan, _) = crate::health::free_probe_plan(p, "/");
                match state.forwarder.client_for(p) {
                    Ok(client) => Some((p.name.clone(), plan, client)),
                    Err(error) => {
                        tracing::warn!(provider = %p.name, %error, "cannot build catalogue TLS client");
                        None
                    }
                }
            })
            .collect()
    };
    let now = Instant::now();
    let limiter = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut fetches = tokio::task::JoinSet::new();
    for (name, plan, client) in due {
        last_attempt.insert(name.clone(), now);
        let limiter = limiter.clone();
        let catalog = state.model_catalog.clone();
        fetches.spawn(async move {
            let Ok(_permit) = limiter.acquire_owned().await else {
                return;
            };
            let fetch = async {
                let resp = plan.build(&client).send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                crate::health::read_catalogue(resp).await
            };
            if let Ok(Some(models)) = tokio::time::timeout(REFRESH_TIMEOUT, fetch).await {
                catalog.record(&name, models);
            }
        });
    }
    while fetches.join_next().await.is_some() {}
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
