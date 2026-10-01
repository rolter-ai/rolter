//! The one HTTP client builder behind the gateway's ClickHouse writers (#2373).
//!
//! The request-log, health-event and mcp tool-call writers each POST batches to
//! ClickHouse and flush serially, so a flush that never returns wedges the whole
//! writer: rows queue to `queue_capacity` and are then dropped. A plain
//! `reqwest::Client` has no timeout at all, and a ClickHouse that accepts the
//! connection but never answers is exactly the failure that exposes it. Building
//! the client here keeps the three writers from drifting apart on the bound.

use std::time::Duration;

/// How long a writer waits to open a connection to ClickHouse. A healthy one
/// accepts in milliseconds, so this only ever spends time on a dead host.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long one batch POST may take end to end. Generous for a large batch on a
/// busy server, yet short enough that a stalled one frees the writer to retry
/// the next batch. It is deliberately not a config key: no deployment has a
/// reason to wait longer for a write whose rows are best-effort telemetry.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The client every ClickHouse writer uses, with the default bounds.
pub(crate) fn client() -> reqwest::Client {
    client_with(CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}

/// A client with explicit bounds; tests use short ones to stall quickly. Falls
/// back to an unbounded client only if the builder itself fails, which cannot
/// happen for a bare timeout configuration.
pub(crate) fn client_with(connect: Duration, request: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .timeout(request)
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_events::{HealthEvent, HealthEventSink, HealthOutcome, HealthSource};
    use crate::logging::{LogSink, RequestLog};
    use crate::mcp_log::{McpEvent, McpEventSink};
    use crate::metrics::Metrics;
    use crate::state::AppState;
    use rolter_core::config::GatewayConfig;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::net::TcpListener;

    const BOUND: Duration = Duration::from_millis(300);
    /// test-level ceiling so a missing timeout fails cleanly instead of hanging
    const CEILING: Duration = Duration::from_secs(5);

    /// A fake ClickHouse that accepts connections and then never answers,
    /// holding every socket open for the life of the test.
    async fn stalled_clickhouse() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        (url, handle)
    }

    fn short_client() -> reqwest::Client {
        client_with(BOUND, BOUND)
    }

    fn health_event() -> HealthEvent {
        HealthEvent {
            ts: chrono::Utc::now(),
            target_id: "t".into(),
            provider: "p".into(),
            org_id: String::new(),
            source: HealthSource::Probe,
            outcome: HealthOutcome::Ok,
            status_code: Some(200),
            latency_ms: 1,
            error_kind: None,
        }
    }

    fn mcp_event() -> McpEvent {
        McpEvent {
            ts: chrono::Utc::now(),
            event_id: "e".into(),
            server: "s".into(),
            tool: "t".into(),
            transport: "http".into(),
            status: "ok",
            latency_ms: 1,
            org_id: String::new(),
            team_id: String::new(),
            project_id: String::new(),
            virtual_key_id: String::new(),
            user_id: String::new(),
            request_id: String::new(),
            trace_id: String::new(),
            arguments: String::new(),
            result: String::new(),
            error: String::new(),
        }
    }

    /// Wait until `counter` reaches `want`, failing cleanly past the ceiling.
    async fn wait_for(counter: impl Fn() -> u64, want: u64) -> Duration {
        let start = Instant::now();
        while counter() < want {
            assert!(start.elapsed() < CEILING, "flush never timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        start.elapsed()
    }

    #[tokio::test]
    async fn every_writer_counts_a_stalled_flush_as_dropped_and_drains() {
        let (url, _server) = stalled_clickhouse().await;
        let metrics = Arc::new(Metrics::default());
        let flush = Duration::from_millis(20);

        let log = LogSink::spawn_with_client(
            url.clone(),
            10,
            flush,
            100,
            metrics.clone(),
            short_client(),
        );
        let health = HealthEventSink::spawn_with_client(
            url.clone(),
            10,
            flush,
            100,
            metrics.clone(),
            short_client(),
        );
        let mcp = McpEventSink::spawn_with_client(
            url.clone(),
            10,
            flush,
            100,
            metrics.clone(),
            short_client(),
        );

        log.log(RequestLog {
            request_id: "r".into(),
            ..Default::default()
        });
        health.emit(health_event());
        mcp.emit(mcp_event());

        // each writer gives up on its batch inside the bound rather than hanging
        let m = metrics.clone();
        let took = wait_for(|| m.logs_dropped_total.load(Relaxed), 1).await;
        assert!(took < Duration::from_secs(3), "took {took:?}");
        let m = metrics.clone();
        wait_for(|| m.health_events_dropped_total.load(Relaxed), 1).await;
        let m = metrics.clone();
        wait_for(|| m.mcp_events_dropped_total.load(Relaxed), 1).await;
        assert_eq!(metrics.logs_written_total.load(Relaxed), 0);

        // with the writers free again, a drain finishes well inside its grace
        let mut state = AppState::new(&GatewayConfig::default());
        state.log = log;
        state.health_events = health;
        state.mcp_events = mcp;
        let start = Instant::now();
        let drained = state.drain_sinks(Duration::from_secs(5)).await;
        assert!(drained, "drain_sinks ran out of grace");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn drain_mid_stalled_flush_is_bounded_by_the_request_timeout() {
        let (url, _server) = stalled_clickhouse().await;
        let metrics = Arc::new(Metrics::default());
        // a long flush interval, so the row is only sent by the shutdown flush
        let sink = McpEventSink::spawn_with_client(
            url,
            10,
            Duration::from_secs(3600),
            100,
            metrics.clone(),
            short_client(),
        );
        sink.emit(mcp_event());
        let start = Instant::now();
        tokio::time::timeout(CEILING, sink.shutdown())
            .await
            .expect("shutdown hung on a stalled clickhouse");
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(metrics.mcp_events_dropped_total.load(Relaxed), 1);
    }
}
