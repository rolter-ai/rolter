//! Which failures a fallback target takes over (#1584).
//!
//! A route target may carry `takes_over`, a set of [`FailureClass`]es. Such a
//! target is a remedy rather than a member of the pool: it is never dialled
//! first and never picked by ordinary balancing, and it is reached only as the
//! successor of an attempt whose failure matches one of its classes. A target
//! without the set is an ordinary member of the pool, exactly as before.
//!
//! The vocabulary is rolter's own, over the failures the gateway's forward loop
//! already tells apart: the statuses `[retry]` treats as transient
//! (`408`, `429`, `5xx`), a call that never got a response, and, new with this
//! module, an upstream that refused the request on its content policy. A
//! refusal is final everywhere else: it is retried only when a target that
//! takes it over is left to try.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Longest category a narrowed refusal class may name.
pub const MAX_REFUSAL_CATEGORY_LEN: usize = 64;

/// The stable phrase every all-restricted-route error carries, so a script
/// can match the failure without parsing the sentence around it.
pub const ALL_TARGETS_RESTRICTED: &str = "all_targets_restricted";

/// What kind of failure an attempt ended with.
///
/// The gateway classifies a failure into one of these itself; a failure is
/// never read from the wire, so the set of kinds a [`Failure`] can carry is
/// closed. What the snapshot carries is a [`FailureClass`], and a class a newer
/// release names decodes as [`FailureKind::Unknown`] rather than failing the
/// whole snapshot (#3347, #3390).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// the upstream answered `429`
    Throttled,
    /// the upstream answered `408` or `504`, or the call timed out before a
    /// response arrived
    Timeout,
    /// the call never got a response: refused or reset connection, TLS or DNS
    /// failure
    Transport,
    /// the upstream answered with another `5xx`
    ServerError,
    /// the upstream refused the request on its content policy
    Refusal,
    /// a class this build does not know, read from a newer control plane's
    /// snapshot. No failure is ever of this kind, so a target that takes it
    /// over takes over nothing for it, and stays out of the pool
    Unknown,
}

impl FailureKind {
    /// The token naming this kind in `takes_over` and in the logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Throttled => "throttled",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::ServerError => "server_error",
            Self::Refusal => "refusal",
            Self::Unknown => "unknown",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "throttled" => Self::Throttled,
            "timeout" => Self::Timeout,
            "transport" => Self::Transport,
            "server_error" => Self::ServerError,
            "refusal" => Self::Refusal,
            _ => return None,
        })
    }
}

/// One failure an attempt ended with: its kind and, for a refusal, the
/// categories the provider reported (its error code, and the filter categories
/// an Azure-style content filter names), lowercase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub categories: Vec<String>,
}

impl Failure {
    /// A failure of `kind` with no categories.
    pub fn of(kind: FailureKind) -> Self {
        Self {
            kind,
            categories: Vec::new(),
        }
    }

    /// The failure an upstream status names, for the statuses `[retry]` treats
    /// as transient. `None` for any other status.
    pub fn from_status(status: u16) -> Option<Self> {
        let kind = match status {
            429 => FailureKind::Throttled,
            408 | 504 => FailureKind::Timeout,
            500..=599 => FailureKind::ServerError,
            _ => return None,
        };
        Some(Self::of(kind))
    }

    /// The failure of a call that got no response, from the error it ended
    /// with: a timeout when the forwarder says it timed out, else transport.
    pub fn from_transport_error(message: &str) -> Self {
        if message.contains("timed out") {
            Self::of(FailureKind::Timeout)
        } else {
            Self::of(FailureKind::Transport)
        }
    }

    /// Whether an error response with `status` and `body` is a content-policy
    /// refusal, and the categories it names if so.
    ///
    /// Only a `400`, `403`, `422` or `451` can be one. `451` is a refusal by
    /// itself; the others are when the body's `error.code` or `error.type`
    /// names a content-policy failure, or when it carries the content-filter
    /// result Azure OpenAI attaches. Anything else is an ordinary client error
    /// and stays final.
    pub fn refusal(status: u16, body: &[u8]) -> Option<Self> {
        if !matches!(status, 400 | 403 | 422 | 451) {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let error = value.get("error").unwrap_or(&value);
        let mut categories = Vec::new();
        let mut marked = status == 451;
        for key in ["code", "type"] {
            if let Some(token) = error.get(key).and_then(|v| v.as_str()) {
                let token = token.trim().to_ascii_lowercase();
                if is_refusal_token(&token) {
                    marked = true;
                    push_category(&mut categories, &token);
                }
            }
        }
        let filter = error
            .get("innererror")
            .and_then(|inner| inner.get("content_filter_result"))
            .or_else(|| error.get("content_filter_result"));
        if let Some(results) = filter.and_then(|f| f.as_object()) {
            marked = true;
            for (name, result) in results {
                if result.get("filtered").and_then(|f| f.as_bool()) == Some(true) {
                    push_category(&mut categories, &name.to_ascii_lowercase());
                }
            }
        }
        marked.then_some(Self {
            kind: FailureKind::Refusal,
            categories,
        })
    }

    /// The token this failure is recorded as on the invocation: the kind, and
    /// for a refusal with a category the first one, `refusal:<category>`.
    pub fn token(&self) -> String {
        match self.categories.first() {
            Some(category) if self.kind == FailureKind::Refusal => {
                format!("refusal:{category}")
            }
            _ => self.kind.as_str().to_string(),
        }
    }
}

/// Error codes and types providers use for a content-policy refusal. Matched
/// as substrings, so `content_policy_violation` and `content_filter` both
/// count, while `invalid_request_error` alone never does.
const REFUSAL_MARKERS: &[&str] = &[
    "content_policy",
    "content_filter",
    "content_management",
    "responsible_ai",
    "safety",
    "moderation",
];

fn is_refusal_token(token: &str) -> bool {
    REFUSAL_MARKERS.iter().any(|marker| token.contains(marker))
}

fn push_category(categories: &mut Vec<String>, category: &str) {
    if valid_category(category) && !categories.iter().any(|c| c == category) {
        categories.push(category.to_string());
    }
}

fn valid_category(category: &str) -> bool {
    !category.is_empty()
        && category.len() <= MAX_REFUSAL_CATEGORY_LEN
        && category
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// One class a fallback target takes over: a [`FailureKind`], and for a
/// refusal optionally the one category it is narrowed to.
///
/// Written as its token: `throttled`, `timeout`, `transport`, `server_error`,
/// `refusal`, or `refusal:<category>` (lowercase letters, digits, `_` and `-`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FailureClass {
    pub kind: FailureKind,
    pub category: Option<String>,
}

impl FailureClass {
    /// Whether this class covers `failure`: the kinds match and, when the class
    /// is narrowed to a category, the failure names it.
    pub fn matches(&self, failure: &Failure) -> bool {
        self.kind != FailureKind::Unknown
            && self.kind == failure.kind
            && self
                .category
                .as_ref()
                .is_none_or(|category| failure.categories.contains(category))
    }
}

impl fmt::Display for FailureClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // an unknown class keeps the token it was read from, so a control plane
        // that does not know it still serves it on unchanged
        if self.kind == FailureKind::Unknown {
            return f.write_str(self.category.as_deref().unwrap_or("unknown"));
        }
        match &self.category {
            Some(category) => write!(f, "{}:{category}", self.kind.as_str()),
            None => f.write_str(self.kind.as_str()),
        }
    }
}

impl FromStr for FailureClass {
    type Err = String;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        let (head, category) = match token.split_once(':') {
            Some((head, category)) => (head, Some(category)),
            None => (token, None),
        };
        let Some(kind) = FailureKind::parse(head) else {
            return Err(format!(
                "unknown failure class '{token}'; expected one of throttled, timeout, \
                 transport, server_error, refusal or refusal:<category>"
            ));
        };
        match category {
            None => Ok(Self {
                kind,
                category: None,
            }),
            Some(_) if kind != FailureKind::Refusal => Err(format!(
                "failure class '{token}' names a category, which only refusal takes"
            )),
            Some(category) if !valid_category(category) => Err(format!(
                "refusal category '{category}' must be 1 to {MAX_REFUSAL_CATEGORY_LEN} \
                 lowercase letters, digits, '_' or '-'"
            )),
            Some(category) => Ok(Self {
                kind,
                category: Some(category.to_string()),
            }),
        }
    }
}

impl Serialize for FailureClass {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Lenient where [`FromStr`] is strict (#3390). A snapshot from a newer
/// control plane may name a class this build does not know; failing the decode
/// would keep the gateway on its last config, revoked keys included. Such a
/// token reads as [`FailureKind::Unknown`], which matches no failure, so the
/// target keeps its `takes_over` set and stays a remedy that is never dialled
/// for it. Dropping the class instead would empty a set and turn the target
/// into an ordinary member of the pool. A config file refuses the token as a
/// typo (`GatewayConfig::from_toml_str`), and the routes API parses with
/// [`FromStr`].
impl<'de> Deserialize<'de> for FailureClass {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let token = String::deserialize(deserializer)?;
        Ok(token.parse().unwrap_or(Self {
            kind: FailureKind::Unknown,
            category: Some(token),
        }))
    }
}

/// Whether a target with the `takes_over` set `classes` may be dialled after
/// an attempt that ended with `failure` (`None` for the first dial).
///
/// A target with no set may be dialled first and after any failure the loop
/// retries. A target with a set never leads, and follows only a failure one of
/// its classes covers. A refusal is never retried onto an ordinary target: it
/// is the same request, and only a target that takes refusals over is a
/// different answer to it.
pub fn may_dial(classes: &[FailureClass], failure: Option<&Failure>) -> bool {
    match failure {
        None => classes.is_empty(),
        Some(failure) if classes.is_empty() => failure.kind != FailureKind::Refusal,
        Some(failure) => classes.iter().any(|class| class.matches(failure)),
    }
}

/// Every problem with the `takes_over` sets of a route's targets, one sentence
/// each, `route` naming the route: a class listed twice, and a route whose
/// every target is restricted, which could never dial anything.
pub fn takes_over_problems<'a>(
    route: &str,
    targets: impl IntoIterator<Item = (&'a str, &'a [FailureClass])>,
) -> Vec<String> {
    let mut any = false;
    let mut all_restricted = true;
    let targets: Vec<_> = targets.into_iter().collect();
    for (_, classes) in &targets {
        any = true;
        all_restricted &= !classes.is_empty();
    }
    let mut problems = duplicate_class_problems(route, targets);
    if any && all_restricted {
        problems.push(all_targets_restricted(route));
    }
    problems
}

/// The targets of `route` that list a `takes_over` class twice, one problem
/// each. Whether the route is left with nothing it may dial is judged apart,
/// over the targets the data plane dials.
pub fn duplicate_class_problems<'a>(
    route: &str,
    targets: impl IntoIterator<Item = (&'a str, &'a [FailureClass])>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (provider, classes) in targets {
        for (i, class) in classes.iter().enumerate() {
            if classes[..i].contains(class) {
                problems.push(format!(
                    "route '{route}' target '{provider}' lists takes_over class '{class}' twice"
                ));
            }
        }
    }
    problems
}

/// The error for a route whose every target sets `takes_over`.
pub fn all_targets_restricted(route: &str) -> String {
    format!(
        "route '{route}' has no target it may dial first: every target sets takes_over, \
         so the route could never serve a request ({ALL_TARGETS_RESTRICTED}); leave \
         takes_over off at least one target"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(token: &str) -> FailureClass {
        token.parse().unwrap()
    }

    #[test]
    fn classes_round_trip_through_their_tokens() {
        for token in [
            "throttled",
            "timeout",
            "transport",
            "server_error",
            "refusal",
            "refusal:content_policy_violation",
        ] {
            assert_eq!(class(token).to_string(), token);
            let json = serde_json::to_string(&class(token)).unwrap();
            assert_eq!(json, format!("\"{token}\""));
            assert_eq!(
                serde_json::from_str::<FailureClass>(&json).unwrap(),
                class(token)
            );
        }
    }

    #[test]
    fn a_malformed_class_is_refused_with_a_reason() {
        for bad in [
            "",
            "rate_limited",
            "throttled:x",
            "refusal:",
            "refusal:Hate",
            "refusal:a b",
        ] {
            assert!(bad.parse::<FailureClass>().is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn statuses_and_transport_errors_classify_as_retry_does() {
        assert_eq!(
            Failure::from_status(429).unwrap().kind,
            FailureKind::Throttled
        );
        assert_eq!(
            Failure::from_status(408).unwrap().kind,
            FailureKind::Timeout
        );
        assert_eq!(
            Failure::from_status(504).unwrap().kind,
            FailureKind::Timeout
        );
        assert_eq!(
            Failure::from_status(503).unwrap().kind,
            FailureKind::ServerError
        );
        assert_eq!(Failure::from_status(400), None);
        assert_eq!(
            Failure::from_transport_error("upstream request timed out after 30s").kind,
            FailureKind::Timeout
        );
        assert_eq!(
            Failure::from_transport_error("error sending request: connection refused").kind,
            FailureKind::Transport
        );
    }

    #[test]
    fn a_content_policy_refusal_is_recognised_with_its_categories() {
        let openai = br#"{"error":{"message":"no","type":"invalid_request_error","code":"content_policy_violation"}}"#;
        let failure = Failure::refusal(400, openai).unwrap();
        assert_eq!(failure.kind, FailureKind::Refusal);
        assert_eq!(failure.categories, vec!["content_policy_violation"]);
        assert_eq!(failure.token(), "refusal:content_policy_violation");

        let azure = br#"{"error":{"code":"content_filter","innererror":{"content_filter_result":{"hate":{"filtered":true},"violence":{"filtered":false}}}}}"#;
        let failure = Failure::refusal(400, azure).unwrap();
        assert_eq!(failure.categories, vec!["content_filter", "hate"]);

        assert_eq!(Failure::refusal(451, b"").unwrap().token(), "refusal");
    }

    #[test]
    fn an_ordinary_client_error_is_not_a_refusal() {
        let bad =
            br#"{"error":{"message":"bad","type":"invalid_request_error","code":"invalid_value"}}"#;
        assert_eq!(Failure::refusal(400, bad), None);
        assert_eq!(
            Failure::refusal(401, br#"{"error":{"code":"content_filter"}}"#),
            None
        );
        assert_eq!(Failure::refusal(400, b"not json"), None);
    }

    #[test]
    fn a_narrowed_refusal_class_matches_only_its_category() {
        let hate = Failure::refusal(
            400,
            br#"{"error":{"code":"content_filter","innererror":{"content_filter_result":{"hate":{"filtered":true}}}}}"#,
        )
        .unwrap();
        assert!(class("refusal").matches(&hate));
        assert!(class("refusal:hate").matches(&hate));
        assert!(!class("refusal:sexual").matches(&hate));
        assert!(!class("throttled").matches(&hate));
    }

    #[test]
    fn a_restricted_target_never_leads_and_follows_only_a_match() {
        let throttled = Failure::of(FailureKind::Throttled);
        let server = Failure::of(FailureKind::ServerError);
        let refusal = Failure::of(FailureKind::Refusal);
        let burst = [class("throttled")];
        // first dial
        assert!(may_dial(&[], None));
        assert!(!may_dial(&burst, None));
        // matched and unmatched successors
        assert!(may_dial(&burst, Some(&throttled)));
        assert!(!may_dial(&burst, Some(&server)));
        // an ordinary target follows a retried failure but never a refusal
        assert!(may_dial(&[], Some(&server)));
        assert!(!may_dial(&[], Some(&refusal)));
        assert!(may_dial(&[class("refusal")], Some(&refusal)));
    }

    #[test]
    fn a_route_whose_every_target_is_restricted_is_refused_by_name() {
        let restricted = [class("refusal")];
        let problems = takes_over_problems("m", [("a", &restricted[..]), ("b", &restricted[..])]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains(ALL_TARGETS_RESTRICTED), "{problems:?}");

        assert!(takes_over_problems("m", [("a", &[][..]), ("b", &restricted[..])]).is_empty());
        assert!(takes_over_problems("m", std::iter::empty()).is_empty());

        let twice = [class("timeout"), class("timeout")];
        let problems = takes_over_problems("m", [("a", &[][..]), ("b", &twice[..])]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("twice"));
    }

    /// #3390: a class a newer release names must not fail the snapshot decode,
    /// and must not empty the set either, which would make the target an
    /// ordinary member of the pool.
    #[test]
    fn an_unknown_class_decodes_matches_nothing_and_keeps_its_token() {
        let classes: Vec<FailureClass> =
            serde_json::from_value(serde_json::json!(["throttled", "quantum_flux"])).unwrap();
        assert_eq!(classes[1].kind, FailureKind::Unknown);
        assert_eq!(classes[1].to_string(), "quantum_flux");
        assert_eq!(
            serde_json::to_value(&classes).unwrap(),
            serde_json::json!(["throttled", "quantum_flux"])
        );
        let only_unknown = &classes[1..];
        for kind in [
            FailureKind::Throttled,
            FailureKind::Timeout,
            FailureKind::Transport,
            FailureKind::ServerError,
            FailureKind::Refusal,
        ] {
            assert!(!classes[1].matches(&Failure::of(kind)));
            assert!(!may_dial(only_unknown, Some(&Failure::of(kind))));
        }
        // still a restricted target: never dialled first
        assert!(!may_dial(only_unknown, None));
        // the API and the store stay strict
        assert!("quantum_flux".parse::<FailureClass>().is_err());
    }
}
