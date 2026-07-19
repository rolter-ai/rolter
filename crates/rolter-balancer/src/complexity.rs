//! Bounded, opt-in complexity tiers for selecting a route before balancing.
//!
//! Policies live in the existing route `params` JSON under a reserved key so
//! they move through control-plane snapshots without widening the core route
//! schema. The gateway removes that key before forwarding upstream.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Reserved route-param key carrying the complexity policy JSON.
pub const POLICY_PARAM: &str = "_rolter_complexity";
const MAX_TIERS: usize = 16;

/// Ordered tiers whose input-size ceilings select configured route models.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComplexityPolicy {
    #[serde(default)]
    pub tiers: Vec<ComplexityTier>,
}

/// One bounded input-size tier and the route it selects.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ComplexityTier {
    /// stable operator-defined label; safe as a bounded metric label
    pub name: String,
    /// inclusive byte ceiling; `None` is the final catch-all tier
    #[serde(default)]
    pub max_input_bytes: Option<usize>,
    /// configured route model to select when the tier matches
    pub route: String,
}

impl ComplexityPolicy {
    /// Parse a policy from route params. Missing policy is intentionally free:
    /// callers can skip all complexity work when this returns `None`.
    pub fn from_params(
        params: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<Self>, String> {
        let Some(value) = params.get(POLICY_PARAM) else {
            return Ok(None);
        };
        let policy: Self = serde_json::from_value(value.clone()).map_err(|error| {
            format!("{POLICY_PARAM} must be a complexity policy object: {error}")
        })?;
        policy.validate_shape()?;
        Ok(Some(policy))
    }

    /// Select the first matching tier. This accepts only a request byte count;
    /// it never receives, clones, logs, or persists prompt content.
    pub fn select(&self, input_bytes: usize) -> Option<&ComplexityTier> {
        self.tiers
            .iter()
            .find(|tier| tier.max_input_bytes.is_none_or(|max| input_bytes <= max))
    }

    /// Validate operator input independent of a particular snapshot.
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.tiers.is_empty() {
            return Err("complexity policy must define at least one tier".to_string());
        }
        if self.tiers.len() > MAX_TIERS {
            return Err(format!(
                "complexity policy supports at most {MAX_TIERS} tiers"
            ));
        }
        let mut names = HashSet::new();
        let mut previous_max = None;
        for (index, tier) in self.tiers.iter().enumerate() {
            if tier.name.trim().is_empty() {
                return Err(format!("complexity tier {index} has an empty name"));
            }
            if !names.insert(tier.name.as_str()) {
                return Err(format!("complexity tier '{}' is duplicated", tier.name));
            }
            if tier.route.trim().is_empty() {
                return Err(format!(
                    "complexity tier '{}' has an empty route",
                    tier.name
                ));
            }
            match tier.max_input_bytes {
                Some(0) => {
                    return Err(format!(
                        "complexity tier '{}' max_input_bytes must be greater than zero",
                        tier.name
                    ))
                }
                Some(max) if previous_max.is_some_and(|previous| max <= previous) => {
                    return Err(format!(
                        "complexity tier '{}' max_input_bytes must be greater than the preceding tier",
                        tier.name
                    ))
                }
                Some(max) => previous_max = Some(max),
                None if index + 1 != self.tiers.len() => {
                    return Err(format!(
                        "complexity tier '{}' is unbounded but not last",
                        tier.name
                    ))
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Validate every tier target against the routes in the current snapshot.
    pub fn validate_routes(&self, route_models: &HashSet<String>) -> Result<(), String> {
        for tier in &self.tiers {
            if !route_models.contains(&tier.route) {
                return Err(format!(
                    "complexity tier '{}' references unknown route '{}'",
                    tier.name, tier.route
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ComplexityPolicy {
        ComplexityPolicy {
            tiers: vec![
                ComplexityTier {
                    name: "simple".to_string(),
                    max_input_bytes: Some(128),
                    route: "fast".to_string(),
                },
                ComplexityTier {
                    name: "complex".to_string(),
                    max_input_bytes: None,
                    route: "capable".to_string(),
                },
            ],
        }
    }

    #[test]
    fn selects_first_matching_bounded_tier() {
        let policy = policy();
        assert_eq!(
            policy.select(128).map(|tier| tier.route.as_str()),
            Some("fast")
        );
        assert_eq!(
            policy.select(129).map(|tier| tier.route.as_str()),
            Some("capable")
        );
    }

    #[test]
    fn rejects_unbounded_nonfinal_tier() {
        let mut policy = policy();
        policy.tiers[0].max_input_bytes = None;
        assert!(policy.validate_shape().unwrap_err().contains("not last"));
    }

    #[test]
    fn validates_targets_against_snapshot_routes() {
        let policy = policy();
        let routes = HashSet::from(["fast".to_string(), "capable".to_string()]);
        assert!(policy.validate_routes(&routes).is_ok());
    }
}
