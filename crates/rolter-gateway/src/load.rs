//! In-flight load counters. Tracks how many requests are currently outstanding
//! against each `(public model, target index)` and hands the balancer a live
//! per-target load snapshot so strategies like power-of-two can steer traffic
//! away from busy targets. State lives outside the routing snapshot (it must
//! survive config hot-reloads) and a count is held for the full lifetime of a
//! request, including the streamed response body, via an RAII [`LoadGuard`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type LoadMap = HashMap<(String, usize), u64>;
/// backing store plus the key a [`LoadGuard`] must decrement on drop
type GuardSlot = (Arc<Mutex<LoadMap>>, (String, usize));

/// Shared, cheaply-cloneable registry of in-flight counts.
#[derive(Clone, Default)]
pub struct LoadTracker {
    inner: Option<Arc<Mutex<LoadMap>>>,
}

impl LoadTracker {
    /// An enabled tracker.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
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

    /// Increment the in-flight count for `(model, idx)` and return a guard that
    /// decrements it on drop.
    pub fn begin(&self, model: &str, idx: usize) -> LoadGuard {
        let Some(inner) = &self.inner else {
            return LoadGuard { inner: None };
        };
        let key = (model.to_string(), idx);
        *inner.lock().unwrap().entry(key.clone()).or_insert(0) += 1;
        LoadGuard {
            inner: Some((inner.clone(), key)),
        }
    }
}

/// Decrements the in-flight count for its target when dropped. Held for the whole
/// request, so the count only falls once the response body is fully streamed (or
/// the client disconnects and the stream is dropped).
pub struct LoadGuard {
    inner: Option<GuardSlot>,
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        let Some((map, key)) = &self.inner else {
            return;
        };
        let mut map = map.lock().unwrap();
        if let Some(v) = map.get_mut(key) {
            *v = v.saturating_sub(1);
            if *v == 0 {
                map.remove(key);
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
