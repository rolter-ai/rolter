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

use crate::target_mask::TargetMask;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
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

/// The phase of a target's breaker, as reported to the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakerPhase {
    Closed,
    Open,
    HalfOpen,
}

/// One target's breaker, copied out of the registry for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakerView {
    pub model: String,
    pub target_index: usize,
    pub state: BreakerPhase,
    pub consecutive_failures: u32,
    /// whole seconds until an open breaker admits its probe
    pub retry_after_secs: Option<u64>,
}

/// Per-target breaker state: its phase, the running count of consecutive
/// transient failures observed while closed, and when that count last moved.
///
/// The stamp is set by failures only, never by reads: `allows` is the hot path
/// and must not pay for an `Instant::now()`, and the only thing a sweep can
/// destroy is a partial failure count — which is precisely what goes stale when
/// a target stops failing. A still-busy but long-healthy target is safe to
/// retire, because an absent entry already means closed with no failures.
struct Entry {
    phase: Phase,
    consecutive_failures: u32,
    last_failure: Instant,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            phase: Phase::Closed,
            consecutive_failures: 0,
            last_failure: Instant::now(),
        }
    }
}

/// Breaker state for one model's targets, addressed by target index.
///
/// A `Vec` rather than a second map: target indices are positions in a route's
/// target list, so they are small and dense, and indexing costs a bounds check
/// instead of a second hash. `None` is a target with no recorded outcome, which
/// is indistinguishable from a closed one.
type ModelBreakers = Vec<Option<Entry>>;

/// Per-target breaker state keyed by public model, then addressed by target index.
///
/// Split rather than a flat `(String, usize)` key: a flat tuple cannot be looked
/// up from a `&str`, so every read had to materialise a `String` just to hash
/// it. `allows` is a read-only admission check run per request per candidate
/// target — including the common case where the target was never seen and the
/// answer is an immediate `true` — so that allocation sat on the hottest path in
/// the gateway. Splitting keeps reads borrowed, hashes the model exactly once
/// and confines allocation to a model's first recorded failure (#1050).
type BreakerMap = HashMap<String, ModelBreakers>;

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
/// `Some` inner can be enabled, disabled and re-tuned live via [`Breaker::reconfigure`].
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
        let mut map = inner.map.lock();
        // borrowed throughout: the never-seen case must not allocate, and it is
        // the overwhelmingly common one
        let Some(entry) = map
            .get_mut(model)
            .and_then(|m| m.get_mut(idx))
            .and_then(Option::as_mut)
        else {
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

    /// Which of a model's first `n` targets would refuse traffic right now (an
    /// open breaker whose window has not elapsed), answered with one lock
    /// acquisition rather than one per target (#1942).
    ///
    /// Read-only, unlike [`Breaker::allows`]: a target whose open window has
    /// elapsed is not in the mask, but is not moved to half-open either. A
    /// caller that goes on to use such a target calls `allows` for it, which
    /// admits the probe.
    pub fn open_mask(&self, model: &str, n: usize) -> TargetMask {
        let mut mask = TargetMask::new();
        let Some(inner) = self.active() else {
            return mask;
        };
        let map = inner.map.lock();
        let Some(targets) = map.get(model) else {
            return mask;
        };
        let mut now = None;
        for (idx, entry) in targets.iter().enumerate().take(n) {
            if let Some(Entry {
                phase: Phase::Open(until),
                ..
            }) = entry
            {
                if *until > *now.get_or_insert_with(Instant::now) {
                    mask.insert(idx);
                }
            }
        }
        mask
    }

    /// Record a successful upstream response for `(model, idx)`. Resets the failure
    /// count and closes the breaker. Returns `true` when this closed a breaker that
    /// was previously open or half-open (a recovery worth counting).
    pub fn on_success(&self, model: &str, idx: usize) -> bool {
        let Some(inner) = self.active() else {
            return false;
        };
        let mut map = inner.map.lock();
        let Some(targets) = map.get_mut(model) else {
            // a target with no entry is already closed with no failures, which
            // is exactly what this call would have written — so the success of
            // a healthy target neither allocates nor grows the map
            return false;
        };
        let Some(slot) = targets.get_mut(idx) else {
            return false;
        };
        let Some(entry) = slot.take() else {
            return false;
        };
        // the entry is *removed* rather than reset: "closed with no failures" is
        // exactly what an absent entry means, so keeping one would be a row that
        // says nothing and never leaves. A recovered target therefore gives its
        // slot back instead of holding it for the process lifetime (#1053)
        if targets.iter().all(Option::is_none) {
            map.remove(model);
        }
        !matches!(entry.phase, Phase::Closed)
    }

    /// Drop entries for targets that have gone quiet, so a long-lived process
    /// does not accumulate a row per `(model, target)` it has ever served —
    /// including models retired by a config hot-reload, which the gateway
    /// applies without restarting.
    ///
    /// Only **closed** entries are eligible: a target that is currently open or
    /// half-open is under active probe, and forgetting it would silently
    /// re-admit traffic to an upstream that is still down. Retiring a closed
    /// entry costs at most a partial failure count that has not moved for
    /// `idle`, which is stale by construction.
    pub fn sweep(&self, idle: Duration) {
        let Some(inner) = &self.inner else {
            return;
        };
        let now = Instant::now();
        let mut map = inner.map.lock();
        map.retain(|_, targets| {
            for slot in targets.iter_mut() {
                let retire = slot.as_ref().is_some_and(|e| {
                    matches!(e.phase, Phase::Closed)
                        && now.saturating_duration_since(e.last_failure) >= idle
                });
                if retire {
                    *slot = None;
                }
            }
            // trailing empty slots are pure overhead once a route shrinks
            while targets.last().is_some_and(Option::is_none) {
                targets.pop();
            }
            !targets.is_empty()
        });
    }

    /// Number of live `(model, target)` entries held. Exported as a gauge so
    /// map growth is observable rather than inferred (#1053).
    pub fn len(&self) -> usize {
        self.inner
            .as_ref()
            .map(|i| {
                i.map
                    .lock()
                    .values()
                    .map(|t| t.iter().filter(|e| e.is_some()).count())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Every `(model, target)` the registry holds an entry for, with the phase
    /// it is in right now. Absent targets are closed with no failures and are
    /// not listed. Meant for the periodic fleet report (#2254), never for a
    /// request: it takes the map lock once and allocates per entry.
    ///
    /// An open entry whose window has elapsed is reported `half_open`: the
    /// registry moves it there on the next `allows`, but nothing is routed to
    /// it until then and an operator should read it as ready to probe, not as
    /// still shedding load.
    pub fn snapshot(&self) -> Vec<BreakerView> {
        let Some(inner) = &self.inner else {
            return Vec::new();
        };
        let now = Instant::now();
        let map = inner.map.lock();
        let mut views = Vec::new();
        for (model, targets) in map.iter() {
            for (idx, slot) in targets.iter().enumerate() {
                let Some(entry) = slot else { continue };
                let (state, retry_after_secs) = match entry.phase {
                    Phase::Closed => (BreakerPhase::Closed, None),
                    Phase::HalfOpen => (BreakerPhase::HalfOpen, None),
                    Phase::Open(until) if now >= until => (BreakerPhase::HalfOpen, None),
                    Phase::Open(until) => (
                        BreakerPhase::Open,
                        Some(until.saturating_duration_since(now).as_secs().max(1)),
                    ),
                };
                views.push(BreakerView {
                    model: model.clone(),
                    target_index: idx,
                    state,
                    consecutive_failures: entry.consecutive_failures,
                    retry_after_secs,
                });
            }
        }
        views
    }

    /// Whether the registry holds no per-target state.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        let open_until = Instant::now() + Duration::from_secs(open_secs);
        let mut map = inner.map.lock();
        // a target that has already failed once is the common case while a
        // fault persists, and it needs no allocation; only a model's first
        // failure owns its name
        if let Some(entry) = map
            .get_mut(model)
            .and_then(|m| m.get_mut(idx))
            .and_then(Option::as_mut)
        {
            return record_failure(entry, failure_threshold, open_until);
        }
        // reached only on a target's *first* failure, at most once per target
        // per process, so owning the model name here costs nothing measurable
        let targets = map.entry(model.to_string()).or_default();
        if targets.len() <= idx {
            targets.resize_with(idx + 1, || None);
        }
        let entry = targets[idx].get_or_insert_with(Entry::default);
        record_failure(entry, failure_threshold, open_until)
    }
}

/// Fold one transient failure into a target's entry. Returns `true` when the
/// call tripped the target open (a closed→open or half-open→open transition).
fn record_failure(entry: &mut Entry, failure_threshold: u32, open_until: Instant) -> bool {
    entry.last_failure = Instant::now();
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_lists_only_targets_with_an_entry_and_their_phase() {
        let b = Breaker::new(true, 2, 30);
        assert!(b.snapshot().is_empty(), "a fresh registry reports nothing");
        // one failure: closed but counting
        b.on_failure("m", 1);
        // two failures: tripped open
        b.on_failure("n", 0);
        b.on_failure("n", 0);
        let mut views = b.snapshot();
        views.sort_by(|a, b| a.model.cmp(&b.model));
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].model, "m");
        assert_eq!(views[0].target_index, 1);
        assert_eq!(views[0].state, BreakerPhase::Closed);
        assert_eq!(views[0].consecutive_failures, 1);
        assert_eq!(views[0].retry_after_secs, None);
        assert_eq!(views[1].state, BreakerPhase::Open);
        assert!(views[1]
            .retry_after_secs
            .is_some_and(|s| (1..=30).contains(&s)));
        // a recovered target leaves the report
        assert!(b.on_success("n", 0));
        assert_eq!(b.snapshot().len(), 1);
    }

    #[test]
    fn an_elapsed_open_window_reads_as_half_open() {
        let b = Breaker::new(true, 1, 0);
        b.on_failure("m", 0);
        assert_eq!(b.snapshot()[0].state, BreakerPhase::HalfOpen);
    }

    #[test]
    fn an_inert_registry_reports_nothing() {
        assert!(Breaker::default().snapshot().is_empty());
    }

    /// A `std::sync::Mutex` would poison here and every later `allows()` would
    /// panic — one transient panic anywhere under the lock would turn into a
    /// permanent, restart-only outage for all traffic (#1049). `parking_lot`
    /// has no poison state, so the registry survives and keeps admitting.
    #[test]
    fn a_panic_under_the_lock_does_not_brick_the_registry() {
        let b = Breaker::new(true, 3, 30);
        // put real state in the map so recovery is observable, not vacuous
        b.on_failure("m", 0);
        b.on_failure("m", 0);
        let inner = b
            .inner
            .clone()
            .expect("an enabled breaker has a backing store");

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = inner.map.lock();
            panic!("a transient panic while the breaker map is locked");
        }));
        assert!(panicked.is_err(), "the test's own panic must have fired");

        // the lock is usable again and the pre-panic state is intact: the third
        // consecutive failure still trips the target, exactly as before
        assert!(b.allows("m", 0));
        assert!(b.on_failure("m", 0));
        assert!(!b.allows("m", 0));
        // and an unrelated target is still admitted rather than panicking
        assert!(b.allows("other", 7));
    }

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

    /// Number of `(model, target)` entries currently held, across all models.
    fn entries(b: &Breaker) -> usize {
        b.inner
            .as_ref()
            .map(|i| i.map.lock().values().map(|m| m.len()).sum())
            .unwrap_or(0)
    }

    /// A healthy target's success is indistinguishable from having no entry at
    /// all, so it must not create one — this is what keeps the all-healthy
    /// steady state both allocation-free and map-growth-free.
    #[test]
    fn a_success_on_a_never_seen_target_records_nothing() {
        let b = Breaker::new(true, 3, 30);
        for _ in 0..1_000 {
            assert!(!b.on_success("m", 0));
        }
        assert_eq!(entries(&b), 0, "healthy traffic must not populate the map");
        // and the target is of course still admitted
        assert!(b.allows("m", 0));
    }

    /// A success must still close a target that actually was tripped — the
    /// no-entry short-circuit above must not swallow a real recovery.
    #[test]
    fn a_success_still_closes_a_tripped_target() {
        let b = Breaker::new(true, 3, 30);
        b.on_failure("m", 0);
        b.on_failure("m", 0);
        assert!(b.on_failure("m", 0), "the third failure trips it open");
        assert!(!b.allows("m", 0));
        assert!(b.on_success("m", 0), "the recovery must be reported");
        assert!(b.allows("m", 0));
        // the failure count was reset too, not merely the phase: two more
        // failures must not be enough to trip it again
        assert!(!b.on_failure("m", 0));
        assert!(!b.on_failure("m", 0));
        assert!(b.allows("m", 0));
        assert_eq!(entries(&b), 1);
    }

    /// The key is nested now; two models sharing a target index must not alias.
    #[test]
    fn models_with_the_same_target_index_do_not_alias() {
        let b = Breaker::new(true, 1, 30);
        assert!(b.on_failure("a", 0)); // trips a/0 open
        assert!(!b.allows("a", 0));
        // same index, different model: untouched and still admitted
        assert!(b.allows("b", 0));
        assert!(b.on_failure("b", 0));
        assert!(!b.allows("b", 0));
        // and a's state did not move when b tripped
        assert!(!b.allows("a", 0));
        assert_eq!(entries(&b), 2);
    }

    /// A model name containing the separator-ish characters a flat key might
    /// have used must still be a distinct model.
    #[test]
    fn model_names_are_compared_whole() {
        let b = Breaker::new(true, 1, 30);
        assert!(b.on_failure("m::v", 0));
        assert!(!b.allows("m::v", 0));
        assert!(b.allows("m", 0));
    }

    /// A recovered target gives its slot back: "closed with no failures" is
    /// exactly what an absent entry means, so holding a row for it would be a
    /// leak that says nothing (#1053).
    #[test]
    fn a_recovery_releases_the_entry() {
        let b = Breaker::new(true, 1, 30);
        assert!(b.on_failure("m", 0));
        assert_eq!(b.len(), 1);
        assert!(b.on_success("m", 0), "the recovery is still reported");
        assert!(b.is_empty(), "a recovered target must not hold a row");
        // and the target is admitted, which is what an absent entry means
        assert!(b.allows("m", 0));
    }

    /// A partial failure count that has not moved for the idle window is stale
    /// by construction, so a sweep may retire it.
    #[test]
    fn a_sweep_retires_a_stale_closed_entry() {
        let b = Breaker::new(true, 5, 30);
        b.on_failure("m", 0); // 1 of 5: closed, but holding a row
        assert_eq!(b.len(), 1);
        // a window the entry cannot have been idle for leaves it alone
        b.sweep(Duration::from_secs(3600));
        assert_eq!(b.len(), 1, "a fresh entry must survive");
        // an elapsed window retires it
        b.sweep(Duration::ZERO);
        assert!(b.is_empty());
        // the target reads as closed either way
        assert!(b.allows("m", 0));
    }

    /// The sweep must not touch a target under probe, in either non-closed
    /// phase, or traffic would be re-admitted to an upstream that is still down.
    #[test]
    fn a_sweep_keeps_open_and_half_open_targets() {
        let b = Breaker::new(true, 1, 3600);
        assert!(b.on_failure("open", 0)); // → Open, long window
        let probing = Breaker::new(true, 1, 0);
        assert!(probing.on_failure("probing", 0));
        assert!(probing.allows("probing", 0)); // → HalfOpen

        b.sweep(Duration::ZERO);
        probing.sweep(Duration::ZERO);

        assert_eq!(b.len(), 1);
        assert!(!b.allows("open", 0), "an open target stays skipped");
        assert_eq!(probing.len(), 1, "a target under probe keeps its entry");
    }

    /// A model whose every target was retired must not leave an empty shell
    /// behind, or the outer map leaks one key per retired model.
    #[test]
    fn a_sweep_drops_a_model_with_no_surviving_target() {
        let b = Breaker::new(true, 5, 30);
        for idx in 0..8 {
            b.on_failure("m", idx);
        }
        assert_eq!(b.len(), 8);
        b.sweep(Duration::ZERO);
        assert!(b.is_empty());
        assert_eq!(
            b.inner.as_ref().map(|i| i.map.lock().len()),
            Some(0),
            "the model key itself must be gone, not merely emptied"
        );
    }

    /// A sweep is a no-op on a registry with no backing store.
    #[test]
    fn a_sweep_is_a_noop_on_inert_default() {
        let b = Breaker::default();
        b.sweep(Duration::ZERO);
        assert!(b.is_empty());
        assert!(b.allows("m", 0));
    }

    #[test]
    fn reconfigure_is_a_noop_on_inert_default() {
        let b = Breaker::default();
        b.reconfigure(true, 1, 1);
        // still inert: no backing store to enable
        assert!(b.allows("m", 0));
        assert!(!b.on_failure("m", 0));
    }

    #[test]
    fn the_open_mask_lists_only_targets_that_would_refuse() {
        let b = Breaker::new(true, 1, 3600);
        b.on_failure("m", 1);
        b.on_failure("m", 70);
        let mask = b.open_mask("m", 100);
        for idx in 0..100 {
            assert_eq!(mask.contains(idx), idx == 1 || idx == 70, "{idx}");
        }
        assert!(b.open_mask("m", 2).contains(1) && !b.open_mask("m", 2).contains(70));
        assert!(b.open_mask("other", 4).is_empty());
        // disabled and inert registries refuse nothing
        b.reconfigure(false, 1, 3600);
        assert!(b.open_mask("m", 100).is_empty());
        assert!(Breaker::default().open_mask("m", 4).is_empty());
    }

    #[test]
    fn the_open_mask_leaves_an_elapsed_window_for_allows_to_admit() {
        let b = Breaker::new(true, 1, 0);
        b.on_failure("m", 0);
        assert!(b.open_mask("m", 1).is_empty());
        // only allows moves the target to half-open
        assert!(b.allows("m", 0));
    }
}
