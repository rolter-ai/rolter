//! Connect-time egress enforcement.
//!
//! [`rolter_core::EgressPolicy`] classifies IP literals at config time, which
//! catches `http://169.254.169.254/…` but not two things that only DNS knows:
//!
//! 1. **hostnames** — `http://metadata.internal/…` has nothing to classify
//!    until it resolves, and resolving during config validation would make
//!    validation depend on live DNS (the "one bad row freezes every gateway"
//!    failure mode) and break air-gapped deployments where DNS may not answer;
//! 2. **DNS rebinding** — a name that resolved to a public address when the
//!    provider was written can resolve to link-local at request time.
//!
//! Both are the same check at a different moment: classify what DNS actually
//! returned, immediately before connecting. This resolver does that, and is
//! installed on every upstream client.

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rolter_core::EgressPolicy;

/// Live egress policy shared by every upstream client. Held behind an
/// [`ArcSwap`] so a config hot-reload re-tunes enforcement in place — the
/// clients are pooled and long-lived, so rebuilding them per policy change
/// would throw away every warm connection.
pub type SharedEgressPolicy = Arc<ArcSwap<EgressPolicy>>;

/// A [`Resolve`] that drops addresses the active [`EgressPolicy`] denies.
pub struct EgressResolver {
    policy: SharedEgressPolicy,
}

impl EgressResolver {
    pub fn new(policy: SharedEgressPolicy) -> Self {
        Self { policy }
    }
}

/// Resolve `name` through the system resolver, then drop denied addresses.
///
/// Kept separate from the trait impl so tests can drive it with a stub
/// resolution instead of live DNS.
fn filter_addrs(
    policy: &EgressPolicy,
    host: &str,
    addrs: Vec<SocketAddr>,
) -> std::result::Result<Vec<SocketAddr>, String> {
    if policy.host_is_allowed(host) {
        return Ok(addrs);
    }
    let mut denied: Option<&'static str> = None;
    let allowed: Vec<SocketAddr> = addrs
        .into_iter()
        .filter(|addr| match policy.deny_reason(addr.ip()) {
            Some(reason) => {
                denied = Some(reason);
                false
            }
            None => true,
        })
        .collect();
    // a partial denial still connects: a multi-homed upstream with one denied
    // address is reachable on the others, and refusing the whole name would
    // take down a legitimate provider. only a name with *nothing* left is an
    // error, and rebinding to a denied address leaves exactly nothing
    if allowed.is_empty() {
        if let Some(reason) = denied {
            return Err(format!(
                "'{host}' resolves only to {reason} addresses, which the egress policy denies \
                 (allow it explicitly via egress.allow_hosts if this is intentional)"
            ));
        }
    }
    Ok(allowed)
}

impl Resolve for EgressResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.load_full();
        Box::pin(async move {
            let host = name.as_str().to_string();
            // port 0: reqwest overwrites it with the request's port. this is
            // the same lookup the default resolver performs
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> { Box::new(err) })?
                .collect();
            let allowed = filter_addrs(&policy, &host, addrs).map_err(|problem| {
                // a denied destination is a policy decision, not a network
                // failure — say so, or it surfaces as an opaque connect error
                tracing::warn!(%host, "egress policy denied an upstream address at connect time");
                Box::<dyn std::error::Error + Send + Sync>::from(problem)
            })?;
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), 0)
    }

    #[test]
    fn a_hostname_resolving_to_link_local_is_refused() {
        // the gap #634 left open: nothing about `metadata.internal` is
        // classifiable until DNS answers
        let policy = EgressPolicy::default();
        let err =
            filter_addrs(&policy, "metadata.internal", vec![addr("169.254.169.254")]).unwrap_err();
        assert!(err.contains("metadata.internal"), "{err}");
        assert!(err.contains("link-local"), "{err}");
    }

    #[test]
    fn rebinding_to_a_denied_address_is_refused_at_connect_time() {
        // same name, same config, different answer: only this check sees it
        let policy = EgressPolicy::default();
        assert!(filter_addrs(&policy, "upstream.example", vec![addr("93.184.216.34")]).is_ok());
        assert!(filter_addrs(&policy, "upstream.example", vec![addr("169.254.169.254")]).is_err());
    }

    #[test]
    fn ipv6_link_local_is_classified_too() {
        let policy = EgressPolicy::default();
        assert!(filter_addrs(&policy, "metadata.internal", vec![addr("fe80::1")]).is_err());
    }

    #[test]
    fn private_and_loopback_upstreams_stay_reachable_by_default() {
        // the air-gapped case: on-prem and sidecar upstreams are the norm, so
        // the default policy must not break them
        let policy = EgressPolicy::default();
        for ip in ["10.0.0.5", "192.168.1.10", "172.16.0.2", "127.0.0.1", "::1"] {
            assert_eq!(
                filter_addrs(&policy, "upstream.internal", vec![addr(ip)]).unwrap(),
                vec![addr(ip)],
                "{ip} should be reachable under the default policy"
            );
        }
    }

    #[test]
    fn opting_into_blocking_private_denies_it() {
        let policy = EgressPolicy {
            block_private: true,
            ..EgressPolicy::default()
        };
        assert!(filter_addrs(&policy, "upstream.internal", vec![addr("10.0.0.5")]).is_err());
    }

    #[test]
    fn allow_hosts_exempts_a_name_entirely() {
        let policy = EgressPolicy {
            allow_hosts: vec!["metadata.internal".to_string()],
            ..EgressPolicy::default()
        };
        let addrs = vec![addr("169.254.169.254")];
        assert_eq!(
            filter_addrs(&policy, "metadata.internal", addrs.clone()).unwrap(),
            addrs
        );
    }

    #[test]
    fn a_multi_homed_name_keeps_its_allowed_addresses() {
        // refusing the whole name would take down a legitimate upstream that
        // happens to also answer with a denied address
        let policy = EgressPolicy::default();
        let resolved = vec![addr("169.254.169.254"), addr("93.184.216.34")];
        assert_eq!(
            filter_addrs(&policy, "upstream.example", resolved).unwrap(),
            vec![addr("93.184.216.34")]
        );
    }

    #[test]
    fn an_empty_resolution_is_not_reported_as_a_policy_denial() {
        // nothing was denied; NXDOMAIN-shaped results belong to the resolver
        let policy = EgressPolicy::default();
        assert!(filter_addrs(&policy, "nowhere.example", vec![])
            .unwrap()
            .is_empty());
    }
}
