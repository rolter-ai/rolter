//! In-flight load counters. Tracks how many requests are currently outstanding
//! against each `(public model, target index)` and hands the balancer a live
//! per-target load snapshot so strategies like power-of-two can steer traffic
//! away from busy targets. State lives outside the routing snapshot (it must
//! survive config hot-reloads) and a count is held for the full lifetime of a
//! request, including the streamed response body, via an RAII [`LoadGuard`].

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// smoothing factor for the per-target latency EWMA: high enough to follow a
/// shifting upstream within a handful of requests, low enough to ride out a
/// single outlier
const LATENCY_EWMA_ALPHA: f64 = 0.3;

type LoadMap = HashMap<(String, usize), u64>;
/// per-model predicted-latency models, keyed by the same namespace the load and
/// latency maps use
type PredictorMap = HashMap<String, Arc<rolter_balancer::predictor::LatencyPredictor>>;
/// per-(model, target) smoothed request latency in milliseconds
type LatencyMap = HashMap<(String, usize), f64>;
/// backing store plus the key a [`LoadGuard`] must decrement on drop
type GuardSlot = (Arc<Mutex<LoadMap>>, (String, usize));

/// Shared, cheaply-cloneable registry of in-flight counts and per-target
/// smoothed latency.
#[derive(Clone, Default)]
pub struct LoadTracker {
    inner: Option<Arc<Mutex<LoadMap>>>,
    latency: Option<Arc<Mutex<LatencyMap>>>,
    /// per-request latency models for the `predicted_latency` strategy (#853).
    /// Lives here rather than in the routing snapshot for the same reason the
    /// latency EWMA does: a config reload must not throw away what the models
    /// learned, or every reload would send the route back to cold behaviour
    predictors: Option<Arc<Mutex<PredictorMap>>>,
}

impl LoadTracker {
    /// An enabled tracker.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
            latency: Some(Arc::new(Mutex::new(HashMap::new()))),
            predictors: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    /// The latency model for `model`'s route of `n` targets, creating it on
    /// first use. Returns `None` on a disabled tracker.
    ///
    /// `n` sizes the model only when it is first created. A route that gains a
    /// target keeps its existing models and the new target simply reads as
    /// unpredicted until the next process start, which is the conservative
    /// direction: a fresh target with no evidence should not be ranked.
    pub fn predictor(
        &self,
        model: &str,
        n: usize,
    ) -> Option<Arc<rolter_balancer::predictor::LatencyPredictor>> {
        let predictors = self.predictors.as_ref()?;
        let mut map = predictors.lock();
        Some(
            map.entry(model.to_string())
                .or_insert_with(|| Arc::new(rolter_balancer::predictor::LatencyPredictor::new(n)))
                .clone(),
        )
    }

    /// Current in-flight counts for targets `0..n` of `model`, indexed to match
    /// the route's target order so it can be passed straight to `pick`.
    pub fn snapshot(&self, model: &str, n: usize) -> Vec<u64> {
        let Some(inner) = &self.inner else {
            return Vec::new();
        };
        let map = inner.lock();
        (0..n)
            .map(|i| map.get(&(model.to_string(), i)).copied().unwrap_or(0))
            .collect()
    }

    /// Every in-flight request this gateway is currently holding, summed
    /// across targets.
    ///
    /// Sampled at scrape time rather than mirrored into a counter on the
    /// request path. It exists because a leaked slot — a request that exits
    /// without dropping its [`LoadGuard`] — is otherwise invisible until the
    /// gateway is saturated and the balancer's load view has quietly been wrong
    /// for hours (#1083).
    pub fn total_in_flight(&self) -> u64 {
        let Some(inner) = &self.inner else {
            return 0;
        };
        inner.lock().values().sum()
    }

    /// Smoothed latency (ms) for targets `0..n` of `model`, route-order
    /// aligned; `0.0` for a target with no successful sample yet.
    pub fn latency_snapshot(&self, model: &str, n: usize) -> Vec<f64> {
        let Some(latency) = &self.latency else {
            return Vec::new();
        };
        let map = latency.lock();
        (0..n)
            .map(|i| map.get(&(model.to_string(), i)).copied().unwrap_or(0.0))
            .collect()
    }

    /// Increment the in-flight count for `(model, idx)` and return a guard that
    /// decrements it on drop.
    pub fn begin(&self, model: &str, idx: usize) -> LoadGuard {
        let Some(inner) = &self.inner else {
            return LoadGuard {
                inner: None,
                latency: None,
                predictor: None,
                started: Instant::now(),
                record: false,
                queue_depth: 0,
                prompt_tokens: 0,
                route_permit: None,
            };
        };
        let key = (model.to_string(), idx);
        // read before the increment: the model learns what queue this request
        // *joined*, not the one it created
        let queue_depth = {
            let mut map = inner.lock();
            let slot = map.entry(key.clone()).or_insert(0);
            let before = *slot;
            *slot += 1;
            before
        };
        LoadGuard {
            inner: Some((inner.clone(), key)),
            latency: self.latency.clone(),
            predictor: self
                .predictors
                .as_ref()
                .and_then(|p| p.lock().get(model).cloned()),
            queue_depth,
            prompt_tokens: 0,
            started: Instant::now(),
            record: false,
            route_permit: None,
        }
    }
}

/// Decrements the in-flight count for its target when dropped. Held for the whole
/// request, so the count only falls once the response body is fully streamed (or
/// the client disconnects and the stream is dropped). When [`LoadGuard::mark_ok`]
/// was called, the drop also folds the request's full duration into the target's
/// latency EWMA — failed attempts are never recorded, so a fast-failing target
/// cannot masquerade as a fast one.
pub struct LoadGuard {
    inner: Option<GuardSlot>,
    latency: Option<Arc<Mutex<LatencyMap>>>,
    predictor: Option<Arc<rolter_balancer::predictor::LatencyPredictor>>,
    started: Instant,
    record: bool,
    /// in-flight count against this target when the request was dispatched
    queue_depth: u64,
    /// prompt size, set by the caller once the body has been parsed
    prompt_tokens: usize,
    /// the slot of the route's `concurrency` cap this request holds (#2936).
    /// Riding on the guard is what keeps it for the whole streamed response
    route_permit: Option<crate::route_limits::ConcurrencyPermit>,
}

impl LoadGuard {
    /// Keep `permit` until this guard drops, so the route's concurrency slot is
    /// held as long as the response is still being delivered.
    pub fn hold_route_permit(&mut self, permit: Option<crate::route_limits::ConcurrencyPermit>) {
        self.route_permit = permit;
    }

    /// Mark the attempt as successful: its duration counts toward the target's
    /// smoothed latency when the guard drops (i.e. once streaming finishes).
    pub fn mark_ok(&mut self) {
        self.record = true;
    }

    /// Record this request's prompt size so a successful completion teaches the
    /// latency model what a prompt of that size cost. Ignored by every strategy
    /// other than `predicted_latency`.
    pub fn set_prompt_tokens(&mut self, tokens: usize) {
        self.prompt_tokens = tokens;
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        let Some((map, key)) = &self.inner else {
            return;
        };
        {
            let mut map = map.lock();
            if let Some(v) = map.get_mut(key) {
                *v = v.saturating_sub(1);
                if *v == 0 {
                    map.remove(key);
                }
            }
        }
        if self.record {
            let sample = self.started.elapsed().as_millis() as f64;
            if let Some(latency) = &self.latency {
                let mut map = latency.lock();
                map.entry(key.clone())
                    .and_modify(|e| *e += LATENCY_EWMA_ALPHA * (sample - *e))
                    .or_insert(sample);
            }
            // same gate as the EWMA: a failed attempt teaches nothing, so a
            // fast-failing target cannot train itself into looking cheap
            if let Some(predictor) = &self.predictor {
                predictor.observe(
                    key.1,
                    rolter_balancer::predictor::Features {
                        queue_depth: self.queue_depth,
                        prompt_tokens: self.prompt_tokens,
                    },
                    sample,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tracker_is_inert() {
        let t = LoadTracker::default();
        let g = t.begin("m", 0);
        assert!(t.snapshot("m", 2).is_empty());
        drop(g);
    }

    #[test]
    fn latency_recorded_only_when_marked_ok() {
        let t = LoadTracker::new();
        // unmarked guard (failed attempt): no sample lands
        let g = t.begin("m", 0);
        drop(g);
        assert_eq!(t.latency_snapshot("m", 1), vec![0.0]);
        // marked guard records its elapsed duration
        let mut g = t.begin("m", 0);
        g.mark_ok();
        drop(g);
        let after = t.latency_snapshot("m", 1)[0];
        assert!(after >= 0.0);
        // second sample folds in as an ewma rather than replacing
        let mut g = t.begin("m", 0);
        g.mark_ok();
        std::thread::sleep(std::time::Duration::from_millis(15));
        drop(g);
        let ewma = t.latency_snapshot("m", 1)[0];
        assert!(
            ewma > 0.0,
            "ewma should reflect the slow sample, got {ewma}"
        );
        assert!(
            ewma < 15.0,
            "ewma should be smoothed below the raw 15ms sample, got {ewma}"
        );
    }

    #[test]
    fn counts_rise_and_fall_with_guards() {
        let t = LoadTracker::new();
        assert_eq!(t.snapshot("m", 2), vec![0, 0]);
        let g0 = t.begin("m", 0);
        let g0b = t.begin("m", 0);
        let g1 = t.begin("m", 1);
        assert_eq!(t.snapshot("m", 2), vec![2, 1]);
        drop(g0);
        assert_eq!(t.snapshot("m", 2), vec![1, 1]);
        drop(g0b);
        drop(g1);
        // fully drained keys are evicted, snapshot reports zeros
        assert_eq!(t.snapshot("m", 2), vec![0, 0]);
    }

    #[test]
    fn a_successful_guard_teaches_the_latency_model_the_queue_it_joined() {
        let t = LoadTracker::new();
        // the model must exist before `begin`, exactly as the snapshot build
        // creates it; a guard opened before that simply teaches nothing
        let predictor = t.predictor("m", 2).unwrap();

        // two requests already in flight against target 0
        let _a = t.begin("m", 0);
        let _b = t.begin("m", 0);
        let mut third = t.begin("m", 0);
        third.mark_ok();
        third.set_prompt_tokens(500);
        // the sample must be non-zero for the update to move anything
        std::thread::sleep(std::time::Duration::from_millis(3));
        drop(third);

        assert_eq!(predictor.samples(0), 1);
        assert_eq!(predictor.observed(), 1);
        // one NLMS step from all-zero weights leaves each coefficient
        // proportional to its own feature, so the coefficients read back the
        // exact feature row: queue 2 (the queue it *joined*, not the three it
        // was part of) and 0.5 ktokens
        let [fixed, per_queued, per_ktok] = predictor.coefficients(0).unwrap();
        assert!(fixed > 0.0, "a real sample should have moved the bias");
        assert!(
            (per_queued / fixed - 2.0).abs() < 1e-9,
            "queue depth should be 2, got ratio {}",
            per_queued / fixed
        );
        assert!(
            (per_ktok / fixed - 0.5).abs() < 1e-9,
            "prompt should be 0.5 ktokens, got ratio {}",
            per_ktok / fixed
        );
    }

    #[test]
    fn a_failed_attempt_teaches_the_latency_model_nothing() {
        let t = LoadTracker::new();
        let predictor = t.predictor("m", 1).unwrap();
        // unmarked guard: the attempt failed, so it is not a latency sample
        drop(t.begin("m", 0));
        assert_eq!(predictor.observed(), 0);
        assert_eq!(predictor.samples(0), 0);
    }

    #[test]
    fn predictors_are_shared_per_namespace_and_survive_a_rebuild() {
        let t = LoadTracker::new();
        let first = t.predictor("m", 2).unwrap();
        let mut g = t.begin("m", 1);
        g.mark_ok();
        drop(g);
        // a config reload asks for the model again; it must be the same one,
        // still holding what it learned
        let second = t.predictor("m", 2).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.observed(), 1);
        // a different namespace (a variant pool) gets its own model
        assert_eq!(t.predictor("m::v", 2).unwrap().observed(), 0);
    }

    #[test]
    fn a_disabled_tracker_has_no_predictor() {
        assert!(LoadTracker::default().predictor("m", 2).is_none());
    }
}
