//! Per-target circuit breaker (closed / open / half-open). Complements the
//! short-lived [`crate::cooldowns`] park: a cooldown shrugs off a single wobble,
//! the breaker sheds sustained load off a target that is down hard. State lives
//! outside the routing snapshot (it must survive config hot-reloads) and is keyed
//! by `(public model, target index)`.
//!
//! State machine per target:
//! - **Closed**: traffic flows; consecutive transient failures are counted. When
//!   the count reaches `failure_threshold` the target trips **open**.
//! - **Open**: traffic is skipped until `open_secs` elapse, then the next probe is
//!   admitted, moving the target to **half-open**.
//! - **Half-open**: a single probe is allowed through; a success closes the
//!   breaker (reset), a failure re-opens it for another `open_secs` window.
//!
//! A derived `Default` registry is permanently inert (no backing store) and
//! admits every target. A registry built with [`Breaker::new`] can be enabled,
//! disabled and re-tuned live by [`Breaker::reconfigure`] on a config hot-reload;
//! while disabled it admits every target and records nothing, but keeps its
//! per-target state so re-enabling resumes where it left off.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The phase a single target's breaker is in.
#[derive(Clone, Copy)]
enum Phase {
    Closed,
    /// tripped; skip traffic until this instant, then probe
    Open(Instant),
    /// probing; a single request has been admitted after the open window
    HalfOpen,
}

/// Per-target breaker state: its phase plus the running count of consecutive
/// transient failures observed while closed.
struct Entry {
    phase: Phase,
    consecutive_failures: u32,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            phase: Phase::Closed,
            consecutive_failures: 0,
        }
    }
}

type BreakerMap = HashMap<(String, usize), Entry>;

/// Shared, interior-mutable breaker state. The map holds per-target phase (kept
/// across config hot-reloads); the atomics hold the enable flag and tuning, which
/// [`Breaker::reconfigure`] updates in place so a reload can toggle or re-tune the
/// breaker without discarding accumulated per-target state.
struct Shared {
    map: Mutex<BreakerMap>,
    enabled: AtomicBool,
    failure_threshold: AtomicU32,
    open_secs: AtomicU64,
}

/// Shared, cheaply-cloneable circuit-breaker registry. A `None` inner is a
/// permanently inert breaker (used by embedders/tests that never reload); a
/// `Some` inner can be enabled, disabled and re-tuned live via [`reconfigure`].
/// While disabled it admits every target and records nothing.
#[derive(Clone, Default)]
pub struct Breaker {
    inner: Option<Arc<Shared>>,
}

impl Breaker {
    /// A reconfigurable registry, initially `enabled` or not. `failure_threshold`
    /// consecutive failures trip a target open; it stays open for `open_secs`
    /// before a half-open probe. Build one even when disabled so a later reload can
    /// enable it in place.
    pub fn new(enabled: bool, failure_threshold: u32, open_secs: u64) -> Self {
        Self {
            inner: Some(Arc::new(Shared {
                map: Mutex::new(HashMap::new()),
                enabled: AtomicBool::new(enabled),
                failure_threshold: AtomicU32::new(failure_threshold.max(1)),
                open_secs: AtomicU64::new(open_secs),
            })),
        }
    }

    /// Apply new tuning from a config hot-reload. Toggles the enable flag and
    /// updates the thresholds atomically; the per-target phase map is preserved, so
    /// a target that is currently open stays open across a tuning-only reload. A
    /// permanently-inert breaker (`inner: None`) ignores the call.
    pub fn reconfigure(&self, enabled: bool, failure_threshold: u32, open_secs: u64) {
        let Some(inner) = &self.inner else {
            return;
        };
        inner
            .failure_threshold
            .store(failure_threshold.max(1), Relaxed);
        inner.open_secs.store(open_secs, Relaxed);
        inner.enabled.store(enabled, Relaxed);
    }

    /// Whether this registry is currently enforcing (enabled with a backing store).
    fn active(&self) -> Option<&Arc<Shared>> {
        let inner = self.inner.as_ref()?;
        inner.enabled.load(Relaxed).then_some(inner)
    }

    /// Whether `(model, idx)` may currently receive traffic. Closed and half-open
    /// targets are admitted; an open target is skipped until its window elapses,
    /// at which point the call transitions it to half-open and admits the probe.
    /// A disabled registry always admits.
    pub fn allows(&self, model: &str, idx: usize) -> bool {
        let Some(inner) = self.active() else {
            return true;
        };
        let mut map = inner.map.lock().unwrap();
        let Some(entry) = map.get_mut(&(model.to_string(), idx)) else {
            return true; // never-seen target is closed by default
        };
        match entry.phase {
            Phase::Closed | Phase::HalfOpen => true,
            Phase::Open(until) => {
                if Instant::now() >= until {
                    entry.phase = Phase::HalfOpen;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful upstream response for `(model, idx)`. Resets the failure
    /// count and closes the breaker. Returns `true` when this closed a breaker that
    /// was previously open or half-open (a recovery worth counting).
    pub fn on_success(&self, model: &str, idx: usize) -> bool {
        let Some(inner) = self.active() else {
            return false;
        };
        let mut map = inner.map.lock().unwrap();
        let entry = map.entry((model.to_string(), idx)).or_default();
        let was_tripped = !matches!(entry.phase, Phase::Closed);
        entry.phase = Phase::Closed;
        entry.consecutive_failures = 0;
        was_tripped
    }

    /// Record a transient failure for `(model, idx)`. A failure while half-open
    /// re-opens immediately; a closed target opens once its consecutive failures
    /// reach the threshold. Returns `true` when this call tripped the target open
    /// (a closed→open or half-open→open transition worth counting).
    pub fn on_failure(&self, model: &str, idx: usize) -> bool {
        let Some(inner) = self.active() else {
            return false;
        };
        let failure_threshold = inner.failure_threshold.load(Relaxed).max(1);
        let open_secs = inner.open_secs.load(Relaxed);
        let mut map = inner.map.lock().unwrap();
        let entry = map.entry((model.to_string(), idx)).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        let open_until = Instant::now() + Duration::from_secs(open_secs);
        match entry.phase {
            // a failed probe drops straight back to open
            Phase::HalfOpen => {
                entry.phase = Phase::Open(open_until);
                true
            }
            Phase::Closed if entry.consecutive_failures >= failure_threshold => {
                entry.phase = Phase::Open(open_until);
                true
            }
            // already open, or not yet at threshold
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_is_inert() {
        let b = Breaker::default();
        assert!(b.allows("m", 0));
        // failures never trip a disabled breaker
        for _ in 0..100 {
            assert!(!b.on_failure("m", 0));
        }
        assert!(b.allows("m", 0));
    }

    #[test]
    fn trips_open_after_threshold() {
        let b = Breaker::new(true, 3, 30);
        // below threshold: still closed, still admitted
        assert!(!b.on_failure("m", 0));
        assert!(!b.on_failure("m", 0));
        assert!(b.allows("m", 0));
        // the third consecutive failure trips it open
        assert!(b.on_failure("m", 0));
        assert!(!b.allows("m", 0));
        // a distinct target is unaffected
        assert!(b.allows("m", 1));
    }

    #[test]
    fn success_resets_failure_count() {
        let b = Breaker::new(true, 3, 30);
        b.on_failure("m", 0);
        b.on_failure("m", 0);
        // a success clears the count so the next two failures do not trip it
        assert!(!b.on_success("m", 0));
        assert!(!b.on_failure("m", 0));
        assert!(!b.on_failure("m", 0));
        assert!(b.allows("m", 0));
    }

    #[test]
    fn half_open_probe_closes_on_success() {
        let b = Breaker::new(true, 1, 0); // open window of 0s → immediately probeable
        assert!(b.on_failure("m", 0)); // trips open
                                       // window already elapsed: the next allow admits a half-open probe
        assert!(b.allows("m", 0));
        // a success on the probe closes the breaker (a recovery)
        assert!(b.on_success("m", 0));
        assert!(b.allows("m", 0));
    }

    #[test]
    fn half_open_probe_reopens_on_failure() {
        let b = Breaker::new(true, 1, 0);
        assert!(b.on_failure("m", 0)); // trips open
        assert!(b.allows("m", 0)); // admits half-open probe
                                   // the probe fails: straight back to open, counted as a trip
        assert!(b.on_failure("m", 0));
    }

    #[test]
    fn reconfigure_can_disable_and_re_enable_preserving_state() {
        let b = Breaker::new(true, 1, 30);
        assert!(b.on_failure("m", 0)); // trips open
        assert!(!b.allows("m", 0));

        // disabling makes it inert: every target admitted, failures ignored
        b.reconfigure(false, 1, 30);
        assert!(b.allows("m", 0));
        assert!(!b.on_failure("m", 0));

        // re-enabling resumes the preserved phase — the target is still open
        b.reconfigure(true, 1, 30);
        assert!(!b.allows("m", 0));
    }

    #[test]
    fn reconfigure_retunes_threshold_in_place() {
        let b = Breaker::new(true, 5, 30);
        // below the original threshold of 5
        for _ in 0..3 {
            assert!(!b.on_failure("m", 0));
        }
        // loosen? no — tighten to 4: the 4th consecutive failure now trips it
        b.reconfigure(true, 4, 30);
        assert!(b.on_failure("m", 0));
        assert!(!b.allows("m", 0));
    }

    #[test]
    fn reconfigure_is_a_noop_on_inert_default() {
        let b = Breaker::default();
        b.reconfigure(true, 1, 1);
        // still inert: no backing store to enable
        assert!(b.allows("m", 0));
        assert!(!b.on_failure("m", 0));
    }
}
