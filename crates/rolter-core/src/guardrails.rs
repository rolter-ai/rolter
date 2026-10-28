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
///
/// Deliberately closed (#3347): a stage says what a rule scans, and no
/// reading of an unknown one narrows what is served without guessing. A new
/// stage is a snapshot format bump (ADR-0038).
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
    /// a value this build does not know (#3347); read as
    /// [`StreamingPostCall::Reject`]
    #[serde(other)]
    Unknown,
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
    /// an action this build does not know (#3347); read as
    /// [`GuardAction::Block`]
    #[serde(other)]
    Unknown,
}

/// A safe starter rule shipped with the gateway. All are opt-in: an operator must
/// list the rule explicitly; nothing scans by default. Patterns are
/// intentionally conservative to limit false positives.
///
/// Deliberately closed (#3347): a detector this build does not have cannot
/// scan, and the only reading of it that never serves more than configured
/// would block every request at its stage. A new detector is a snapshot format
/// bump (ADR-0038).
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
    /// (or a generic `[REDACTED]`) when omitted, empty or whitespace-only
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
        // an empty token would silently delete matches, so it means "default"
        let token = match rule.replacement.as_deref() {
            Some(custom) if !custom.trim().is_empty() => custom.to_string(),
            _ => default_token.to_string(),
        };
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
        // entries are trimmed like the rule names they are matched against
        let rule = rule.trim();
        if self.enable.iter().any(|name| name.trim() == rule) {
            return true;
        }
        !self.disable.iter().any(|name| name.trim() == rule)
    }

    /// Names referenced here that no configured rule defines.
    pub fn unknown_rules(&self, configured: &[String]) -> Vec<String> {
        self.disable
            .iter()
            .chain(self.enable.iter())
            .filter(|name| {
                let name = name.trim();
                !configured.iter().any(|known| known.trim() == name)
            })
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
    /// names of the rules that rewrote content, once each per request
    pub redacted_by: Vec<String>,
    /// names of the `annotate` rules that matched, once each per request.
    /// Annotate forwards the request unchanged, so this is the only trace one
    /// leaves (#2158)
    pub annotated_by: Vec<String>,
}

impl GuardrailReport {
    fn record(&mut self, rule: &str, count: usize) {
        if let Some(entry) = self.hits.iter_mut().find(|(name, _)| name == rule) {
            entry.1 += count;
        } else {
            self.hits.push((rule.to_string(), count));
        }
    }

    /// The per-rule outcomes of this request as `<action>:<rule>` events
    /// (`blocked`, `redacted`, `annotated`), the form the request log stores.
    /// Rule names only, never matched text.
    pub fn events(&self) -> Vec<String> {
        let mut events = Vec::new();
        if let Some(rule) = &self.blocked_by {
            events.push(format!("blocked:{rule}"));
        }
        events.extend(self.redacted_by.iter().map(|r| format!("redacted:{r}")));
        events.extend(self.annotated_by.iter().map(|r| format!("annotated:{r}")));
        events
    }

    fn note(list: &mut Vec<String>, rule: &str) {
        if !list.iter().any(|name| name == rule) {
            list.push(rule.to_string());
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
        self.scan_stage_traced(text, stage, is_system, budget, report, selection, None)
    }

    /// [`Self::scan_stage`], optionally recording one [`PolicyStep`] per rule
    /// it walks. The trace is how a dry run reports each rule from the very
    /// loop the request path runs; with `None` (every real request) the only
    /// cost is a branch on a rule that matched.
    #[allow(clippy::too_many_arguments)]
    fn scan_stage_traced(
        &self,
        text: &str,
        stage: GuardStage,
        is_system: bool,
        budget: &mut usize,
        report: &mut GuardrailReport,
        selection: &RuleSelection,
        mut trace: Option<&mut Vec<PolicyStep>>,
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
            let skip = if rule.stage != stage {
                Some(DryRunSkip::OtherStage)
            } else if !selection.allows(idx) {
                // a route override; a dry run never carries one, so the label
                // only has to be truthful that the rule did not run
                Some(DryRunSkip::OtherStage)
            } else if is_system && !rule.include_system {
                Some(DryRunSkip::SystemMessage)
            } else {
                None
            };
            if let Some(skip) = skip {
                if let Some(trace) = trace.as_deref_mut() {
                    trace.push(PolicyStep::skipped(rule, skip));
                }
                continue;
            }
            let count = rule.regex.find_iter(&current).count();
            if count == 0 {
                if let Some(trace) = trace.as_deref_mut() {
                    trace.push(PolicyStep::evaluated(rule, DryRunOutcome::NoMatch, 0));
                }
                continue;
            }
            report.record(&rule.name, count);
            match rule.action {
                GuardAction::Annotate => {
                    GuardrailReport::note(&mut report.annotated_by, &rule.name);
                    if let Some(trace) = trace.as_deref_mut() {
                        trace.push(PolicyStep::evaluated(rule, DryRunOutcome::Annotated, count));
                    }
                }
                GuardAction::Block | GuardAction::Unknown => {
                    report.blocked_by = Some(rule.name.clone());
                    if let Some(trace) = trace.as_deref_mut() {
                        trace.push(PolicyStep::evaluated(rule, DryRunOutcome::Blocked, count));
                    }
                    return ScanOutcome::Blocked(rule.name.clone());
                }
                GuardAction::Redact => {
                    report.redactions += count;
                    GuardrailReport::note(&mut report.redacted_by, &rule.name);
                    let replaced = rule
                        .regex
                        .replace_all(&current, rule.token.as_str())
                        .into_owned();
                    current = std::borrow::Cow::Owned(replaced);
                    if let Some(trace) = trace.as_deref_mut() {
                        trace.push(PolicyStep::evaluated(rule, DryRunOutcome::Redacted, count));
                    }
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

/// Most match spans a dry run reports. The count is exact; only the list is
/// capped, so a sample full of matches cannot balloon the response.
pub const MAX_DRY_RUN_SPANS: usize = 200;

/// What a rule would do to a sample, as the request path would decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunOutcome {
    /// nothing matched; the sample is forwarded unchanged
    NoMatch,
    /// the rule matched and only tallied it; the sample is forwarded unchanged
    Annotated,
    /// the rule matched and rewrote the sample
    Redacted,
    /// the rule matched and would reject the whole request
    Blocked,
    /// the rule would not look at this sample at all; see [`DryRunSkip`]
    Skipped,
}

/// Why a dry run did not scan the sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunSkip {
    /// the sample was marked as a system message and the rule has
    /// `include_system` off
    SystemMessage,
    /// the sample is longer than the per-request scan budget, so the request
    /// path would leave it unscanned
    OverScanBudget,
    /// a policy dry run only: the rule belongs to the other stage (or a
    /// route override turned it off), so it does not run on this sample
    OtherStage,
    /// a policy dry run only: an earlier rule blocked the request, so the
    /// scan never reached this one
    AfterBlock,
    /// a policy dry run only: the guardrails switch is off, so no rule runs
    PolicyDisabled,
    /// a policy dry run only: the rule does not compile, so the gateway drops
    /// it when it builds the policy
    NotCompiled,
}

/// One match, in UTF-16 code units so a browser can index its own string with
/// the numbers as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DryRunSpan {
    pub start: usize,
    pub end: usize,
}

/// The result of running one unsaved rule against a sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DryRun {
    pub outcome: DryRunOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<DryRunSkip>,
    /// total matches, even when `spans` is capped
    pub match_count: usize,
    pub spans: Vec<DryRunSpan>,
    /// the sample after redaction; present only for [`DryRunOutcome::Redacted`]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<String>,
}

/// Evaluate one rule against `text` with the matcher the gateway runs.
///
/// The rule goes through the same validation and compilation as a stored one,
/// so a pattern the gateway would refuse is refused here with the same
/// message. `Err` carries those problems. The outcome comes from the request
/// path's own scan (`scan_segment` or `scan_output`), so the two cannot
/// disagree; only the spans are computed separately, since a scan reports
/// counts and never positions. Nothing here logs or retains `text`.
pub fn dry_run_rule(
    rule: &GuardrailRule,
    text: &str,
    is_system: bool,
) -> Result<DryRun, Vec<String>> {
    let config = GuardrailsConfig {
        enabled: true,
        rules: vec![rule.clone()],
        ..GuardrailsConfig::default()
    };
    let problems = config.validate();
    if !problems.is_empty() {
        return Err(problems);
    }
    let Some(compiled_rule) = CompiledRule::from_config(rule) else {
        return Err(vec!["the rule could not be compiled".to_string()]);
    };
    let guard = CompiledGuardrails::from_config(&config);

    let skip = if rule.stage == GuardStage::PreCall && is_system && !rule.include_system {
        Some(DryRunSkip::SystemMessage)
    } else if text.len() > guard.scan_budget() {
        Some(DryRunSkip::OverScanBudget)
    } else {
        None
    };
    if skip.is_some() {
        return Ok(DryRun {
            outcome: DryRunOutcome::Skipped,
            skip,
            match_count: 0,
            spans: Vec::new(),
            redacted: None,
        });
    }

    let mut budget = guard.scan_budget();
    let mut report = GuardrailReport::default();
    let selection = RuleSelection::default();
    let scanned = match rule.stage {
        GuardStage::PreCall => {
            guard.scan_segment_with(text, is_system, &mut budget, &mut report, &selection)
        }
        GuardStage::PostCall => guard.scan_output(text, &mut budget, &mut report, &selection),
    };

    let mut spans = Vec::new();
    let mut match_count = 0;
    let (mut utf16, mut last_byte) = (0usize, 0usize);
    for found in compiled_rule.regex.find_iter(text) {
        match_count += 1;
        if spans.len() < MAX_DRY_RUN_SPANS {
            utf16 += text[last_byte..found.start()].encode_utf16().count();
            let start = utf16;
            utf16 += text[found.start()..found.end()].encode_utf16().count();
            last_byte = found.end();
            spans.push(DryRunSpan { start, end: utf16 });
        }
    }

    let (outcome, redacted) = match scanned {
        ScanOutcome::Blocked(_) => (DryRunOutcome::Blocked, None),
        ScanOutcome::Redacted(out) => (DryRunOutcome::Redacted, Some(out)),
        ScanOutcome::Unchanged if report.matched() => (DryRunOutcome::Annotated, None),
        ScanOutcome::Unchanged => (DryRunOutcome::NoMatch, None),
    };
    Ok(DryRun {
        outcome,
        skip: None,
        match_count,
        spans,
        redacted,
    })
}

/// What one rule did at its place in a policy dry run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyStep {
    pub rule: String,
    pub stage: GuardStage,
    pub action: GuardAction,
    pub outcome: DryRunOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<DryRunSkip>,
    /// matches against the text as this rule saw it, which is after the
    /// redactions of every earlier rule
    pub match_count: usize,
}

impl PolicyStep {
    fn skipped(rule: &CompiledRule, skip: DryRunSkip) -> Self {
        Self {
            rule: rule.name.clone(),
            stage: rule.stage,
            action: rule.action,
            outcome: DryRunOutcome::Skipped,
            skip: Some(skip),
            match_count: 0,
        }
    }

    fn evaluated(rule: &CompiledRule, outcome: DryRunOutcome, match_count: usize) -> Self {
        Self {
            rule: rule.name.clone(),
            stage: rule.stage,
            action: rule.action,
            outcome,
            skip: None,
            match_count,
        }
    }

    fn unreached(rule: &GuardrailRule, skip: DryRunSkip) -> Self {
        Self {
            rule: rule.name.trim().to_string(),
            stage: rule.stage,
            action: rule.action,
            outcome: DryRunOutcome::Skipped,
            skip: Some(skip),
            match_count: 0,
        }
    }
}

/// The result of running a whole rule set against a sample.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyRun {
    /// the verdict on the sample: `blocked` if any rule blocked, `redacted` if
    /// the text changed, `annotated` if only tallies were left, `skipped` if no
    /// rule looked at it, otherwise `no_match`
    pub outcome: DryRunOutcome,
    /// one entry per rule, in the order the scan walks them
    pub steps: Vec<PolicyStep>,
    /// the rule that rejected the request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
    /// the sample after every redaction; present only when it changed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted: Option<String>,
}

/// Run every rule of `config` against `text` at `stage`, in order, with the
/// scan the gateway runs on a request.
///
/// The steps come from [`CompiledGuardrails`]' own scan loop, so redactions
/// feeding later rules, a block ending the scan and the system-message and
/// scan-budget exemptions cannot drift from the request path. Route overrides
/// are not applied: the answer is for the policy, not for one route. Nothing
/// here logs or retains `text`.
pub fn dry_run_policy(
    config: &GuardrailsConfig,
    stage: GuardStage,
    text: &str,
    is_system: bool,
) -> PolicyRun {
    // the gateway drops a rule that does not compile, so the trace has to line
    // up the surviving rules with the configured ones by hand
    let compiled: Vec<Option<CompiledRule>> =
        config.rules.iter().map(CompiledRule::from_config).collect();
    let guard = CompiledGuardrails {
        enabled: config.enabled,
        max_scan_bytes: config.max_scan_bytes.unwrap_or(DEFAULT_MAX_SCAN_BYTES),
        streaming_post_call: config.streaming_post_call,
        rules: compiled.iter().flatten().cloned().collect(),
    };

    let mut trace = Vec::new();
    let mut budget = guard.scan_budget();
    let mut report = GuardrailReport::default();
    let scanned = guard.scan_stage_traced(
        text,
        stage,
        is_system,
        &mut budget,
        &mut report,
        &RuleSelection::default(),
        Some(&mut trace),
    );

    // a scan that bailed out before its loop recorded nothing; say why
    let blanket = if !config.enabled {
        Some(DryRunSkip::PolicyDisabled)
    } else if text.len() > guard.scan_budget() {
        Some(DryRunSkip::OverScanBudget)
    } else {
        None
    };
    let mut walked = trace.into_iter();
    let mut blocked = false;
    let steps: Vec<PolicyStep> = config
        .rules
        .iter()
        .zip(&compiled)
        .map(|(rule, compiled)| {
            if compiled.is_none() {
                return PolicyStep::unreached(rule, DryRunSkip::NotCompiled);
            }
            if let Some(skip) = blanket {
                return PolicyStep::unreached(rule, skip);
            }
            if blocked {
                return PolicyStep::unreached(rule, DryRunSkip::AfterBlock);
            }
            match walked.next() {
                Some(step) => {
                    blocked = step.outcome == DryRunOutcome::Blocked;
                    step
                }
                None => PolicyStep::unreached(rule, DryRunSkip::AfterBlock),
            }
        })
        .collect();

    let (outcome, blocked_by, redacted) = match scanned {
        ScanOutcome::Blocked(rule) => (DryRunOutcome::Blocked, Some(rule), None),
        ScanOutcome::Redacted(out) => (DryRunOutcome::Redacted, None, Some(out)),
        ScanOutcome::Unchanged if report.matched() => (DryRunOutcome::Annotated, None, None),
        ScanOutcome::Unchanged
            if !steps.is_empty() && steps.iter().all(|s| s.outcome == DryRunOutcome::Skipped) =>
        {
            (DryRunOutcome::Skipped, None, None)
        }
        ScanOutcome::Unchanged => (DryRunOutcome::NoMatch, None, None),
    };
    PolicyRun {
        outcome,
        steps,
        blocked_by,
        redacted,
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
    fn empty_or_blank_replacement_uses_the_default_token() {
        for blank in ["", "   "] {
            let mut r = rule("email", BuiltinRule::Email, GuardAction::Redact);
            r.replacement = Some(blank.to_string());
            let g = compiled(vec![r]);
            let mut budget = g.scan_budget();
            let mut report = GuardrailReport::default();
            let out = g.scan_segment("a@example.com", false, &mut budget, &mut report);
            assert_eq!(out, ScanOutcome::Redacted("[REDACTED:EMAIL]".to_string()));
        }
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
        // the only trace an annotate rule leaves, once however often it matched
        let again = g.scan_segment("x@y.io z@y.io", false, &mut budget, &mut report);
        assert_eq!(again, ScanOutcome::Unchanged);
        assert_eq!(report.events(), vec!["annotated:email".to_string()]);
    }

    #[test]
    fn events_name_each_rule_once_by_what_it_did() {
        let g = compiled(vec![
            rule("email", BuiltinRule::Email, GuardAction::Redact),
            rule("phone", BuiltinRule::Phone, GuardAction::Annotate),
        ]);
        let mut budget = g.scan_budget();
        let mut report = GuardrailReport::default();
        g.scan_segment("a@b.co and c@d.co", false, &mut budget, &mut report);
        g.scan_segment("e@f.co", false, &mut budget, &mut report);
        assert_eq!(report.events(), vec!["redacted:email".to_string()]);
        assert!(!report.events().iter().any(|e| e.contains("@")));
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
    fn route_override_ignores_surrounding_whitespace() {
        let route = RouteGuardrails {
            disable: vec!["pii ".to_string()],
            enable: vec![" card".to_string()],
        };
        assert!(!route.allows("pii"));
        assert!(!route.allows(" pii "));
        assert!(route.allows("card"));
        assert!(route
            .unknown_rules(&["pii ".to_string(), "card".to_string()])
            .is_empty());
        assert!(route.unknown_rules(&["pii".to_string()]).len() == 1);
    }

    #[test]
    fn padded_rule_is_disabled_by_padded_override() {
        let g = compiled(vec![rule("pii ", BuiltinRule::Email, GuardAction::Redact)]);
        let route = RouteGuardrails {
            disable: vec!["pii ".to_string()],
            enable: Vec::new(),
        };
        assert!(!g.resolve_selection(&route).is_unrestricted());
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

    fn draft(pattern: &str, action: GuardAction) -> GuardrailRule {
        GuardrailRule {
            name: "draft".to_string(),
            builtin: None,
            pattern: Some(pattern.to_string()),
            stage: GuardStage::PreCall,
            action,
            replacement: None,
            include_system: false,
        }
    }

    #[test]
    fn dry_run_reports_each_action_like_the_request_path() {
        let text = "mail a@b.io and c@d.io today";
        let mut email = rule("email", BuiltinRule::Email, GuardAction::Redact);
        let run = dry_run_rule(&email, text, false).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Redacted);
        assert_eq!(run.match_count, 2);
        assert_eq!(
            run.spans,
            vec![
                DryRunSpan { start: 5, end: 11 },
                DryRunSpan { start: 16, end: 22 }
            ]
        );
        assert_eq!(
            run.redacted.as_deref(),
            Some("mail [REDACTED:EMAIL] and [REDACTED:EMAIL] today")
        );

        email.action = GuardAction::Block;
        let run = dry_run_rule(&email, text, false).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Blocked);
        assert!(run.redacted.is_none());

        email.action = GuardAction::Annotate;
        let run = dry_run_rule(&email, text, false).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Annotated);
        assert!(run.redacted.is_none());

        let run = dry_run_rule(&email, "nothing here", false).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::NoMatch);
        assert_eq!(run.match_count, 0);
    }

    #[test]
    fn dry_run_spans_count_utf16_units() {
        // the emoji is one char, four utf-8 bytes and two utf-16 units
        let run =
            dry_run_rule(&draft("secret", GuardAction::Annotate), "😀 secret", false).unwrap();
        assert_eq!(run.spans, vec![DryRunSpan { start: 3, end: 9 }]);
    }

    #[test]
    fn dry_run_refuses_what_validation_refuses() {
        let problems = dry_run_rule(&draft("[", GuardAction::Block), "x", false).unwrap_err();
        assert!(problems.join(" ").contains("invalid or unbounded pattern"));
        let mut both = draft("x", GuardAction::Block);
        both.builtin = Some(BuiltinRule::Email);
        assert!(dry_run_rule(&both, "x", false).is_err());
    }

    #[test]
    fn dry_run_skips_system_text_unless_the_rule_opts_in() {
        let mut rule = draft("secret", GuardAction::Block);
        let run = dry_run_rule(&rule, "secret", true).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Skipped);
        assert_eq!(run.skip, Some(DryRunSkip::SystemMessage));
        rule.include_system = true;
        let run = dry_run_rule(&rule, "secret", true).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Blocked);
    }

    #[test]
    fn dry_run_skips_text_past_the_scan_budget() {
        let text = "x".repeat(DEFAULT_MAX_SCAN_BYTES + 1);
        let run = dry_run_rule(&draft("x", GuardAction::Block), &text, false).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Skipped);
        assert_eq!(run.skip, Some(DryRunSkip::OverScanBudget));
    }

    #[test]
    fn dry_run_caps_the_span_list_but_not_the_count() {
        let text = "x ".repeat(MAX_DRY_RUN_SPANS + 50);
        let run = dry_run_rule(&draft("x", GuardAction::Annotate), &text, false).unwrap();
        assert_eq!(run.match_count, MAX_DRY_RUN_SPANS + 50);
        assert_eq!(run.spans.len(), MAX_DRY_RUN_SPANS);
    }

    #[test]
    fn dry_run_applies_the_post_call_stage_to_output() {
        let mut post = draft("secret", GuardAction::Redact);
        post.stage = GuardStage::PostCall;
        // system exemption is an input-side rule and does not apply to output
        let run = dry_run_rule(&post, "a secret", true).unwrap();
        assert_eq!(run.outcome, DryRunOutcome::Redacted);
    }

    fn policy(rules: Vec<GuardrailRule>) -> GuardrailsConfig {
        GuardrailsConfig {
            enabled: true,
            rules,
            ..GuardrailsConfig::default()
        }
    }

    #[test]
    fn policy_dry_run_feeds_redactions_to_later_rules() {
        // the redact rule hides the email, so the block rule behind it has
        // nothing left to match
        let config = policy(vec![
            rule("mask", BuiltinRule::Email, GuardAction::Redact),
            rule("deny", BuiltinRule::Email, GuardAction::Block),
        ]);
        let run = dry_run_policy(&config, GuardStage::PreCall, "mail a@b.io", false);
        assert_eq!(run.outcome, DryRunOutcome::Redacted);
        assert_eq!(run.steps[0].outcome, DryRunOutcome::Redacted);
        assert_eq!(run.steps[0].match_count, 1);
        assert_eq!(run.steps[1].outcome, DryRunOutcome::NoMatch);
        assert_eq!(run.redacted.as_deref(), Some("mail [REDACTED:EMAIL]"));
        assert_eq!(run.blocked_by, None);
    }

    #[test]
    fn policy_dry_run_stops_at_the_first_block() {
        let config = policy(vec![
            rule("deny", BuiltinRule::Email, GuardAction::Block),
            rule("mask", BuiltinRule::Email, GuardAction::Redact),
        ]);
        let run = dry_run_policy(&config, GuardStage::PreCall, "a@b.io", false);
        assert_eq!(run.outcome, DryRunOutcome::Blocked);
        assert_eq!(run.blocked_by.as_deref(), Some("deny"));
        assert_eq!(run.steps[1].outcome, DryRunOutcome::Skipped);
        assert_eq!(run.steps[1].skip, Some(DryRunSkip::AfterBlock));
        assert_eq!(run.redacted, None);
    }

    #[test]
    fn policy_dry_run_agrees_with_the_request_path_scan() {
        let config = policy(vec![
            rule("note", BuiltinRule::Phone, GuardAction::Annotate),
            rule("mask", BuiltinRule::Email, GuardAction::Redact),
        ]);
        let text = "call +1 415 555 0100 or a@b.io";
        let run = dry_run_policy(&config, GuardStage::PreCall, text, false);
        let guard = CompiledGuardrails::from_config(&config);
        let mut report = GuardrailReport::default();
        let mut budget = guard.scan_budget();
        let scanned = guard.scan_segment(text, false, &mut budget, &mut report);
        assert_eq!(
            scanned,
            ScanOutcome::Redacted(run.redacted.clone().unwrap())
        );
        assert_eq!(
            run.steps.iter().map(|s| s.match_count).sum::<usize>(),
            report.hits.iter().map(|(_, n)| n).sum::<usize>()
        );
    }

    #[test]
    fn policy_dry_run_labels_why_a_rule_did_not_run() {
        let mut post = rule("out", BuiltinRule::Email, GuardAction::Redact);
        post.stage = GuardStage::PostCall;
        let config = policy(vec![
            post,
            rule("deny", BuiltinRule::Email, GuardAction::Block),
        ]);
        let run = dry_run_policy(&config, GuardStage::PreCall, "a@b.io", true);
        assert_eq!(run.steps[0].skip, Some(DryRunSkip::OtherStage));
        assert_eq!(run.steps[1].skip, Some(DryRunSkip::SystemMessage));
        assert_eq!(run.outcome, DryRunOutcome::Skipped);

        let mut off = config.clone();
        off.enabled = false;
        let run = dry_run_policy(&off, GuardStage::PreCall, "a@b.io", false);
        assert!(run
            .steps
            .iter()
            .all(|s| s.skip == Some(DryRunSkip::PolicyDisabled)));

        let text = "x".repeat(DEFAULT_MAX_SCAN_BYTES + 1);
        let run = dry_run_policy(&config, GuardStage::PreCall, &text, false);
        assert_eq!(run.steps[1].skip, Some(DryRunSkip::OverScanBudget));
    }

    #[test]
    fn policy_dry_run_lines_steps_up_when_a_rule_does_not_compile() {
        let mut broken = rule("broken", BuiltinRule::Email, GuardAction::Block);
        broken.builtin = None;
        broken.pattern = Some("[".to_string());
        let config = policy(vec![
            broken,
            rule("mask", BuiltinRule::Email, GuardAction::Redact),
        ]);
        let run = dry_run_policy(&config, GuardStage::PreCall, "a@b.io", false);
        assert_eq!(run.steps[0].skip, Some(DryRunSkip::NotCompiled));
        assert_eq!(run.steps[1].outcome, DryRunOutcome::Redacted);
    }

    #[test]
    fn an_empty_policy_matches_nothing() {
        let run = dry_run_policy(&policy(Vec::new()), GuardStage::PreCall, "x", false);
        assert_eq!(run.outcome, DryRunOutcome::NoMatch);
        assert!(run.steps.is_empty());
    }
}
