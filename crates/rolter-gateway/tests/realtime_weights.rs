//! A realtime route draws its primary variant once per session (#3553).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

async fn realtime_upstream() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let sessions = Arc::new(AtomicUsize::new(0));
    let accepted = Arc::clone(&sessions);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let accepted = Arc::clone(&accepted);
            tokio::spawn(async move {
                if let Ok(socket) = tokio_tungstenite::accept_async(stream).await {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let _socket = socket;
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });
        }
    });
    (addr, sessions)
}

#[tokio::test]
async fn a_fifty_fifty_route_leads_with_both_variants_across_sessions() {
    let (stable_addr, stable_sessions) = realtime_upstream().await;
    let (canary_addr, canary_sessions) = realtime_upstream().await;
    let mut config = GatewayConfig::default();
    for (name, addr) in [("stable-up", stable_addr), ("canary-up", canary_addr)] {
        config.providers.push(ProviderConfig {
            name: name.into(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: format!("http://{addr}"),
            ..Default::default()
        });
    }
    config.routes.push(
        serde_json::from_value(json!({
            "model": MODEL,
            "strategy": "round_robin",
            "targets": [],
            "variants": [
                {"name": "stable", "weight": 50,
                 "targets": [{"provider": "stable-up", "model": "gpt-realtime-upstream", "weight": 1}]},
                {"name": "canary", "weight": 50,
                 "targets": [{"provider": "canary-up", "model": "gpt-realtime-upstream", "weight": 1}]}
            ]
        }))
        .unwrap(),
    );
    let state = rolter_gateway::AppState::with_logging(&config, None);
    let gw = serve(rolter_gateway::build_router(
        state,
        "/metrics",
        32 * 1024 * 1024,
    ))
    .await;

    for _ in 0..64 {
        let (mut client, _) =
            tokio_tungstenite::connect_async(format!("ws://{gw}/v1/realtime?model={MODEL}"))
                .await
                .expect("the session is admitted");
        client.close(None).await.unwrap();
    }

    // a fair draw puts 32 of 64 on each side with a standard deviation of 4;
    // 16 is four deviations out, so a pass is near-certain for a weighted pick
    // and a draw biased toward the first variant fails it
    let stable = stable_sessions.load(Ordering::SeqCst);
    let canary = canary_sessions.load(Ordering::SeqCst);
    assert!(stable >= 16, "stable led {stable} of 64 sessions");
    assert!(canary >= 16, "canary led {canary} of 64 sessions");
}
