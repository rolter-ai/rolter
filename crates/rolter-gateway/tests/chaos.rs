//! Chaos / resilience contracts that the black-box e2e harness cannot drive
//! deterministically (#639, deferred from #620).
//!
//! Both scenarios need a request pinned *inside* the gateway at a known point,
//! which the compose-based chaos suite can only approximate with sleeps. Here a
//! mock upstream blocks on a semaphore the test owns, so every assertion is
//! driven by a signal rather than by elapsed time:
//!
//! - **bounded-queue backpressure** — with `capacity = 1, workers = 1` and
//!   `backpressure = "error"`, one pinned request occupies the worker, exactly
//!   one surplus request takes the queue slot, and the rest must come back as a
//!   `queue_full` envelope (HTTP 429) rather than queueing without bound
//! - **graceful SIGTERM drain** — a real `rolter-gateway` child process is sent
//!   `SIGTERM` while a request is pinned upstream; the in-flight request must
//!   still return 200, new connections must be refused, and the process must
//!   exit 0

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rolter_core::{
    BackpressurePolicy, BalancingStrategy, GatewayConfig, ModelRoute, ProviderConfig, ProviderKind,
    QueueConfig, Target,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Semaphore};

/// A mock upstream that announces each admitted request and then blocks until
/// the test hands out a semaphore permit. Nothing here is time-based: a request
/// stays pinned upstream for exactly as long as the test wants it pinned.
#[derive(Clone)]
struct Hold {
    arrived: mpsc::UnboundedSender<()>,
    release: Arc<Semaphore>,
}

async fn hold_handler(State(hold): State<Hold>, Json(_body): Json<Value>) -> impl IntoResponse {
    let _ = hold.arrived.send(());
    // the permit is returned on drop, so a single `add_permits(1)` drains every
    // waiter in turn rather than only the first one
    let _ = hold.release.acquire().await;
    Json(json!({
        "id": "chatcmpl-hold",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "pong"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    }))
    .into_response()
}

/// Serve the holding upstream on an ephemeral port, returning its address, the
/// stream of admission notifications and the release handle.
async fn serve_holding_upstream() -> (SocketAddr, mpsc::UnboundedReceiver<()>, Arc<Semaphore>) {
    let (arrived, arrivals) = mpsc::unbounded_channel();
    let release = Arc::new(Semaphore::new(0));
    let app = Router::new()
        .route("/v1/chat/completions", post(hold_handler))
        .with_state(Hold {
            arrived,
            release: release.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, arrivals, release)
}

/// POST a minimal chat completion and return `(status, body)`.
async fn chat(client: &reqwest::Client, gw: SocketAddr, model: &str) -> (u16, Value) {
    let response = client
        .post(format!("http://{gw}/v1/chat/completions"))
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "ping"}]}))
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

/// Read a single counter out of the prometheus exposition body.
fn counter(body: &str, name: &str) -> u64 {
    body.lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .unwrap_or_else(|| panic!("metric {name} missing from /metrics"))
        .trim()
        .parse()
        .unwrap()
}

/// A one-route config pointing at the holding upstream, with the provider queue
/// tightened to the smallest bound that still admits work.
fn backpressure_config(model: &str, upstream: SocketAddr) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.providers.push(ProviderConfig {
        name: "hold".to_string(),
        kind: ProviderKind::OpenaiCompatible,
        api_base: format!("http://{upstream}"),
        ..Default::default()
    });
    config.routes.push(ModelRoute {
        model: model.to_string(),
        strategy: BalancingStrategy::RoundRobin,
        targets: vec![Target {
            provider: "hold".to_string(),
            model: None,
            weight: 1,
        }],
        params: Default::default(),
        param_policy: Default::default(),
        advanced: Default::default(),
        cache: None,
        variants: Default::default(),
        tenancy: None,
    });
    config.queue = QueueConfig {
        enabled: true,
        capacity: 1,
        workers: 1,
        backpressure: BackpressurePolicy::Error,
        block_timeout_ms: 0,
    };
    config
}

#[tokio::test]
async fn a_full_provider_queue_sheds_surplus_with_a_queue_full_envelope() {
    let (upstream, mut arrivals, release) = serve_holding_upstream().await;
    let config = backpressure_config("chaos-backpressure", upstream);
    let app = rolter_gateway::build_router_from_config(&config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    // pin the single worker: this request reaches the upstream and stays there
    let pinned = tokio::spawn({
        let client = client.clone();
        async move { chat(&client, gw, "chaos-backpressure").await }
    });
    arrivals.recv().await.unwrap();

    // the worker cannot drain the channel while it is pinned, so of the four
    // surplus requests exactly one takes the single queue slot and the other
    // three must be rejected at admission
    let (results, mut inbox) = mpsc::unbounded_channel();
    for _ in 0..4 {
        let (client, results) = (client.clone(), results.clone());
        tokio::spawn(async move {
            let _ = results.send(chat(&client, gw, "chaos-backpressure").await);
        });
    }

    for _ in 0..3 {
        let (status, body) = inbox.recv().await.unwrap();
        assert_eq!(status, 429, "surplus request should be shed: {body}");
        assert_eq!(body["error"]["code"], "queue_full");
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert_eq!(body["error"]["message"], "provider queue full");
    }

    // releasing lets the pinned request and the one queued behind it finish, so
    // backpressure sheds the surplus without losing admitted work
    release.add_permits(1);
    let (queued_status, queued_body) = inbox.recv().await.unwrap();
    assert_eq!(queued_status, 200, "queued request should complete");
    assert_eq!(queued_body["choices"][0]["message"]["content"], "pong");
    let (pinned_status, pinned_body) = pinned.await.unwrap();
    assert_eq!(pinned_status, 200);
    assert_eq!(pinned_body["choices"][0]["message"]["content"], "pong");

    // asserted as a real counter transition, not just a status code
    let metrics = reqwest::get(format!("http://{gw}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        counter(&metrics, "rolter_provider_queue_rejections_total"),
        3
    );
    assert_eq!(counter(&metrics, "rolter_provider_queue_timeouts_total"), 0);
}

#[cfg(unix)]
mod drain {
    use super::*;
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    /// Claim an ephemeral port and release it, so the child can bind it. The
    /// gateway needs an address the test can reach before it starts.
    async fn reserve_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    /// Poll `/healthz` until the child is serving. Bounded by attempts rather
    /// than a fixed sleep, so a slow machine waits longer instead of flaking.
    async fn wait_until_serving(gw: SocketAddr, child: &mut Child) {
        for _ in 0..600 {
            if let Ok(Some(status)) = child.try_wait() {
                panic!("gateway exited before serving: {status}");
            }
            if reqwest::get(format!("http://{gw}/healthz")).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("gateway never became reachable on {gw}");
    }

    /// Poll until the listener is gone. After `SIGTERM` axum stops accepting,
    /// so a fresh connection must be refused rather than served.
    async fn wait_until_refusing(gw: SocketAddr) {
        for _ in 0..600 {
            if tokio::net::TcpStream::connect(gw).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("gateway kept accepting new connections after SIGTERM");
    }

    #[tokio::test]
    async fn sigterm_drains_in_flight_requests_and_exits_cleanly() {
        let (upstream, mut arrivals, release) = serve_holding_upstream().await;
        let port = reserve_port().await;
        let gw: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

        let dir = std::env::temp_dir().join(format!("rolter-chaos-drain-{port}"));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("rolter.toml");
        let mut file = std::fs::File::create(&config_path).unwrap();
        write!(
            file,
            r#"
[server]
host = "127.0.0.1"
port = {port}

[[providers]]
name = "hold"
kind = "openai_compatible"
api_base = "http://{upstream}"

[[routes]]
model = "chaos-drain"
strategy = "round_robin"
[[routes.targets]]
provider = "hold"
"#
        )
        .unwrap();
        drop(file);

        let mut child = Command::new(env!("CARGO_BIN_EXE_rolter-gateway"))
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_until_serving(gw, &mut child).await;

        // pin one request inside the child: it has reached the upstream and is
        // waiting there when the signal lands
        let inflight = tokio::spawn(async move {
            let client = reqwest::Client::new();
            chat(&client, gw, "chaos-drain").await
        });
        arrivals.recv().await.unwrap();

        // sigterm is what orchestrators send on rollout / scale-down
        let pid = child.id() as libc::pid_t;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);

        // new work is turned away while the in-flight request is still pinned
        wait_until_refusing(gw).await;
        assert!(
            child.try_wait().unwrap().is_none(),
            "gateway exited while a request was still in flight"
        );

        // and the drained request still gets its answer
        release.add_permits(1);
        let (status, body) = inflight.await.unwrap();
        assert_eq!(status, 200, "in-flight request was dropped by the drain");
        assert_eq!(body["choices"][0]["message"]["content"], "pong");

        let status = tokio::task::spawn_blocking(move || child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.code(), Some(0), "gateway did not exit cleanly");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
