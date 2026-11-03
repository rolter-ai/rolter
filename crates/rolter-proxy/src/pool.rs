//! Connection-pool observability for the upstream clients (#834).
//!
//! Pool exhaustion presents as latency with perfectly healthy providers — the
//! one failure mode the existing metrics cannot explain, because every upstream
//! reports fast and the request is still slow. The time is spent waiting for,
//! or rebuilding, a connection.
//!
//! ## Why this is instrumented rather than read
//!
//! `reqwest` and `hyper` expose no pool introspection whatsoever: there is no
//! accessor for idle count, active count, or waiters. What they *do* expose,
//! since reqwest 0.12.9, is [`ClientBuilder::connector_layer`], a tower layer
//! wrapping the connector. Every call to that layer is hyper deciding it has no
//! reusable connection and building a new one, which is exactly the event worth
//! counting.
//!
//! So this measures connection *establishment*, not pool occupancy:
//!
//! - a healthy pool serves a high request rate with a near-flat connection
//!   count, because connections are reused
//! - a pool that is churning — idle timeout too low, `pool_max_idle_per_host`
//!   too small for the concurrency, or a provider hanging up between requests —
//!   shows a connection rate that tracks the request rate
//!
//! That ratio, `rate(connections_total) / rate(requests_total)`, is the signal.
//! At 0 the pool is doing its job; approaching 1 every request is paying a fresh
//! TCP and TLS handshake, and on a cross-region provider that is most of the
//! latency.
//!
//! Idle-vs-active counts remain genuinely unavailable. Getting them would mean
//! wrapping the connection object itself to observe its drop, and the connector
//! layer's response type is `reqwest`'s opaque `Conn` — it cannot be wrapped
//! from outside the crate. Counting establishment is what the API permits, and
//! it answers the question the issue actually posed.
//!
//! [`ClientBuilder::connector_layer`]: reqwest::ClientBuilder::connector_layer

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::task::{Context, Poll};

use tower::{Layer, Service};

/// Connections successfully established to upstream providers.
static CONNECTIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Connection attempts that failed before a connection existed.
static CONNECT_ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Connections established since start, across every pooled client.
#[must_use]
pub fn connections_total() -> u64 {
    CONNECTIONS_TOTAL.load(Relaxed)
}

/// Failed connection attempts since start.
///
/// Distinct from an upstream *request* error: this counts failures to reach the
/// provider at all — DNS, TCP, TLS or an egress-policy rejection — which is a
/// different fix from a provider returning 500.
#[must_use]
pub fn connect_errors_total() -> u64 {
    CONNECT_ERRORS_TOTAL.load(Relaxed)
}

/// Layer that counts every connection the pool has to build.
#[derive(Clone, Copy, Debug, Default)]
pub struct CountingConnectorLayer;

impl<S> Layer<S> for CountingConnectorLayer {
    type Service = CountingConnector<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CountingConnector { inner }
    }
}

/// The connector wrapper produced by [`CountingConnectorLayer`].
#[derive(Clone, Copy, Debug)]
pub struct CountingConnector<S> {
    inner: S,
}

impl<S, Request> Service<Request> for CountingConnector<S>
where
    S: Service<Request>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    // boxed because the counting happens after the inner future resolves, and
    // the inner future type is unnameable from here
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let inner = self.inner.call(request);
        Box::pin(async move {
            // counted on completion rather than on call, so a connection that
            // was attempted and failed is never reported as established
            match inner.await {
                Ok(connection) => {
                    CONNECTIONS_TOTAL.fetch_add(1, Relaxed);
                    Ok(connection)
                }
                Err(error) => {
                    CONNECT_ERRORS_TOTAL.fetch_add(1, Relaxed);
                    Err(error)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counters are process-global, so two tests reading a before/after
    /// delta concurrently would see each other's increments. Serialising them
    /// is the whole fix; the counters themselves are correct under concurrency,
    /// which is the point of using atomics.
    static SERIAL: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn serially() -> parking_lot::MutexGuard<'static, ()> {
        SERIAL.lock()
    }

    /// A stand-in connector, so the counting is asserted without opening a
    /// socket: the layer's contract is "increment exactly once per resolved
    /// call, on the arm matching the outcome".
    #[derive(Clone)]
    struct Stub(bool);

    impl Service<()> for Stub {
        type Response = ();
        type Error = &'static str;
        type Future = Pin<Box<dyn Future<Output = Result<(), &'static str>> + Send>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: ()) -> Self::Future {
            let ok = self.0;
            Box::pin(async move {
                if ok {
                    Ok(())
                } else {
                    Err("refused")
                }
            })
        }
    }

    /// Held across a `block_on` rather than an `await`: the guard has to span
    /// the whole read-act-read sequence, and holding a std mutex across an
    /// await point would be wrong in any code that mattered.
    fn on_a_runtime<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime")
            .block_on(future)
    }

    #[test]
    fn a_successful_connect_is_counted_once() {
        let _serial = serially();
        let before = connections_total();
        on_a_runtime(async {
            let mut service = CountingConnectorLayer.layer(Stub(true));
            service.call(()).await.expect("stub connects");
        });
        assert_eq!(connections_total(), before + 1);
    }

    /// A failed attempt must not inflate the establishment count — that would
    /// make a provider rolter cannot reach look like a healthy busy one.
    #[test]
    fn a_failed_connect_counts_as_an_error_not_a_connection() {
        let _serial = serially();
        let connections = connections_total();
        let errors = connect_errors_total();
        on_a_runtime(async {
            let mut service = CountingConnectorLayer.layer(Stub(false));
            service.call(()).await.expect_err("stub refuses");
        });
        assert_eq!(connections_total(), connections);
        assert_eq!(connect_errors_total(), errors + 1);
    }
}
