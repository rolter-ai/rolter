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
    /// Reserved: the built-in engine compiles and validates `post_call` rules but
    /// the gateway does not yet run the output stage. Streaming/SSE boundaries and
    /// response buffering are documented and gated as follow-up work before output
    /// masking is enabled (see the issue's phasing).
    PostCall,
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
/// list the rule explicitly (or enable it via `default_on`); nothing scans by
/// default. Patterns are intentionally conservative to limit false positives.
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
    /// apply this rule without requiring a client opt-in
    #[serde(default)]
    pub default_on: bool,
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
    pub default_on: bool,
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
            default_on: rule.default_on,
            include_system: rule.include_system,
        })
    }
}

/// Compiled guardrails held in the immutable snapshot and shared across requests.
#[derive(Debug, Clone, Default)]
pub struct CompiledGuardrails {
    enabled: bool,
    max_scan_bytes: usize,
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
            rules,
        }
    }

    /// Whether any pre-call rule is active. The gateway uses this to skip all
    /// scanning work (JSON walk included) when guardrails add nothing.
    pub fn pre_call_active(&self) -> bool {
        self.enabled
            && self
                .rules
                .iter()
                .any(|rule| rule.stage == GuardStage::PreCall)
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
        if !self.enabled || *budget == 0 || text.len() > *budget {
            if text.len() > *budget {
                *budget = 0;
            }
            return ScanOutcome::Unchanged;
        }
        *budget -= text.len();

        let mut current = std::borrow::Cow::Borrowed(text);
        for rule in &self.rules {
            if rule.stage != GuardStage::PreCall {
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
            default_on: true,
            include_system: false,
        }
    }

    fn compiled(rules: Vec<GuardrailRule>) -> CompiledGuardrails {
        CompiledGuardrails::from_config(&GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            rules,
        })
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
            rules: vec![GuardrailRule {
                name: "x".to_string(),
                builtin: Some(BuiltinRule::Email),
                pattern: Some("a".to_string()),
                stage: GuardStage::PreCall,
                action: GuardAction::Block,
                replacement: None,
                default_on: false,
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
    fn validate_rejects_invalid_custom_pattern() {
        let cfg = GuardrailsConfig {
            enabled: true,
            max_scan_bytes: None,
            rules: vec![GuardrailRule {
                name: "bad".to_string(),
                builtin: None,
                pattern: Some("(".to_string()),
                stage: GuardStage::PreCall,
                action: GuardAction::Block,
                replacement: None,
                default_on: false,
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
