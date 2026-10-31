# Request pipeline

Every model request the gateway forwards runs one pipeline in
`crates/rolter-gateway/src/handlers.rs`. `proxy` runs it for a JSON body
(chat, completions, Responses, Messages, embeddings, rerank, images, speech,
the decision endpoint) and `proxy_multipart` for an upload
(`/v1/audio/transcriptions`, `/v1/audio/translations`). Until #1041 `proxy`
was one 1,770-line function nested eleven levels deep, and `proxy_multipart`
repeated about 800 lines of it, so a fix to the pipeline had to be made twice.
Each step is now a function of its own, and the two handlers call the steps in
order.

## The shape of a stage

A stage that runs once the caller is known is a method of `RequestCtx`, which
holds the snapshot the request is served under, the endpoint, the caller's key
and the `Refusals` that logs whatever a stage turns away (#2807). It holds only
borrows. Its two lifetimes keep what a stage builds from the snapshot (`'s`)
apart from the values the handler owns for this request (`'r`: the key, its
scope, the model name), which the handler moves into the log row once the
stages are done.

A stage returns a `Stage<T>`: `Next(value)` hands on what the later stages
need, and `Answer(response)` ends the request with an answer the stage has
already logged, or one it served itself (the built-in model, a cache hit). The
handler unwraps it with `proceed!`. `Stage` is not a `Result` on purpose: a
cache hit is no error, and nothing here is converted or boxed.

## The stages, in order

| #   | Stage                        | Function                                                                                                        | `proxy_multipart`                                                                  |
| --- | ---------------------------- | --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| 1   | Parse                        | in `proxy`: the JSON body and its `model`                                                                       | its own: the boundary and one pass over the form fields (`multipart::text_fields`) |
| 2   | Scope identity               | `authenticate`, `request_scope`, then `RequestCtx` is built                                                     | shared                                                                             |
| 3   | Model gate, budget block     | `RequestCtx::admit_model`, `admit_budget`                                                                       | shared                                                                             |
| 4   | `pre_route` plugins          | `RequestCtx::request_plugins(PreRoute, …)`                                                                      | not run: no plugin reads audio                                                     |
| 5   | Throughput cap               | `RequestCtx::admit_rate`                                                                                        | shared                                                                             |
| 6   | Built-in `fake-llm`          | `RequestCtx::builtin`, with the reply to give                                                                   | shared, with a transcription reply                                                 |
| 7   | Route resolution and gates   | `Snapshot::resolve_for` (ADR-0017), `RequestCtx::route_miss`, `admit_route`                                     | shared                                                                             |
| 8   | Complexity tier              | `complexity_tier`, between resolution and `admit_route`                                                         | not run                                                                            |
| 9   | Output token cap, route caps | `RequestCtx::output_token_cap`, `route_permit` (#2936)                                                          | `route_permit` shared                                                              |
| 10  | Prompt templates             | `RequestCtx::prompt_templates`                                                                                  | not run                                                                            |
| 11  | Built-in guardrails          | `RequestCtx::builtin_guardrails`                                                                                | not run                                                                            |
| 12  | Guardrail webhook            | `RequestCtx::semantic_guard`, then `guardrail_webhook` for the pre_call providers                               | `semantic_guard` shared, kept only for a policy that runs post_call                |
| 13  | PII sanitizer                | `RequestCtx::sanitize_request`                                                                                  | not run: no placeholder goes upstream                                              |
| 14  | `pre_upstream` plugins       | `RequestCtx::request_plugins(PreUpstream, …)`                                                                   | not run                                                                            |
| 15  | Forward body                 | `forward_body`                                                                                                  | the upload goes as it came                                                         |
| 16  | Response-leg admission       | `RequestCtx::output_guard`, `admit_semantic_leg`, `admit_pii_stream`, `post_response_plugins`, `admit_unpriced` | shared, in the same order (#3538)                                                  |
| 17  | Cache lookup                 | `cache_lookup`: exact, then semantic, one `cache.lookup` span                                                   | not run                                                                            |
| 18  | Forward and variants         | `forward` → `forward_classic` or `forward_variants`, both driving one `ForwardLoop`                             | shared, with a `ForwardPayload::Multipart`                                         |
| 19  | Response legs                | `unanswered`, `withhold_unrequested_stream`, `store_and_deliver`, `stream_response`                             | `unanswered` and `stream_response` shared                                          |

The order is the contract. A request that two checks would both refuse gets the
answer of the earlier one, and that answer is what its row records, so moving a
stage changes behaviour even when every stage still runs. Both pipelines run
the response-leg checks and the unpriced policy in the order above: the output
guard, the semantic policy's stream check, the sanitizer's stream check, the
post_response plugins, then `admit_unpriced`. Until #3538 the upload pipeline
applied the unpriced policy first and asked the plugins before the sanitizer,
so a streamed upload that two of them refused could get another answer than
the same request on `/v1/chat/completions`.

## Forwarding

`Forward` is what a forward loop reads from the request (the snapshot, the
route, the route context, the payload, the trace headers, the key), built once
from borrows. `forward_classic` walks the route's classic pool through a
`TargetPool`, which picks a target per attempt (`TargetPool::pick`, or
`pick_read_only` for the realtime dial walk), and `forward_variants` walks the
weighted-variant chain's flattened candidates (`next_candidate`). Both hand
each dialled attempt to the same `ForwardLoop`: `settle_response` handles a
refusal another target takes over, a key-level `429`/`401`, a retryable status
and a good answer, and `settle_error` a transport failure. So the retry,
cooldown, breaker and failover bookkeeping exists once
([load-balancing.md](load-balancing.md)). The loop returns a `ForwardOutcome`.

Every attempt either loop dials opens an `upstream.request` span
(`genai_attempt_span`) and injects its trace context from inside it
(`attempt_trace_context`, #805): on the classic pool and the variant chain, for
a JSON body and an upload, under the target the attempt dialled and the
endpoint's GenAI operation. The answering attempt's span leaves the loop as
`ForwardOutcome::genai_span`. Until #3535 only a JSON attempt on the classic
pool was traced. With no pipeline installed `genai_attempt_span` returns the
disabled span before naming it, nothing is injected, and `genai_span` stays
`None`, so the stream wrapper parses no response body to record onto it.

## The response

Every row a forwarded request leaves starts from one `RequestLogCtx`, built
once the caller is admitted: the cancel guard's template, a cache hit's row,
the success row and the no-answer row (`RequestLogCtx::row`). A response's
billing, logging and in-flight slot travel together as an `Accounting`, moved
down the response tail into the `UsageLoggingStream` that delivers it, and the
post-response policy is one `DeliveryPolicy` for a live answer, a cacheable miss
and a hit alike (#1477).

## Hot path

The stages add no allocation, lock or boxed future to a request. `RequestCtx`,
`Forward`, `TargetPool`, `ForwardLoop` and `Accounting` hold borrows or values
the handler already had, and are moved rather than cloned; the `Stage` enum is
returned by value. The post_response plugins are lent to the delivery policy
rather than cloned for the stream, which drops one clone per request. The
`hot_path` and `admission` benches (`crates/rolter-gateway/benches/`) measure
the forwarding primitives; see [performance.md](performance.md).
