//! Built-in, zero-dependency guardrails: named regex rules for PII entities and
//! prompt-injection signals that run inside the gateway with no external service
//! and no network hop (ROL-261).
//!
//! The core is deliberately narrow and deterministic. Rules are compiled once
//! during config validation, using the linear-time `regex` engine (RE2-style, no
//! catastrophic backtracking) plus explicit compile-size and match-input limits
//! so a hostile configuration cannot turn matching into a regex-DoS. Evaluation
//! never retains or logs the raw matched text: callers get a redacted copy or a
//! block decision plus per-rule counters only.
//!
//! This built-in component performs no reversible mapping or restoration; that
//! remains the territory of the external/custom PII engines (ROL-258). It also
//! complements — never replaces — the custom guardrail webhook path (ROL-257).

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// Maximum size of a single compiled rule's program, in bytes. Bounds the memory
/// a hostile pattern can force the engine to allocate at compile time.
const REGEX_SIZE_LIMIT: usize = 1 << 20; // 1 MiB

/// Default cap on the number of bytes scanned across a request's message set. A
/// linear-time engine already bounds work per byte; this bounds the byte count.
pub const DEFAULT_MAX_SCAN_BYTES: usize = 256 * 1024;

/// Longest replacement token accepted, so redaction can never expand a match
/// into an unbounded amount of output.
const MAX_REPLACEMENT_LEN: usize = 64;

/// Stage at which a rule evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardStage {
    /// request content, before proxying upstream
    #[default]
    PreCall,
    /// non-streaming response content, before delivering to the client.
    ///
    /// The gateway buffers a non-streaming response body, applies these rules and
    /// then delivers the masked copy. Streamed (SSE) responses cannot be masked
    /// without buffering the whole completion, so they are governed separately by
    /// [`StreamingPostCall`] rather than silently passing through unscanned.
    PostCall,
}

/// What to do when a request asks for a streamed response on a route that has
/// `post_call` rules.
///
/// Output masking needs the whole text before it can decide: a match can straddle
/// any number of token boundaries, so a rule that redacts `a@b.com` cannot act on
/// a frame that so far holds only `a@b`. Buffering the completion would remove the
/// only property streaming has. So the operator chooses, and the default fails
/// closed — a masking rule that silently stops applying because the client passed
/// `"stream": true` is the failure mode worth ruling out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingPostCall {
    /// refuse the request with an OpenAI-compatible error naming the stage
    #[default]
    Reject,
    /// serve the stream with output rules not applied (input rules still run)
    Passthrough,
}

/// What a rule does when it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardAction {
    /// forward unchanged but record the match in telemetry counters
    #[default]
    Annotate,
    /// reject the request with an OpenAI-compatible error
    Block,
    /// replace each match with a fixed token such as `[REDACTED:EMAIL]`
    Redact,
}

/// A safe starter rule shipped with the gateway. All are opt-in: an operator must
/// list the rule explicitly; nothing scans by default. Patterns are
/// intentionally conservative to limit false positives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinRule {
    /// e-mail addresses
    Email,
    /// E.164-style and common separated phone numbers
    Phone,
    /// common provider API-token shapes (`sk-…`, `ghp_…`, AWS `AKIA…`, Slack `xox…`)
    ApiToken,
    /// 13–19 digit payment-card candidates (Luhn is not checked here)
    PaymentCard,
}

impl BuiltinRule {
    /// The linear-time pattern backing this built-in rule.
    pub fn pattern(self) -> &'static str {
        match self {
            // localpart@domain.tld — bounded character classes, no backtracking
            Self::Email => r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,24}",
            // optional +country, then 7–14 digits with space/dot/hyphen separators
            Self::Phone => r"\+?\d[\d .\-]{6,18}\d",
            Self::ApiToken => {
                r"(?:sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{20,}|AKIA[A-Z0-9]{16}|xox[baprs]-[A-Za-z0-9\-]{10,})"
            }
            // 13–19 digits, optionally grouped by single space/hyphen separators
            Self::PaymentCard => r"\b(?:\d[ \-]?){12,18}\d\b",
        }
    }

    /// Default redaction token for this entity.
    pub fn default_token(self) -> &'static str {
        match self {
            Self::Email => "[REDACTED:EMAIL]",
            Self::Phone => "[REDACTED:PHONE]",
            Self::ApiToken => "[REDACTED:API_TOKEN]",
            Self::PaymentCard => "[REDACTED:CARD]",
        }
    }
}

/// One configured guardrail rule. Provide exactly one of `builtin` or `pattern`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuardrailRule {
    /// stable, unique rule name; surfaced in telemetry, never carries match text
    pub name: String,
    /// a built-in starter entity; mutually exclusive with `pattern`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<BuiltinRule>,
    /// a custom user regex; mutually exclusive with `builtin`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default)]
    pub stage: GuardStage,
    #[serde(default)]
    pub action: GuardAction,
    /// replacement token for `redact`; falls back to the built-in default token
    /// (or a generic `[REDACTED]`) when omitted
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// also scan system messages. Off by default: operator-authored system
    /// instructions are trusted and excluded from scanning unless opted in.
    #[serde(default)]
    pub include_system: bool,
}

/// Guardrails configuration block (`[guardrails]`). Disabled by default; an empty
/// or disabled block adds no hot-path cost.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GuardrailsConfig {
    #[serde(default)]
    pub enabled: bool,
    /// cap on total bytes scanned per request; defaults to [`DEFAULT_MAX_SCAN_BYTES`]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_scan_bytes: Option<usize>,
    /// how streamed responses behave when `post_call` rules exist
    #[serde(default)]
    pub streaming_post_call: StreamingPostCall,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<GuardrailRule>,
}

impl GuardrailsConfig {
    /// Validate every rule by compiling it under the safe-regex limits and
    /// checking structural constraints. Returns human-readable problems for the
    /// aggregate config validator; an empty vec means the block is safe to load.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            let name = rule.name.trim();
            if name.is_empty() {
                problems.push("guardrail rule has an empty name".to_string());
                continue;
            }
            if !seen.insert(name) {
                problems.push(format!("duplicate guardrail rule name '{name}'"));
            }
            match (rule.builtin, rule.pattern.as_deref()) {
                (Some(_), Some(_)) => problems.push(format!(
                    "guardrail rule '{name}' sets both builtin and pattern (choose one)"
                )),
                (None, None) => problems.push(format!(
                    "guardrail rule '{name}' must set either builtin or pattern"
                )),
                (_, Some(pattern)) => {
                    if let Err(err) = compile(pattern) {
                        problems.push(format!(
                            "guardrail rule '{name}' has an invalid or unbounded pattern: {err}"
                        ));
                    }
                }
                (Some(_), None) => {}
            }
            if let Some(replacement) = &rule.replacement {
                if replacement.len() > MAX_REPLACEMENT_LEN {
                    problems.push(format!(
                        "guardrail rule '{name}' replacement exceeds {MAX_REPLACEMENT_LEN} bytes"
                    ));
                }
            }
        }
        problems
    }
}

/// Compile a pattern with linear-time semantics and a bounded program size.
fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
}

/// A rule compiled and ready for the request path.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub name: String,
    regex: Regex,
    pub stage: GuardStage,
    pub action: GuardAction,
    token: String,
    pub include_system: bool,
}

impl CompiledRule {
    fn from_config(rule: &GuardrailRule) -> Option<Self> {
        let (regex, default_token) = match (rule.builtin, rule.pattern.as_deref()) {
            (Some(builtin), _) => (compile(builtin.pattern()).ok()?, builtin.default_token()),
            (None, Some(pattern)) => (compile(pattern).ok()?, "[REDACTED]"),
            (None, None) => return None,
        };
        let token = rule
            .replacement
            .clone()
            .unwrap_or_else(|| default_token.to_string());
        Some(Self {
            name: rule.name.trim().to_string(),
            regex,
            stage: rule.stage,
            action: rule.action,
            token,
            include_system: rule.include_system,
        })
    }
}

/// Per-route guardrail selection, layered over the global rule set.
///
/// Named rules only: a route names the rules it wants turned off or on, so
/// adding a rule globally still reaches every route that has not opted out of
/// it by name. A name matching no configured rule is rejected at validation
/// rather than ignored — a typo in `disable` would otherwise read as "this
/// rule is off here" while the rule kept running.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct RouteGuardrails {
    /// rules that do not apply on this route
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,
    /// rules that apply on this route, overriding `disable` on a conflict
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enable: Vec<String>,
}

impl RouteGuardrails {
    pub fn is_empty(&self) -> bool {
        self.disable.is_empty() && self.enable.is_empty()
    }

    /// Whether `rule` applies on this route. `enable` wins a conflict, so a
    /// route that names the same rule in both is explicitly opting in.
    fn allows(&self, rule: &str) -> bool {
        if self.enable.iter().any(|name| name == rule) {
            return true;
        }
        !self.disable.iter().any(|name| name == rule)
    }

    /// Names referenced here that no configured rule defines.
    pub fn unknown_rules(&self, configured: &[String]) -> Vec<String> {
        self.disable
            .iter()
            .chain(self.enable.iter())
            .filter(|name| !configured.iter().any(|known| known == *name))
            .cloned()
            .collect()
    }
}

/// The rules active for one route, resolved once per snapshot rather than per
/// request. `None` means "every configured rule", which is the common case and
/// costs nothing to check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSelection {
    /// index-aligned with [`CompiledGuardrails::rules`]; empty = all active
    active: Vec<bool>,
}

impl RuleSelection {
    fn allows(&self, index: usize) -> bool {
        self.active.is_empty() || self.active.get(index).copied().unwrap_or(true)
    }

    /// Whether this selection turns anything off (used to keep the "no
    /// guardrails work at all" fast path exact).
    pub fn is_unrestricted(&self) -> bool {
        self.active.is_empty() || self.active.iter().all(|on| *on)
    }
}

/// Compiled guardrails held in the immutable snapshot and shared across requests.
#[derive(Debug, Clone, Default)]
pub struct CompiledGuardrails {
    enabled: bool,
    max_scan_bytes: usize,
    streaming_post_call: StreamingPostCall,
    rules: Vec<CompiledRule>,
}

/// Outcome of scanning one text segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanOutcome {
    /// forward the segment unchanged
    Unchanged,
    /// replace the segment with this redacted copy
    Redacted(String),
    /// block the whole request; carries the offending rule name (never match text)
    Blocked(String),
}

/// Per-request tally of rule hits, keyed by rule name. Safe to log: it exposes
/// rule name and a count only, never the matched value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardrailReport {
    pub hits: Vec<(String, usize)>,
    pub blocked_by: Option<String>,
    pub redactions: usize,
}

impl GuardrailReport {
    fn record(&mut self, rule: &str, count: usize) {
        if let Some(entry) = self.hits.iter_mut().find(|(name, _)| name == rule) {
            entry.1 += count;
        } else {
            self.hits.push((rule.to_string(), count));
        }
    }

    /// True when any rule matched at least once.
    pub fn matched(&self) -> bool {
        !self.hits.is_empty()
    }
}

impl CompiledGuardrails {
    /// Compile a config into the snapshot form. Rules that fail to compile are
    /// dropped defensively; the aggregate [`GuardrailsConfig::validate`] runs at
    /// load time and rejects such config before a snapshot is ever built.
    pub fn from_config(config: &GuardrailsConfig) -> Self {
        let rules = config
            .rules
            .iter()
            .filter_map(CompiledRule::from_config)
            .collect();
        Self {
            enabled: config.enabled,
            max_scan_bytes: config.max_scan_bytes.unwrap_or(DEFAULT_MAX_SCAN_BYTES),
            streaming_post_call: config.streaming_post_call,
            rules,
        }
    }

    /// Whether any pre-call rule is active. The gateway uses this to skip all
    /// scanning work (JSON walk included) when guardrails add nothing.
    pub fn pre_call_active(&self) -> bool {
        self.stage_active_for(GuardStage::PreCall, &RuleSelection::default())
    }

    /// Whether any pre-call rule survives `selection`. A route that disables
    /// every rule must skip the scan entirely, not walk the JSON to find
    /// nothing.
    pub fn pre_call_active_for(&self, selection: &RuleSelection) -> bool {
        self.stage_active_for(GuardStage::PreCall, selection)
    }

    /// Whether any post-call rule survives `selection`, i.e. whether a response
    /// body has to be buffered and scanned at all.
    pub fn post_call_active_for(&self, selection: &RuleSelection) -> bool {
        self.stage_active_for(GuardStage::PostCall, selection)
    }

    /// The operator's choice for streamed responses on a route with post-call
    /// rules.
    pub fn streaming_post_call(&self) -> StreamingPostCall {
        self.streaming_post_call
    }

    fn stage_active_for(&self, stage: GuardStage, selection: &RuleSelection) -> bool {
        self.enabled
            && self
                .rules
                .iter()
                .enumerate()
                .any(|(idx, rule)| rule.stage == stage && selection.allows(idx))
    }

    /// Names of every configured rule, for validating route overrides.
    pub fn rule_names(&self) -> Vec<String> {
        self.rules.iter().map(|rule| rule.name.clone()).collect()
    }

    /// Resolve a route's override into an index mask, once per snapshot.
    ///
    /// Doing it here rather than per request keeps the hot path to an indexed
    /// bool check, and means an override naming a rule that no longer exists
    /// simply selects nothing rather than costing a string comparison per
    /// segment per request.
    pub fn resolve_selection(&self, route: &RouteGuardrails) -> RuleSelection {
        if route.is_empty() {
            return RuleSelection::default();
        }
        RuleSelection {
            active: self.rules.iter().map(|r| route.allows(&r.name)).collect(),
        }
    }

    /// Scan one text segment against every active pre-call rule, in order.
    ///
    /// `is_system` marks operator-authored system content (skipped unless a rule
    /// opts in via `include_system`). `budget` is the remaining scan-byte budget;
    /// segments past the cap are left unchanged so total work stays bounded.
    /// Block wins over redaction: the first blocking match short-circuits.
    pub fn scan_segment(
        &self,
        text: &str,
        is_system: bool,
        budget: &mut usize,
        report: &mut GuardrailReport,
    ) -> ScanOutcome {
        self.scan_segment_with(text, is_system, budget, report, &RuleSelection::default())
    }

    /// [`Self::scan_segment`] restricted to the rules `selection` leaves active
    /// (see [`Self::resolve_selection`]).
    pub fn scan_segment_with(
        &self,
        text: &str,
        is_system: bool,
        budget: &mut usize,
        report: &mut GuardrailReport,
        selection: &RuleSelection,
    ) -> ScanOutcome {
        self.scan_stage(
            text,
            GuardStage::PreCall,
            is_system,
            budget,
            report,
            selection,
        )
    }

    /// Scan one segment of a response body against the active post-call rules.
    ///
    /// Model output has no trusted-author distinction to make — everything here
    /// came back from the provider — so unlike the input stage there is no
    /// `include_system` exemption.
    pub fn scan_output(
        &self,
        text: &str,
        budget: &mut usize,
        report: &mut GuardrailReport,
        selection: &RuleSelection,
    ) -> ScanOutcome {
        self.scan_stage(text, GuardStage::PostCall, false, budget, report, selection)
    }

    fn scan_stage(
        &self,
        text: &str,
        stage: GuardStage,
        is_system: bool,
        budget: &mut usize,
        report: &mut GuardrailReport,
        selection: &RuleSelection,
    ) -> ScanOutcome {
        if !self.enabled || *budget == 0 || text.len() > *budget {
            if text.len() > *budget {
                *budget = 0;
            }
            return ScanOutcome::Unchanged;
        }
        *budget -= text.len();

        let mut current = std::borrow::Cow::Borrowed(text);
        for (idx, rule) in self.rules.iter().enumerate() {
            if rule.stage != stage {
                continue;
            }
            if !selection.allows(idx) {
                continue;
            }
            if is_system && !rule.include_system {
                continue;
            }
            let count = rule.regex.find_iter(&current).count();
            if count == 0 {
                continue;
            }
            report.record(&rule.name, count);
            match rule.action {
                GuardAction::Annotate => {}
                GuardAction::Block => {
                    report.blocked_by = Some(rule.name.clone());
                    return ScanOutcome::Blocked(rule.name.clone());
                }
                GuardAction::Redact => {
                    report.redactions += count;
                    let replaced = rule
                        .regex
                        .replace_all(&current, rule.token.as_str())
                        .into_owned();
                    current = std::borrow::Cow::Owned(replaced);
                }
            }
        }

        match current {
            std::borrow::Cow::Borrowed(_) => ScanOutcome::Unchanged,
            std::borrow::Cow::Owned(s) => ScanOutcome::Redacted(s),
        }
    }

    /// Remaining scan-byte budget for a fresh request.
    pub fn scan_budget(&self) -> usize {
        self.max_scan_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, builtin: BuiltinRule, action: GuardAction) -> GuardrailRule {
        GuardrailRule {
            name: name.to_string(),
            builtin: Some(builtin),
            pattern: None,
            stage: GuardStage::PreCall,
            action,
            replacement: None,
            include_system: false,
        }
    }

    fn compiled(rules: Vec<GuardrailRule>) -> CompiledGuardrails {
        CompiledGuardrails::from_config(&GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            streaming_post_call: StreamingPostCall::default(),
            rules,
        })
    }

    // `default_on` was removed in #665: it gated a client opt-in that never
    // existed, so every rule applied whatever it was set to. Configs still
    // carrying the key must keep loading, and keep scanning, unchanged
    #[test]
    fn legacy_default_on_key_is_ignored_not_rejected() {
        let cfg: GuardrailsConfig = toml::from_str(
            r#"
            enabled = true
            [[rules]]
            name = "email"
            builtin = "email"
            action = "redact"
            default_on = false
            "#,
        )
        .expect("a config carrying the removed key still deserializes");
        assert!(cfg.validate().is_empty());

        let g = CompiledGuardrails::from_config(&cfg);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment("a.b@example.com", false, &mut budget, &mut report);
        assert_eq!(out, ScanOutcome::Redacted("[REDACTED:EMAIL]".to_string()));
    }

    #[test]
    fn redacts_email_leaving_surrounding_text() {
        let g = compiled(vec![rule("email", BuiltinRule::Email, GuardAction::Redact)]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment(
            "mail me at a.b@example.com now",
            false,
            &mut budget,
            &mut report,
        );
        assert_eq!(
            out,
            ScanOutcome::Redacted("mail me at [REDACTED:EMAIL] now".to_string())
        );
        assert_eq!(report.redactions, 1);
        assert_eq!(report.hits, vec![("email".to_string(), 1)]);
    }

    #[test]
    fn output_rules_run_only_at_the_output_stage() {
        let mut out = rule("email", BuiltinRule::Email, GuardAction::Redact);
        out.stage = GuardStage::PostCall;
        let g = compiled(vec![out]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();

        // the input stage must not see it...
        assert_eq!(
            g.scan_segment("a@b.com", false, &mut budget, &mut report),
            ScanOutcome::Unchanged
        );
        assert!(!g.pre_call_active_for(&RuleSelection::default()));

        // ...and the output stage must
        assert_eq!(
            g.scan_output(
                "a@b.com",
                &mut budget,
                &mut report,
                &RuleSelection::default()
            ),
            ScanOutcome::Redacted("[REDACTED:EMAIL]".to_string())
        );
        assert!(g.post_call_active_for(&RuleSelection::default()));
    }

    #[test]
    fn output_scanning_ignores_the_system_exemption() {
        // `include_system` describes operator-authored input; model output has no
        // equivalent, so an output rule applies to everything it is given
        let mut out = rule("email", BuiltinRule::Email, GuardAction::Redact);
        out.stage = GuardStage::PostCall;
        out.include_system = false;
        let g = compiled(vec![out]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        assert_eq!(
            g.scan_output(
                "a@b.com",
                &mut budget,
                &mut report,
                &RuleSelection::default()
            ),
            ScanOutcome::Redacted("[REDACTED:EMAIL]".to_string())
        );
    }

    #[test]
    fn streamed_responses_fail_closed_by_default() {
        // an operator who never writes the key must not silently lose masking
        assert_eq!(StreamingPostCall::default(), StreamingPostCall::Reject);
        assert_eq!(
            compiled(vec![]).streaming_post_call(),
            StreamingPostCall::Reject
        );
        let opted_out = CompiledGuardrails::from_config(&GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            streaming_post_call: StreamingPostCall::Passthrough,
            rules: vec![],
        });
        assert_eq!(
            opted_out.streaming_post_call(),
            StreamingPostCall::Passthrough
        );
    }

    #[test]
    fn block_action_short_circuits() {
        let g = compiled(vec![rule(
            "card",
            BuiltinRule::PaymentCard,
            GuardAction::Block,
        )]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment(
            "pay with 4111 1111 1111 1111",
            false,
            &mut budget,
            &mut report,
        );
        assert_eq!(out, ScanOutcome::Blocked("card".to_string()));
        assert_eq!(report.blocked_by.as_deref(), Some("card"));
    }

    #[test]
    fn system_content_excluded_unless_opted_in() {
        let g = compiled(vec![rule("email", BuiltinRule::Email, GuardAction::Redact)]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment("contact ops@corp.com", true, &mut budget, &mut report);
        assert_eq!(out, ScanOutcome::Unchanged);
        assert!(!report.matched());
    }

    #[test]
    fn system_content_scanned_when_included() {
        let mut r = rule("email", BuiltinRule::Email, GuardAction::Redact);
        r.include_system = true;
        let g = compiled(vec![r]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment("contact ops@corp.com", true, &mut budget, &mut report);
        assert_eq!(
            out,
            ScanOutcome::Redacted("contact [REDACTED:EMAIL]".to_string())
        );
    }

    #[test]
    fn annotate_counts_without_mutating() {
        let g = compiled(vec![rule(
            "email",
            BuiltinRule::Email,
            GuardAction::Annotate,
        )]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment("x@y.io", false, &mut budget, &mut report);
        assert_eq!(out, ScanOutcome::Unchanged);
        assert_eq!(report.hits, vec![("email".to_string(), 1)]);
        assert_eq!(report.redactions, 0);
    }

    #[test]
    fn budget_stops_scanning_oversized_segment() {
        let g = CompiledGuardrails::from_config(&GuardrailsConfig {
            enabled: true,
            max_scan_bytes: Some(8),
            streaming_post_call: StreamingPostCall::default(),
            rules: vec![rule("email", BuiltinRule::Email, GuardAction::Block)],
        });
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        let out = g.scan_segment(
            "way over the tiny budget a@b.com",
            false,
            &mut budget,
            &mut report,
        );
        assert_eq!(out, ScanOutcome::Unchanged);
        assert_eq!(budget, 0);
    }

    #[test]
    fn validate_rejects_both_builtin_and_pattern() {
        let cfg = GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            streaming_post_call: StreamingPostCall::default(),
            rules: vec![GuardrailRule {
                name: "x".to_string(),
                builtin: Some(BuiltinRule::Email),
                pattern: Some("a".to_string()),
                stage: GuardStage::PreCall,
                action: GuardAction::Block,
                replacement: None,
                include_system: false,
            }],
        };
        assert!(cfg.validate().iter().any(|p| p.contains("choose one")));
    }

    #[test]
    fn validate_rejects_duplicate_and_empty_names() {
        let cfg = GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            streaming_post_call: StreamingPostCall::default(),
            rules: vec![
                rule("dup", BuiltinRule::Email, GuardAction::Block),
                rule("dup", BuiltinRule::Phone, GuardAction::Block),
                GuardrailRule {
                    name: "  ".to_string(),
                    ..rule("blank", BuiltinRule::Email, GuardAction::Block)
                },
            ],
        };
        let problems = cfg.validate();
        assert!(problems.iter().any(|p| p.contains("duplicate")));
        assert!(problems.iter().any(|p| p.contains("empty name")));
    }

    #[test]
    fn route_override_allows_and_denies_by_name() {
        let route = RouteGuardrails {
            disable: vec!["email".to_string()],
            enable: vec!["card".to_string()],
        };
        assert!(!route.allows("email"));
        assert!(route.allows("card"));
        // a rule the route says nothing about keeps the global behaviour, so
        // adding a rule globally still reaches every route
        assert!(route.allows("phone"));
    }

    #[test]
    fn route_override_reports_names_no_rule_defines() {
        let route = RouteGuardrails {
            disable: vec!["emial".to_string()],
            enable: Vec::new(),
        };
        assert_eq!(
            route.unknown_rules(&["email".to_string()]),
            vec!["emial".to_string()]
        );
        assert!(route.unknown_rules(&["emial".to_string()]).is_empty());
    }

    #[test]
    fn validate_rejects_invalid_custom_pattern() {
        let cfg = GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            streaming_post_call: StreamingPostCall::default(),
            rules: vec![GuardrailRule {
                name: "bad".to_string(),
                builtin: None,
                pattern: Some("(".to_string()),
                stage: GuardStage::PreCall,
                action: GuardAction::Block,
                replacement: None,
                include_system: false,
            }],
        };
        assert!(cfg
            .validate()
            .iter()
            .any(|p| p.contains("invalid or unbounded")));
    }

    #[test]
    fn disabled_config_is_inert() {
        let g = CompiledGuardrails::from_config(&GuardrailsConfig {
            enabled: false,
            max_scan_bytes: None,
            streaming_post_call: StreamingPostCall::default(),
            rules: vec![rule("email", BuiltinRule::Email, GuardAction::Block)],
        });
        assert!(!g.pre_call_active());
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        assert_eq!(
            g.scan_segment("a@b.com", false, &mut budget, &mut report),
            ScanOutcome::Unchanged
        );
    }
}
