//! Value redaction for captured request and response payloads (#1835).
//!
//! `redact_fields` can only blank the value of a JSON key it knows by name. The
//! secrets that actually reach LLM traffic sit inside free text — a key pasted
//! into a prompt, a connection string in a tool argument — so the capture path
//! also scans string values for credential shapes and replaces each match with
//! a typed marker such as `[redacted:aws_key]`.
//!
//! The policy is compiled once into a [`PayloadRedactor`] (per snapshot in the
//! gateway, per request in the control plane's preview) and applied before the
//! body is truncated or written anywhere, so an unredacted copy never reaches
//! the log writer. Matching uses the `regex` crate, which is linear in the
//! input and has no backtracking, and every operator-supplied pattern is bound
//! by [`MAX_PATTERN_LEN`], [`MAX_CUSTOM_PATTERNS`] and a compiled-size limit.

use std::borrow::Cow;
use std::collections::BTreeMap;

use regex::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What the value of a redacted JSON key is replaced with.
pub const FIELD_MARKER: &str = "[REDACTED]";

/// The longest operator-supplied pattern, in bytes.
pub const MAX_PATTERN_LEN: usize = 256;

/// How many operator-supplied patterns one policy may carry.
pub const MAX_CUSTOM_PATTERNS: usize = 16;

/// Compiled-program size ceiling for one pattern, in bytes. Bounds the memory a
/// hostile pattern such as `(\w{100}){100}` may claim before it is rejected.
const REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// Lazy-DFA cache ceiling for one pattern, in bytes.
const REGEX_DFA_SIZE_LIMIT: usize = 1024 * 1024;

/// A built-in credential shape.
struct Detector {
    /// the name an operator enables it by and the marker it leaves behind
    name: &'static str,
    pattern: &'static str,
    /// replacement template; `None` replaces the whole match with the marker
    keep: Option<&'static str>,
}

/// The built-in detectors, in the order they are applied. More specific shapes
/// come first so a generic one never claims their prefix.
const DETECTORS: &[Detector] = &[
    Detector {
        name: "private_key",
        pattern: r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
        keep: None,
    },
    Detector {
        name: "anthropic_key",
        pattern: r"\bsk-ant-[A-Za-z0-9_-]{20,}",
        keep: None,
    },
    Detector {
        name: "openai_key",
        pattern: r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}",
        keep: None,
    },
    Detector {
        name: "google_api_key",
        pattern: r"\bAIza[0-9A-Za-z_-]{35}",
        keep: None,
    },
    Detector {
        name: "github_token",
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})",
        keep: None,
    },
    Detector {
        name: "slack_token",
        pattern: r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        keep: None,
    },
    Detector {
        name: "stripe_key",
        pattern: r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}",
        keep: None,
    },
    Detector {
        name: "aws_key",
        pattern: r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        keep: None,
    },
    Detector {
        name: "jwt",
        pattern: r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        keep: None,
    },
    Detector {
        name: "bearer_token",
        pattern: r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{20,}",
        keep: None,
    },
    // scheme://user:password@host — only the password goes, so the reader
    // still sees which service and account the string pointed at
    Detector {
        name: "connection_string",
        pattern: r#"\b([A-Za-z][A-Za-z0-9+.-]{1,20}://[^\s:/@"'<>]+:)[^\s@/"'<>]+(@)"#,
        keep: Some("${1}[redacted:connection_string]${2}"),
    },
];

/// The name of every built-in detector, in application order.
pub fn detector_names() -> Vec<String> {
    DETECTORS.iter().map(|d| d.name.to_string()).collect()
}

/// An operator-supplied value pattern.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RedactPattern {
    /// short label, `[a-z0-9_]{1,32}`; it becomes the marker `[redacted:<name>]`
    pub name: String,
    /// regular expression in the `regex` crate's syntax (no look-around, no
    /// backreferences); the whole match is replaced
    pub pattern: String,
}

/// Why a policy could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RedactionPolicyError {
    #[error("unknown detector `{0}`")]
    UnknownDetector(String),
    #[error("at most {MAX_CUSTOM_PATTERNS} custom patterns are allowed")]
    TooManyPatterns,
    #[error("pattern name `{0}` must be 1-32 characters of a-z, 0-9 or _")]
    BadPatternName(String),
    #[error("pattern `{0}` is longer than {MAX_PATTERN_LEN} bytes")]
    PatternTooLong(String),
    #[error("pattern `{name}` does not compile: {reason}")]
    BadPattern { name: String, reason: String },
}

impl RedactionPolicyError {
    /// The request field the error belongs to, for field-level validation.
    pub fn field(&self) -> &'static str {
        match self {
            Self::UnknownDetector(_) => "payload_capture_redact_detectors",
            _ => "payload_capture_redact_patterns",
        }
    }
}

/// One compiled rule.
#[derive(Debug)]
struct Rule {
    name: String,
    regex: Regex,
    /// replacement template, expanded with the match's capture groups
    template: String,
}

/// What a redaction pass found, by kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RedactionReport {
    /// JSON values blanked because their key is in `redact_fields`
    pub fields: u32,
    /// value matches per detector or custom pattern name
    pub detections: BTreeMap<String, u32>,
}

impl RedactionReport {
    /// Whether anything was redacted.
    pub fn is_empty(&self) -> bool {
        self.fields == 0 && self.detections.is_empty()
    }

    fn found(&mut self, name: &str, count: u32) {
        if count > 0 {
            *self.detections.entry(name.to_string()).or_default() += count;
        }
    }
}

/// A compiled payload-redaction policy.
///
/// `Default` is the empty policy, which redacts nothing.
#[derive(Debug, Default)]
pub struct PayloadRedactor {
    fields: Vec<String>,
    rules: Vec<Rule>,
    /// one pass over a string tells which rules can match, so a body that holds
    /// no credential costs one scan rather than one per rule
    prefilter: Option<RegexSet>,
}

impl PayloadRedactor {
    /// Compile a policy: JSON keys to blank, enabled built-in detectors and
    /// operator patterns.
    pub fn compile(
        fields: &[String],
        detectors: &[String],
        patterns: &[RedactPattern],
    ) -> Result<Self, RedactionPolicyError> {
        validate_patterns(patterns)?;
        for name in detectors {
            if !DETECTORS.iter().any(|d| d.name == name) {
                return Err(RedactionPolicyError::UnknownDetector(name.clone()));
            }
        }
        let mut rules = Vec::new();
        // application order, not the order the operator listed them in
        for detector in DETECTORS {
            if detectors.iter().any(|name| name == detector.name) {
                let regex =
                    build(detector.pattern).map_err(|reason| RedactionPolicyError::BadPattern {
                        name: detector.name.to_string(),
                        reason,
                    })?;
                rules.push(Rule {
                    name: detector.name.to_string(),
                    regex,
                    template: detector
                        .keep
                        .map(str::to_string)
                        .unwrap_or_else(|| marker(detector.name)),
                });
            }
        }
        for pattern in patterns {
            let regex =
                build(&pattern.pattern).map_err(|reason| RedactionPolicyError::BadPattern {
                    name: pattern.name.clone(),
                    reason,
                })?;
            rules.push(Rule {
                name: pattern.name.clone(),
                regex,
                template: marker(&pattern.name),
            });
        }
        let prefilter = if rules.is_empty() {
            None
        } else {
            let sources = rules.iter().map(|rule| rule.regex.as_str());
            Some(
                RegexSetBuilder::new(sources)
                    .size_limit(REGEX_SIZE_LIMIT * rules.len())
                    .dfa_size_limit(REGEX_DFA_SIZE_LIMIT * rules.len())
                    .build()
                    .map_err(|err| RedactionPolicyError::BadPattern {
                        name: "policy".to_string(),
                        reason: err.to_string(),
                    })?,
            )
        };
        Ok(Self {
            fields: fields.to_vec(),
            rules,
            prefilter,
        })
    }

    /// A redactor that only blanks keys. Also the fallback when a stored value
    /// policy no longer compiles: the key rule must keep holding.
    pub fn fields_only(fields: &[String]) -> Self {
        Self {
            fields: fields.to_vec(),
            rules: Vec::new(),
            prefilter: None,
        }
    }

    /// Whether this policy redacts nothing at all.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.rules.is_empty()
    }

    /// Redact a free-text string in place of its matches.
    pub fn redact_text<'a>(&self, text: &'a str, report: &mut RedactionReport) -> Cow<'a, str> {
        let Some(prefilter) = &self.prefilter else {
            return Cow::Borrowed(text);
        };
        let hits = prefilter.matches(text);
        if !hits.matched_any() {
            return Cow::Borrowed(text);
        }
        let mut current = Cow::Borrowed(text);
        for index in hits.iter() {
            let rule = &self.rules[index];
            let mut count = 0u32;
            let replaced = rule
                .regex
                .replace_all(&current, |caps: &regex::Captures<'_>| {
                    count += 1;
                    let mut out = String::new();
                    caps.expand(&rule.template, &mut out);
                    out
                });
            if count > 0 {
                report.found(&rule.name, count);
                current = Cow::Owned(replaced.into_owned());
            }
        }
        current
    }

    /// Redact a parsed JSON document: blank the configured keys, then scan every
    /// remaining string value.
    pub fn redact_json(&self, value: &mut Value, report: &mut RedactionReport) {
        match value {
            Value::Object(object) => {
                for (key, nested) in object.iter_mut() {
                    if self.fields.iter().any(|f| f.eq_ignore_ascii_case(key)) {
                        *nested = Value::String(FIELD_MARKER.to_string());
                        report.fields += 1;
                    } else {
                        self.redact_json(nested, report);
                    }
                }
            }
            Value::Array(values) => {
                for nested in values {
                    self.redact_json(nested, report);
                }
            }
            Value::String(text) => {
                if let Cow::Owned(redacted) = self.redact_text(text, report) {
                    *text = redacted;
                }
            }
            _ => {}
        }
    }

    /// Render a body for the capture store: redacted first, then truncated to
    /// `max_bytes`, so a credential can never survive at the edge of the kept
    /// prefix. JSON is redacted structurally; anything else as UTF-8 text.
    pub fn capture(&self, body: &[u8], max_bytes: usize, report: &mut RedactionReport) -> String {
        if max_bytes == 0 || body.is_empty() {
            return String::new();
        }
        let mut rendered = match serde_json::from_slice::<Value>(body) {
            Ok(mut json) => {
                self.redact_json(&mut json, report);
                serde_json::to_string(&json).unwrap_or_default()
            }
            Err(_) => {
                let text = String::from_utf8_lossy(body);
                self.redact_text(&text, report).into_owned()
            }
        };
        if rendered.len() > max_bytes {
            let end = rendered.floor_char_boundary(max_bytes);
            rendered.truncate(end);
            rendered.push_str("…[truncated]");
        }
        rendered
    }
}

fn marker(name: &str) -> String {
    format!("[redacted:{name}]")
}

fn build(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .nest_limit(20)
        .build()
        .map_err(|err| {
            // the regex crate's message quotes the whole pattern; the first
            // line is the reason
            err.to_string()
                .lines()
                .last()
                .unwrap_or("invalid")
                .to_string()
        })
}

/// Check the operator patterns without compiling them: count, names, length.
pub fn validate_patterns(patterns: &[RedactPattern]) -> Result<(), RedactionPolicyError> {
    if patterns.len() > MAX_CUSTOM_PATTERNS {
        return Err(RedactionPolicyError::TooManyPatterns);
    }
    for pattern in patterns {
        let name_ok = !pattern.name.is_empty()
            && pattern.name.len() <= 32
            && pattern
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !name_ok {
            return Err(RedactionPolicyError::BadPatternName(pattern.name.clone()));
        }
        if pattern.pattern.is_empty() || pattern.pattern.len() > MAX_PATTERN_LEN {
            return Err(RedactionPolicyError::PatternTooLong(pattern.name.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> PayloadRedactor {
        PayloadRedactor::compile(&["password".to_string()], &detector_names(), &[]).unwrap()
    }

    fn redact(text: &str) -> String {
        all()
            .redact_text(text, &mut RedactionReport::default())
            .into_owned()
    }

    #[test]
    fn each_builtin_detector_replaces_its_credential_with_a_typed_marker() {
        // the shapes a secret scanner also knows are split with `concat!`, so the
        // source carries no literal that gitleaks or GitGuardian reads as a key
        let cases = [
            ("use AKIAIOSFODNN7EXAMPLE now", "aws_key"),
            ("key sk-proj-abcdefghijklmnopqrstuvwx", "openai_key"),
            ("key sk-ant-api03-abcdefghijklmnopqrstuv", "anthropic_key"),
            (
                concat!("AIza", "SyA1234567890abcdefghijklmnopqrstuv"),
                "google_api_key",
            ),
            ("ghp_abcdefghijklmnopqrstuvwxyz0123456789", "github_token"),
            ("xoxb-1234567890-abcdefghij", "slack_token"),
            (concat!("sk_", "live_abcdefghijklmnop"), "stripe_key"),
            (
                concat!(
                    "eyJhbGciOiJIUzI1NiJ9",
                    ".eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP"
                ),
                "jwt",
            ),
            (
                concat!("Authorization: Bearer", " abcdefghijklmnopqrstuvwxyz"),
                "bearer_token",
            ),
            (
                concat!(
                    "-----BEGIN RSA PRIVATE",
                    " KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----"
                ),
                "private_key",
            ),
        ];
        for (input, kind) in cases {
            let out = redact(input);
            assert!(
                out.contains(&format!("[redacted:{kind}]")),
                "{kind}: {input} -> {out}"
            );
        }
    }

    #[test]
    fn an_anthropic_key_is_not_claimed_by_the_generic_openai_shape() {
        let out = redact("sk-ant-api03-abcdefghijklmnopqrstuv");
        assert_eq!(out, "[redacted:anthropic_key]");
    }

    #[test]
    fn a_connection_string_loses_only_its_password() {
        assert_eq!(
            redact("postgres://app:hunter2@db.internal:5432/prod"),
            "postgres://app:[redacted:connection_string]@db.internal:5432/prod"
        );
    }

    #[test]
    fn a_truncated_private_key_block_is_still_redacted_to_the_end() {
        let out = redact("see -----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg");
        assert_eq!(out, "see [redacted:private_key]");
    }

    #[test]
    fn ordinary_prose_and_code_pass_through_untouched() {
        let text = "Bearer authentication is described in RFC 6750; set sk to 4 and AKIA later.";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn json_keys_and_nested_string_values_are_both_redacted() {
        let redactor = all();
        let mut report = RedactionReport::default();
        let out = redactor.capture(
            br#"{"password":"p","messages":[{"content":"mine is AKIAIOSFODNN7EXAMPLE ok"}]}"#,
            4096,
            &mut report,
        );
        assert_eq!(
            out,
            r#"{"messages":[{"content":"mine is [redacted:aws_key] ok"}],"password":"[REDACTED]"}"#
        );
        assert_eq!(report.fields, 1);
        assert_eq!(report.detections["aws_key"], 1);
    }

    #[test]
    fn redaction_runs_before_truncation() {
        let key = "AKIAIOSFODNN7EXAMPLE";
        let body = format!(r#"{{"c":"{} {key}"}}"#, "x".repeat(10));
        let out = all().capture(body.as_bytes(), 24, &mut RedactionReport::default());
        assert!(!out.contains("AKIA"), "{out}");
    }

    #[test]
    fn a_non_json_body_is_scanned_as_text() {
        let out = all().capture(
            b"data: key AKIAIOSFODNN7EXAMPLE\n\n",
            4096,
            &mut RedactionReport::default(),
        );
        assert!(out.contains("[redacted:aws_key]"), "{out}");
    }

    #[test]
    fn a_custom_pattern_leaves_its_own_marker() {
        let redactor = PayloadRedactor::compile(
            &[],
            &[],
            &[RedactPattern {
                name: "employee_id".into(),
                pattern: r"EMP-\d{6}".into(),
            }],
        )
        .unwrap();
        let out = redactor.redact_text("badge EMP-123456 ok", &mut RedactionReport::default());
        assert_eq!(out, "badge [redacted:employee_id] ok");
    }

    #[test]
    fn an_unknown_detector_and_a_bad_pattern_are_rejected() {
        assert_eq!(
            PayloadRedactor::compile(&[], &["nope".into()], &[]).unwrap_err(),
            RedactionPolicyError::UnknownDetector("nope".into())
        );
        let bad = |name: &str, pattern: &str| {
            PayloadRedactor::compile(
                &[],
                &[],
                &[RedactPattern {
                    name: name.into(),
                    pattern: pattern.into(),
                }],
            )
            .unwrap_err()
        };
        assert!(matches!(
            bad("x", "(unclosed"),
            RedactionPolicyError::BadPattern { .. }
        ));
        assert!(matches!(
            bad("Bad Name", "a"),
            RedactionPolicyError::BadPatternName(_)
        ));
        assert!(matches!(
            bad("long", &"a".repeat(MAX_PATTERN_LEN + 1)),
            RedactionPolicyError::PatternTooLong(_)
        ));
        // look-around is outside the linear-time engine
        assert!(matches!(
            bad("look", "(?=a)a"),
            RedactionPolicyError::BadPattern { .. }
        ));
        // a program past the size limit is refused rather than built
        assert!(matches!(
            bad("big", r"(\w{50}){50}"),
            RedactionPolicyError::BadPattern { .. }
        ));
    }

    #[test]
    fn too_many_custom_patterns_are_rejected() {
        let many: Vec<_> = (0..=MAX_CUSTOM_PATTERNS)
            .map(|i| RedactPattern {
                name: format!("p{i}"),
                pattern: "a".into(),
            })
            .collect();
        assert_eq!(
            PayloadRedactor::compile(&[], &[], &many).unwrap_err(),
            RedactionPolicyError::TooManyPatterns
        );
    }

    #[test]
    fn an_empty_policy_changes_nothing() {
        let redactor = PayloadRedactor::default();
        assert!(redactor.is_empty());
        let out = redactor.capture(
            b"AKIAIOSFODNN7EXAMPLE",
            100,
            &mut RedactionReport::default(),
        );
        assert_eq!(out, "AKIAIOSFODNN7EXAMPLE");
    }
}
