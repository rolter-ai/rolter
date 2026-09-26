//! Client-disconnect accounting for the request path (#1083).
//!
//! An LLM gateway sees cancellation constantly: an agent CLI takes `ctrl-c`, a
//! browser tab closes, a proxy in front of rolter times out. Axum's answer is to
//! drop the handler future, which is correct but silent — the request simply
//! stops existing, and with it the log row that would have said what it cost.
//!
//! That silence is the problem. A request the caller abandoned is not free: the
//! upstream may have generated (and billed) thousands of tokens before it went
//! away. Three outcomes were possible before this module and nothing said which
//! one happened — logged as delivered, logged as an error, or not logged at all.
//!
//! [`CancelGuard`] closes the case where nothing was logged. It is armed while
//! the handler waits on the upstream and disarmed once a row's ownership has
//! passed to a path that always emits it. If the future is dropped while armed,
//! the guard emits the row itself, marked
//! [`CLIENT_DISCONNECT_STATUS`](crate::logging::CLIENT_DISCONNECT_STATUS), and
//! counts it in `rolter_client_disconnects_total`.
//!
//! Once the forward loop has picked a target the row names it (#1816), and the
//! drop counts the abandoned attempt against that target like any other
//! failure. The exception is the backoff between a failed attempt and the next
//! one: that failure was already charged to the target by
//! [`CancelGuard::record_failed_attempt`], so a caller who leaves while the loop
//! sleeps still gets a named row, but the target is not charged a second time.
//!
//! The streamed case is handled at the other end, by `UsageLoggingStream`'s
//! `Drop`: there a row already exists and carries the tokens produced before the
//! caller left, so it is marked rather than re-created.
//!
//! Cancellation is never retried. Retries live inside the forward loop, and a
//! dropped future stops polling that loop — there is no path from a disconnect
//! to another upstream attempt.

use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;

use crate::logging::{
    FailedAttempt, LogSink, RequestLog, CLIENT_DISCONNECT_ERROR, CLIENT_DISCONNECT_STATUS,
};

/// Emits a client-disconnect log row if it is dropped while still armed.
pub struct CancelGuard {
    sink: LogSink,
    started: Instant,
    /// the row to emit on an abandoned request; `None` once disarmed
    row: Option<RequestLog>,
    /// the attempt the row names was already charged to its target, so the
    /// drop must not charge it again
    counted: bool,
}

impl CancelGuard {
    /// Arm a guard over `row`, which should carry whatever attribution is known
    /// at the point the upstream call begins.
    pub fn new(sink: LogSink, started: Instant, row: RequestLog) -> Self {
        Self {
            sink,
            started,
            row: Some(row),
            counted: false,
        }
    }

    /// Name the target the forward loop just picked, so a request abandoned
    /// while it waits on that upstream is logged against it (#1816).
    ///
    /// Called on every attempt: a retry that moves to another target moves the
    /// row with it. A request abandoned before any pick keeps the empty
    /// provider it was armed with, which is how that case stays
    /// distinguishable. The row's strings are rewritten in place, so a
    /// re-attribution reuses their buffers.
    ///
    /// A new attempt has not been counted anywhere yet, so this also clears
    /// the mark [`CancelGuard::record_failed_attempt`] left for the previous
    /// one.
    pub fn attribute(&mut self, provider: &str, target: &str, variant: &str) {
        self.counted = false;
        let Some(row) = self.row.as_mut() else {
            return;
        };
        for (slot, value) in [
            (&mut row.provider, provider),
            (&mut row.target, target),
            (&mut row.variant, variant),
        ] {
            slot.clear();
            slot.push_str(value);
        }
    }

    /// Funnel an attempt that failed and is about to be superseded, through
    /// [`LogSink::record_failed_attempt`], and remember that the target the
    /// row names has now been charged for it.
    ///
    /// The forward loops back off between that failure and the next attempt,
    /// and a caller can leave during the wait. The row still names the target
    /// then, but its drop takes only the request-level signals from it, the
    /// same way the loop's own error row does, so one upstream failure never
    /// counts twice against the target's error rate or its health rollup.
    pub fn record_failed_attempt(&mut self, attempt: &FailedAttempt<'_>) {
        self.sink.record_failed_attempt(attempt);
        self.counted = true;
    }

    /// Hand responsibility for the row to a path that logs it itself.
    pub fn disarm(&mut self) {
        self.row = None;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        let Some(mut row) = self.row.take() else {
            return;
        };
        row.status = CLIENT_DISCONNECT_STATUS;
        if row.error.is_empty() {
            row.error = CLIENT_DISCONNECT_ERROR.to_string();
        }
        row.latency_ms = self.started.elapsed().as_millis() as u32;
        self.sink
            .metrics()
            .client_disconnects_total
            .fetch_add(1, Relaxed);
        if self.counted {
            self.sink.log_recorded_attempt(row);
        } else {
            self.sink.log(row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;
    use std::sync::Arc;

    fn row() -> RequestLog {
        RequestLog {
            request_id: "req-1".to_string(),
            model: "gpt-4o".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn an_armed_guard_counts_the_disconnect_when_dropped() {
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::disabled(metrics.clone());
        drop(CancelGuard::new(sink, Instant::now(), row()));
        assert_eq!(metrics.client_disconnects_total.load(Relaxed), 1);
    }

    /// The provider, target and variant an armed guard would log.
    fn attributed(guard: &CancelGuard) -> (String, String, String) {
        let row = guard.row.as_ref().expect("still armed");
        (
            row.provider.clone(),
            row.target.clone(),
            row.variant.clone(),
        )
    }

    #[test]
    fn the_row_follows_the_target_each_attempt_picks() {
        let metrics = Arc::new(Metrics::default());
        let mut guard = CancelGuard::new(LogSink::disabled(metrics), Instant::now(), row());
        // abandoned before any pick: nothing to name, and nothing is invented
        assert_eq!(attributed(&guard), Default::default());

        guard.attribute("vllm-spot-02", "deepseek-r1", "");
        assert_eq!(
            attributed(&guard),
            ("vllm-spot-02".into(), "deepseek-r1".into(), String::new())
        );
        // a retry that fails over moves the row to the new target
        guard.attribute("vllm-spot-01", "deepseek-r1-distill", "canary");
        assert_eq!(
            attributed(&guard),
            (
                "vllm-spot-01".into(),
                "deepseek-r1-distill".into(),
                "canary".into()
            )
        );

        // and a disarmed guard has no row left to attribute
        guard.disarm();
        guard.attribute("other", "other", "");
        assert!(guard.row.is_none());
    }

    #[test]
    fn a_disarmed_guard_is_silent() {
        let metrics = Arc::new(Metrics::default());
        let sink = LogSink::disabled(metrics.clone());
        let mut guard = CancelGuard::new(sink, Instant::now(), row());
        guard.disarm();
        drop(guard);
        assert_eq!(
            metrics.client_disconnects_total.load(Relaxed),
            0,
            "a request that reached a logging path must not also count as abandoned"
        );
    }

    /// The `error` sample of `rolter_target_requests_total` for one target.
    fn target_errors(metrics: &Metrics, provider: &str, target: &str) -> u64 {
        let series = format!(
            "rolter_target_requests_total{{provider=\"{provider}\",target=\"{target}\",outcome=\"error\"}} "
        );
        metrics
            .render()
            .lines()
            .find_map(|line| line.strip_prefix(series.as_str())?.parse().ok())
            .unwrap_or(0)
    }

    fn failed_429<'a>(provider: &'a str, target: &'a str) -> FailedAttempt<'a> {
        FailedAttempt {
            provider,
            target,
            status: 429,
            latency_ms: 12,
            error: "",
        }
    }

    #[test]
    fn a_caller_leaving_during_the_backoff_is_not_charged_to_the_target_again() {
        let metrics = Arc::new(Metrics::default());
        let mut guard = CancelGuard::new(LogSink::disabled(metrics.clone()), Instant::now(), row());
        guard.attribute("vllm-spot-02", "deepseek-r1", "");
        // the upstream answered 429 and the loop is about to back off
        guard.record_failed_attempt(&failed_429("vllm-spot-02", "deepseek-r1"));
        assert_eq!(target_errors(&metrics, "vllm-spot-02", "deepseek-r1"), 1);

        // the caller hangs up while the loop sleeps
        drop(guard);
        assert_eq!(
            target_errors(&metrics, "vllm-spot-02", "deepseek-r1"),
            1,
            "one upstream failure must count once"
        );
        // the abandonment itself is still counted
        assert_eq!(metrics.client_disconnects_total.load(Relaxed), 1);
    }

    #[test]
    fn a_caller_leaving_during_the_next_attempt_is_charged_to_that_target() {
        let metrics = Arc::new(Metrics::default());
        let mut guard = CancelGuard::new(LogSink::disabled(metrics.clone()), Instant::now(), row());
        guard.attribute("vllm-spot-02", "deepseek-r1", "");
        guard.record_failed_attempt(&failed_429("vllm-spot-02", "deepseek-r1"));
        // the retry fails over, and the caller leaves while it is in flight
        guard.attribute("vllm-spot-01", "deepseek-r1", "");
        drop(guard);

        assert_eq!(target_errors(&metrics, "vllm-spot-02", "deepseek-r1"), 1);
        assert_eq!(
            target_errors(&metrics, "vllm-spot-01", "deepseek-r1"),
            1,
            "the attempt in flight was never counted, so the drop counts it"
        );
    }
}
