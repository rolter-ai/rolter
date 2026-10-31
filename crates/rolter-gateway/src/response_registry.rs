use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use rolter_core::ResponsesConfig;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleCapabilities {
    pub retrieve: bool,
    pub delete: bool,
    pub cancel: bool,
    pub input_items: bool,
}

impl LifecycleCapabilities {
    pub const NATIVE_OPENAI: Self = Self {
        retrieve: true,
        delete: true,
        cancel: true,
        input_items: true,
    };
    pub const UNSUPPORTED: Self = Self {
        retrieve: false,
        delete: false,
        cancel: false,
        input_items: false,
    };
}

#[derive(Debug, Clone)]
pub struct ResponseRoute {
    /// the provider that holds the response: its display name, owned by
    /// `provider_org`, or its runtime id when `provider_org` is `None`
    pub provider: String,
    /// the org that owns `provider`, empty for a global one. A response keeps
    /// the provider's stable identity, `(org, name)`, rather than its runtime
    /// id, because the runtime id changes from `openai` to `openai@<org>` as
    /// soon as a second org takes the name (ADR-0040), and a lifecycle call
    /// for a response stored before that must still reach it (#3461). `None`
    /// is an entry that recorded the runtime id, resolved as it always was
    pub provider_org: Option<String>,
    pub target: String,
    /// the model id the caller named when creating the response
    pub model: String,
    /// the route that actually served it: a configured route name, or the
    /// `provider-slug/model` address itself for a pinned request. differs from
    /// `model` when a complexity tier moved the request onto another route, and
    /// is what a lifecycle call re-authorizes against (#1779)
    pub route: String,
    pub provider_native_id: String,
    pub provider_key_fingerprint: Option<String>,
    pub capabilities: LifecycleCapabilities,
    created_at: Instant,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
pub struct RouteTemplate {
    pub tenant: String,
    pub provider: String,
    pub provider_org: Option<String>,
    pub target: String,
    pub model: String,
    pub route: String,
    pub provider_key_fingerprint: Option<String>,
    pub capabilities: LifecycleCapabilities,
    created_at: Instant,
    expires_at: Instant,
}

impl RouteTemplate {
    /// Record the route that served the response, for lifecycle calls to
    /// re-authorize against.
    pub fn with_route(mut self, route: String) -> Self {
        self.route = route;
        self
    }

    /// Record the provider by its stable identity, its display `name` and
    /// owning `org` (empty for a global provider), in place of the runtime id
    /// the template was made with (#3461).
    pub fn with_provider_identity(mut self, name: String, org: String) -> Self {
        self.provider = name;
        self.provider_org = Some(org);
        self
    }
}

impl ResponseRoute {
    /// The runtime id of the provider holding the response in `snap`, the
    /// key of [`Snapshot::providers`](crate::state::Snapshot::providers).
    /// `None` when that provider is gone from the snapshot.
    pub fn provider_id<'a>(&'a self, snap: &'a crate::state::Snapshot) -> Option<&'a str> {
        match &self.provider_org {
            Some(org) => snap.provider_id(org, &self.provider),
            None => snap
                .providers
                .contains_key(&self.provider)
                .then_some(self.provider.as_str()),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RegistryKey {
    tenant: String,
    response_id: String,
}

#[derive(Clone)]
pub struct ResponseRegistry {
    entries: Arc<DashMap<RegistryKey, ResponseRoute>>,
    ttl_secs: Arc<AtomicU64>,
    max_entries: Arc<AtomicUsize>,
}

impl ResponseRegistry {
    pub fn new(config: &ResponsesConfig) -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            ttl_secs: Arc::new(AtomicU64::new(config.registry_ttl_secs)),
            max_entries: Arc::new(AtomicUsize::new(config.registry_max_entries)),
        }
    }

    pub fn reconfigure(&self, config: &ResponsesConfig) {
        self.ttl_secs.store(config.registry_ttl_secs, Relaxed);
        self.max_entries.store(config.registry_max_entries, Relaxed);
        self.prune(Instant::now());
    }

    pub fn template(
        &self,
        tenant: String,
        provider: String,
        target: String,
        model: String,
        provider_key_fingerprint: Option<String>,
        capabilities: LifecycleCapabilities,
    ) -> Option<RouteTemplate> {
        let ttl = self.ttl_secs.load(Relaxed);
        let max_entries = self.max_entries.load(Relaxed);
        if ttl == 0 || max_entries == 0 {
            return None;
        }
        let created_at = Instant::now();
        Some(RouteTemplate {
            tenant,
            provider,
            provider_org: None,
            target,
            // the served route defaults to the named model; the request path
            // overrides it with `with_route` once it knows which route served
            route: model.clone(),
            model,
            provider_key_fingerprint,
            capabilities,
            created_at,
            expires_at: created_at + Duration::from_secs(ttl),
        })
    }

    pub fn record_body(&self, template: RouteTemplate, is_sse: bool, body: &[u8]) {
        let Some(response_id) = response_id(body, is_sse) else {
            return;
        };
        self.insert(template, response_id.clone(), response_id);
    }

    pub fn get(&self, tenant: &str, response_id: &str) -> Option<ResponseRoute> {
        let key = RegistryKey {
            tenant: tenant.to_string(),
            response_id: response_id.to_string(),
        };
        let route = self.entries.get(&key)?.clone();
        if Instant::now() >= route.expires_at {
            self.entries.remove(&key);
            return None;
        }
        Some(route)
    }

    pub fn remove(&self, tenant: &str, response_id: &str) {
        self.entries.remove(&RegistryKey {
            tenant: tenant.to_string(),
            response_id: response_id.to_string(),
        });
    }

    fn insert(&self, template: RouteTemplate, response_id: String, provider_native_id: String) {
        let now = Instant::now();
        if now >= template.expires_at {
            return;
        }
        self.prune(now);
        let max_entries = self.max_entries.load(Relaxed);
        while self.entries.len() >= max_entries {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|entry| entry.value().created_at)
                .map(|entry| entry.key().clone());
            match oldest {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => break,
            }
        }
        self.entries.insert(
            RegistryKey {
                tenant: template.tenant,
                response_id,
            },
            ResponseRoute {
                provider: template.provider,
                provider_org: template.provider_org,
                target: template.target,
                model: template.model,
                route: template.route,
                provider_native_id,
                provider_key_fingerprint: template.provider_key_fingerprint,
                capabilities: template.capabilities,
                created_at: template.created_at,
                expires_at: template.expires_at,
            },
        );
    }

    fn prune(&self, now: Instant) {
        self.entries.retain(|_, route| now < route.expires_at);
        let max_entries = self.max_entries.load(Relaxed);
        while self.entries.len() > max_entries {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|entry| entry.value().created_at)
                .map(|entry| entry.key().clone());
            match oldest {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => break,
            }
        }
    }
}

fn response_id(body: &[u8], is_sse: bool) -> Option<String> {
    if !is_sse {
        return serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|value| response_id_from_value(&value));
    }
    std::str::from_utf8(body).ok()?.lines().find_map(|line| {
        let data = line.strip_prefix("data:")?.trim();
        if data == "[DONE]" {
            return None;
        }
        serde_json::from_str::<Value>(data)
            .ok()
            .and_then(|value| response_id_from_value(&value))
    })
}

fn response_id_from_value(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("id"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(ttl: u64, max: usize) -> ResponseRegistry {
        ResponseRegistry::new(&ResponsesConfig {
            registry_ttl_secs: ttl,
            registry_max_entries: max,
        })
    }

    fn template(registry: &ResponseRegistry, tenant: &str) -> RouteTemplate {
        registry
            .template(
                tenant.to_string(),
                "openai".to_string(),
                "gpt-4o".to_string(),
                "gpt-4o".to_string(),
                None,
                LifecycleCapabilities::NATIVE_OPENAI,
            )
            .unwrap()
    }

    #[test]
    fn records_json_and_sse_ids_per_tenant() {
        let registry = registry(60, 10);
        registry.record_body(template(&registry, "a"), false, br#"{"id":"resp_json"}"#);
        registry.record_body(
            template(&registry, "a"),
            true,
            b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_sse\"}}\n\n",
        );
        assert!(registry.get("a", "resp_json").is_some());
        assert!(registry.get("a", "resp_sse").is_some());
        assert!(registry.get("b", "resp_json").is_none());
    }

    #[test]
    fn records_the_route_that_served_the_response() {
        let registry = registry(60, 10);
        registry.record_body(template(&registry, "a"), false, br#"{"id":"resp_named"}"#);
        registry.record_body(
            template(&registry, "a").with_route("tier-route".to_string()),
            false,
            br#"{"id":"resp_tier"}"#,
        );
        // without an override the served route is the model the caller named
        assert_eq!(registry.get("a", "resp_named").unwrap().route, "gpt-4o");
        let tier = registry.get("a", "resp_tier").unwrap();
        assert_eq!(tier.route, "tier-route");
        assert_eq!(tier.model, "gpt-4o");
    }

    /// #3461: a route recorded with the provider's `(org, name)` finds its
    /// provider again after a second org shares the name and the runtime id
    /// becomes `openai@org-a`; one recorded with the runtime id, as entries
    /// did before, resolves exactly as it did, so it finds the bare id while
    /// it exists and nothing once it is gone.
    #[test]
    fn a_stored_identity_resolves_across_a_name_becoming_shared() {
        let provider = |org: &str| rolter_core::ProviderConfig {
            name: "openai".to_string(),
            kind: rolter_core::ProviderKind::Openai,
            api_base: "https://api.openai.com/v1".to_string(),
            tenancy: Some(rolter_core::Tenancy {
                org_id: org.to_string(),
                project_id: None,
            }),
            ..Default::default()
        };
        let snapshot = |orgs: &[&str]| {
            let config = rolter_core::GatewayConfig {
                providers: orgs.iter().map(|org| provider(org)).collect(),
                ..Default::default()
            };
            crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default())
        };
        let registry = registry(60, 10);
        registry.record_body(
            template(&registry, "a")
                .with_provider_identity("openai".to_string(), "org-a".to_string()),
            false,
            br#"{"id":"resp_stable"}"#,
        );
        registry.record_body(template(&registry, "a"), false, br#"{"id":"resp_legacy"}"#);
        let stable = registry.get("a", "resp_stable").unwrap();
        let legacy = registry.get("a", "resp_legacy").unwrap();
        assert_eq!(stable.provider_org.as_deref(), Some("org-a"));
        assert_eq!(legacy.provider_org, None);

        let unshared = snapshot(&["org-a"]);
        assert_eq!(stable.provider_id(&unshared), Some("openai"));
        assert_eq!(legacy.provider_id(&unshared), Some("openai"));

        let shared = snapshot(&["org-a", "org-b"]);
        assert_eq!(stable.provider_id(&shared), Some("openai@org-a"));
        assert_eq!(legacy.provider_id(&shared), None);
        // another org's identity never resolves to org-a's provider
        let mut foreign = stable.clone();
        foreign.provider_org = Some("org-c".to_string());
        assert_eq!(foreign.provider_id(&shared), None);
    }

    #[test]
    fn disabled_registry_does_not_create_templates() {
        assert!(registry(0, 10)
            .template(
                "a".into(),
                "p".into(),
                "t".into(),
                "m".into(),
                None,
                LifecycleCapabilities::NATIVE_OPENAI,
            )
            .is_none());
    }

    #[test]
    fn expired_records_are_removed_on_lookup() {
        let registry = registry(60, 10);
        registry.record_body(template(&registry, "a"), false, br#"{"id":"resp_old"}"#);
        let key = RegistryKey {
            tenant: "a".to_string(),
            response_id: "resp_old".to_string(),
        };
        registry.entries.get_mut(&key).unwrap().expires_at = Instant::now();
        assert!(registry.get("a", "resp_old").is_none());
        assert!(!registry.entries.contains_key(&key));
    }
}
