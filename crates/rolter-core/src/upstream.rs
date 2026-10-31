//! Which upstream endpoint a provider kind's requests are sent to.
//!
//! The gateway's forwarder (`rolter_proxy`) turns a client request path into an
//! upstream path per kind: Anthropic's API is `/v1/messages`, native Gemini
//! embeds the model in the path, Gemini Interactions is one model-less
//! endpoint, a System One decision server answers `/v1/systemone`, and the rest
//! speak OpenAI's chat completions. The dashboard shows
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

/// Cohere/Jina-style rerank. Of the kinds with a dialect of their own only
/// Infinity is sent it (`{url_prefix}/rerank`, through the `/v1` rule); every
/// other kind receives the gateway path unchanged.
pub const RERANK_PATH: &str = "/v1/rerank";

/// Gemini Interactions: a single model-less endpoint, the model travels in the
/// body.
pub const GEMINI_INTERACTIONS_PATH: &str = "/interactions";

/// System One typed decisions: one endpoint for every decision model, the
/// model travels in the body. The gateway serves the same path and forwards it
/// unchanged.
pub const SYSTEM_ONE_PATH: &str = "/v1/systemone";

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
    /// a System One typed decision
    Decision,
}

impl ProviderKind {
    /// What this kind is chiefly called for.
    ///
    /// A text-embeddings server and a System One decision server are the two
    /// that answer something other than chat, so the request the dashboard
    /// previews for them is an embedding or a decision, not a completion they
    /// would answer `404` to.
    pub fn primary_request(self) -> RequestKind {
        match self {
            ProviderKind::Tei | ProviderKind::Infinity => RequestKind::Embeddings,
            ProviderKind::SystemOne => RequestKind::Decision,
            _ => RequestKind::Chat,
        }
    }

    /// Whether this kind answers only System One decisions.
    ///
    /// A decision model cannot generate text, embed or call tools, so the
    /// gateway refuses any other endpoint for a route whose targets are all of
    /// this kind instead of forwarding a request the upstream would `404`.
    pub fn decision_only(self) -> bool {
        self == ProviderKind::SystemOne
    }

    /// Whether this kind answers only embeddings (and, for Infinity, reranking).
    ///
    /// A text-embeddings server has no chat, messages, responses, image or
    /// audio endpoint, so the gateway refuses those for a route whose targets
    /// are all of this kind instead of forwarding a request the upstream would
    /// `404` (#3247). [`EMBEDDINGS_PATH`] and [`RERANK_PATH`] are left alone.
    pub fn embeddings_only(self) -> bool {
        matches!(self, ProviderKind::Tei | ProviderKind::Infinity)
    }

    /// Whether a request for [`SYSTEM_ONE_PATH`] can be forwarded to this kind.
    ///
    /// The dedicated kind, and the two self-hosted kinds known to serve the
    /// same endpoint beside their chat api: an openai-compatible server (which
    /// is what another gateway fronting decision models looks like) and Ollama,
    /// which runs decision models over it. Every other kind either translates
    /// the request into its own dialect or is a hosted chat api that has no
    /// such endpoint.
    pub fn serves_decisions(self) -> bool {
        matches!(
            self,
            ProviderKind::SystemOne | ProviderKind::OpenaiCompatible | ProviderKind::Ollama
        )
    }

    /// The upstream path, before the `/v1` rule of
    /// [`Self::resolve_upstream_url`], that [`Self::primary_request`] is sent to.
    pub fn primary_upstream_path(self) -> &'static str {
        match self {
            ProviderKind::Tei | ProviderKind::Infinity => EMBEDDINGS_PATH,
            ProviderKind::Anthropic => ANTHROPIC_MESSAGES_PATH,
            ProviderKind::GeminiNative => GEMINI_GENERATE_PATH,
            ProviderKind::GeminiInteractions => GEMINI_INTERACTIONS_PATH,
            ProviderKind::SystemOne => SYSTEM_ONE_PATH,
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
        assert_eq!(path(ProviderKind::Infinity), "/v1/embeddings");
        assert_eq!(
            path(ProviderKind::GeminiNative),
            "/models/{model}:generateContent"
        );
        assert_eq!(path(ProviderKind::GeminiInteractions), "/interactions");
        assert_eq!(path(ProviderKind::SystemOne), "/v1/systemone");
    }

    #[test]
    fn the_embeddings_only_kinds_are_the_ones_that_chiefly_embed() {
        for kind in ProviderKind::ALL {
            assert_eq!(
                kind.embeddings_only(),
                kind.primary_request() == RequestKind::Embeddings,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn only_the_embedding_servers_embed_and_only_system_one_decides() {
        for kind in ProviderKind::ALL {
            let expected = match kind {
                ProviderKind::Tei | ProviderKind::Infinity => RequestKind::Embeddings,
                ProviderKind::SystemOne => RequestKind::Decision,
                _ => RequestKind::Chat,
            };
            assert_eq!(kind.primary_request(), expected, "{kind:?}");
        }
    }

    #[test]
    fn a_decision_only_kind_serves_decisions() {
        for kind in ProviderKind::ALL {
            if kind.decision_only() {
                assert!(kind.serves_decisions(), "{kind:?}");
                assert_eq!(kind.primary_request(), RequestKind::Decision, "{kind:?}");
            }
        }
        assert!(!ProviderKind::Anthropic.serves_decisions());
        assert!(!ProviderKind::GeminiInteractions.serves_decisions());
        assert!(!ProviderKind::OpenaiCompatible.decision_only());
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
        // infinity's default `--url-prefix` is empty, so its routes sit at the
        // server root and the base carries no `/v1` of its own
        assert_eq!(
            ProviderKind::Infinity.primary_upstream_url("http://infinity:7997"),
            "http://infinity:7997/embeddings"
        );
        assert_eq!(
            ProviderKind::Infinity.resolve_upstream_url("http://infinity:7997/v1/", RERANK_PATH),
            "http://infinity:7997/v1/rerank"
        );
        assert_eq!(
            ProviderKind::GeminiInteractions
                .primary_upstream_url("https://generativelanguage.googleapis.com/v1beta"),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
        assert_eq!(
            ProviderKind::SystemOne.primary_upstream_url("https://api.typesafe.ai"),
            "https://api.typesafe.ai/v1/systemone"
        );
    }

    #[test]
    fn every_path_is_rooted() {
        for kind in ProviderKind::ALL {
            assert!(kind.primary_upstream_path().starts_with('/'), "{kind:?}");
        }
    }
}
