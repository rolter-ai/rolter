//! What happened to a request, as the `outcome` column of `request_logs`
//! records it (#3307).
//!
//! The maintainer's decision on #1395 was that a request refused after its key
//! resolves is written to the request log with a distinct outcome. Before this
//! a refusal was recognisable only by inference (no provider, no target,
//! `attempts = 0` and a 4xx status), so every filter, rollup and alert had to
//! re-derive the rule, and a later change to how failed upstream calls are
//! logged could have made it ambiguous. The gateway now names the outcome on
//! every row it writes, and the control plane filters on the name.
//!
//! The names are published: they are stored in ClickHouse, returned by
//! `GET /api/v1/analytics/invocations` and accepted by its `outcome` filter.
//! A name is never renamed or reused; new ones may be added.

use serde::{Deserialize, Serialize};

/// The outcome of one request, as the gateway recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    /// The caller got a successful answer: from an upstream, the response
    /// cache or the built-in `fake-llm`.
    Served,
    /// The request reached an upstream and the caller got an error: the
    /// provider's own, or the gateway's after every attempt failed.
    UpstreamError,
    /// The upstream answered and was billed, but a post-call policy (an output
    /// guardrail or a post-response plugin) withheld the answer.
    Withheld,
    /// The caller went away before the answer was delivered.
    Cancelled,
    /// Refused before routing: the key's, team's, project's or org's budget is
    /// spent.
    RefusedBudget,
    /// Refused before routing: a rate limit or a route's own cap.
    RefusedRateLimit,
    /// Refused before routing by a guardrail or the PII sanitizer.
    RefusedGuardrail,
    /// Refused before routing by a plugin.
    RefusedPlugin,
    /// Refused before routing: the model is unknown or not visible to the key,
    /// or the key may not use it.
    RefusedAccess,
    /// Refused before routing: the request itself cannot be served as sent
    /// (a missing parameter, an endpoint the route's servers lack, a cap
    /// exceeded, an unpriced model under a refusing policy).
    RefusedInvalid,
    /// Refused before routing for a reason none of the above names, or no
    /// target was left to try.
    Refused,
}

impl RequestOutcome {
    /// Every outcome, in the order the dashboard lists them.
    pub const ALL: [RequestOutcome; 11] = [
        Self::Served,
        Self::UpstreamError,
        Self::Withheld,
        Self::Cancelled,
        Self::RefusedBudget,
        Self::RefusedRateLimit,
        Self::RefusedGuardrail,
        Self::RefusedPlugin,
        Self::RefusedAccess,
        Self::RefusedInvalid,
        Self::Refused,
    ];

    /// The outcome's published name, the value of the `outcome` column.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::UpstreamError => "upstream_error",
            Self::Withheld => "withheld",
            Self::Cancelled => "cancelled",
            Self::RefusedBudget => "refused_budget",
            Self::RefusedRateLimit => "refused_rate_limit",
            Self::RefusedGuardrail => "refused_guardrail",
            Self::RefusedPlugin => "refused_plugin",
            Self::RefusedAccess => "refused_access",
            Self::RefusedInvalid => "refused_invalid",
            Self::Refused => "refused",
        }
    }

    /// The outcome named `name`, `None` for any other string.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == name)
    }

    /// Whether the gateway turned the request away before routing it.
    pub const fn is_refusal(self) -> bool {
        matches!(
            self,
            Self::RefusedBudget
                | Self::RefusedRateLimit
                | Self::RefusedGuardrail
                | Self::RefusedPlugin
                | Self::RefusedAccess
                | Self::RefusedInvalid
                | Self::Refused
        )
    }

    /// The refusal a gateway answer of `status` with OpenAI-style error `code`
    /// stands for. The code decides where it names the reason; the status
    /// covers a refusal answered without one.
    pub fn for_refusal(status: u16, code: Option<&str>) -> Self {
        match code {
            Some("insufficient_quota") => return Self::RefusedBudget,
            Some("rate_limit_exceeded") => return Self::RefusedRateLimit,
            Some(
                "guardrail_blocked"
                | "guardrail_streaming_unsupported"
                | "pii_streaming_unsupported"
                | "pii_sanitizer_unavailable"
                | "sanitizer_unsupported_on_realtime",
            ) => return Self::RefusedGuardrail,
            Some("plugin_blocked" | "plugin_streaming_unsupported") => return Self::RefusedPlugin,
            Some("model_not_found" | "invalid_api_key") => return Self::RefusedAccess,
            _ => {}
        }
        match status {
            402 => Self::RefusedBudget,
            429 => Self::RefusedRateLimit,
            401 | 403 | 404 => Self::RefusedAccess,
            400 | 405..=428 | 430..=499 => Self::RefusedInvalid,
            _ => Self::Refused,
        }
    }
}

/// What a row written before the `outcome` column existed reads as: the
/// column's default in `clickhouse/030_request_outcome.sql`. Not an outcome a
/// gateway writes, so it is not in [`RequestOutcome::ALL`].
pub const UNKNOWN_OUTCOME: &str = "unknown";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for outcome in RequestOutcome::ALL {
            assert!(seen.insert(outcome.as_str()), "{outcome:?}");
            assert_eq!(RequestOutcome::parse(outcome.as_str()), Some(outcome));
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                serde_json::json!(outcome.as_str())
            );
        }
        assert_eq!(RequestOutcome::parse(UNKNOWN_OUTCOME), None);
        assert_eq!(RequestOutcome::parse(""), None);
    }

    #[test]
    fn a_refusal_is_named_by_its_code_first() {
        use RequestOutcome::*;
        for (status, code, want) in [
            (402, Some("insufficient_quota"), RefusedBudget),
            (429, Some("insufficient_quota"), RefusedBudget),
            (429, Some("rate_limit_exceeded"), RefusedRateLimit),
            (403, Some("guardrail_blocked"), RefusedGuardrail),
            (400, Some("pii_streaming_unsupported"), RefusedGuardrail),
            (403, Some("plugin_blocked"), RefusedPlugin),
            (404, Some("model_not_found"), RefusedAccess),
            (403, None, RefusedAccess),
            (400, Some("missing_required_parameter"), RefusedInvalid),
            (400, Some("unsupported_endpoint"), RefusedInvalid),
            (429, None, RefusedRateLimit),
            (503, None, Refused),
        ] {
            assert_eq!(
                RequestOutcome::for_refusal(status, code),
                want,
                "{status} {code:?}"
            );
            assert!(want.is_refusal());
        }
        for outcome in [Served, UpstreamError, Withheld, Cancelled] {
            assert!(!outcome.is_refusal());
        }
    }
}
