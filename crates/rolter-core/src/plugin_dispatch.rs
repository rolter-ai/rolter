//! Pluggable request/response middleware for the gateway (#509).
//!
//! An org (optionally scoped further to a project) registers webhook plugin
//! instances against the control-plane registry (`crates/rolter-control/src/plugins.rs`,
//! #567). Each instance names a [`PluginStage`] — `pre_route`, `pre_upstream` or
//! `post_response` — and the gateway consults every enabled instance for the
//! caller's org/project at that stage, in `position` order, before continuing.
//!
//! This module owns only the config model carried in the snapshot. The wire
//! contract (`allow`/`block`/`transform`/`annotate`) and the HTTP call itself
//! are shared with the custom guardrail webhook (#257) —
//! [`WebhookDecision`](crate::guardrail_webhook::WebhookDecision),
//! [`WebhookAuth`] and [`FailureMode`] are the same types, since a plugin and a
//! guardrail webhook make the same kind of decision about the same kind of
//! content. Only the stage vocabulary differs (three gateway hook points here,
//! versus guardrail's request/response pair), so it gets its own enum.

use serde::{Deserialize, Serialize};

use crate::guardrail_webhook::{FailureMode, WebhookAuth, WebhookTenant};

/// Where in the request lifecycle a plugin runs.
///
/// Deliberately closed (#3347): a stage says where a plugin runs, and no
/// reading of an unknown one is safe to guess. A new stage is a snapshot
/// format bump (ADR-0038).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStage {
    /// before the route is resolved, while only the caller's virtual key and
    /// raw request are known
    PreRoute,
    /// after the route is resolved, before forwarding to the upstream provider
    PreUpstream,
    /// after the upstream response is received, before it is delivered to the
    /// caller. Non-streaming responses only; see the gateway's dispatch module
    PostResponse,
}

impl PluginStage {
    /// Parse the database's `stage` text column. Unrecognized values fall back
    /// to `PreUpstream`, keeping every stored row dispatchable rather than
    /// silently dropping a plugin the operator explicitly enabled.
    pub fn from_db_str(value: &str) -> Self {
        match value {
            "pre_route" => Self::PreRoute,
            "post_response" => Self::PostResponse,
            _ => Self::PreUpstream,
        }
    }
}

/// One enabled plugin instance, as carried in the gateway snapshot.
///
/// Only `kind = "webhook"` instances are ever produced here — it's the only
/// kind the registry accepts today (`plugins.rs::validate`), so there is
/// nothing else for the gateway to branch on.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginInstanceConfig {
    /// stable identifier for logs, metrics labels and error messages
    pub slug: String,
    pub org_id: String,
    /// unscoped when absent: applies to every project in the org
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub stage: PluginStage,
    /// dispatch order within (org, project, stage); lower runs first
    pub position: i32,
    pub failure_mode: FailureMode,
    pub endpoint: String,
    /// present when the registry row names a `secret_env`; the plugin's
    /// contract only knows bearer auth, unlike the guardrail webhook's
    /// bearer/shared-secret choice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebhookAuth>,
}

/// Every enabled plugin instance across every org, as carried in the snapshot.
/// An empty list is the common case and costs nothing on the request path.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PluginsConfig {
    #[serde(default)]
    pub instances: Vec<PluginInstanceConfig>,
}

impl PluginsConfig {
    /// Validate every instance. A malformed endpoint would otherwise only
    /// surface as an opaque connect failure on the first live request.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for instance in &self.instances {
            let endpoint = instance.endpoint.trim();
            if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
                problems.push(format!(
                    "plugin '{}' endpoint '{endpoint}' must be an http:// or https:// URL",
                    instance.slug
                ));
            }
        }
        problems
    }

    /// Plugins active at `stage` for `org_id`/`project_id`, in dispatch order.
    /// An org-level instance (`project_id: None`) applies to every project in
    /// that org; a project-level instance applies only to its own project.
    /// Both may fire for the same request, ordered together by `position` so
    /// an operator controls interleaving explicitly rather than the org tier
    /// always running first or last.
    pub fn for_stage(
        &self,
        stage: PluginStage,
        org_id: &str,
        project_id: Option<&str>,
    ) -> Vec<&PluginInstanceConfig> {
        let mut matched: Vec<&PluginInstanceConfig> = self
            .instances
            .iter()
            .filter(|p| p.stage == stage && p.org_id == org_id)
            .filter(|p| match &p.project_id {
                None => true,
                Some(scoped) => Some(scoped.as_str()) == project_id,
            })
            .collect();
        matched.sort_by_key(|p| p.position);
        matched
    }
}

/// Version of the request body the gateway POSTs to a plugin, sent as
/// `contract_version` in every envelope (#1405).
///
/// A plugin author reads it to know which body they are looking at, and rolter
/// bumps it to change the body incompatibly. The rule for bumping is written
/// down for plugin authors in `docs/user-docs/configuration/plugins.mdx`: an
/// additive change (a new optional field, a new value of an existing field the
/// plugin was told to expect) keeps the number; removing, renaming or
/// retyping a field, or changing what one means, raises it. The gateway sends
/// exactly one version, the current one, so a bump is a release note, not a
/// negotiation.
pub const PLUGIN_CONTRACT_VERSION: u32 = 1;

/// The JSON envelope the gateway POSTs to a plugin's endpoint. Shares its
/// shape with the guardrail webhook's [`crate::guardrail_webhook::WebhookRequest`]
/// deliberately — the wire contract is the same idea (content in, a decision
/// out) — but `stage` carries the three plugin hook points rather than the
/// guardrail's pre/post-call pair, so it is its own type.
#[derive(Debug, Clone, Serialize)]
pub struct PluginRequest<'a> {
    /// the version of this body; see [`PLUGIN_CONTRACT_VERSION`]
    pub contract_version: u32,
    /// the plugin instance's slug, so one endpoint serving several instances
    /// can tell them apart
    pub plugin: &'a str,
    pub stage: PluginStage,
    pub model: &'a str,
    pub route: &'a str,
    pub trace_id: &'a str,
    pub tenant: WebhookTenant,
    /// true when `content` was truncated to the dispatcher's body-size cap
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub content: &'a serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(
        slug: &str,
        org: &str,
        project: Option<&str>,
        stage: PluginStage,
        position: i32,
    ) -> PluginInstanceConfig {
        PluginInstanceConfig {
            slug: slug.to_string(),
            org_id: org.to_string(),
            project_id: project.map(str::to_string),
            stage,
            position,
            failure_mode: FailureMode::FailOpen,
            endpoint: "https://plugins.internal/hook".to_string(),
            auth: None,
        }
    }

    #[test]
    fn stage_parses_the_three_db_values_and_falls_back_to_pre_upstream() {
        assert_eq!(PluginStage::from_db_str("pre_route"), PluginStage::PreRoute);
        assert_eq!(
            PluginStage::from_db_str("pre_upstream"),
            PluginStage::PreUpstream
        );
        assert_eq!(
            PluginStage::from_db_str("post_response"),
            PluginStage::PostResponse
        );
        assert_eq!(
            PluginStage::from_db_str("nonsense"),
            PluginStage::PreUpstream
        );
    }

    #[test]
    fn org_scoped_plugin_applies_to_every_project() {
        let config = PluginsConfig {
            instances: vec![instance(
                "audit",
                "org-a",
                None,
                PluginStage::PreUpstream,
                10,
            )],
        };
        assert_eq!(
            config
                .for_stage(PluginStage::PreUpstream, "org-a", Some("project-1"))
                .len(),
            1
        );
        assert_eq!(
            config
                .for_stage(PluginStage::PreUpstream, "org-a", None)
                .len(),
            1
        );
        assert!(config
            .for_stage(PluginStage::PreUpstream, "org-b", Some("project-1"))
            .is_empty());
    }

    #[test]
    fn project_scoped_plugin_applies_only_to_its_own_project() {
        let config = PluginsConfig {
            instances: vec![instance(
                "audit-override",
                "org-a",
                Some("project-1"),
                PluginStage::PreUpstream,
                10,
            )],
        };
        assert_eq!(
            config
                .for_stage(PluginStage::PreUpstream, "org-a", Some("project-1"))
                .len(),
            1
        );
        assert!(config
            .for_stage(PluginStage::PreUpstream, "org-a", Some("project-2"))
            .is_empty());
        assert!(config
            .for_stage(PluginStage::PreUpstream, "org-a", None)
            .is_empty());
    }

    #[test]
    fn org_and_project_scoped_plugins_interleave_by_position() {
        let config = PluginsConfig {
            instances: vec![
                instance("org-wide", "org-a", None, PluginStage::PreUpstream, 20),
                instance(
                    "project-only",
                    "org-a",
                    Some("project-1"),
                    PluginStage::PreUpstream,
                    10,
                ),
            ],
        };
        let matched = config.for_stage(PluginStage::PreUpstream, "org-a", Some("project-1"));
        let slugs: Vec<&str> = matched.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["project-only", "org-wide"]);
    }

    #[test]
    fn wrong_stage_does_not_match() {
        let config = PluginsConfig {
            instances: vec![instance("audit", "org-a", None, PluginStage::PreRoute, 10)],
        };
        assert!(config
            .for_stage(PluginStage::PreUpstream, "org-a", None)
            .is_empty());
    }

    #[test]
    fn validate_rejects_a_non_http_endpoint() {
        let mut config = PluginsConfig {
            instances: vec![instance(
                "audit",
                "org-a",
                None,
                PluginStage::PreUpstream,
                10,
            )],
        };
        config.instances[0].endpoint = "ftp://plugins.internal/hook".to_string();
        assert_eq!(config.validate().len(), 1);
    }

    #[test]
    fn empty_config_has_no_problems() {
        assert!(PluginsConfig::default().validate().is_empty());
    }
}
