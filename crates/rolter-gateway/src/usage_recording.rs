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
use crate::sink_drain::SinkTasks;

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
    /// stop handle for the workers; `None` on an inert sink
    tasks: Option<Arc<SinkTasks>>,
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
        let tasks = Arc::new(SinkTasks::default());
        for _ in 0..workers.max(1) {
            let rx = rx.clone();
            let stop = tasks.token();
            tasks.track(tokio::spawn(async move {
                loop {
                    let next = async { rx.lock().await.recv().await };
                    let record = tokio::select! {
                        // stop first: with a backlog both branches are ready,
                        // and an unbiased pick would keep taking records from
                        // an open queue, so a send racing the drain could land
                        // and the queue would close only once it ran dry
                        biased;
                        // shutdown: close the shared receiver so the workers
                        // apply what is queued and then see `None`. a worker
                        // stopped mid-wait loses nothing, `recv` is cancel-safe
                        _ = stop.cancelled() => {
                            rx.lock().await.close();
                            rx.lock().await.recv().await
                        }
                        record = next => record,
                    };
                    let Some(record) = record else {
                        break; // every sender dropped, or drained at shutdown
                    };
                    record.apply().await;
                }
            }));
        }
        Self {
            tx: Some(tx),
            metrics: Some(metrics),
            tasks: Some(tasks),
        }
    }

    /// Apply every record still queued and stop the workers. Returns once they
    /// have exited; the caller bounds the wait. A no-op on an inert sink.
    pub async fn shutdown(&self) {
        if let Some(tasks) = &self.tasks {
            tasks.stop().await;
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

    /// A decimal literal for tests. `rust_decimal`'s `dec!` macro would read
    /// slightly better, but its `macros` feature pulls `rust_decimal_macros`,
    /// `proc-macro-crate`, `toml_edit` and `borsh` into the dependency graph in
    /// production position, which is a poor trade for test ergonomics (#967).
    fn d(literal: &str) -> rust_decimal::Decimal {
        literal.parse().expect("a valid decimal literal")
    }

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
            limit_usd: d("1000.0"),
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

    /// A Redis stand-in that speaks just enough RESP for the budget recorder:
    /// every command is answered `+OK` except `INCRBYFLOAT`, which is held for
    /// `latency` before it is counted and answered. The count is what reached
    /// "Redis", so a record the sink lost never shows up in it.
    async fn slow_redis(
        latency: std::time::Duration,
    ) -> (String, Arc<std::sync::atomic::AtomicU64>) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the redis stand-in");
        let addr = listener.local_addr().expect("stand-in address");
        let applied = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = applied.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut stream = BufReader::new(stream);
                    let mut line = String::new();
                    loop {
                        // a command is `*<n>` followed by n `$<len>` bulk strings
                        line.clear();
                        if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let Some(argc) = line.trim_end().strip_prefix('*') else {
                            return;
                        };
                        let argc: usize = argc.parse().unwrap_or(0);
                        let mut args = Vec::with_capacity(argc);
                        for _ in 0..argc {
                            line.clear();
                            if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                                return;
                            }
                            let len: usize = line
                                .trim_end()
                                .strip_prefix('$')
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                            let mut arg = vec![0; len + 2];
                            if stream.read_exact(&mut arg).await.is_err() {
                                return;
                            }
                            arg.truncate(len);
                            args.push(arg);
                        }
                        let reply: &[u8] = if args
                            .first()
                            .is_some_and(|cmd| cmd.eq_ignore_ascii_case(b"INCRBYFLOAT"))
                        {
                            tokio::time::sleep(latency).await;
                            counter.fetch_add(1, Relaxed);
                            b"$1\r\n1\r\n"
                        } else {
                            b"+OK\r\n"
                        };
                        if stream.get_mut().write_all(reply).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (format!("redis://{addr}"), applied)
    }

    /// A recorder against `url` whose single applicable budget makes each
    /// record exactly one `INCRBYFLOAT`.
    fn spend_recorder(url: &str) -> SpendRecorder {
        SpendRecorder::new(crate::budgets::BudgetEnforcer::new(url), budgets(), scope())
    }

    /// The shutdown drain applies every record still queued rather than
    /// dropping the workers' backlog with the runtime (#2374). The stand-in
    /// answers slowly enough that nearly the whole batch is still queued when
    /// the drain starts, and the drain is driven through `drain_sinks`, the
    /// call `run()` makes after `SIGTERM`.
    #[tokio::test]
    async fn the_shutdown_drain_applies_every_queued_record() {
        const RECORDS: u64 = 16;
        // well under the 500 ms response timeout even with both workers'
        // commands pipelined on the one multiplexed connection
        let (url, applied) = slow_redis(std::time::Duration::from_millis(40)).await;
        let metrics = Arc::new(Metrics::default());
        let sink = UsageRecorderSink::spawn(64, 2, metrics.clone());
        let recorder = spend_recorder(&url);
        for _ in 0..RECORDS {
            sink.record(UsageRecord::Spend {
                recorder: recorder.clone(),
                cost: 0.01,
            });
        }
        assert!(
            applied.load(Relaxed) < RECORDS,
            "every record was applied before the drain, so it proves nothing"
        );

        let mut state = crate::state::AppState::new(&rolter_core::GatewayConfig::default());
        state.log = state.log.clone().with_usage_recorders(sink.clone());
        let drained = state.drain_sinks(std::time::Duration::from_secs(10)).await;

        assert!(drained, "drain_sinks ran out of grace");
        assert_eq!(
            applied.load(Relaxed),
            RECORDS,
            "the drain returned before every queued record reached redis"
        );
        assert_eq!(metrics.usage_records_dropped_total.load(Relaxed), 0);
    }

    /// The drain closes the queue on a worker's first turn after the stop, so
    /// a record offered while the workers are still applying the backlog is
    /// refused and counted like a full queue — never silently lost, and never
    /// stretching the drain past what was queued when it began.
    #[tokio::test]
    async fn a_send_racing_the_shutdown_close_is_counted_as_dropped() {
        let (url, applied) = slow_redis(std::time::Duration::from_millis(100)).await;
        let metrics = Arc::new(Metrics::default());
        let sink = UsageRecorderSink::spawn(16, 1, metrics.clone());
        let recorder = spend_recorder(&url);
        for _ in 0..4 {
            sink.record(UsageRecord::Spend {
                recorder: recorder.clone(),
                cost: 0.01,
            });
        }
        // let the worker take the first record, so the stop lands mid-apply
        for _ in 0..1_000 {
            if sink.queued() == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert_eq!(sink.queued(), 3, "the worker never took a record");

        let draining = tokio::spawn({
            let sink = sink.clone();
            async move { sink.shutdown().await }
        });
        // wait for the close itself, not for the drain to finish
        let tx = sink.tx.clone().expect("a spawned sink has a channel");
        for _ in 0..1_000 {
            if tx.is_closed() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(tx.is_closed(), "the drain never closed the queue");
        // the worker took one record before the stop and must close the queue
        // on its very next turn, not once the rest of the backlog ran dry
        assert_eq!(
            applied.load(Relaxed),
            1,
            "the queue stayed open while the backlog was applied"
        );

        sink.record(UsageRecord::Spend {
            recorder: recorder.clone(),
            cost: 0.01,
        });
        assert_eq!(metrics.usage_records_dropped_total.load(Relaxed), 1);

        tokio::time::timeout(std::time::Duration::from_secs(10), draining)
            .await
            .expect("the drain finished")
            .expect("the drain task did not panic");
        assert_eq!(
            applied.load(Relaxed),
            4,
            "only the queued backlog is applied"
        );

        // and once the workers have exited, a send still counts
        sink.record(UsageRecord::Spend {
            recorder,
            cost: 0.01,
        });
        assert_eq!(metrics.usage_records_dropped_total.load(Relaxed), 2);
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
