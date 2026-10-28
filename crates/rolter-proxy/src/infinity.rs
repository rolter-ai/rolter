//! Request and response translation for a michaelfeil/infinity rerank.
//!
//! Infinity's embeddings endpoint is OpenAI's own shape (`OpenAIEmbeddingResult`
//! in `infinity_emb/fastapi_schemas/pymodels.py`: `object: "list"`, `data[]` of
//! `{object, embedding, index}`, `model`, `usage.{prompt_tokens,total_tokens}`),
//! so an embedding passes through untouched and needs nothing here.
//!
//! Its rerank is Cohere-shaped but not identical to what rolter serves on
//! `/v1/rerank` (the Cohere/Jina form the built-in `fake-llm` answers with), in
//! two places, both read from the same `pymodels.py`:
//!
//! - `RerankInput.documents` is `conlist(str)`. Cohere v1 and Jina also accept
//!   a document as `{"text": ...}`, which Infinity's validation rejects with a
//!   `422`, so such documents are flattened to their text on the way up.
//! - `_ReRankObject.document` is `Optional[str]`: with `return_documents` set a
//!   result carries the bare string, where Cohere and Jina carry
//!   `{"text": ...}`. It is wrapped on the way back, so a client written
//!   against either SDK reads `document.text` whichever upstream answered.
//!
//! Everything else already agrees: `query`, `top_n` (Infinity requires it
//! positive), `return_documents` and `model` go up as sent, and the answer's
//! `results[].{index,relevance_score}` come back sorted by descending score
//! (`BatchHandler.rerank` sorts before applying `top_n`) with `usage` in
//! OpenAI's `prompt_tokens`/`total_tokens` form, which the request log already
//! reads. Fields Infinity does not know, such as Cohere's `max_tokens_per_doc`,
//! are ignored by its pydantic models rather than refused, so they are left in.

// spec-ref: infinity, rerank
use rolter_core::{Error, Result};
use serde_json::{json, Value};

/// Flatten `{"text": ...}` documents to the bare strings Infinity accepts.
///
/// A document that is neither a string nor an object with a string `text` has
/// no equivalent upstream, so it fails closed as an `unsupported_content_part`
/// (a `400` naming the document) rather than being dropped, which would shift
/// every later `index` in the answer onto the wrong document.
pub(crate) fn rerank_request(mut body: Value) -> Result<Value> {
    let Some(documents) = body.get_mut("documents").and_then(Value::as_array_mut) else {
        // a missing or malformed `documents` is Infinity's to refuse, in its own
        // words, exactly as any other upstream would
        return Ok(body);
    };
    for (index, document) in documents.iter_mut().enumerate() {
        if document.is_string() {
            continue;
        }
        let Some(text) = document.get("text").and_then(Value::as_str) else {
            return Err(Error::Config(format!(
                "unsupported_content_part: an infinity rerank takes each document as a \
                 string or as {{\"text\": ...}}; documents[{index}] is neither"
            )));
        };
        *document = Value::String(text.to_string());
    }
    Ok(body)
}

/// Wrap each result's bare `document` string as `{"text": ...}`, and drop a
/// `null` one, which Cohere and Jina omit when documents were not asked for.
pub(crate) fn rerank_response(mut body: Value) -> Value {
    let Some(results) = body.get_mut("results").and_then(Value::as_array_mut) else {
        return body;
    };
    for result in results.iter_mut().filter_map(Value::as_object_mut) {
        match result.get("document") {
            Some(Value::String(text)) => {
                let wrapped = json!({ "text": text });
                result.insert("document".into(), wrapped);
            }
            Some(Value::Null) => {
                result.remove("document");
            }
            _ => {}
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_objects_are_flattened_and_strings_kept() {
        let body = json!({
            "model": "mxbai-rerank",
            "query": "q",
            "documents": ["plain", {"text": "wrapped"}],
            "top_n": 1,
            "return_documents": true,
        });
        let out = rerank_request(body).unwrap();
        assert_eq!(out["documents"], json!(["plain", "wrapped"]));
        assert_eq!(out["top_n"], 1);
        assert_eq!(out["return_documents"], true);
        assert_eq!(out["query"], "q");
    }

    #[test]
    fn a_document_without_text_is_refused_by_position() {
        let body = json!({"query": "q", "documents": ["a", {"title": "b"}]});
        let err = rerank_request(body).unwrap_err().to_string();
        assert!(err.contains("unsupported_content_part: "), "{err}");
        assert!(err.contains("documents[1]"), "{err}");
    }

    #[test]
    fn a_body_without_documents_is_left_for_infinity_to_judge() {
        let body = json!({"query": "q"});
        assert_eq!(rerank_request(body.clone()).unwrap(), body);
    }

    /// the answer `ReRankResult.to_rerank_response` builds with
    /// `return_documents`, and without it
    #[test]
    fn documents_come_back_in_the_cohere_shape() {
        let answer = json!({
            "object": "rerank",
            "results": [
                {"relevance_score": 0.9, "index": 1, "document": "second"},
                {"relevance_score": 0.1, "index": 0, "document": null},
                {"relevance_score": 0.05, "index": 2},
            ],
            "model": "mxbai-rerank",
            "usage": {"prompt_tokens": 12, "total_tokens": 12},
            "id": "infinity-1",
            "created": 1,
        });
        let out = rerank_response(answer);
        assert_eq!(out["results"][0]["document"], json!({"text": "second"}));
        assert!(out["results"][1].get("document").is_none());
        assert!(out["results"][2].get("document").is_none());
        assert_eq!(out["results"][0]["index"], 1);
        assert_eq!(out["usage"]["prompt_tokens"], 12);
        assert_eq!(out["model"], "mxbai-rerank");
    }

    /// only an infinity rerank is reshaped: its embeddings, and a rerank to any
    /// other kind, stay byte-for-byte passthrough
    #[test]
    fn the_plan_reshapes_only_an_infinity_rerank() {
        use crate::translation::TranslationPlan;
        use bytes::Bytes;
        use rolter_core::{upstream, ProviderKind, RoleProfile};

        let rerank = TranslationPlan::resolve(
            upstream::RERANK_PATH,
            ProviderKind::Infinity,
            RoleProfile::Openai,
        );
        assert!(rerank.rewrites_response());
        assert_eq!(rerank.upstream_path("/v1/rerank"), "/v1/rerank");
        let up = rerank
            .translate_request(Bytes::from(r#"{"query":"q","documents":[{"text":"a"}]}"#))
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&up).unwrap()["documents"],
            json!(["a"])
        );
        let down = rerank.translate_response(
            Bytes::from(r#"{"results":[{"index":0,"relevance_score":0.5,"document":"a"}]}"#),
            false,
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&down).unwrap()["results"][0]["document"]["text"],
            "a"
        );
        // a malformed answer is handed on as it came, for the caller to see
        let garbage = Bytes::from_static(b"<html>bad gateway</html>");
        assert_eq!(rerank.translate_response(garbage.clone(), false), garbage);

        let embeddings = TranslationPlan::resolve(
            "/v1/embeddings",
            ProviderKind::Infinity,
            RoleProfile::Openai,
        );
        assert!(!embeddings.rewrites_response());
        assert_eq!(embeddings.upstream_path("/v1/embeddings"), "/v1/embeddings");
        for kind in [
            ProviderKind::Tei,
            ProviderKind::OpenaiCompatible,
            ProviderKind::Cohere,
        ] {
            let plan = TranslationPlan::resolve(upstream::RERANK_PATH, kind, RoleProfile::Openai);
            assert!(!plan.rewrites_response(), "{kind:?}");
        }
    }

    #[test]
    fn a_body_that_is_not_a_ranking_is_returned_as_is() {
        let error = json!({"error": {"message": "Invalid model", "code": 400}});
        assert_eq!(rerank_response(error.clone()), error);
    }
}
