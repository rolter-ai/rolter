//! Which upstream endpoint a provider kind's requests are sent to.
//!
//! The gateway's forwarder (`rolter_proxy`) turns a client request path into an
//! upstream path per kind: Anthropic's API is `/v1/messages`, native Gemini
//! embeds the model in the path, Gemini Interactions is one model-less
//! endpoint, and the rest speak OpenAI's chat completions. The dashboard shows
//! the resulting address while an operator types an API base, and a preview
//! that disagrees with the forwarder is worse than none: it is the one place
//! the URL is checked before a route depends on it (#2811, which found the
//! sheet previewing `/v1/chat/completions` for every kind).
//!
//! So the paths are written once, here, and `TranslationPlan::upstream_path`
//! returns these constants rather than its own literals. Which constant a kind
//! maps to is [`ProviderKind::primary_upstream_path`], and a test in
//! `rolter_proxy` walks [`ProviderKind::ALL`] and fails when the forwarder
//! picks another path than this one for the request the kind is chiefly called
//! for.

use serde::Serialize;

use crate::ProviderKind;

/// OpenAI chat completions, and the path every chat-shaped kind receives.
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

/// OpenAI responses.
pub const RESPONSES_PATH: &str = "/v1/responses";

/// Anthropic messages.
pub const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";

/// OpenAI embeddings, which is what a text-embeddings server answers.
pub const EMBEDDINGS_PATH: &str = "/v1/embeddings";

/// Gemini Interactions: a single model-less endpoint, the model travels in the
/// body.
pub const GEMINI_INTERACTIONS_PATH: &str = "/interactions";

/// Gemini native `generateContent`. `{model}` stands for the upstream model
/// name the forwarder substitutes per request; streaming swaps the method for
/// `streamGenerateContent?alt=sse`.
pub const GEMINI_GENERATE_PATH: &str = "/models/{model}:generateContent";

/// The kind of request a provider kind is chiefly called for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// a chat completion (or the same exchange in another dialect)
    Chat,
    /// a text embedding
    Embeddings,
}

impl ProviderKind {
    /// What this kind is chiefly called for.
    ///
    /// Only a text-embeddings server answers something other than chat, so the
    /// request the dashboard previews for it is an embedding, not a completion
    /// it would answer `404` to.
    pub fn primary_request(self) -> RequestKind {
        match self {
            ProviderKind::Tei => RequestKind::Embeddings,
            _ => RequestKind::Chat,
        }
    }

    /// The upstream path, before the `/v1` rule of
    /// [`Self::resolve_upstream_url`], that [`Self::primary_request`] is sent to.
    pub fn primary_upstream_path(self) -> &'static str {
        match self {
            ProviderKind::Tei => EMBEDDINGS_PATH,
            ProviderKind::Anthropic => ANTHROPIC_MESSAGES_PATH,
            ProviderKind::GeminiNative => GEMINI_GENERATE_PATH,
            ProviderKind::GeminiInteractions => GEMINI_INTERACTIONS_PATH,
            _ => CHAT_COMPLETIONS_PATH,
        }
    }

    /// The address [`Self::primary_request`] is sent to for `api_base`.
    pub fn primary_upstream_url(self, api_base: &str) -> String {
        self.resolve_upstream_url(api_base, self.primary_upstream_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_dialect_has_its_own_path() {
        let path = |kind: ProviderKind| kind.primary_upstream_path();
        assert_eq!(path(ProviderKind::Openai), "/v1/chat/completions");
        assert_eq!(path(ProviderKind::OpenaiCompatible), "/v1/chat/completions");
        assert_eq!(path(ProviderKind::Anthropic), "/v1/messages");
        assert_eq!(path(ProviderKind::Tei), "/v1/embeddings");
        assert_eq!(
            path(ProviderKind::GeminiNative),
            "/models/{model}:generateContent"
        );
        assert_eq!(path(ProviderKind::GeminiInteractions), "/interactions");
    }

    #[test]
    fn only_tei_is_called_for_embeddings() {
        for kind in ProviderKind::ALL {
            let expected = if kind == ProviderKind::Tei {
                RequestKind::Embeddings
            } else {
                RequestKind::Chat
            };
            assert_eq!(kind.primary_request(), expected, "{kind:?}");
        }
    }

    /// The preview is the stripping rule applied to the kind's own path, so a
    /// kind that appends `/v1` itself previews without it and one that does
    /// not, with it.
    #[test]
    fn the_url_follows_the_kinds_v1_rule() {
        assert_eq!(
            ProviderKind::Openai.primary_upstream_url("https://api.openai.com/"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            ProviderKind::Mistral.primary_upstream_url("https://api.mistral.ai/v1"),
            "https://api.mistral.ai/v1/chat/completions"
        );
        assert_eq!(
            ProviderKind::Anthropic.primary_upstream_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            ProviderKind::Tei.primary_upstream_url("http://tei:80"),
            "http://tei:80/v1/embeddings"
        );
        assert_eq!(
            ProviderKind::GeminiInteractions
                .primary_upstream_url("https://generativelanguage.googleapis.com/v1beta"),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
    }

    #[test]
    fn every_path_is_rooted() {
        for kind in ProviderKind::ALL {
            assert!(kind.primary_upstream_path().starts_with('/'), "{kind:?}");
        }
    }
}
