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

/// How long an idle pooled connection to ClickHouse may be reused.
///
/// ClickHouse closes an idle keep-alive connection after its own
/// `keep_alive_timeout` (10 seconds by default, 3 on older releases), and
/// reqwest's default of 90 seconds keeps handing that connection out
/// regardless. A flush written onto a socket the server is closing at that
/// instant gets no response (hyper reports an incomplete message), so the batch
/// is dropped and counted although ClickHouse would have accepted it (#2696,
/// the same race as #1940 on the control plane). Retiring connections well
/// inside the server's window means a reused connection is one the server still
/// holds open.
pub(crate) const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(2);

// below the shortest keep_alive_timeout ClickHouse has shipped as a default
const _: () = assert!(POOL_IDLE_TIMEOUT.as_secs() < 3);

/// The client every ClickHouse writer uses, with the default bounds.
pub(crate) fn client() -> reqwest::Client {
    client_with(CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}

/// A client with explicit bounds; tests use short ones to stall quickly. Falls
/// back to an unbounded client only if the builder itself fails, which cannot
/// happen for a bare timeout configuration.
pub(crate) fn client_with(connect: Duration, request: Duration) -> reqwest::Client {
    client_with_pool_idle(connect, request, POOL_IDLE_TIMEOUT)
}

/// [`client_with`] with an explicit idle-connection bound, so a test can cross
/// it without waiting out the production value.
fn client_with_pool_idle(
    connect: Duration,
    request: Duration,
    pool_idle: Duration,
) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .timeout(request)
        .pool_idle_timeout(pool_idle)
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

    /// A server that answers the first request on each connection, keeps the
    /// connection alive, and drops it unanswered when a second request arrives
    /// on it: what a client sees when it reuses a connection at the instant
    /// ClickHouse's `keep_alive_timeout` closes it.
    struct ClosesReusedConnections {
        url: String,
        connections: Arc<std::sync::atomic::AtomicUsize>,
        _task: tokio::task::JoinHandle<()>,
    }

    /// Read one HTTP/1.1 request (head plus a `content-length` body); `false`
    /// when the peer closed first.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> bool {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        while buf.len() < head_end + length {
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        true
    }

    impl ClosesReusedConnections {
        async fn start() -> Self {
            use tokio::io::AsyncWriteExt;
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let port = listener.local_addr().expect("addr").port();
            let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = connections.clone();
            let task = tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    counted.fetch_add(1, Relaxed);
                    tokio::spawn(async move {
                        if !read_request(&mut socket).await {
                            return;
                        }
                        let reply = b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: keep-alive\r\n\r\n";
                        if socket.write_all(reply).await.is_err() {
                            return;
                        }
                        // the reused connection: closed with the request unanswered
                        read_request(&mut socket).await;
                    });
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}"),
                connections,
                _task: task,
            }
        }
    }

    /// A connection idle past the pool bound is retired rather than reused, so
    /// a flush after a pause never lands on a socket ClickHouse is closing
    /// (#2696). The fake server drops any second request on a connection:
    /// reuse fails the post, a fresh connection succeeds.
    #[tokio::test]
    async fn an_idle_connection_is_retired_before_the_server_closes_it() {
        let server = ClosesReusedConnections::start().await;
        let pool_idle = Duration::from_millis(200);
        let http = client_with_pool_idle(Duration::from_secs(1), Duration::from_secs(5), pool_idle);
        for round in 0..2 {
            let sent = tokio::time::timeout(CEILING, http.post(&server.url).body("x").send())
                .await
                .expect("bounded");
            assert!(sent.is_ok(), "post {round} must be answered: {sent:?}");
            // wall-clock, not paused tokio time: the pool stamps idle
            // connections with std's clock
            tokio::time::sleep(pool_idle * 3).await;
        }
        assert_eq!(server.connections.load(Relaxed), 2);
    }

    #[test]
    fn production_client_retires_idle_connections_inside_the_server_window() {
        assert!(POOL_IDLE_TIMEOUT < Duration::from_secs(3));
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
