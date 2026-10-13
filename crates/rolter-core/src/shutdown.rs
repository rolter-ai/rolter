//! The process shutdown signal both planes drain on (#2378).

/// Resolve once the process receives a shutdown signal (Ctrl-C on all
/// platforms, or `SIGTERM` on Unix, which is what orchestrators send on a
/// rollout or scale-down).
///
/// Every caller installs its own listener; tokio broadcasts a signal to all of
/// them, so the gateway's and the control plane's handlers coexist when both
/// share a process under `rolter easy-up`.
///
/// A `SIGTERM` handler that cannot be installed leaves only Ctrl-C to wait on.
/// Resolving instead would start a drain nobody asked for.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => {
                tracing::warn!(%err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received ctrl-c, draining"),
        _ = terminate => tracing::info!("received SIGTERM, draining"),
    }
}
