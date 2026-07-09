//! Per-target cooldowns. After a target returns a transient upstream failure
//! (429/5xx or a connection error) it is parked for a short window so the
//! balancer skips it and load shifts to healthy siblings. State lives outside
//! the routing snapshot (it must survive config hot-reloads) and is keyed by
//! `(public model, target index)`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Map of parked targets keyed by `(public model, target index)` to the instant
/// their cooldown expires.
type ParkedMap = HashMap<(String, usize), Instant>;

/// Shared, cheaply-cloneable registry of parked targets. Disabled instances
/// (`base_secs = 0`) short-circuit every method to a no-op.
#[derive(Clone, Default)]
pub struct Cooldowns {
    inner: Option<Arc<Mutex<ParkedMap>>>,
}

impl Cooldowns {
    /// An enabled registry.
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    /// Whether `(model, idx)` is currently parked. Expired entries are evicted
    /// lazily on read.
    pub fn is_parked(&self, model: &str, idx: usize) -> bool {
        let Some(inner) = &self.inner else {
            return false;
        };
        let key = (model.to_string(), idx);
        let mut map = inner.lock().unwrap();
        match map.get(&key) {
            Some(until) if *until > Instant::now() => true,
            Some(_) => {
                map.remove(&key);
                false
            }
            None => false,
        }
    }

    /// Park `(model, idx)` for `secs`. Extends an existing cooldown, never
    /// shortens it. A zero duration is a no-op.
    pub fn park(&self, model: &str, idx: usize, secs: u64) {
        let Some(inner) = &self.inner else {
            return;
        };
        if secs == 0 {
            return;
        }
        let until = Instant::now() + Duration::from_secs(secs);
        let mut map = inner.lock().unwrap();
        let slot = map.entry((model.to_string(), idx)).or_insert(until);
        if until > *slot {
            *slot = until;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_is_inert() {
        // the derived default has no backing map and never parks
        let c = Cooldowns::default();
        c.park("m", 0, 10);
        assert!(!c.is_parked("m", 0));
    }

    #[test]
    fn parks_and_reports() {
        let c = Cooldowns::new();
        assert!(!c.is_parked("m", 1));
        c.park("m", 1, 60);
        assert!(c.is_parked("m", 1));
        // distinct target unaffected
        assert!(!c.is_parked("m", 0));
        // zero duration is a no-op
        c.park("m", 0, 0);
        assert!(!c.is_parked("m", 0));
    }

    #[test]
    fn expired_entry_is_evicted() {
        let c = Cooldowns::new();
        // park for zero-past by inserting an already-elapsed instant directly
        c.park("m", 2, 1);
        // force expiry by rewriting the deadline into the past
        if let Some(inner) = &c.inner {
            inner.lock().unwrap().insert(
                ("m".to_string(), 2),
                Instant::now() - Duration::from_secs(1),
            );
        }
        assert!(!c.is_parked("m", 2));
        // and the stale key was removed
        assert!(c
            .inner
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .get(&("m".to_string(), 2))
            .is_none());
    }
}
