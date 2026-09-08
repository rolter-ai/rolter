//! Bounded, asynchronous sink for post-response usage accounting (#1051).
//!
//! After a response finishes the gateway has to add its cost to the applicable
//! budget counters and its tokens to the applicable rate-limit windows. Both
//! write to Redis, so neither can run inline on the response path.
//!
//! The previous shape was `tokio::spawn` per request per recorder: correct in
//! the happy case, but fire-and-forget with no bound is a queue with infinite
//! depth. When the counter store slows down, tasks accumulate without limit —
//! memory grows, and the pile-up is worst exactly when the system is already
//! under stress.
//!
//! This module applies the treatment the request-log sink already uses
//! ([`crate::logging::LogSink`]): one bounded channel, a small pool of drain
//! workers, a non-blocking `try_send`, and an explicit drop policy with a
//! counter so overflow is observable rather than silent. Dropping is the right
//! failure mode here — a lost record under-counts spend for one request, while
//! an unbounded queue takes the gateway down.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::budgets::SpendRecorder;
use crate::metrics::Metrics;
use crate::rate_limits::TokenRecorder;

/// One unit of post-response accounting.
pub enum UsageRecord {
    /// add a request's cost to its applicable budget counters
    Spend { recorder: SpendRecorder, cost: f64 },
    /// add a request's tokens to its applicable `tpm` windows
    Tokens {
        recorder: TokenRecorder,
        tokens: u64,
    },
}

impl UsageRecord {
    async fn apply(self) {
        match self {
            UsageRecord::Spend { recorder, cost } => recorder.record(cost).await,
            UsageRecord::Tokens { recorder, tokens } => recorder.record(tokens).await,
        }
    }
}

/// Cheaply-cloneable handle used from the response path. A sink with no channel
/// (the derived default, and what tests and embedders get) discards silently
/// without counting: nothing was configured, so nothing was lost.
#[derive(Clone, Default)]
pub struct UsageRecorderSink {
    tx: Option<mpsc::Sender<UsageRecord>>,
    metrics: Option<Arc<Metrics>>,
}

impl UsageRecorderSink {
    /// Build a sink and spawn `workers` drain tasks behind a queue of
    /// `queue_capacity`. Must be called from within a Tokio runtime.
    pub fn spawn(queue_capacity: usize, workers: usize, metrics: Arc<Metrics>) -> Self {
        let (tx, rx) = mpsc::channel::<UsageRecord>(queue_capacity.max(1));
        // a single shared receiver behind a mutex, the same shape the provider
        // queues use: each worker takes the next record and releases the lock
        // before awaiting its Redis round trip, so the workers overlap
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = rx.clone();
            tokio::spawn(async move {
                loop {
                    let Some(record) = ({ rx.lock().await.recv().await }) else {
                        break; // every sender dropped
                    };
                    record.apply().await;
                }
            });
        }
        Self {
            tx: Some(tx),
            metrics: Some(metrics),
        }
    }

    /// Enqueue a record without blocking. Drops and counts it when the queue is
    /// full or every worker has stopped — the response path must never wait on
    /// the counter store.
    pub fn record(&self, record: UsageRecord) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx.try_send(record).is_err() {
            if let Some(metrics) = &self.metrics {
                metrics.usage_records_dropped_total.fetch_add(1, Relaxed);
            }
        }
    }

    /// Records currently waiting to be written. Exposed as a gauge so queue
    /// pressure is visible before it turns into drops.
    pub fn queued(&self) -> usize {
        self.tx
            .as_ref()
            .map(|tx| tx.max_capacity() - tx.capacity())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{BudgetConfig, BudgetPeriod, BudgetScope};

    fn scope() -> crate::budgets::ScopeIds {
        crate::budgets::ScopeIds {
            org: "org-1".to_string(),
            ..Default::default()
        }
    }

    fn budgets() -> Arc<Vec<BudgetConfig>> {
        Arc::new(vec![BudgetConfig {
            scope: BudgetScope::Org,
            id: "org-1".to_string(),
            limit_usd: 1_000.0,
            period: BudgetPeriod::Monthly,
            unpriced_policy: None,
        }])
    }

    /// A recorder pointed at a Redis that does not answer. Every record it
    /// receives occupies a worker until the connection attempt gives up, which
    /// is exactly the stall this queue exists to survive.
    fn stalling_spend_recorder() -> SpendRecorder {
        // a routable-but-dead address: connecting hangs rather than failing fast
        let enforcer = crate::budgets::BudgetEnforcer::new("redis://192.0.2.1:6379");
        SpendRecorder::new(enforcer, budgets(), scope())
    }

    /// The default sink is inert: no channel, no workers, no counting.
    #[tokio::test]
    async fn a_default_sink_discards_without_counting() {
        let sink = UsageRecorderSink::default();
        sink.record(UsageRecord::Spend {
            recorder: stalling_spend_recorder(),
            cost: 1.0,
        });
        assert_eq!(sink.queued(), 0);
    }

    /// The point of the change: a stalled backend must not let work accumulate
    /// without limit. Far more records are offered than the queue can hold, and
    /// the queue must stay at its capacity while the rest are dropped and
    /// counted — never queued, never blocking the caller.
    #[tokio::test]
    async fn a_stalled_backend_bounds_the_queue_and_counts_the_drops() {
        let metrics = Arc::new(Metrics::default());
        // one worker, tiny queue, so the stall is reached immediately
        let sink = UsageRecorderSink::spawn(4, 1, metrics.clone());

        for _ in 0..1_000 {
            sink.record(UsageRecord::Spend {
                recorder: stalling_spend_recorder(),
                cost: 0.01,
            });
        }

        assert!(
            sink.queued() <= 4,
            "the queue must stay bounded, held {}",
            sink.queued()
        );
        let dropped = metrics.usage_records_dropped_total.load(Relaxed);
        assert!(
            dropped >= 900,
            "the overflow must be counted, not silent; counted {dropped}"
        );
    }

    /// `record` is called from the response path and must return immediately
    /// regardless of what the counter store is doing.
    #[tokio::test]
    async fn recording_never_blocks_the_caller() {
        let metrics = Arc::new(Metrics::default());
        let sink = UsageRecorderSink::spawn(2, 1, metrics);
        let started = std::time::Instant::now();
        for _ in 0..10_000 {
            sink.record(UsageRecord::Spend {
                recorder: stalling_spend_recorder(),
                cost: 0.01,
            });
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "10k enqueues took {:?}; the response path is blocking on the sink",
            started.elapsed()
        );
    }

    /// Records that fit are delivered, not merely accepted — a bounded queue
    /// that quietly ate everything would pass the tests above.
    #[tokio::test]
    async fn queued_records_reach_a_worker() {
        let metrics = Arc::new(Metrics::default());
        // a disabled enforcer: `record` completes immediately, so the worker
        // draining the queue is observable through the queue depth alone
        let sink = UsageRecorderSink::spawn(64, 2, metrics.clone());
        let recorder = SpendRecorder::new(
            crate::budgets::BudgetEnforcer::disabled(),
            budgets(),
            scope(),
        );
        for _ in 0..32 {
            sink.record(UsageRecord::Spend {
                recorder: recorder.clone(),
                cost: 0.01,
            });
        }
        // yield until the workers have drained everything
        for _ in 0..1_000 {
            if sink.queued() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            sink.queued(),
            0,
            "the workers should have drained the queue"
        );
        assert_eq!(
            metrics.usage_records_dropped_total.load(Relaxed),
            0,
            "nothing should have been dropped at this depth"
        );
    }
}
