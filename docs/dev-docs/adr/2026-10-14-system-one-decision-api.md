# System One decisions: one generic kind on a native pass-through route

**Status:** Accepted · **Date:** 14 Oct 2026 · **Issues:** [#3150](https://github.com/rolter-ai/rolter/issues/3150), [#2535](https://github.com/rolter-ai/rolter/issues/2535)
**Relates:** ADR-0014 (fail-closed translation), the host-pin ADR (2 Sep 2026)

## Context

Every provider kind rolter had was a text-generation dialect. Decision models
are not. They answer typed questions about a `state` (pick one of N options,
score on a rubric, yes/no) and return probabilities and a confidence, and they
cannot generate text, embed or call tools.

Several of them now speak one wire protocol, the System One API:

- `POST {base}/v1/systemone` with a Bearer key;
- a body of `model`, `state` (a string, an object or an array) and `questions`,
  a map keyed by ids the caller chooses, each with a `type`, `instructions` and
  usually `criteria`;
- a response of `model`, `answers` under the same ids, and
  `usage: {input_tokens, output_tokens}`.

TypeSafe's Jev is hosted at `api.typesafe.ai`. vLLM Semantic Router serves its
Decision and Vela models over the same endpoint from `vllm-sr serve`, Ollama
0.35 and later runs decision models over it, and Pydantic AI ships a generic
client for it. The question types are not a closed set: Jev documents `choice`,
`score` and `noul`, Vela adds `span` and `set`, and d3 takes images and video in
the same body.

The first draft of #2535 asked for a `jev` kind. By the time it was picked up it
was clear that most of the work (the kind, the route, metering, logging, tests)
was the same for every one of these servers.

## Decision

1. **One kind, `system_one`, for any compatible endpoint.** It names the
   protocol rather than a vendor, so TypeSafe, a self-hosted vllm-sr engine and
   a local Ollama are all `kind = "system_one"` with a different `api_base`.
   Like `openai_compatible` it has no host pin and does not insist on an
   environment key: most of its endpoints are self-hosted, and an open vllm-sr
   listener takes no key at all.
2. **A native route, `POST /v1/systemone`, forwarded unchanged.** It runs the
   same pipeline as `/v1/rerank`: virtual keys, budgets, rate limits, route
   caps, routing on `model` (with the target's upstream model written in),
   balancing, retries, the breaker, usage and cost. Nothing translates the body,
   so a question type or a media field the gateway has never seen reaches the
   upstream as the caller sent it.
3. **No mapping from an existing surface.** OpenAI `tool_choice` or structured
   output could be mapped onto choice questions, but every such mapping would
   be lossy in one direction (probabilities and confidence have nowhere to go in
   a chat completion) and ADR-0014 already rules that a translator fails closed
   rather than drop content. A caller who wants decisions calls the decision
   endpoint.
4. **A decision-only boundary, judged per route.** `ProviderKind::decision_only`
   is true for `system_one`; `serves_decisions` is true for it and for the two
   self-hosted kinds known to serve the endpoint beside chat
   (`openai_compatible`, `ollama`). The gateway refuses with
   `400 unsupported_endpoint` a decision for a route none of whose targets
   serves decisions, and any other endpoint for a route all of whose targets are
   decision-only. A route that mixes the two is left to balance, since refusing
   it would refuse what its other targets answer.
5. **Usage is the upstream's own.** The response's `usage` uses Anthropic's
   field names, which the log parser already reads, so a decision is reported
   usage and never `usage_unknown`. Output is free on Jev; that is an
   `output_per_mtok = 0` price row, which the price table already accepts.
   rolter still ships no price list.
6. **Guardrails scan every string in `state` and in each question's
   `instructions` and `criteria`.** The decision endpoint is an egress channel
   like any other, and a `state` is often a whole ticket or document. Ids, keys
   and `type` are left alone, so a redaction never changes the shape the
   upstream validates. Images and videos are not text and are not scanned.
   Output guardrails do not apply: an answer is a label the caller supplied and
   a probability.
7. **The health probe is `GET {base}/v1/models`.** TypeSafe documents it
   (a `models` list of names) and Ollama serves the OpenAI one, so it is a
   catalogue probe. A vllm-sr engine lists its models at `/v1/systemone/models`
   instead. If it answers `/v1/models` with a `404` it still reads healthy (the
   free probe's rule is any status below 500) but lists no models; `health.path`
   is global, so it cannot be pointed elsewhere for one provider. The opt-in
   end-to-end check asks one yes/no question.
8. **`fake-llm` answers decisions.** Deterministic `choice`, `score` and `noul`
   answers, a per-question `invalid_question` error for anything else, and
   input-only usage, so the endpoint is testable with no key and no GPU.

## Consequences

- A hosted TypeSafe key can sit on a provider with any `api_base`, the way an
  `openai_compatible` key can. If that becomes a concern, a pinned hosted kind
  for TypeSafe can be added beside `system_one` later without changing the
  route or the protocol handling.
- The gateway does not bound the question count. The upstreams do (Ollama at 64
  questions, TypeSafe at a 64k-token context), and `max_body_bytes` bounds the
  body; vllm-sr accepts bodies up to 48 MiB for media, so an operator fronting
  d3 with video may need to raise it.
- Request logs need no new column. A decision's answers are its response body,
  which payload capture already stores and the Logs screen already shows.
- The protocol is young. LLM Gateway added an OpenAI-compatible `/v1/decisions`
  on 2026-10-12 and says it will deprecate `/v1/systemone` for TypeSafe models.
  TypeSafe itself still documents `/v1/systemone` only. If the vendors move,
  the move is a second route and an upstream path per kind, not a new kind.
- The external PII sanitizer and the guardrail webhook receive the whole
  request body whatever the endpoint, so they cover decisions with no change.
