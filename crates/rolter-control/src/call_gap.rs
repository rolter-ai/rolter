//! The least time between two runs of an expensive action by one caller.
//!
//! Some control-plane actions cost something outside the process: the SMTP
//! relay check dials the relay, negotiates TLS and authenticates on every call
//! (#3394). [`CallGap`] answers how long a caller must still wait, and the
//! handler turns that into `429` with `Retry-After` through
//! [`ApiError::TooManyAttempts`](crate::crud::ApiError). It is a clock per
//! caller, not a flag: a caller that waits is always let through, and a
//! caller's entry is dropped once its gap has passed, so the map holds only
//! the callers that ran something in the last gap.
//!
//! Process-local on purpose, like the export gate: it protects this process's
//! outbound connections, and with several replicas each holds its own.

// the action this guards only exists in a postgres build, so a default-feature
// build compiles the gap and never calls it. `cargo hack check --each-feature`
// runs that build
#![cfg_attr(not(feature = "postgres"), allow(dead_code))]

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::Mutex;
use std::time::Instant;

/// How long one caller waits between relay checks.
pub(crate) const MAIL_CHECK_GAP: Duration = Duration::from_secs(5);
/// How long anyone waits after any caller's relay check: the relay is one
/// shared resource, so N admins would otherwise get N runs per caller gap.
pub(crate) const MAIL_CHECK_SHARED_GAP: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Clocks {
    by_caller: HashMap<String, Instant>,
    /// when the last run, by anyone, started
    last: Option<Instant>,
}

pub(crate) struct CallGap {
    /// the least time between two runs by one caller
    gap: Duration,
    /// the least time between two runs by anyone
    shared_gap: Duration,
    clocks: Mutex<Clocks>,
}

impl CallGap {
    pub(crate) fn new(gap: Duration, shared_gap: Duration) -> Self {
        Self {
            gap,
            shared_gap,
            clocks: Mutex::new(Clocks::default()),
        }
    }

    /// Claim a run for `caller`: `Ok` records it, `Err` is how long to wait,
    /// the longer of the caller's own gap and the shared one. A refused claim
    /// records nothing.
    pub(crate) fn claim(&self, caller: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut clocks = self.clocks.lock();
        clocks
            .by_caller
            .retain(|_, at| now.saturating_duration_since(*at) < self.gap);
        let left =
            |at: Instant, gap: Duration| gap.saturating_sub(now.saturating_duration_since(at));
        let own = clocks.by_caller.get(caller).map(|at| left(*at, self.gap));
        let shared = clocks.last.map(|at| left(at, self.shared_gap));
        let wait = own.into_iter().chain(shared).max().unwrap_or_default();
        if !wait.is_zero() {
            return Err(wait);
        }
        clocks.by_caller.insert(caller.to_string(), now);
        clocks.last = Some(now);
        Ok(())
    }
}

impl Default for CallGap {
    fn default() -> Self {
        Self::new(MAIL_CHECK_GAP, MAIL_CHECK_SHARED_GAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_waits_out_the_gap() {
        let gap = CallGap::new(Duration::from_millis(300), Duration::ZERO);
        assert!(gap.claim("a").is_ok());
        assert!(gap.claim("b").is_ok(), "another caller has its own clock");
        let wait = gap.claim("a").unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_millis(300));
        std::thread::sleep(Duration::from_millis(350));
        assert!(gap.claim("a").is_ok());
    }

    #[test]
    fn anyone_waits_out_the_shared_gap() {
        let gap = CallGap::new(Duration::from_secs(60), Duration::from_millis(200));
        assert!(gap.claim("a").is_ok());
        // a different caller is held by the relay's shared clock
        let wait = gap.claim("b").unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_millis(200));
        std::thread::sleep(Duration::from_millis(250));
        assert!(gap.claim("b").is_ok());
        // a refused claim records nothing: "a" is still on its own long gap
        assert!(gap.claim("a").unwrap_err() > Duration::from_secs(30));
    }

    #[test]
    fn a_callers_entry_is_dropped_once_its_gap_has_passed() {
        let gap = CallGap::new(Duration::from_millis(100), Duration::ZERO);
        for caller in ["a", "b", "c"] {
            assert!(gap.claim(caller).is_ok());
        }
        std::thread::sleep(Duration::from_millis(150));
        assert!(gap.claim("d").is_ok());
        assert_eq!(gap.clocks.lock().by_caller.len(), 1);
    }
}
