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

pub(crate) struct CallGap {
    gap: Duration,
    last: Mutex<HashMap<String, Instant>>,
}

impl CallGap {
    pub(crate) fn new(gap: Duration) -> Self {
        Self {
            gap,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Claim a run for `caller`: `Ok` records it, `Err` is how long to wait.
    pub(crate) fn claim(&self, caller: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let mut last = self.last.lock();
        last.retain(|_, at| now.saturating_duration_since(*at) < self.gap);
        if let Some(at) = last.get(caller) {
            return Err(self.gap.saturating_sub(now.saturating_duration_since(*at)));
        }
        last.insert(caller.to_string(), now);
        Ok(())
    }
}

impl Default for CallGap {
    fn default() -> Self {
        Self::new(MAIL_CHECK_GAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_waits_out_the_gap_and_others_are_not_held() {
        let gap = CallGap::new(Duration::from_millis(300));
        assert!(gap.claim("a").is_ok());
        assert!(gap.claim("b").is_ok(), "another caller has its own clock");
        let wait = gap.claim("a").unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_millis(300));
        std::thread::sleep(Duration::from_millis(350));
        assert!(gap.claim("a").is_ok());
    }

    #[test]
    fn a_callers_entry_is_dropped_once_its_gap_has_passed() {
        let gap = CallGap::new(Duration::from_millis(100));
        for caller in ["a", "b", "c"] {
            assert!(gap.claim(caller).is_ok());
        }
        std::thread::sleep(Duration::from_millis(150));
        assert!(gap.claim("d").is_ok());
        assert_eq!(gap.last.lock().len(), 1);
    }
}
