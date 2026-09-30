//! Connect-time egress enforcement for the control plane's outbound clients.
//!
//! [`rolter_core::EgressPolicy::check_url`] classifies IP literals when a URL
//! is saved and again before it is sent, which catches
//! `http://169.254.169.254/…` but not a hostname such as `metadata.internal`,
//! nor a name that rebinds to link-local after it was saved. Only DNS knows
//! what a name means, so the check has to run on what DNS returned,
//! immediately before connecting. This is the control plane's counterpart of
//! the gateway's resolver in `rolter-proxy`, and both defer to
//! [`EgressPolicy::filter_resolved`] so the two planes cannot disagree on
//! which addresses are denied.
//!
//! Every control-plane path that sends to an operator-supplied URL builds its
//! client here: alert delivery, the connector test probe and MCP OAuth. None
//! of them follows redirects, because a `3xx` is how an endpoint that passed
//! the check hands the request to one that would not have.

use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rolter_core::EgressPolicy;

/// A [`Resolve`] that drops addresses the [`EgressPolicy`] denies.
struct EgressResolver {
    policy: Arc<EgressPolicy>,
}

impl Resolve for EgressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            // port 0: reqwest overwrites it with the request's port
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> { Box::new(err) })?
                .collect();
            let allowed = policy.filter_resolved(&host, addrs).map_err(|problem| {
                // a denied destination is a policy decision, not a network
                // failure. the host is logged, never the url, whose query
                // string can carry a token
                tracing::warn!(%host, "egress policy denied an address at connect time");
                Box::<dyn std::error::Error + Send + Sync>::from(problem)
            })?;
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// A client builder whose every connection is classified against `policy` and
/// which never follows a redirect. Callers add their own timeouts.
///
/// Built per use rather than shared: the policy is fixed inside the resolver,
/// and these calls (an alert transition, a connector test, an OAuth exchange)
/// are rare enough that a pooled connection buys nothing.
pub(crate) fn builder(policy: &Arc<EgressPolicy>) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(EgressResolver {
            policy: policy.clone(),
        }))
        .redirect(reqwest::redirect::Policy::none())
}

/// Test support shared by the callers' connect-time tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use rolter_core::EgressPolicy;

    /// A listener that counts the connections it accepts, so a test can prove
    /// a request was refused before any connection was made.
    pub(crate) struct Counter {
        pub(crate) port: u16,
        accepted: Arc<AtomicUsize>,
    }

    impl Counter {
        pub(crate) async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind a local port");
            let port = listener.local_addr().expect("a local address").port();
            let accepted = Arc::new(AtomicUsize::new(0));
            let seen = accepted.clone();
            tokio::spawn(async move {
                // hold accepted sockets open so the client sees a live peer
                let mut held = Vec::new();
                while let Ok((socket, _)) = listener.accept().await {
                    seen.fetch_add(1, Ordering::SeqCst);
                    held.push(socket);
                }
            });
            Self { port, accepted }
        }

        pub(crate) fn accepted(&self) -> usize {
            self.accepted.load(Ordering::SeqCst)
        }

        /// A hostname that resolves only to this loopback listener, so a
        /// policy that denies loopback denies exactly this name.
        pub(crate) fn url(&self, path: &str) -> String {
            format!("http://localhost:{}{path}", self.port)
        }
    }

    /// A policy that denies loopback, and so everything `localhost` resolves to.
    pub(crate) fn deny_loopback() -> Arc<EgressPolicy> {
        Arc::new(EgressPolicy {
            block_loopback: true,
            ..EgressPolicy::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use testing::{deny_loopback, Counter};

    #[tokio::test]
    async fn a_name_resolving_only_to_a_denied_address_is_refused_before_connecting() {
        let listener = Counter::start().await;
        let client = builder(&deny_loopback())
            .timeout(Duration::from_secs(5))
            .build()
            .expect("a client");
        let error = client
            .get(listener.url("/"))
            .send()
            .await
            .expect_err("the connect must be refused");
        assert!(error.is_connect(), "{error:?}");
        assert_eq!(listener.accepted(), 0);
    }

    #[tokio::test]
    async fn the_default_policy_still_reaches_a_loopback_upstream() {
        // the counter is only evidence if it can count: the same request
        // under the default policy must connect
        let listener = Counter::start().await;
        let client = builder(&Arc::default())
            .timeout(Duration::from_millis(300))
            .build()
            .expect("a client");
        let _ = client.get(listener.url("/")).send().await;
        assert_eq!(listener.accepted(), 1);
    }
}
