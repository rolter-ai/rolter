//! Connect-time egress enforcement for the gateway's own outbound clients.
//!
//! The upstream forwarder installs the egress resolver in `rolter-proxy`; the
//! clients here reach URLs an operator wrote that are not a provider's
//! `api_base`: the guardrail webhook, the PII sanitizer, each plugin endpoint,
//! a provider's status page and its lmcache endpoint. Config validation
//! classifies IP literals only, so a hostname that resolves to cloud instance
//! metadata, or rebinds to it after it was saved, is stopped here, on what DNS
//! returned, immediately before connecting.
//!
//! None of these follows a redirect: a `3xx` is how an endpoint that passed the
//! check hands the request to one that would not have.

use std::sync::Arc;
use std::time::Duration;

use rolter_proxy::egress_resolver::{EgressResolver, SharedEgressPolicy};

/// A pooled client plus the live policy it answers to.
///
/// Two checks, because neither covers the other: the resolver sees what DNS
/// returned for a hostname, but reqwest never resolves an IP literal, so
/// `http://169.254.169.254/` reaches the connector untouched and only
/// [`EgressPolicy::check_url`](rolter_core::EgressPolicy::check_url) catches it.
/// Every request therefore passes [`Self::post`] or [`Self::get`], which run the
/// literal check against the policy as it is now (a hot reload that tightens it
/// applies to the next request).
#[derive(Clone)]
pub struct EgressClient {
    client: reqwest::Client,
    policy: SharedEgressPolicy,
}

impl EgressClient {
    pub(crate) fn new(policy: SharedEgressPolicy) -> Self {
        let client = builder(&policy).build().unwrap_or_else(|error| {
            // building only fails on a broken TLS backend, and falling back to
            // a plain client would silently drop the policy, so every request
            // fails closed instead
            tracing::error!(%error, "egress-checked client could not be built");
            reqwest::Client::builder()
                .dns_resolver(Arc::new(DenyAll))
                .build()
                .unwrap_or_default()
        });
        Self { client, policy }
    }

    /// Begin a POST, or `None` when the policy denies `url`.
    pub(crate) fn post(&self, url: &str) -> Option<reqwest::RequestBuilder> {
        self.permits(url).then(|| self.client.post(url))
    }

    /// Begin a GET, or `None` when the policy denies `url`.
    pub(crate) fn get(&self, url: &str) -> Option<reqwest::RequestBuilder> {
        self.permits(url).then(|| self.client.get(url))
    }

    /// Whether the live policy permits `url`. Logs the reason, never the url,
    /// whose query string can carry a token.
    pub(crate) fn permits(&self, url: &str) -> bool {
        match self.policy.load().url_deny_reason(url) {
            Some(reason) => {
                tracing::warn!(reason, "outbound call refused by the egress policy");
                false
            }
            None => true,
        }
    }

    pub(crate) fn policy(&self) -> &SharedEgressPolicy {
        &self.policy
    }
}

fn builder(egress: &SharedEgressPolicy) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(EgressResolver::new(egress.clone())))
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(Duration::from_secs(90))
}

/// Resolves nothing, so a client that could not be built with the policy
/// fails every request closed.
struct DenyAll;

impl reqwest::dns::Resolve for DenyAll {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async {
            Err::<reqwest::dns::Addrs, _>(Box::<dyn std::error::Error + Send + Sync>::from(
                "egress policy client unavailable",
            ))
        })
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use rolter_core::EgressPolicy;
    use rolter_proxy::egress_resolver::SharedEgressPolicy;

    /// A listener that counts accepted connections, so a test can prove a
    /// request was refused before any connection was made.
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
    pub(crate) fn deny_loopback() -> SharedEgressPolicy {
        Arc::new(ArcSwap::from_pointee(EgressPolicy {
            block_loopback: true,
            ..EgressPolicy::default()
        }))
    }

    pub(crate) fn default_policy() -> SharedEgressPolicy {
        Arc::new(ArcSwap::from_pointee(EgressPolicy::default()))
    }

    /// A client over the default policy, for tests that reach a local fake.
    pub(crate) fn permissive() -> super::EgressClient {
        super::EgressClient::new(default_policy())
    }
}
