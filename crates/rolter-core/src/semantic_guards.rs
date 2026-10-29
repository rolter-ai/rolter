//! Scoped semantic guardrail policies over several providers (#459).
//!
//! A semantic guardrail provider is a self-hosted classifier spoken to over the
//! vendor-neutral webhook contract in [`crate::guardrail_webhook`], or one the
//! gateway runs in process (`kind = "local"`, #3329): it judges a request body
//! (`pre_call`) or a response body (`post_call`) by meaning and answers allow,
//! block or transform, optionally with scored `categories`.
//!
//! This module owns the policy model that decides which providers see a
//! request and what a score means:
//!
//! - **Providers** (`[[semantic_guards.providers]]`, or the dashboard registry)
//!   can be enabled side by side, each with its own stage, timeout, retries and
//!   fail-open/fail-closed mode.
//! - **Policies** (`[[semantic_guards.policies]]`) are scoped to the
//!   deployment, an organization, a project, a route or a virtual key. Exactly
//!   one policy applies to a request: the most specific enabled one, in the
//!   order key, route, project, org, deployment. It replaces the broader ones
//!   rather than merging with them, so the policy that applies can be read off
//!   one row.
//! - A provider that no policy lists is **unattached** and runs on every request
//!   no policy matches, which is how the single deployment-wide webhook behaved
//!   before policies existed.
//! - **Thresholds** turn a provider's scored categories into violations: a
//!   score at or above the policy's threshold for that category (or for `"*"`)
//!   violates it, and the policy's `on_violation` blocks the request or only
//!   records the violation.
//!
//! Guards decide; sanitizers transform. Redacting or restoring personal data is
//! the PII sanitizer's job ([`crate::pii_sanitizer`], #848), and masking with
//! regular expressions is the built-in rules' ([`crate::guardrails`]).
//!
//! Only the configuration model, its validation and the resolution live here;
//! the HTTP calls and the request-path wiring are in the gateway.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::guardrail_webhook::{GuardrailWebhookConfig, WebhookDecision, WebhookStage};

/// The threshold key that applies to every category a policy does not name.
pub const ANY_CATEGORY: &str = "*";
/// The longest category name a policy may set a threshold for.
pub const MAX_CATEGORY_LEN: usize = 64;
/// The most thresholds one policy may set.
pub const MAX_THRESHOLDS: usize = 64;
/// The most scored categories read from one provider answer; the rest are
/// ignored, so a misbehaving provider cannot make the gateway walk an
/// unbounded map.
pub const MAX_SCORED_CATEGORIES: usize = 256;
/// The most `local_concurrency` may be set to.
pub const MAX_LOCAL_CONCURRENCY: usize = 4096;
/// The metric label a scored category gets when the policy sets no threshold
/// of its own for it, which keeps the label set bounded by configuration.
pub const OTHER_CATEGORY: &str = "other";

/// `[semantic_guards]`: the providers and the scoped policies that pick them.
/// Empty by default; an empty table adds no hot-path cost.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SemanticGuardsConfig {
    /// guardrail providers, each the same shape as `[guardrail_webhook]`; only
    /// enabled ones run
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<GuardrailWebhookConfig>,
    /// scoped policies naming the providers that apply and their thresholds
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<GuardrailPolicy>,
    /// how many `local` provider calls one gateway runs at once, across every
    /// local provider (#3329, ADR-0039); unset means the CPUs the process may
    /// use. A call that waits longer than its provider's `timeout_ms` for a
    /// slot is the overload case and follows the provider's `failure_mode`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_concurrency: Option<usize>,
    /// the directory every `local` provider's `model_path` must resolve
    /// under, symlinks and `..` resolved first (#3454); unset, a path may
    /// point anywhere on the gateway host. Read from the gateway's own file
    /// at start and never serialized, so it does not travel in
    /// `/internal/snapshot`: each gateway confines its own file system
    #[serde(default, skip_serializing)]
    pub model_root: Option<String>,
}

/// What a policy is scoped to, from broadest to narrowest.
///
/// Deliberately closed (#3347): exactly one policy applies to a request,
/// chosen by specificity, and an unknown scope has no place in that order: read
/// as broader it could loosen a narrower policy, read as narrower it could
/// replace the policy that should apply. A new scope is a snapshot format bump
/// (ADR-0038).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyScope {
    /// every request
    #[default]
    Deployment,
    /// requests made with a key of this organization (`scope_id` is its id)
    Org,
    /// requests made with a key of this project (`scope_id` is its id)
    Project,
    /// requests for this route (`scope_id` is the route's public model name)
    Route,
    /// requests made with this virtual key (`scope_id` is the key's id)
    Key,
}

impl PolicyScope {
    /// Every scope, narrowest first: the order a request is matched in.
    pub const PRECEDENCE: [PolicyScope; 5] = [
        PolicyScope::Key,
        PolicyScope::Route,
        PolicyScope::Project,
        PolicyScope::Org,
        PolicyScope::Deployment,
    ];

    /// The stored and serialized spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::Org => "org",
            Self::Project => "project",
            Self::Route => "route",
            Self::Key => "key",
        }
    }

    /// Parse the stored spelling.
    pub fn parse(value: &str) -> Option<Self> {
        Self::PRECEDENCE
            .into_iter()
            .find(|scope| scope.as_str() == value)
    }
}

/// What a policy does when a scored category crosses its threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationAction {
    /// refuse the request, or withhold the response
    #[default]
    Block,
    /// deliver it, and record the violation on the request log and metrics
    Annotate,
    /// an action this build does not know (#3347); read as
    /// [`ViolationAction::Block`]
    #[serde(other)]
    Unknown,
}

impl ViolationAction {
    /// The stored and serialized spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Annotate => "annotate",
            Self::Unknown => "unknown",
        }
    }
}

/// How a policy with a `post_call` provider treats a streamed response
/// (ADR-0037). There is deliberately no value that streams with the
/// `post_call` providers skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingStrategy {
    /// refuse `"stream": true` with `400 guardrail_streaming_unsupported`
    #[default]
    Reject,
    /// hold the whole stream, check it once, then replay or withhold it
    Buffer,
    /// a strategy this build does not know (#3347); read as
    /// [`StreamingStrategy::Reject`]
    #[serde(other)]
    Unknown,
}

impl StreamingStrategy {
    /// The stored and serialized spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Buffer => "buffer",
            Self::Unknown => "unknown",
        }
    }
}

fn default_true() -> bool {
    true
}

/// One scoped policy (`[[semantic_guards.policies]]`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct GuardrailPolicy {
    /// unique name, recorded on the request log and as a metric label
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub scope: PolicyScope,
    /// the org, project or key id, or the route's public model name; absent
    /// for the deployment scope
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
    /// provider names, consulted in this order
    #[serde(default)]
    pub providers: Vec<String>,
    /// category -> score in `[0, 1]` at or above which the category is
    /// violated; `"*"` covers every category not named
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub thresholds: BTreeMap<String, f64>,
    #[serde(default)]
    pub on_violation: ViolationAction,
    #[serde(default)]
    pub streaming: StreamingStrategy,
}

/// Problems with one policy's own fields, without reference to the providers
/// it names. Shared by the config validator and the control plane, which
/// checks a row before saving it.
pub fn policy_field_problems(
    scope: PolicyScope,
    scope_id: Option<&str>,
    thresholds: &BTreeMap<String, f64>,
) -> Vec<(&'static str, String)> {
    let mut problems = Vec::new();
    let scope_id = scope_id.map(str::trim).filter(|id| !id.is_empty());
    match (scope, scope_id) {
        (PolicyScope::Deployment, Some(_)) => problems.push((
            "scope_id",
            "a deployment-wide policy takes no scope_id".to_string(),
        )),
        (PolicyScope::Deployment, None) => {}
        (scope, None) => problems.push((
            "scope_id",
            format!("a policy scoped to a {} needs a scope_id", scope.as_str()),
        )),
        (_, Some(_)) => {}
    }
    if thresholds.len() > MAX_THRESHOLDS {
        problems.push((
            "thresholds",
            format!("a policy sets at most {MAX_THRESHOLDS} thresholds"),
        ));
    }
    for (category, threshold) in thresholds {
        let trimmed = category.trim();
        if trimmed.is_empty() || trimmed != category {
            problems.push((
                "thresholds",
                format!("category '{category}' must be non-empty with no surrounding spaces"),
            ));
        } else if category.len() > MAX_CATEGORY_LEN {
            problems.push((
                "thresholds",
                format!("category '{category}' is longer than {MAX_CATEGORY_LEN} bytes"),
            ));
        }
        if !threshold.is_finite() || !(0.0..=1.0).contains(threshold) {
            problems.push((
                "thresholds",
                format!("the threshold for '{category}' must be between 0 and 1"),
            ));
        }
    }
    problems
}

impl SemanticGuardsConfig {
    /// Validate the providers and the policies against each other. `legacy`
    /// is `[guardrail_webhook]`, whose name a policy may also list. An empty
    /// vec means the table is safe to load.
    pub fn validate(&self, legacy: &GuardrailWebhookConfig) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(cap) = self.local_concurrency {
            if cap == 0 || cap > MAX_LOCAL_CONCURRENCY {
                problems.push(format!(
                    "semantic_guards.local_concurrency must be between 1 and \
                     {MAX_LOCAL_CONCURRENCY}"
                ));
            }
        }
        if let Some(root) = &self.model_root {
            if !Path::new(root.trim()).is_absolute() {
                problems.push("semantic_guards.model_root must be an absolute path".to_string());
            }
        }
        let mut names: HashSet<&str> = HashSet::new();
        if legacy.enabled && !legacy.name.trim().is_empty() {
            names.insert(legacy.name.trim());
        }
        for (index, provider) in self.providers.iter().enumerate() {
            let name = provider.name.trim();
            if name.is_empty() {
                problems.push(format!("semantic_guards.providers[{index}].name is empty"));
                continue;
            }
            if !names.insert(name) {
                problems.push(format!(
                    "semantic_guards.providers: the name '{name}' is used twice"
                ));
            }
            problems.extend(
                provider
                    .validate()
                    .into_iter()
                    .map(|problem| format!("semantic_guards.providers '{name}': {problem}")),
            );
        }
        let mut policy_names: HashSet<&str> = HashSet::new();
        let mut scopes: HashSet<(PolicyScope, &str)> = HashSet::new();
        for (index, policy) in self.policies.iter().enumerate() {
            let name = policy.name.trim();
            if name.is_empty() {
                problems.push(format!("semantic_guards.policies[{index}].name is empty"));
                continue;
            }
            if !policy_names.insert(name) {
                problems.push(format!(
                    "semantic_guards.policies: the name '{name}' is used twice"
                ));
            }
            for (field, problem) in
                policy_field_problems(policy.scope, policy.scope_id.as_deref(), &policy.thresholds)
            {
                problems.push(format!(
                    "semantic_guards.policies '{name}'.{field}: {problem}"
                ));
            }
            let scope_id = policy.scope_id.as_deref().unwrap_or("").trim();
            if policy.enabled && !scopes.insert((policy.scope, scope_id)) {
                problems.push(format!(
                    "semantic_guards.policies '{name}': another enabled policy already covers \
                     {} '{scope_id}'",
                    policy.scope.as_str()
                ));
            }
            let mut listed: HashSet<&str> = HashSet::new();
            for provider in &policy.providers {
                if !names.contains(provider.as_str()) {
                    problems.push(format!(
                        "semantic_guards.policies '{name}': no provider is named '{provider}'"
                    ));
                }
                if !listed.insert(provider.as_str()) {
                    problems.push(format!(
                        "semantic_guards.policies '{name}': '{provider}' is listed twice"
                    ));
                }
            }
        }
        problems
    }
}

/// The identity a request is matched against. Empty strings never match a
/// scope, so a config-file key (which has no id) only ever reaches the route
/// and deployment scopes.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyRequest<'a> {
    pub org: &'a str,
    pub project: &'a str,
    pub route: &'a str,
    pub key: &'a str,
}

impl PolicyRequest<'_> {
    fn id_for(&self, scope: PolicyScope) -> Option<&str> {
        let id = match scope {
            PolicyScope::Deployment => return Some(""),
            PolicyScope::Org => self.org,
            PolicyScope::Project => self.project,
            PolicyScope::Route => self.route,
            PolicyScope::Key => self.key,
        };
        (!id.is_empty()).then_some(id)
    }
}

/// A policy ready for the request path: provider indices split by stage.
#[derive(Debug, Clone)]
pub struct CompiledPolicy {
    /// the policy's name; empty for the unattached providers' implicit policy
    pub name: String,
    pub scope: PolicyScope,
    pre_call: Vec<usize>,
    post_call: Vec<usize>,
    pub thresholds: BTreeMap<String, f64>,
    pub on_violation: ViolationAction,
    pub streaming: StreamingStrategy,
}

/// One scored category that crossed its threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub category: String,
    pub score: f64,
    pub threshold: f64,
}

impl CompiledPolicy {
    /// The label this policy is recorded under: its name, or `default` for the
    /// implicit policy of the unattached providers.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            "default"
        } else {
            &self.name
        }
    }

    /// Whether any provider of this policy runs at `stage`.
    pub fn runs_at(&self, stage: WebhookStage) -> bool {
        !self.indices(stage).is_empty()
    }

    fn indices(&self, stage: WebhookStage) -> &[usize] {
        match stage {
            WebhookStage::PreCall => &self.pre_call,
            WebhookStage::PostCall => &self.post_call,
        }
    }

    /// The categories in `scores` that violate this policy, in category
    /// order. A category with no threshold of its own uses `"*"`; one covered
    /// by neither is never a violation.
    pub fn violations(&self, scores: &BTreeMap<String, f64>) -> Vec<Violation> {
        if self.thresholds.is_empty() {
            return Vec::new();
        }
        let any = self.thresholds.get(ANY_CATEGORY).copied();
        scores
            .iter()
            .filter_map(|(category, score)| {
                let threshold = self.thresholds.get(category).copied().or(any)?;
                (*score >= threshold).then(|| Violation {
                    category: category.clone(),
                    score: *score,
                    threshold,
                })
            })
            .collect()
    }

    /// The metric label for `category`: itself when this policy sets a
    /// threshold for it by name, else [`OTHER_CATEGORY`]. Category names come
    /// from the provider, so only configured ones may become label values.
    pub fn category_label<'c>(&self, category: &'c str) -> &'c str {
        if category != ANY_CATEGORY && self.thresholds.contains_key(category) {
            category
        } else {
            OTHER_CATEGORY
        }
    }
}

/// The providers and policies of one snapshot, compiled for the request path.
#[derive(Debug, Clone, Default)]
pub struct CompiledSemanticGuards {
    providers: Vec<GuardrailWebhookConfig>,
    policies: HashMap<(PolicyScope, String), CompiledPolicy>,
    implicit: Option<CompiledPolicy>,
}

impl CompiledSemanticGuards {
    /// Compile `config` plus the legacy `[guardrail_webhook]`, which joins the
    /// providers when it is enabled. Disabled providers and policies are left
    /// out; a policy naming a provider that is not enabled simply does not run
    /// it.
    pub fn from_config(config: &SemanticGuardsConfig, legacy: &GuardrailWebhookConfig) -> Self {
        let mut providers: Vec<GuardrailWebhookConfig> = Vec::new();
        if legacy.enabled {
            providers.push(legacy.clone());
        }
        providers.extend(config.providers.iter().filter(|p| p.enabled).cloned());
        let by_name: HashMap<&str, usize> = providers
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.name.trim().is_empty())
            .map(|(index, p)| (p.name.trim(), index))
            .collect();
        let split = |indices: &[usize]| -> (Vec<usize>, Vec<usize>) {
            indices
                .iter()
                .copied()
                .partition(|&index| providers[index].stage == WebhookStage::PreCall)
        };

        // a provider any policy lists, enabled or not, is confined to policies:
        // pausing a policy must not widen its providers to every request
        let listed: HashSet<&str> = config
            .policies
            .iter()
            .flat_map(|policy| policy.providers.iter().map(String::as_str))
            .collect();

        let mut policies = HashMap::new();
        for policy in config.policies.iter().filter(|p| p.enabled) {
            let indices: Vec<usize> = policy
                .providers
                .iter()
                .filter_map(|name| by_name.get(name.as_str()).copied())
                .collect();
            let (pre_call, post_call) = split(&indices);
            let scope_id = policy.scope_id.as_deref().unwrap_or("").trim().to_string();
            policies.insert(
                (policy.scope, scope_id),
                CompiledPolicy {
                    name: policy.name.trim().to_string(),
                    scope: policy.scope,
                    pre_call,
                    post_call,
                    thresholds: policy.thresholds.clone(),
                    on_violation: policy.on_violation,
                    streaming: policy.streaming,
                },
            );
        }

        let unattached: Vec<usize> = (0..providers.len())
            .filter(|&index| !listed.contains(providers[index].name.trim()))
            .collect();
        let implicit = (!unattached.is_empty()).then(|| {
            let (pre_call, post_call) = split(&unattached);
            CompiledPolicy {
                name: String::new(),
                scope: PolicyScope::Deployment,
                pre_call,
                post_call,
                thresholds: BTreeMap::new(),
                on_violation: ViolationAction::Block,
                streaming: StreamingStrategy::Reject,
            }
        });

        Self {
            providers,
            policies,
            implicit,
        }
    }

    /// Whether no provider runs anywhere, so the request path can skip the
    /// resolution altogether.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// The policy that applies to `request`: the most specific enabled match,
    /// else the implicit policy of the unattached providers, else none.
    pub fn resolve(&self, request: &PolicyRequest<'_>) -> Option<&CompiledPolicy> {
        if self.providers.is_empty() {
            return None;
        }
        PolicyScope::PRECEDENCE
            .into_iter()
            .find_map(|scope| {
                let id = request.id_for(scope)?;
                self.policies.get(&(scope, id.to_string()))
            })
            .or(self.implicit.as_ref())
    }

    /// The providers `policy` consults at `stage`, in its order.
    pub fn providers_for<'s>(
        &'s self,
        policy: &'s CompiledPolicy,
        stage: WebhookStage,
    ) -> impl Iterator<Item = &'s GuardrailWebhookConfig> + 's {
        policy
            .indices(stage)
            .iter()
            .filter_map(|&index| self.providers.get(index))
    }

    /// The names of the enabled providers no policy lists.
    pub fn unattached(&self) -> Vec<&str> {
        self.implicit
            .iter()
            .flat_map(|policy| policy.pre_call.iter().chain(&policy.post_call))
            .filter_map(|&index| self.providers.get(index).map(|p| p.name.as_str()))
            .collect()
    }
}

/// A provider's answer: its decision plus the categories it scored.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredDecision {
    pub decision: WebhookDecision,
    pub categories: BTreeMap<String, f64>,
}

impl ScoredDecision {
    /// Parse a provider's response body strictly. `None` means the body is not
    /// a decision — not JSON, not an object, an unknown `action`, or neither an
    /// `action` nor `categories` — and the caller resolves it through the
    /// provider's failure mode rather than reading it as an allow.
    ///
    /// A body with `categories` and no `action` is an allow carrying scores,
    /// which is how a pure classifier answers. Scores outside `[0, 1]` or not
    /// finite are dropped, as are names longer than [`MAX_CATEGORY_LEN`] and
    /// anything past [`MAX_SCORED_CATEGORIES`].
    pub fn parse(body: &[u8]) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_slice(body).ok()?;
        let object = value.as_object()?;
        let categories: BTreeMap<String, f64> = object
            .get("categories")
            .and_then(serde_json::Value::as_object)
            .map(|scores| {
                scores
                    .iter()
                    .filter(|(name, _)| !name.is_empty() && name.len() <= MAX_CATEGORY_LEN)
                    .filter_map(|(name, score)| Some((name.clone(), score.as_f64()?)))
                    .filter(|(_, score)| score.is_finite() && (0.0..=1.0).contains(score))
                    .take(MAX_SCORED_CATEGORIES)
                    .collect()
            })
            .unwrap_or_default();
        let decision = if object.contains_key("action") {
            serde_json::from_value::<WebhookDecision>(value).ok()?
        } else if object.contains_key("categories") {
            WebhookDecision::Allow
        } else {
            return None;
        };
        Some(Self {
            decision,
            categories,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, stage: WebhookStage) -> GuardrailWebhookConfig {
        GuardrailWebhookConfig {
            enabled: true,
            name: name.to_string(),
            url: format!("https://{name}.internal/check"),
            stage,
            ..GuardrailWebhookConfig::default()
        }
    }

    fn policy(
        name: &str,
        scope: PolicyScope,
        id: Option<&str>,
        providers: &[&str],
    ) -> GuardrailPolicy {
        GuardrailPolicy {
            name: name.to_string(),
            enabled: true,
            scope,
            scope_id: id.map(str::to_string),
            providers: providers.iter().map(|p| p.to_string()).collect(),
            thresholds: BTreeMap::new(),
            on_violation: ViolationAction::Block,
            streaming: StreamingStrategy::Reject,
        }
    }

    fn config() -> SemanticGuardsConfig {
        SemanticGuardsConfig {
            providers: vec![
                provider("injection", WebhookStage::PreCall),
                provider("toxicity", WebhookStage::PostCall),
                provider("everywhere", WebhookStage::PreCall),
            ],
            local_concurrency: None,
            model_root: None,
            policies: vec![
                policy("all", PolicyScope::Deployment, None, &["injection"]),
                policy(
                    "org-a",
                    PolicyScope::Org,
                    Some("org-a"),
                    &["injection", "toxicity"],
                ),
                policy("project-p", PolicyScope::Project, Some("p"), &["toxicity"]),
                policy("route-r", PolicyScope::Route, Some("gpt-4o"), &[]),
                policy("key-k", PolicyScope::Key, Some("k"), &["injection"]),
            ],
        }
    }

    fn request<'a>(
        org: &'a str,
        project: &'a str,
        route: &'a str,
        key: &'a str,
    ) -> PolicyRequest<'a> {
        PolicyRequest {
            org,
            project,
            route,
            key,
        }
    }

    fn resolved(guards: &CompiledSemanticGuards, req: PolicyRequest<'_>) -> String {
        guards
            .resolve(&req)
            .map(|policy| policy.label().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn the_most_specific_policy_wins_in_documented_order() {
        let guards = CompiledSemanticGuards::from_config(&config(), &Default::default());
        assert_eq!(
            resolved(&guards, request("org-a", "p", "gpt-4o", "k")),
            "key-k"
        );
        assert_eq!(
            resolved(&guards, request("org-a", "p", "gpt-4o", "other")),
            "route-r"
        );
        assert_eq!(
            resolved(&guards, request("org-a", "p", "claude", "")),
            "project-p"
        );
        assert_eq!(
            resolved(&guards, request("org-a", "q", "claude", "")),
            "org-a"
        );
        assert_eq!(resolved(&guards, request("org-b", "", "claude", "")), "all");
    }

    #[test]
    fn a_policy_replaces_broader_ones_rather_than_merging() {
        let guards = CompiledSemanticGuards::from_config(&config(), &Default::default());
        let project = guards.resolve(&request("org-a", "p", "x", "")).unwrap();
        let pre: Vec<&str> = guards
            .providers_for(project, WebhookStage::PreCall)
            .map(|p| p.name.as_str())
            .collect();
        let post: Vec<&str> = guards
            .providers_for(project, WebhookStage::PostCall)
            .map(|p| p.name.as_str())
            .collect();
        assert!(
            pre.is_empty(),
            "org-a's injection must not leak in: {pre:?}"
        );
        assert_eq!(post, ["toxicity"]);
    }

    #[test]
    fn unattached_providers_run_where_no_policy_matches() {
        let mut cfg = config();
        cfg.policies.retain(|p| p.scope != PolicyScope::Deployment);
        let guards = CompiledSemanticGuards::from_config(&cfg, &Default::default());
        assert_eq!(guards.unattached(), ["everywhere"]);
        let fallback = guards.resolve(&request("org-b", "", "claude", "")).unwrap();
        assert_eq!(fallback.label(), "default");
        let names: Vec<&str> = guards
            .providers_for(fallback, WebhookStage::PreCall)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["everywhere"]);
    }

    #[test]
    fn a_paused_policy_does_not_widen_its_providers() {
        let mut cfg = config();
        cfg.policies.retain(|p| p.name == "org-a");
        cfg.policies[0].enabled = false;
        let guards = CompiledSemanticGuards::from_config(&cfg, &Default::default());
        assert_eq!(guards.unattached(), ["everywhere"]);
        // org-a's providers do not fall through to the implicit policy
        let policy = guards.resolve(&request("org-a", "", "x", "")).unwrap();
        assert_eq!(policy.label(), "default");
        assert!(!policy.runs_at(WebhookStage::PostCall));
    }

    #[test]
    fn several_providers_can_run_and_the_legacy_webhook_joins_them() {
        let legacy = GuardrailWebhookConfig {
            enabled: true,
            url: "https://legacy/check".to_string(),
            ..GuardrailWebhookConfig::default()
        };
        let cfg = SemanticGuardsConfig {
            providers: vec![
                provider("a", WebhookStage::PreCall),
                provider("b", WebhookStage::PreCall),
            ],
            policies: Vec::new(),
            local_concurrency: None,
            model_root: None,
        };
        let guards = CompiledSemanticGuards::from_config(&cfg, &legacy);
        let policy = guards.resolve(&PolicyRequest::default()).unwrap();
        let urls: Vec<&str> = guards
            .providers_for(policy, WebhookStage::PreCall)
            .map(|p| p.url.as_str())
            .collect();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://legacy/check");
    }

    #[test]
    fn nothing_enabled_resolves_to_nothing() {
        let guards = CompiledSemanticGuards::from_config(&config_disabled(), &Default::default());
        assert!(guards.is_empty());
        assert!(guards.resolve(&request("org-a", "", "", "")).is_none());
    }

    fn config_disabled() -> SemanticGuardsConfig {
        let mut cfg = config();
        for p in &mut cfg.providers {
            p.enabled = false;
        }
        cfg
    }

    #[test]
    fn thresholds_name_categories_and_star_covers_the_rest() {
        let mut compiled = CompiledSemanticGuards::from_config(&config(), &Default::default());
        let policy = compiled
            .policies
            .get_mut(&(PolicyScope::Deployment, String::new()))
            .unwrap();
        policy.thresholds =
            BTreeMap::from([("violence".to_string(), 0.8), ("*".to_string(), 0.95)]);
        let scores = BTreeMap::from([
            ("violence".to_string(), 0.8),
            ("self_harm".to_string(), 0.9),
            ("hate".to_string(), 0.97),
        ]);
        let violations = policy.violations(&scores);
        let names: Vec<&str> = violations.iter().map(|v| v.category.as_str()).collect();
        assert_eq!(names, ["hate", "violence"]);
        assert_eq!(policy.category_label("violence"), "violence");
        assert_eq!(policy.category_label("hate"), OTHER_CATEGORY);
    }

    #[test]
    fn no_thresholds_means_scores_alone_never_violate() {
        let guards = CompiledSemanticGuards::from_config(&config(), &Default::default());
        let policy = guards.resolve(&PolicyRequest::default()).unwrap();
        let scores = BTreeMap::from([("violence".to_string(), 1.0)]);
        assert!(policy.violations(&scores).is_empty());
    }

    #[test]
    fn validation_catches_scope_threshold_and_reference_mistakes() {
        let mut cfg = config();
        cfg.policies
            .push(policy("dup-scope", PolicyScope::Org, Some("org-a"), &[]));
        cfg.policies
            .push(policy("no-id", PolicyScope::Key, None, &[]));
        cfg.policies.push(policy(
            "ghost",
            PolicyScope::Route,
            Some("r2"),
            &["missing", "missing"],
        ));
        let mut bad = policy("bad-threshold", PolicyScope::Deployment, Some("x"), &[]);
        bad.enabled = false;
        bad.thresholds.insert("violence".to_string(), 1.5);
        bad.thresholds.insert(" spaced".to_string(), 0.5);
        cfg.policies.push(bad);
        let problems = cfg.validate(&Default::default()).join("\n");
        assert!(
            problems.contains("already covers org 'org-a'"),
            "{problems}"
        );
        assert!(problems.contains("'no-id'.scope_id"), "{problems}");
        assert!(
            problems.contains("no provider is named 'missing'"),
            "{problems}"
        );
        assert!(problems.contains("'missing' is listed twice"), "{problems}");
        assert!(problems.contains("takes no scope_id"), "{problems}");
        assert!(problems.contains("between 0 and 1"), "{problems}");
        assert!(problems.contains("surrounding spaces"), "{problems}");
        assert!(config().validate(&Default::default()).is_empty());
    }

    #[test]
    fn a_provider_name_may_not_repeat_the_legacy_webhook() {
        let legacy = GuardrailWebhookConfig {
            enabled: true,
            name: "injection".to_string(),
            url: "https://legacy/check".to_string(),
            ..GuardrailWebhookConfig::default()
        };
        let problems = config().validate(&legacy).join("\n");
        assert!(problems.contains("'injection' is used twice"), "{problems}");
    }

    #[test]
    fn scored_decisions_parse_strictly() {
        let scored =
            ScoredDecision::parse(br#"{"categories":{"violence":0.9,"bad":7,"nan":"x"}}"#).unwrap();
        assert_eq!(scored.decision, WebhookDecision::Allow);
        assert_eq!(
            scored.categories,
            BTreeMap::from([("violence".to_string(), 0.9)])
        );
        let block =
            ScoredDecision::parse(br#"{"action":"block","reason":"r","categories":{"x":1}}"#)
                .unwrap();
        assert!(matches!(block.decision, WebhookDecision::Block { .. }));
        assert_eq!(block.categories.len(), 1);
        assert!(ScoredDecision::parse(br#"{"action":"nuke"}"#).is_none());
        assert!(ScoredDecision::parse(b"not json").is_none());
        assert!(ScoredDecision::parse(br#"{"verdict":"ok"}"#).is_none());
        assert!(ScoredDecision::parse(br#"[]"#).is_none());
    }

    #[test]
    fn the_table_round_trips_through_toml() {
        let parsed: SemanticGuardsConfig = toml::from_str(
            r#"
            [[providers]]
            name = "toxicity"
            url = "https://tox.internal/check"
            enabled = true
            stage = "post_call"
            failure_mode = "fail_closed"

            [[policies]]
            name = "org-a"
            scope = "org"
            scope_id = "org-a"
            providers = ["toxicity"]
            thresholds = { violence = 0.8, "*" = 0.95 }
            on_violation = "annotate"
            streaming = "buffer"
            "#,
        )
        .unwrap();
        assert!(parsed.validate(&Default::default()).is_empty());
        assert_eq!(parsed.policies[0].streaming, StreamingStrategy::Buffer);
        assert_eq!(parsed.policies[0].on_violation, ViolationAction::Annotate);
        assert_eq!(parsed.policies[0].thresholds.get("*"), Some(&0.95));
    }

    #[test]
    fn model_root_is_read_from_a_file_and_never_serialized() {
        let parsed: SemanticGuardsConfig =
            toml::from_str(r#"model_root = "/etc/rolter/models""#).unwrap();
        assert_eq!(parsed.model_root.as_deref(), Some("/etc/rolter/models"));
        assert!(parsed.validate(&Default::default()).is_empty());
        // gateway-side: a control plane's root never reaches a gateway
        let json = serde_json::to_value(&parsed).unwrap();
        assert!(json.get("model_root").is_none(), "{json}");
        let relative = SemanticGuardsConfig {
            model_root: Some("models".to_string()),
            ..SemanticGuardsConfig::default()
        };
        assert!(relative
            .validate(&Default::default())
            .join("")
            .contains("model_root must be an absolute path"));
    }

    #[test]
    fn a_local_provider_and_the_concurrency_cap_round_trip_and_validate() {
        let parsed: SemanticGuardsConfig = toml::from_str(&format!(
            r#"
            local_concurrency = 2

            [[providers]]
            name = "injection"
            kind = "local"
            model_path = "/models/injection.json"
            model_sha256 = "{}"
            enabled = true
            failure_mode = "fail_closed"
            "#,
            "0f".repeat(32)
        ))
        .unwrap();
        assert!(parsed.validate(&Default::default()).is_empty());
        assert_eq!(parsed.local_concurrency, Some(2));
        assert_eq!(
            parsed.providers[0].kind,
            crate::guardrail_webhook::GuardProviderKind::Local
        );
        let mut zero = parsed.clone();
        zero.local_concurrency = Some(0);
        assert!(zero
            .validate(&Default::default())
            .join("")
            .contains("local_concurrency"));
    }
}
