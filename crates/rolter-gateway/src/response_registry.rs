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
    pub provider: String,
    pub target: String,
    pub model: String,
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
    pub target: String,
    pub model: String,
    pub provider_key_fingerprint: Option<String>,
    pub capabilities: LifecycleCapabilities,
    created_at: Instant,
    expires_at: Instant,
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
            target,
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
                target: template.target,
                model: template.model,
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
