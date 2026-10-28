//! What dialect translation did to the caller's request parameters (#1586).
//!
//! When a translator cannot carry a parameter to the upstream dialect it
//! records that here instead of discarding it silently. The gateway turns the
//! record into the [`PARAM_ADJUSTMENTS_HEADER`] response header and into the
//! invocation's `param_adjustments` column, so a caller learns that its
//! `top_k` did nothing without having to notice a different answer first.
//!
//! The record holds parameter names and dispositions, never values, and only
//! names from a fixed vocabulary of known dialect parameters: a key the caller
//! made up never reaches a header or a log row. An empty record allocates
//! nothing, which is every request whose route does not translate.

use std::fmt;

/// The response header naming each parameter translation did not forward as
/// sent, as `<param>=<disposition>` entries joined by `", "` (an RFC 8941
/// dictionary of tokens). Absent when nothing was adjusted.
pub const PARAM_ADJUSTMENTS_HEADER: &str = "x-rolter-param-adjustments";

/// What translation did to one caller-supplied parameter.
///
/// The vocabulary is open: a later release may add a disposition, and a
/// client reading the header must ignore one it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The upstream dialect has no equivalent, so the parameter was not sent.
    Dropped,
    /// The value was sent, limited to the range the upstream accepts.
    Clamped,
    /// The parameter was sent as an approximation that does not carry its
    /// whole meaning (a forced function choice sent as "any function").
    Translated,
}

impl Disposition {
    /// The disposition named `name`, `None` for any other string.
    pub fn parse(name: &str) -> Option<Self> {
        [Self::Dropped, Self::Clamped, Self::Translated]
            .into_iter()
            .find(|disposition| disposition.as_str() == name)
    }

    /// The disposition's published name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dropped => "dropped",
            Self::Clamped => "clamped",
            Self::Translated => "translated",
        }
    }
}

impl fmt::Display for Disposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One adjusted parameter: its name in the caller's dialect and what happened
/// to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamAdjustment {
    pub param: &'static str,
    pub disposition: Disposition,
}

/// Every parameter one request's translation dropped, clamped or translated,
/// in the order the translator met them, one entry per parameter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamAdjustments(Vec<ParamAdjustment>);

impl ParamAdjustments {
    /// Whether translation forwarded every parameter as sent.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ParamAdjustment> {
        self.0.iter()
    }

    /// Record `param`. A parameter already recorded keeps its first
    /// disposition, so a translation that runs in two stages reports it once.
    pub fn record(&mut self, param: &'static str, disposition: Disposition) {
        if self.0.iter().any(|entry| entry.param == param) {
            return;
        }
        self.0.push(ParamAdjustment { param, disposition });
    }

    /// Record `name` as dropped when it is a known dialect parameter; any
    /// other key is left out rather than echoed (see the module docs).
    pub(crate) fn record_known(&mut self, name: &str, disposition: Disposition) {
        if let Some(param) = known_param(name) {
            self.record(param, disposition);
        }
    }

    /// Rebuild a record from its [`ParamAdjustments::log_entries`], as a
    /// response cache entry keeps it (#3326). An entry that does not read as
    /// `<disposition>:<known parameter>` is left out, so a stored value can
    /// never put a name outside the fixed list into a header.
    pub fn from_log_entries<'a>(entries: impl IntoIterator<Item = &'a str>) -> Self {
        let mut adjustments = Self::default();
        for entry in entries {
            let Some((disposition, param)) = entry.split_once(':') else {
                continue;
            };
            if let (Some(disposition), Some(param)) =
                (Disposition::parse(disposition), known_param(param))
            {
                adjustments.record(param, disposition);
            }
        }
        adjustments
    }

    /// Withdraw an entry a later translation stage restored after all.
    pub(crate) fn forget(&mut self, param: &str) {
        self.0.retain(|entry| entry.param != param);
    }

    /// The [`PARAM_ADJUSTMENTS_HEADER`] value, `None` when nothing was
    /// adjusted.
    pub fn header_value(&self) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        let mut out = String::with_capacity(self.0.len() * 24);
        for (i, entry) in self.0.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(entry.param);
            out.push('=');
            out.push_str(entry.disposition.as_str());
        }
        Some(out)
    }

    /// The invocation log's entries, `<disposition>:<param>` each, in the
    /// shape of the other event columns of `request_logs`.
    pub fn log_entries(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|entry| format!("{}:{}", entry.disposition, entry.param))
            .collect()
    }
}

/// Request parameters of the dialects rolter translates between, the only
/// names a [`ParamAdjustments`] built from a caller's leftover keys may hold.
/// `model`, `stream` and `stream_options` are absent on purpose: the gateway
/// carries or answers those itself on every dialect.
const KNOWN_PARAMS: &[&str] = &[
    // openai chat completions
    "audio",
    "frequency_penalty",
    "function_call",
    "functions",
    "logit_bias",
    "logprobs",
    "max_completion_tokens",
    "max_tokens",
    "metadata",
    "modalities",
    "n",
    "parallel_tool_calls",
    "prediction",
    "presence_penalty",
    "prompt_cache_key",
    "reasoning_effort",
    "response_format",
    "safety_identifier",
    "seed",
    "service_tier",
    "stop",
    "store",
    "temperature",
    "tool_choice",
    "tools",
    "top_logprobs",
    "top_p",
    "user",
    "verbosity",
    "web_search_options",
    // anthropic messages
    "cache_control",
    "container",
    "mcp_servers",
    "stop_sequences",
    "thinking",
    "top_k",
    // openai responses
    "background",
    "conversation",
    "include",
    "instructions",
    "max_output_tokens",
    "max_tool_calls",
    "previous_response_id",
    "prompt",
    "reasoning",
    "text",
    "truncation",
    // gemini interactions
    "previous_interaction_id",
];

fn known_param(name: &str) -> Option<&'static str> {
    KNOWN_PARAMS.iter().copied().find(|known| *known == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_round_trips_through_its_log_entries() {
        let mut adjustments = ParamAdjustments::default();
        adjustments.record("top_k", Disposition::Dropped);
        adjustments.record("temperature", Disposition::Clamped);
        adjustments.record("tool_choice", Disposition::Translated);
        let entries = adjustments.log_entries();
        let restored = ParamAdjustments::from_log_entries(entries.iter().map(String::as_str));
        assert_eq!(restored, adjustments);
        assert_eq!(restored.header_value(), adjustments.header_value());
    }

    #[test]
    fn an_entry_outside_the_vocabulary_is_never_restored() {
        let restored = ParamAdjustments::from_log_entries([
            "dropped:x-caller-secret",
            "vanished:top_k",
            "top_k",
            "dropped:top_k",
        ]);
        assert_eq!(restored.header_value().as_deref(), Some("top_k=dropped"));
    }

    #[test]
    fn an_empty_record_has_no_header_and_no_allocation() {
        let adjustments = ParamAdjustments::default();
        assert!(adjustments.is_empty());
        assert_eq!(adjustments.header_value(), None);
        assert!(adjustments.log_entries().is_empty());
        assert_eq!(adjustments.0.capacity(), 0);
    }

    #[test]
    fn the_header_is_a_dictionary_of_tokens_in_record_order() {
        let mut adjustments = ParamAdjustments::default();
        adjustments.record("top_k", Disposition::Dropped);
        adjustments.record("tool_choice", Disposition::Translated);
        adjustments.record("temperature", Disposition::Clamped);
        assert_eq!(
            adjustments.header_value().as_deref(),
            Some("top_k=dropped, tool_choice=translated, temperature=clamped")
        );
        assert_eq!(
            adjustments.log_entries(),
            [
                "dropped:top_k",
                "translated:tool_choice",
                "clamped:temperature"
            ]
        );
    }

    #[test]
    fn a_parameter_is_reported_once_with_its_first_disposition() {
        let mut adjustments = ParamAdjustments::default();
        adjustments.record("tool_choice", Disposition::Translated);
        adjustments.record("tool_choice", Disposition::Dropped);
        assert_eq!(
            adjustments.header_value().as_deref(),
            Some("tool_choice=translated")
        );
    }

    #[test]
    fn an_unknown_key_is_never_echoed() {
        let mut adjustments = ParamAdjustments::default();
        adjustments.record_known("x-caller-field", Disposition::Dropped);
        adjustments.record_known("model", Disposition::Dropped);
        adjustments.record_known("stream", Disposition::Dropped);
        assert!(adjustments.is_empty());
        adjustments.record_known("top_k", Disposition::Dropped);
        assert_eq!(adjustments.header_value().as_deref(), Some("top_k=dropped"));
    }

    #[test]
    fn every_known_name_is_a_header_token() {
        for name in KNOWN_PARAMS {
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{name}"
            );
        }
    }
}
