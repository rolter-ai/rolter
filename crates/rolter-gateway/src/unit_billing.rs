//! Requests a provider bills by something other than tokens (#2937).
//!
//! An image is billed per image, a transcription per minute of audio and a
//! text-to-speech request per character of input. Which of those a request is
//! follows from its endpoint, and how many there were is read from the request
//! (characters, the images asked for) or the response (the images returned, the
//! audio duration), so it is known only when the request is finished.
//! [`UnitBasis`] is what is known at request time; [`UnitBasis::units`] turns it
//! and the response body into the count [`rolter_core::ModelPriceConfig::unit_cost`]
//! prices.

use rolter_core::{BilledUnits, ModelPriceConfig};
use rust_decimal::Decimal;
use serde_json::Value;

/// What a request will be billed by, as far as the request itself says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitBasis {
    /// a token-billed endpoint: chat, responses, embeddings, rerank
    #[default]
    Tokens,
    /// `/v1/images/generations`; `requested` is the body's `n`, the count used
    /// when the response does not carry a `data` array
    Images { requested: u32 },
    /// `/v1/audio/transcriptions` and `/v1/audio/translations`
    Transcription,
    /// `/v1/audio/speech`; `chars` is the length of the body's `input`
    Speech { chars: u64 },
}

impl UnitBasis {
    /// The basis of a JSON request to `path`.
    pub fn for_json(path: &str, body: &Value) -> Self {
        match path {
            "/v1/images/generations" => Self::Images {
                requested: body
                    .get("n")
                    .and_then(Value::as_u64)
                    .map_or(1, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            },
            "/v1/audio/speech" => Self::Speech {
                chars: body
                    .get("input")
                    .and_then(Value::as_str)
                    .map_or(0, |input| input.chars().count() as u64),
            },
            _ => Self::Tokens,
        }
    }

    /// The basis of a multipart request to `path`.
    pub fn for_multipart(path: &str) -> Self {
        match path {
            "/v1/audio/transcriptions" | "/v1/audio/translations" => Self::Transcription,
            _ => Self::Tokens,
        }
    }

    /// Whether the endpoint is billed by something other than tokens.
    pub fn is_unit_billed(self) -> bool {
        self != Self::Tokens
    }

    /// Whether `price` sets the rate this basis is billed at.
    pub fn has_rate(self, price: &ModelPriceConfig) -> bool {
        match self {
            Self::Tokens => false,
            Self::Images { .. } => price.image_per_unit.is_some(),
            Self::Transcription => price.audio_input_per_minute.is_some(),
            Self::Speech { .. } => price.speech_per_mchar.is_some(),
        }
    }

    /// The count of billed units, or `None` when this answer states none (a
    /// transcription returned as plain text or subtitles, which carries no
    /// duration). `body` is the upstream response; an empty one is a response
    /// the gateway withheld, which the provider still billed.
    pub fn units(self, body: &[u8]) -> Option<BilledUnits> {
        match self {
            Self::Tokens => None,
            Self::Images { requested } => {
                let returned = serde_json::from_slice::<Value>(body)
                    .ok()
                    .and_then(|value| value.get("data").and_then(Value::as_array).map(Vec::len));
                Some(BilledUnits::Images(
                    returned.map_or(requested, |n| u32::try_from(n).unwrap_or(u32::MAX)),
                ))
            }
            Self::Transcription => audio_seconds(body).map(BilledUnits::AudioSeconds),
            Self::Speech { chars } => Some(BilledUnits::SpeechChars(chars)),
        }
    }
}

/// The audio duration a transcription response states: `verbose_json`'s
/// top-level `duration`, or the `usage` object of type `duration` the newer
/// models answer with. Seconds, as the providers state them.
fn audio_seconds(body: &[u8]) -> Option<Decimal> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let usage = value
        .get("usage")
        .filter(|usage| usage.get("type").and_then(Value::as_str) == Some("duration"));
    let seconds = value
        .get("duration")
        .or_else(|| usage.and_then(|usage| usage.get("seconds")))?;
    // read from the number's own text rather than through `f64`, so a duration
    // of 12.3 seconds is exactly 12.3
    let seconds: Decimal = seconds.as_number()?.to_string().parse().ok()?;
    (seconds >= Decimal::ZERO).then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(text: &str) -> Decimal {
        text.parse().unwrap()
    }

    #[test]
    fn an_image_request_counts_the_images_returned_over_the_ones_asked_for() {
        let basis = UnitBasis::for_json("/v1/images/generations", &json!({"n": 4}));
        assert_eq!(basis, UnitBasis::Images { requested: 4 });
        let body = br#"{"data":[{"url":"a"},{"url":"b"}]}"#;
        assert_eq!(basis.units(body), Some(BilledUnits::Images(2)));
    }

    #[test]
    fn an_image_request_falls_back_to_n_and_then_to_one() {
        let basis = UnitBasis::for_json("/v1/images/generations", &json!({"n": 3}));
        assert_eq!(basis.units(b""), Some(BilledUnits::Images(3)));
        assert_eq!(basis.units(b"not json"), Some(BilledUnits::Images(3)));
        let bare = UnitBasis::for_json("/v1/images/generations", &json!({"prompt": "x"}));
        assert_eq!(bare.units(b""), Some(BilledUnits::Images(1)));
    }

    #[test]
    fn speech_counts_characters_not_bytes() {
        let basis = UnitBasis::for_json("/v1/audio/speech", &json!({"input": "héllo 世界"}));
        assert_eq!(basis, UnitBasis::Speech { chars: 8 });
        assert_eq!(
            basis.units(b"\x00binary"),
            Some(BilledUnits::SpeechChars(8))
        );
    }

    #[test]
    fn a_transcription_reads_its_duration_from_either_shape() {
        let basis = UnitBasis::for_multipart("/v1/audio/transcriptions");
        assert_eq!(
            basis.units(br#"{"text":"hi","duration":12.3}"#),
            Some(BilledUnits::AudioSeconds(d("12.3")))
        );
        assert_eq!(
            basis.units(br#"{"text":"hi","usage":{"type":"duration","seconds":30}}"#),
            Some(BilledUnits::AudioSeconds(d("30")))
        );
    }

    #[test]
    fn a_transcription_that_states_no_duration_has_no_units() {
        let basis = UnitBasis::for_multipart("/v1/audio/translations");
        assert_eq!(basis.units(br#"{"text":"hi"}"#), None);
        assert_eq!(basis.units(b"1\n00:00:00,000 --> 00:00:01,000\nhi"), None);
        // a token-metered usage is not a duration
        assert_eq!(
            basis.units(br#"{"text":"hi","usage":{"type":"tokens","seconds":9}}"#),
            None
        );
        assert_eq!(basis.units(br#"{"duration":-1}"#), None);
    }

    #[test]
    fn token_endpoints_have_no_unit_basis() {
        assert_eq!(
            UnitBasis::for_json("/v1/chat/completions", &json!({"n": 2})),
            UnitBasis::Tokens
        );
        assert!(!UnitBasis::Tokens.is_unit_billed());
        assert_eq!(UnitBasis::Tokens.units(b"{}"), None);
    }
}
