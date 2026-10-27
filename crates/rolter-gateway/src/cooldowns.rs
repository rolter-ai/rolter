//! Per-target cooldowns. After a target returns a transient upstream failure
//! (429/5xx or a connection error) it is parked for a short window so the
//! balancer skips it and load shifts to healthy siblings. State lives outside
//! the routing snapshot (it must survive config hot-reloads) and is keyed by
//! `(public model, target index)`.
//!
//! The registry splits the key — a map from model to a slot vector addressed by
//! target index — rather than using a flat `(String, usize)` map. A flat tuple
//! key cannot be looked up from a `&str`, so every read had to materialise a
//! `String` just to hash it; `is_parked` runs per request per candidate target,
//! so that was an allocation on the hottest admission path in the gateway. The
//! split keeps the read borrowed, hashes the model exactly once and confines
//! allocation to a model's first park (#1050).

use crate::target_mask::TargetMask;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Parked targets of one model, addressed by target index: `Some(instant)` is
/// the moment that target's cooldown expires, `None` a target that is not
/// parked. A `Vec` rather than a second map because target indices are
/// positions in a route's target list — small, dense, and cheaper to index than
/// to hash.
type ModelParks = Vec<Option<Instant>>;

/// Map of parked targets keyed by public model, then addressed by target index.
type ParkedMap = HashMap<String, ModelParks>;

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
    /// lazily on read. Allocation-free: the lookup borrows `model`.
    pub fn is_parked(&self, model: &str, idx: usize) -> bool {
        let Some(inner) = &self.inner else {
            return false;
        };
        let mut map = inner.lock();
        let Some(parks) = map.get_mut(model) else {
            return false;
        };
        match parks.get(idx).copied().flatten() {
            Some(until) if until > Instant::now() => true,
            Some(_) => {
                parks[idx] = None;
                // a model with no parked target left holds no information, and
                // keeping its slot vector would leak one key per retired model
                if parks.iter().all(Option::is_none) {
                    map.remove(model);
                }
                false
            }
            None => false,
        }
    }

    /// Which of a model's first `n` targets are parked right now, answered with
    /// one lock acquisition rather than one per target (#1942). Evicts expired
    /// entries as [`Cooldowns::is_parked`] does. Allocation-free for up to 64
    /// targets.
    pub fn parked_mask(&self, model: &str, n: usize) -> TargetMask {
        let mut mask = TargetMask::new();
        let Some(inner) = &self.inner else {
            return mask;
        };
        let mut map = inner.lock();
        let Some(parks) = map.get_mut(model) else {
            return mask;
        };
        let now = Instant::now();
        for (idx, slot) in parks.iter_mut().enumerate() {
            match *slot {
                Some(until) if until > now => {
                    if idx < n {
                        mask.insert(idx);
                    }
                }
                Some(_) => *slot = None,
                None => {}
            }
        }
        if parks.iter().all(Option::is_none) {
            map.remove(model);
        }
        mask
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
        let mut map = inner.lock();
        // a model that already has a parked target is the common case under a
        // sustained fault, and it needs no allocation; only the first park of
        // a model owns its name
        if let Some(parks) = map.get_mut(model) {
            extend_park(parks, idx, until);
            return;
        }
        // reached only on a model's first park, so owning its name here costs
        // nothing measurable
        extend_park(map.entry(model.to_string()).or_default(), idx, until);
    }
}

/// Park `idx` until `until`, growing the slot vector as needed. Extends an
/// existing deadline, never shortens it.
fn extend_park(parks: &mut ModelParks, idx: usize, until: Instant) {
    if parks.len() <= idx {
        parks.resize(idx + 1, None);
    }
    match &mut parks[idx] {
        Some(slot) if *slot >= until => {}
        slot => *slot = Some(until),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parked deadline for `(model, idx)`, reaching past the public API so
    /// extension semantics can be asserted on the stored value itself.
    fn deadline(c: &Cooldowns, model: &str, idx: usize) -> Option<Instant> {
        c.inner.as_ref().and_then(|i| {
            i.lock()
                .get(model)
                .and_then(|p| p.get(idx).copied())
                .flatten()
        })
    }

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

    /// A re-park of an already-parked model is the path that now skips the
    /// allocation, so its extend-never-shorten semantics are worth pinning.
    #[test]
    fn a_second_park_extends_but_never_shortens() {
        let c = Cooldowns::new();
        c.park("m", 0, 600);
        let long = deadline(&c, "m", 0).expect("the target is parked");
        c.park("m", 0, 1);
        assert_eq!(
            deadline(&c, "m", 0),
            Some(long),
            "a shorter park must not truncate the window"
        );
        c.park("m", 0, 3600);
        assert!(
            deadline(&c, "m", 0).is_some_and(|d| d > long),
            "a longer park must extend the window"
        );
    }

    /// Distinct models must not collide now that the key is nested, and a
    /// second model must reach the allocating branch without disturbing the first.
    #[test]
    fn models_are_isolated_from_each_other() {
        let c = Cooldowns::new();
        c.park("a", 0, 60);
        assert!(c.is_parked("a", 0));
        assert!(!c.is_parked("b", 0));
        c.park("b", 0, 60);
        c.park("a", 1, 60);
        assert!(c.is_parked("a", 0));
        assert!(c.is_parked("a", 1));
        assert!(c.is_parked("b", 0));
        assert!(!c.is_parked("b", 1));
    }

    #[test]
    fn expired_entry_is_evicted() {
        let c = Cooldowns::new();
        c.park("m", 2, 1);
        // park a sibling so the model entry survives and the target-level
        // eviction is what the assertion actually observes
        c.park("m", 3, 600);
        // force expiry by rewriting the deadline into the past
        if let Some(inner) = &c.inner {
            // written straight into the slot: `park` deliberately refuses to
            // shorten a window, so it cannot be used to rewind one
            inner.lock().entry("m".to_string()).or_default()[2] =
                Some(Instant::now() - Duration::from_secs(1));
        }
        assert!(!c.is_parked("m", 2));
        // and the stale key was removed
        assert!(deadline(&c, "m", 2).is_none());
        assert!(c.is_parked("m", 3), "the sibling must be untouched");
    }

    /// Expiring a model's last target must drop the model entry too, or a
    /// long-lived process accumulates one empty key per retired model.
    #[test]
    fn expiring_the_last_target_drops_the_model_entry() {
        let c = Cooldowns::new();
        c.park("m", 0, 1);
        if let Some(inner) = &c.inner {
            inner.lock().entry("m".to_string()).or_default()[0] =
                Some(Instant::now() - Duration::from_secs(1));
        }
        assert!(!c.is_parked("m", 0));
        assert!(
            c.inner.as_ref().unwrap().lock().is_empty(),
            "the model's now-empty map should have been removed"
        );
    }

    #[test]
    fn the_mask_agrees_with_asking_per_target() {
        let c = Cooldowns::new();
        c.park("m", 1, 60);
        c.park("m", 70, 60);
        let mask = c.parked_mask("m", 100);
        for idx in 0..100 {
            assert_eq!(mask.contains(idx), c.is_parked("m", idx), "{idx}");
        }
        // targets past the asked width are left out, and an unknown model or a
        // disabled registry has nothing parked
        assert!(!c.parked_mask("m", 2).contains(70));
        assert!(c.parked_mask("other", 4).is_empty());
        assert!(Cooldowns::default().parked_mask("m", 4).is_empty());
    }

    #[test]
    fn the_mask_evicts_expired_parks_like_a_read() {
        let c = Cooldowns::new();
        c.park("m", 0, 60);
        c.inner.as_ref().unwrap().lock().get_mut("m").unwrap()[0] =
            Some(Instant::now() - Duration::from_secs(1));
        assert!(c.parked_mask("m", 1).is_empty());
        assert!(c.inner.as_ref().unwrap().lock().get("m").is_none());
    }
}
