//! In-flight load counters. Tracks how many requests are currently outstanding
//! against each `(public model, target index)` and hands the balancer a live
//! per-target load snapshot so strategies like power-of-two can steer traffic
//! away from busy targets. State lives outside the routing snapshot (it must
//! survive config hot-reloads) and a count is held for the full lifetime of a
//! request, including the streamed response body, via an RAII [`LoadGuard`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// smoothing factor for the per-target latency EWMA: high enough to follow a
/// shifting upstream within a handful of requests, low enough to ride out a
/// single outlier
const LATENCY_EWMA_ALPHA: f64 = 0.3;

type LoadMap = HashMap<(String, usize), u64>;
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
}

impl LoadTracker {
    /// An enabled tracker.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
            latency: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    /// Current in-flight counts for targets `0..n` of `model`, indexed to match
    /// the route's target order so it can be passed straight to `pick`.
    pub fn snapshot(&self, model: &str, n: usize) -> Vec<u64> {
        let Some(inner) = &self.inner else {
            return Vec::new();
        };
        let map = inner.lock().unwrap();
        (0..n)
            .map(|i| map.get(&(model.to_string(), i)).copied().unwrap_or(0))
            .collect()
    }

    /// Smoothed latency (ms) for targets `0..n` of `model`, route-order
    /// aligned; `0.0` for a target with no successful sample yet.
    pub fn latency_snapshot(&self, model: &str, n: usize) -> Vec<f64> {
        let Some(latency) = &self.latency else {
            return Vec::new();
        };
        let map = latency.lock().unwrap();
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
                started: Instant::now(),
                record: false,
            };
        };
        let key = (model.to_string(), idx);
        *inner.lock().unwrap().entry(key.clone()).or_insert(0) += 1;
        LoadGuard {
            inner: Some((inner.clone(), key)),
            latency: self.latency.clone(),
            started: Instant::now(),
            record: false,
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
    started: Instant,
    record: bool,
}

impl LoadGuard {
    /// Mark the attempt as successful: its duration counts toward the target's
    /// smoothed latency when the guard drops (i.e. once streaming finishes).
    pub fn mark_ok(&mut self) {
        self.record = true;
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        let Some((map, key)) = &self.inner else {
            return;
        };
        {
            let mut map = map.lock().unwrap();
            if let Some(v) = map.get_mut(key) {
                *v = v.saturating_sub(1);
                if *v == 0 {
                    map.remove(key);
                }
            }
        }
        if self.record {
            if let Some(latency) = &self.latency {
                let sample = self.started.elapsed().as_millis() as f64;
                let mut map = latency.lock().unwrap();
                map.entry(key.clone())
                    .and_modify(|e| *e += LATENCY_EWMA_ALPHA * (sample - *e))
                    .or_insert(sample);
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
}
