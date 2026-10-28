//! A realtime session feeds the circuit breaker like an HTTP request (#3248):
//! ordering the dial list takes no half-open probe slot, and a good handshake
//! closes a probing breaker.

use std::net::SocketAddr;

use axum::Router;
use rolter_core::{GatewayConfig, ProviderConfig, ProviderKind};
use serde_json::json;

const MODEL: &str = "gpt-realtime";

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A websocket upstream that accepts every session and holds it open.
async fn realtime_upstream() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                if let Ok(socket) = tokio_tungstenite::accept_async(stream).await {
                    // hold the socket until the gateway closes it
                    let _socket = socket;
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });
        }
    });
    addr
}

/// A route over `upstreams`, one target each, with the breaker tripping on the
/// first failure and probing at once.
fn config(upstreams: &[SocketAddr]) -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.breaker.enabled = true;
    config.breaker.failure_threshold = 1;
    config.breaker.open_secs = 0;
    let mut targets = Vec::new();
    for (i, upstream) in upstreams.iter().enumerate() {
        let name = format!("up-{i}");
        config.providers.push(ProviderConfig {
            name: name.clone(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{upstream}"),
            ..Default::default()
        });
        targets.push(json!({"provider": name, "model": "gpt-realtime-upstream", "weight": 1}));
    }
    config.routes.push(
        serde_json::from_value(json!({
            "model": MODEL, "strategy": "round_robin", "targets": targets
        }))
        .unwrap(),
    );
    config
}

async fn open_session(gw: SocketAddr) {
    let (mut client, _) =
        tokio_tungstenite::connect_async(format!("ws://{gw}/v1/realtime?model={MODEL}"))
            .await
            .expect("the session is admitted");
    let _ = client.close(None).await;
}

#[tokio::test]
async fn a_half_open_realtime_target_closes_after_one_good_connect() {
    let upstream = realtime_upstream().await;
    let config = config(&[upstream]);
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state.clone(),
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    assert!(state.breaker.on_failure(MODEL, 0), "tripped open");

    open_session(gw).await;

    assert!(
        state.breaker.is_empty(),
        "the good handshake closed the breaker: {:?}",
        state.breaker.snapshot()
    );
}

/// The dial list is ordered over every target; a probing target that is not
/// the one dialled must not be left holding its probe slot.
#[tokio::test]
async fn ordering_the_dial_list_takes_no_probe_slot() {
    let config = config(&[realtime_upstream().await, realtime_upstream().await]);
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state.clone(),
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;
    assert!(state.breaker.on_failure(MODEL, 0), "target 0 tripped open");

    open_session(gw).await;

    // either target 0 was dialled and closed, or it was not dialled and its
    // probe is still there to take: never refusing for a probe nobody sent
    assert!(
        !state.breaker.refuses(MODEL, 0),
        "{:?}",
        state.breaker.snapshot()
    );
}
