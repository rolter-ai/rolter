//! External PII sanitizer contract and configuration (#848).
//!
//! The custom guardrail webhook (ROL-257) returns a *decision* — allow, block,
//! transform. A PII sanitizer has a different job: it returns **transformed
//! content**, and optionally undoes the transform on the way back. That is a
//! distinct contract, which is why it is a distinct module rather than another
//! `action` on [`crate::guardrail_webhook::WebhookDecision`].
//!
//! # Where the plaintext lives
//!
//! Nowhere in rolter. The sanitizer service replaces each detected entity with
//! a deterministic placeholder and keeps the placeholder→plaintext mapping on
//! its own side, handing back an **opaque restoration token**. To restore, the
//! gateway posts that token and the placeholder-bearing text back to the
//! service.
//!
//! This is the whole privacy argument. The gateway never holds a mapping it
//! could leak, so "never persist plaintext mappings in logs, metrics or traces"
//! is true by construction rather than by discipline: there is no plaintext
//! mapping in the process to persist. Findings carry entity types, counts and
//! placeholders — never matched values.
//!
//! # Restoration is a policy, not a default
//!
//! Restoring PII into a response undoes the protection, so it is off unless an
//! operator asks for it, and a token is bound to the tenant and route that
//! produced it — see [`RestorationPolicy`] and [`TokenScope`].
//!
//! This module owns the config model, validation and the wire contract. The
//! async HTTP client and request-path wiring live in the gateway.

use serde::{Deserialize, Serialize};

pub use crate::guardrail_webhook::{FailureMode, WebhookAuth, WebhookTenant};

/// Default per-call timeout, in milliseconds. Higher than the guardrail
/// webhook's: entity recognition over a long prompt is real NLP work, not a
/// classifier lookup, and a Presidio analyze call over a few KB routinely
/// exceeds 2s on modest hardware.
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// Default cap on the content bytes forwarded to the sanitizer.
pub const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024;

/// Header a caller sets to authorize restoration under
/// [`RestorationPolicy::CallerAuthorized`].
pub const RESTORE_HEADER: &str = "x-rolter-pii-restore";

/// Which legs of the exchange the sanitizer sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SanitizeDirection {
    /// sanitize the request before it reaches the provider. the common case:
    /// the point is to keep PII away from a third-party model
    #[default]
    Request,
    /// sanitize the response before it reaches the caller. for a provider that
    /// may echo or generate PII
    Response,
    /// both legs
    Both,
    /// a direction this build does not know (#3347); read as
    /// [`SanitizeDirection::Both`]
    #[serde(other)]
    Unknown,
}

impl SanitizeDirection {
    #[must_use]
    pub fn covers_request(self) -> bool {
        matches!(self, Self::Request | Self::Both)
    }

    #[must_use]
    pub fn covers_response(self) -> bool {
        matches!(self, Self::Response | Self::Both)
    }
}

/// What happens to a **streamed** response when the sanitizer's response leg
/// is active (response sanitization, restoration, or both).
///
/// The buffered response leg needs the whole body: an entity — or a
/// placeholder — can straddle any SSE chunk boundary, and neither detection
/// nor reversal is correct on a fragment. Two ways round that are ruled out
/// for good (ADR `2026-10-27-pii-streaming-restoration`):
///
/// - a restore round trip **per chunk** — one network call per token, which
///   caps the stream at one frame per round trip;
/// - holding the placeholder→plaintext **mapping in the gateway** so chunks can
///   be reversed locally — which destroys the property the whole design rests
///   on, that rolter never holds the mapping.
///
/// What remains is refusing, serving the stream with the response leg not
/// applied, or — for restoration only — piping the stream's text through the
/// sanitizer's streaming restore endpoint, which holds the mapping and the
/// placeholder holdback on its own side. This mirrors
/// [`crate::guardrails::StreamingPostCall`] for the first two. The *request*
/// leg is unaffected either way: a streamed request is still sanitized before
/// it reaches the provider, which is the case that matters most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingResponse {
    /// refuse the request with an error naming the stage.
    ///
    /// The default, because the alternative silently delivers a response the
    /// operator believes was sanitized and was not.
    #[default]
    Reject,
    /// serve the stream with the response leg not applied. Request-leg
    /// sanitization still runs, so PII still does not reach the provider —
    /// only provider-generated PII in the response is unchecked, and
    /// placeholders reach the caller unrestored.
    Passthrough,
    /// restore the stream through `restore_stream_url`: the gateway sends the
    /// text of each delta to the sanitizer over one full-duplex request per
    /// stream and forwards what comes back (#911).
    ///
    /// Applies to restoration only. A stream whose response leg also
    /// *sanitizes* (`direction` covers the response) is refused as under
    /// [`Self::Reject`], because detecting entities in a stream needs context
    /// the placeholder holdback does not give.
    Restore,
    /// a value this build does not know (#3347); read as
    /// [`StreamingResponse::Reject`]
    #[serde(other)]
    Unknown,
}

/// When the gateway may turn placeholders back into plaintext.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorationPolicy {
    /// never restore. placeholders reach the caller.
    ///
    /// The default, because restoring is the operation that undoes the
    /// protection: a deployment that has not thought about it should get the
    /// safe behaviour, not the convenient one.
    #[default]
    Never,
    /// restore automatically before the response leaves the gateway.
    ///
    /// Named for what it assumes: the caller is the party that supplied the PII
    /// in the first place, so returning it to them discloses nothing new. Only
    /// correct when the gateway's own clients are trusted — an internal service
    /// mesh, not a public API.
    TrustedDownstream,
    /// restore only when the caller explicitly asked, via the
    /// [`RESTORE_HEADER`] request header.
    ///
    /// Lets one deployment serve both trusted and untrusted callers: the
    /// default stays redacted and a client opts in per request.
    CallerAuthorized,
    /// a policy this build does not know (#3347); read as
    /// [`RestorationPolicy::Never`]
    #[serde(other)]
    Unknown,
}

impl RestorationPolicy {
    /// Whether a response should be restored, given whether the caller asked.
    ///
    /// The caller's request is *only* consulted under `CallerAuthorized`: under
    /// `Never` a caller-set header must not be able to talk the gateway into
    /// disclosure, which is the whole point of the policy being operator-owned.
    #[must_use]
    pub fn should_restore(self, caller_requested: bool) -> bool {
        match self {
            Self::Never | Self::Unknown => false,
            Self::TrustedDownstream => true,
            Self::CallerAuthorized => caller_requested,
        }
    }
}

/// The tenant and route a restoration token belongs to.
///
/// A token is only ever valid for the exchange that produced it. Binding it to
/// a scope makes a cross-tenant restore impossible to reach even if a token
/// leaks or a future change starts caching them: the check is on the value, not
/// on the control flow that happens to surround it today.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenScope {
    pub org: String,
    pub team: String,
    pub project: String,
    pub route: String,
}

impl TokenScope {
    /// Whether a token issued for `self` may be redeemed in `other`.
    #[must_use]
    pub fn allows(&self, other: &TokenScope) -> bool {
        self == other
    }
}

/// An opaque restoration token plus the scope it is valid in.
///
/// The token's contents are the sanitizer's business; rolter treats it as
/// bytes. It is never logged, never recorded on a span, and never returned to
/// the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorationTicket {
    token: String,
    scope: TokenScope,
}

impl RestorationTicket {
    #[must_use]
    pub fn new(token: String, scope: TokenScope) -> Self {
        Self { token, scope }
    }

    /// The token, but only for the scope it was issued in. Returns `None` on a
    /// scope mismatch rather than the token, so a caller cannot accidentally
    /// use it by ignoring a boolean.
    #[must_use]
    pub fn token_for(&self, scope: &TokenScope) -> Option<&str> {
        self.scope.allows(scope).then_some(self.token.as_str())
    }

    #[must_use]
    pub fn scope(&self) -> &TokenScope {
        &self.scope
    }
}

/// Deliberately opaque: a token in a log line is the one thing that could turn
/// this design back into a plaintext-disclosure path.
impl std::fmt::Display for RestorationTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<restoration token redacted>")
    }
}

/// External PII sanitizer configuration (`[pii_sanitizer]`). Disabled by
/// default; an inert block adds no hot-path cost.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PiiSanitizerConfig {
    #[serde(default)]
    pub enabled: bool,
    /// http(s) endpoint that sanitizes content and returns placeholders
    #[serde(default)]
    pub url: String,
    /// http(s) endpoint that turns placeholders back into plaintext. Required
    /// only when `restoration` is not `never`
    #[serde(default)]
    pub restore_url: String,
    #[serde(default)]
    pub direction: SanitizeDirection,
    #[serde(default)]
    pub restoration: RestorationPolicy,
    /// what a streamed response does when the response leg is active
    #[serde(default)]
    pub streaming: StreamingResponse,
    /// http(s) endpoint that restores a stream over one full-duplex NDJSON
    /// exchange (see [`RestoreStreamDelta`]). Required only when `streaming`
    /// is `restore`
    #[serde(default)]
    pub restore_stream_url: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// additional attempts on a transient failure (connect/timeout/5xx); 0 = none
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub failure_mode: FailureMode,
    /// cap on the content bytes forwarded; oversized content is truncated and flagged
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// entity types to detect. Empty means "whatever the service is configured
    /// for" — rolter does not second-guess the operator's recognizer set
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebhookAuth>,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn default_max_body_bytes() -> usize {
    DEFAULT_MAX_BODY_BYTES
}

impl Default for PiiSanitizerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            restore_url: String::new(),
            direction: SanitizeDirection::default(),
            restoration: RestorationPolicy::default(),
            streaming: StreamingResponse::default(),
            restore_stream_url: String::new(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_retries: 0,
            failure_mode: FailureMode::default(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            entities: Vec::new(),
            auth: None,
        }
    }
}

impl PiiSanitizerConfig {
    /// Validate the sanitizer config. Returns human-readable problems for the
    /// aggregate config validator; an empty vec means it is safe to load. A
    /// disabled block is never a problem, even if other fields are unset.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.enabled {
            return problems;
        }
        let url = self.url.trim();
        if !is_http_url(url) {
            problems.push(format!(
                "pii_sanitizer.url '{url}' must be an http:// or https:// URL"
            ));
        }
        // a restoration policy with nowhere to restore from is a config that
        // silently does nothing — the operator asked for a behaviour they will
        // not get, so it is rejected rather than defaulted away
        if self.restoration != RestorationPolicy::Never {
            let restore = self.restore_url.trim();
            if restore.is_empty() {
                problems.push(
                    "pii_sanitizer.restore_url is required when restoration is not 'never'"
                        .to_string(),
                );
            } else if !is_http_url(restore) {
                problems.push(format!(
                    "pii_sanitizer.restore_url '{restore}' must be an http:// or https:// URL"
                ));
            }
        }
        // the same rule as restore_url: asking for a streamed restore with
        // nowhere to send it, or with nothing that would ever be restored, is
        // a behaviour the operator would not get
        if self.streaming == StreamingResponse::Restore {
            let url = self.restore_stream_url.trim();
            if url.is_empty() {
                problems.push(
                    "pii_sanitizer.restore_stream_url is required when streaming is 'restore'"
                        .to_string(),
                );
            } else if !is_http_url(url) {
                problems.push(format!(
                    "pii_sanitizer.restore_stream_url '{url}' must be an http:// or https:// URL"
                ));
            }
            if self.restoration == RestorationPolicy::Never {
                problems.push(
                    "pii_sanitizer.streaming = 'restore' needs a restoration policy other than \
                     'never'"
                        .to_string(),
                );
            }
        }
        if self.timeout_ms == 0 {
            problems.push("pii_sanitizer.timeout_ms must be greater than zero".to_string());
        }
        if self.max_body_bytes == 0 {
            problems.push("pii_sanitizer.max_body_bytes must be greater than zero".to_string());
        }
        if let Some(auth) = &self.auth {
            if auth.env_var().trim().is_empty() {
                problems.push(
                    "pii_sanitizer.auth references an empty environment variable name".to_string(),
                );
            }
        }
        problems
    }

    /// Whether the request leg should be sanitized.
    #[must_use]
    pub fn sanitizes_request(&self) -> bool {
        self.enabled && self.direction.covers_request()
    }

    /// Whether the response leg should be sanitized.
    #[must_use]
    pub fn sanitizes_response(&self) -> bool {
        self.enabled && self.direction.covers_response()
    }

    /// How a streamed response is handled, given whether the caller asked for
    /// restoration. `None` when nothing happens on the response leg, so the
    /// stream is served as the upstream sent it.
    ///
    /// `restore` only covers restoration: a response leg that also sanitizes
    /// answers [`StreamingResponse::Reject`], since entity detection over a
    /// stream is not something the holdback can do.
    #[must_use]
    pub fn stream_handling(&self, caller_requested_restore: bool) -> Option<StreamingResponse> {
        if !self.touches_response(caller_requested_restore) {
            return None;
        }
        Some(match self.streaming {
            StreamingResponse::Restore if self.sanitizes_response() => StreamingResponse::Reject,
            other => other,
        })
    }

    /// Whether anything at all happens on the response leg — sanitization,
    /// restoration, or both. This is what decides whether a streamed response
    /// is affected, because both need the whole body for the same reason.
    #[must_use]
    pub fn touches_response(&self, caller_requested_restore: bool) -> bool {
        self.enabled
            && (self.direction.covers_response()
                || self.restoration.should_restore(caller_requested_restore))
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// One entity class the sanitizer replaced, and where.
///
/// Carries the entity type, how many were replaced, and the placeholders that
/// went into the text — deliberately **not** the matched values. A finding is
/// safe to log, count and put on a span; a matched value is the thing this
/// whole feature exists to keep out of those places.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Finding {
    /// e.g. `PERSON`, `EMAIL_ADDRESS`, `CREDIT_CARD`
    pub entity_type: String,
    #[serde(default)]
    pub count: usize,
    /// the deterministic placeholders substituted in, e.g. `<PERSON_1>`
    #[serde(default)]
    pub placeholders: Vec<String>,
}

/// Bound the content handed to the sanitizer, returning the payload to send and
/// whether it had to be truncated.
///
/// An oversized body becomes a truncated string preview rather than being sent
/// in full — the same contract the guardrail webhook uses. The caller passes the
/// flag on so the service can decide for itself whether a partial view is worth
/// scanning; rolter does not silently pretend it inspected the whole thing.
/// Borrows in the common case: a body under the ceiling is forwarded as-is, so
/// the hot path never deep-clones the whole request just to hand it on.
#[must_use]
pub fn bound_content(
    content: &serde_json::Value,
    max_body_bytes: usize,
) -> (std::borrow::Cow<'_, serde_json::Value>, bool) {
    let serialized = serde_json::to_vec(content).unwrap_or_default();
    if serialized.len() <= max_body_bytes {
        return (std::borrow::Cow::Borrowed(content), false);
    }
    // `chars().take()` rather than a byte slice: cutting mid-codepoint would
    // produce a string serde cannot encode
    let preview: String = content.to_string().chars().take(max_body_bytes).collect();
    (
        std::borrow::Cow::Owned(serde_json::Value::String(preview)),
        true,
    )
}

/// The JSON envelope the gateway POSTs to the sanitizer. Borrows so the request
/// path serializes without extra allocation.
#[derive(Debug, Clone, Serialize)]
pub struct SanitizeRequest<'a> {
    /// `"request"` or `"response"` — which leg is being sanitized
    pub direction: &'a str,
    pub model: &'a str,
    pub route: &'a str,
    /// trace/correlation id propagated end to end
    pub trace_id: &'a str,
    pub tenant: WebhookTenant,
    /// entity types the operator asked for; empty means the service's own set
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub entities: &'a [String],
    /// whether the gateway will ask for restoration later. lets a service skip
    /// storing a mapping it will never be asked for
    pub reversible: bool,
    /// true when `content` was truncated to `max_body_bytes`
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// the body under inspection (already size-bounded)
    pub content: &'a serde_json::Value,
}

/// The sanitizer's reply.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SanitizeResponse {
    /// the body with entities replaced by placeholders
    pub content: serde_json::Value,
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// opaque handle the service will accept back to restore this exchange.
    /// Absent when the service was not asked for, or cannot provide, reversal
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restoration_token: Option<String>,
}

impl SanitizeResponse {
    /// Total entities replaced across all findings.
    #[must_use]
    pub fn replaced(&self) -> usize {
        self.findings.iter().map(|f| f.count).sum()
    }
}

/// The JSON envelope the gateway POSTs to the restore endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreRequest<'a> {
    pub trace_id: &'a str,
    pub tenant: WebhookTenant,
    /// the opaque token from the matching [`SanitizeResponse`]
    pub restoration_token: &'a str,
    /// placeholder-bearing content to reverse
    pub content: &'a serde_json::Value,
}

/// The restore endpoint's reply.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RestoreResponse {
    /// content with placeholders replaced by their originals
    pub content: serde_json::Value,
    #[serde(default)]
    pub restored: usize,
}

/// The first line of a streamed restore (`POST restore_stream_url`): who is
/// asking, and the token to restore with. It is not answered; every line after
/// it is.
#[derive(Clone, Serialize)]
pub struct RestoreStreamOpen<'a> {
    pub trace_id: &'a str,
    pub tenant: WebhookTenant,
    /// the opaque token from the matching [`SanitizeResponse`]
    pub restoration_token: &'a str,
}

/// One text delta of a streamed restore.
///
/// `channel` names one text field of the stream — a choice's content, a tool
/// call's arguments, a content block — and the sanitizer keeps a separate
/// placeholder holdback per channel. `format` is `"json"` when `text` is JSON
/// source (tool-call arguments, Anthropic's `partial_json`): a value restored
/// into it is written JSON-string-escaped, as it would sit between quotes.
#[derive(Clone, Serialize)]
pub struct RestoreStreamDelta<'a> {
    pub seq: u64,
    pub channel: &'a str,
    pub text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
}

/// Opaque on purpose: `text` is the provider's text, which a log line has no
/// business holding.
impl std::fmt::Debug for RestoreStreamDelta<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreStreamDelta")
            .field("seq", &self.seq)
            .field("channel", &self.channel)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// The most characters a streamed-restore reply may hold back for one channel.
/// A placeholder is far shorter; a sanitizer that holds more is not running a
/// placeholder holdback, and the gateway treats the reply as out of contract.
pub const MAX_STREAM_HELD_CHARS: usize = 1024;

/// The end of one channel: the sanitizer answers with whatever it still
/// holds for it and forgets it.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreStreamClose<'a> {
    pub seq: u64,
    pub close: &'a str,
}

/// The end of the stream: the sanitizer may discard the exchange's state.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreStreamEnd {
    pub seq: u64,
    pub end: bool,
}

/// The sanitizer's answer to one line, in the order the lines were sent.
///
/// `text` is the restored text to emit for that line and `held` how many input
/// characters (Unicode scalar values) of that line's channel the sanitizer is
/// holding back because they could be the start of a placeholder. Held text
/// comes out later, in the reply to a later line of the same channel or to its
/// close. `held` never exceeds [`MAX_STREAM_HELD_CHARS`].
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct RestoreStreamReply {
    pub seq: u64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub held: usize,
}

/// Opaque on purpose: a reply carries restored plaintext, and nothing on the
/// request path may print it.
impl std::fmt::Debug for RestoreStreamReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreStreamReply")
            .field("seq", &self.seq)
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disabled_config_is_never_a_problem() {
        let cfg = PiiSanitizerConfig {
            enabled: false,
            url: String::new(),
            timeout_ms: 0,
            restoration: RestorationPolicy::TrustedDownstream,
            ..PiiSanitizerConfig::default()
        };
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn enabled_requires_valid_url_timeout_and_cap() {
        let cfg = PiiSanitizerConfig {
            enabled: true,
            url: "ftp://x".to_string(),
            timeout_ms: 0,
            max_body_bytes: 0,
            ..PiiSanitizerConfig::default()
        };
        let problems = cfg.validate();
        assert!(problems.iter().any(|p| p.contains("http")));
        assert!(problems.iter().any(|p| p.contains("timeout_ms")));
        assert!(problems.iter().any(|p| p.contains("max_body_bytes")));
    }

    /// A restoration policy with no endpoint to restore from would silently do
    /// nothing — the operator asked for a behaviour they would not get.
    #[test]
    fn restoration_without_a_restore_url_is_rejected() {
        for policy in [
            RestorationPolicy::TrustedDownstream,
            RestorationPolicy::CallerAuthorized,
        ] {
            let cfg = PiiSanitizerConfig {
                enabled: true,
                url: "https://presidio.internal/sanitize".to_string(),
                restoration: policy,
                ..PiiSanitizerConfig::default()
            };
            assert!(
                cfg.validate().iter().any(|p| p.contains("restore_url")),
                "{policy:?} accepted without a restore_url"
            );
        }

        // ...and `never` needs none
        let cfg = PiiSanitizerConfig {
            enabled: true,
            url: "https://presidio.internal/sanitize".to_string(),
            restoration: RestorationPolicy::Never,
            ..PiiSanitizerConfig::default()
        };
        assert!(cfg.validate().is_empty());
    }

    /// The operator owns the policy. A caller-set header must not be able to
    /// talk a `never` deployment into disclosing plaintext.
    #[test]
    fn a_caller_cannot_override_a_never_policy() {
        assert!(!RestorationPolicy::Never.should_restore(true));
        assert!(!RestorationPolicy::Never.should_restore(false));

        assert!(RestorationPolicy::TrustedDownstream.should_restore(false));
        assert!(RestorationPolicy::TrustedDownstream.should_restore(true));

        assert!(!RestorationPolicy::CallerAuthorized.should_restore(false));
        assert!(RestorationPolicy::CallerAuthorized.should_restore(true));
    }

    fn scope(org: &str, project: &str, route: &str) -> TokenScope {
        TokenScope {
            org: org.to_string(),
            team: "t".to_string(),
            project: project.to_string(),
            route: route.to_string(),
        }
    }

    /// Tenant and route isolation: a token is only redeemable in the scope that
    /// produced it, so restoration can never be triggered across tenants.
    #[test]
    fn a_token_is_only_redeemable_in_its_own_scope() {
        let issued = scope("acme", "billing", "gpt-4o");
        let ticket = RestorationTicket::new("tok_123".to_string(), issued.clone());

        assert_eq!(ticket.token_for(&issued), Some("tok_123"));
        // a different org, project or route each independently refuses
        assert_eq!(
            ticket.token_for(&scope("globex", "billing", "gpt-4o")),
            None
        );
        assert_eq!(ticket.token_for(&scope("acme", "support", "gpt-4o")), None);
        assert_eq!(ticket.token_for(&scope("acme", "billing", "claude")), None);
    }

    /// A token reaching a log line is the one thing that could turn this design
    /// back into a disclosure path.
    #[test]
    fn a_ticket_never_renders_its_token() {
        let ticket = RestorationTicket::new("tok_secret".to_string(), TokenScope::default());
        assert!(!ticket.to_string().contains("tok_secret"));
        assert!(!format!("{ticket}").contains("tok_secret"));
    }

    #[test]
    fn direction_gates_each_leg() {
        assert!(SanitizeDirection::Request.covers_request());
        assert!(!SanitizeDirection::Request.covers_response());
        assert!(!SanitizeDirection::Response.covers_request());
        assert!(SanitizeDirection::Response.covers_response());
        assert!(SanitizeDirection::Both.covers_request());
        assert!(SanitizeDirection::Both.covers_response());

        // and a disabled config covers neither, whatever the direction says
        let cfg = PiiSanitizerConfig {
            enabled: false,
            direction: SanitizeDirection::Both,
            ..PiiSanitizerConfig::default()
        };
        assert!(!cfg.sanitizes_request());
        assert!(!cfg.sanitizes_response());
    }

    #[test]
    fn a_sanitize_response_parses_and_totals_its_findings() {
        let parsed: SanitizeResponse = serde_json::from_value(json!({
            "content": {"messages": [{"role": "user", "content": "call <PERSON_1>"}]},
            "findings": [
                {"entity_type": "PERSON", "count": 2, "placeholders": ["<PERSON_1>", "<PERSON_2>"]},
                {"entity_type": "EMAIL_ADDRESS", "count": 1, "placeholders": ["<EMAIL_ADDRESS_1>"]}
            ],
            "restoration_token": "tok_abc"
        }))
        .unwrap();
        assert_eq!(parsed.replaced(), 3);
        assert_eq!(parsed.restoration_token.as_deref(), Some("tok_abc"));
        // a finding never carries the matched value
        for finding in &parsed.findings {
            for placeholder in &finding.placeholders {
                assert!(placeholder.starts_with('<') && placeholder.ends_with('>'));
            }
        }
    }

    /// A service that returns no token is a service that cannot reverse — the
    /// gateway must read that as "do not restore", not as an error.
    #[test]
    fn a_missing_restoration_token_parses_as_none() {
        let parsed: SanitizeResponse = serde_json::from_value(json!({"content": {}})).unwrap();
        assert!(parsed.restoration_token.is_none());
        assert_eq!(parsed.replaced(), 0);
    }

    /// The response leg needs the whole body, so a streamed response is
    /// refused by default rather than silently delivered unsanitized.
    #[test]
    fn a_streamed_response_is_refused_by_default() {
        assert_eq!(StreamingResponse::default(), StreamingResponse::Reject);
    }

    fn restoring_stream() -> PiiSanitizerConfig {
        PiiSanitizerConfig {
            enabled: true,
            url: "https://presidio.internal/sanitize".to_string(),
            restore_url: "https://presidio.internal/restore".to_string(),
            restore_stream_url: "https://presidio.internal/restore-stream".to_string(),
            restoration: RestorationPolicy::TrustedDownstream,
            streaming: StreamingResponse::Restore,
            ..PiiSanitizerConfig::default()
        }
    }

    #[test]
    fn a_streamed_restore_needs_its_url_and_a_restoration_policy() {
        assert!(restoring_stream().validate().is_empty());

        let missing = PiiSanitizerConfig {
            restore_stream_url: " ".to_string(),
            ..restoring_stream()
        };
        assert!(missing
            .validate()
            .iter()
            .any(|p| p.contains("restore_stream_url is required")));

        let bad = PiiSanitizerConfig {
            restore_stream_url: "ftp://x".to_string(),
            ..restoring_stream()
        };
        assert!(bad
            .validate()
            .iter()
            .any(|p| p.contains("restore_stream_url 'ftp://x'")));

        let never = PiiSanitizerConfig {
            restoration: RestorationPolicy::Never,
            ..restoring_stream()
        };
        assert!(never.validate().iter().any(|p| p.contains("'never'")));

        // the url is only required by `restore`
        let reject = PiiSanitizerConfig {
            restore_stream_url: String::new(),
            streaming: StreamingResponse::Reject,
            ..restoring_stream()
        };
        assert!(reject.validate().is_empty());
    }

    /// `restore` covers restoration only: a response leg that also sanitizes
    /// is refused, whatever the restoration policy.
    #[test]
    fn restore_applies_to_restoration_only() {
        let config = restoring_stream();
        assert_eq!(
            config.stream_handling(false),
            Some(StreamingResponse::Restore)
        );
        for direction in [SanitizeDirection::Response, SanitizeDirection::Both] {
            let config = PiiSanitizerConfig {
                direction,
                ..restoring_stream()
            };
            assert_eq!(
                config.stream_handling(false),
                Some(StreamingResponse::Reject)
            );
        }
        // nothing on the response leg: the stream is served as it is
        let opt_in = PiiSanitizerConfig {
            restoration: RestorationPolicy::CallerAuthorized,
            ..restoring_stream()
        };
        assert_eq!(opt_in.stream_handling(false), None);
        assert_eq!(
            opt_in.stream_handling(true),
            Some(StreamingResponse::Restore)
        );
        // the other two modes are unchanged
        let passthrough = PiiSanitizerConfig {
            direction: SanitizeDirection::Both,
            streaming: StreamingResponse::Passthrough,
            ..restoring_stream()
        };
        assert_eq!(
            passthrough.stream_handling(false),
            Some(StreamingResponse::Passthrough)
        );
    }

    #[test]
    fn the_streaming_modes_read_from_toml_and_survive_a_round_trip() {
        let config: PiiSanitizerConfig = serde_json::from_value(json!({
            "enabled": true,
            "url": "https://s/sanitize",
            "restore_url": "https://s/restore",
            "restore_stream_url": "https://s/restore-stream",
            "restoration": "trusted_downstream",
            "streaming": "restore"
        }))
        .unwrap();
        assert_eq!(config.streaming, StreamingResponse::Restore);
        // the snapshot carries the block as serde writes it
        let again: PiiSanitizerConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(again.streaming, StreamingResponse::Restore);
        assert_eq!(again.restore_stream_url, "https://s/restore-stream");
    }

    /// A reply carries restored plaintext, so its `Debug` must not.
    #[test]
    fn a_stream_reply_never_prints_its_text() {
        let reply: RestoreStreamReply =
            serde_json::from_value(json!({"seq": 3, "text": "Jan Kowalski", "held": 2})).unwrap();
        let printed = format!("{reply:?}");
        assert!(!printed.contains("Kowalski"), "{printed}");
        assert!(printed.contains("held: 2"), "{printed}");
    }

    #[test]
    fn the_stream_lines_serialize_as_documented() {
        let delta = RestoreStreamDelta {
            seq: 17,
            channel: "0/content",
            text: "_ADDRESS",
            format: None,
        };
        assert_eq!(
            serde_json::to_value(&delta).unwrap(),
            json!({"seq": 17, "channel": "0/content", "text": "_ADDRESS"})
        );
        assert!(!format!("{delta:?}").contains("_ADDRESS"));
        let json_delta = RestoreStreamDelta {
            format: Some("json"),
            ..delta
        };
        assert_eq!(serde_json::to_value(&json_delta).unwrap()["format"], "json");
        assert_eq!(
            serde_json::to_value(RestoreStreamClose {
                seq: 18,
                close: "0/content"
            })
            .unwrap(),
            json!({"seq": 18, "close": "0/content"})
        );
        assert_eq!(
            serde_json::to_value(RestoreStreamEnd { seq: 19, end: true }).unwrap(),
            json!({"seq": 19, "end": true})
        );
    }

    /// `touches_response` is what decides whether a stream is affected, and it
    /// must be true for restoration alone — restoring needs the whole body for
    /// the same reason sanitizing does.
    #[test]
    fn restoration_alone_counts_as_touching_the_response() {
        let restore_only = PiiSanitizerConfig {
            enabled: true,
            direction: SanitizeDirection::Request,
            restoration: RestorationPolicy::TrustedDownstream,
            ..PiiSanitizerConfig::default()
        };
        assert!(!restore_only.sanitizes_response());
        assert!(restore_only.touches_response(false));

        let request_only = PiiSanitizerConfig {
            enabled: true,
            direction: SanitizeDirection::Request,
            restoration: RestorationPolicy::Never,
            ..PiiSanitizerConfig::default()
        };
        assert!(!request_only.touches_response(true));

        // caller-authorized only touches the response when the caller asked
        let opt_in = PiiSanitizerConfig {
            restoration: RestorationPolicy::CallerAuthorized,
            ..request_only.clone()
        };
        assert!(!opt_in.touches_response(false));
        assert!(opt_in.touches_response(true));

        // and a disabled config never touches anything
        let off = PiiSanitizerConfig {
            enabled: false,
            ..opt_in
        };
        assert!(!off.touches_response(true));
    }
}
