//! Per-route admission (#2936): the caps a route sets in `advanced.limits`.
//!
//! `rpm` and `tpm` are counted in Redis by [`crate::rate_limits`], on a key of
//! the route's own, so they hold across every gateway replica. `concurrency` is
//! counted here, in process: a permit is taken before the upstream call and
//! released when the response has been delivered, the streamed body included.
//! The count is per replica. A fleet of three replicas with `concurrency = 4`
//! serves up to twelve at once, which is the price of not putting a Redis
//! round trip and a leaked-slot repair problem in front of every call. It works
//! with no Redis at all, where `rpm` and `tpm` do not.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

/// `(org, route)`: route names are unique per org, not globally.
type RouteKey = (Box<str>, Box<str>);

/// In-flight requests per route on this gateway. Cheap to clone.
#[derive(Clone, Default)]
pub struct RouteConcurrency {
    inner: Arc<Mutex<HashMap<RouteKey, u32>>>,
}

impl RouteConcurrency {
    /// Take a slot of `route`'s `cap`, or `None` when all of them are in use.
    ///
    /// The map holds an entry only while a request is in flight, so a route
    /// that is never called, or was deleted by a reload, leaves nothing behind.
    /// Only a route that sets `concurrency` ever gets here.
    pub fn try_acquire(&self, org: &str, route: &str, cap: u32) -> Option<ConcurrencyPermit> {
        let key: RouteKey = (org.into(), route.into());
        {
            let mut map = self.inner.lock();
            let held = map.entry(key.clone()).or_insert(0);
            if *held >= cap {
                return None;
            }
            *held += 1;
        }
        Some(ConcurrencyPermit {
            inner: self.inner.clone(),
            key,
        })
    }

    /// Slots currently taken on `route` of `org`.
    #[cfg(test)]
    fn in_flight(&self, org: &str, route: &str) -> u32 {
        let key: RouteKey = (org.into(), route.into());
        self.inner.lock().get(&key).copied().unwrap_or(0)
    }
}

/// A slot of a route's `concurrency` cap, released on drop. Held across the
/// whole request, so a client that disconnects mid-stream frees it when the
/// response body is dropped.
pub struct ConcurrencyPermit {
    inner: Arc<Mutex<HashMap<RouteKey, u32>>>,
    key: RouteKey,
}

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        let mut map = self.inner.lock();
        if let Some(held) = map.get_mut(&self.key) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                map.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_admits_up_to_its_cap_and_refuses_the_next() {
        let gate = RouteConcurrency::default();
        let first = gate.try_acquire("org", "gpt", 2).expect("first slot");
        let _second = gate.try_acquire("org", "gpt", 2).expect("second slot");
        assert!(gate.try_acquire("org", "gpt", 2).is_none());
        assert_eq!(gate.in_flight("org", "gpt"), 2);

        drop(first);
        assert!(gate.try_acquire("org", "gpt", 2).is_some());
    }

    #[test]
    fn a_refusal_takes_no_slot() {
        let gate = RouteConcurrency::default();
        let _held = gate.try_acquire("org", "gpt", 1).unwrap();
        for _ in 0..5 {
            assert!(gate.try_acquire("org", "gpt", 1).is_none());
        }
        assert_eq!(gate.in_flight("org", "gpt"), 1);
    }

    #[test]
    fn the_same_route_name_in_two_orgs_is_two_routes() {
        let gate = RouteConcurrency::default();
        let _a = gate.try_acquire("org-a", "gpt", 1).unwrap();
        assert!(gate.try_acquire("org-b", "gpt", 1).is_some());
        assert!(gate.try_acquire("", "gpt", 1).is_some());
    }

    #[test]
    fn dropping_the_last_permit_leaves_no_entry() {
        let gate = RouteConcurrency::default();
        drop(gate.try_acquire("org", "gpt", 3).unwrap());
        assert!(gate.inner.lock().is_empty());
    }

    #[test]
    fn a_lowered_cap_applies_to_the_next_request_without_evicting() {
        let gate = RouteConcurrency::default();
        let _a = gate.try_acquire("org", "gpt", 5).unwrap();
        let _b = gate.try_acquire("org", "gpt", 5).unwrap();
        // a reload lowered the cap below what is in flight: running requests
        // finish, new ones wait for the count to fall under the new cap
        assert!(gate.try_acquire("org", "gpt", 1).is_none());
        assert_eq!(gate.in_flight("org", "gpt"), 2);
    }
}
