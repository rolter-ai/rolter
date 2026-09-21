# Billed but withheld

A post-call policy decides what the caller receives. It does not decide what
the provider charged. By the time an output guardrail or a post-response plugin
runs, the upstream has already generated the answer and billed for it, so a
refusal at that point is a delivery outcome, not a billing one. This page states
how rolter accounts for that case (#1478).

Before #1478 a blocked response returned its `403` before the accounting stream
existed. Spend never reached the budget counters, tokens never reached the
rate-limit windows, and no row was written. A caller could spend real money,
repeatedly, while every budget and dashboard read zero.

## The guarantees

1. **Usage is read before any policy runs.** On the buffered path (a
   translation, output guardrails, post-response plugins or the PII sanitizer's
   response leg), the gateway parses usage from the upstream body right after
   dialect translation and before any policy touches it. That value is what gets
   billed. A guardrail mask, a plugin rewrite that drops or zeroes the `usage`
   object, or a sanitizer re-serialization changes what the caller sees and
   nothing else.
2. **A refusal is billed exactly once.** When a guardrail or plugin blocks, the
   refusal goes through the same `UsageLoggingStream::finalize` as a delivered
   response, which enqueues spend and token records and writes the row. Nothing
   else writes that request's spend. A retried request is billed only for the
   attempt that answered, because superseded attempts produced no usage and write
   no row of their own.
3. **The row keeps both outcomes.** `status` is what the caller got (`403`),
   `withheld = 1` marks the divergence, `error` names the policy
   (`guardrail_blocked: <rule>` or `plugin_blocked: <reason>`), and the token and
   cost columns hold the provider's figures.
4. **The rejected content is never recorded.** The refused body is never handed
   to the accounting stream, so it can't reach `request_payloads` even with
   payload capture on. The request payload is still captured as usual.
5. **Cache hits stay free.** A withheld response-cache hit writes a row with
   `cache_hit = 1`, `withheld = 1` and zero tokens and cost. Nothing was
   generated upstream for it. The miss that populated the entry was billed when
   it happened.
6. **Unknown usage is not zero usage.** `usage_unknown = 1` marks a row whose
   upstream answered successfully without reporting any usage, so its zeros are
   unknown rather than free. This covers every row, not only withheld ones. The
   usual cause is an OpenAI-style stream without `stream_options.include_usage`.

`rolter_withheld_responses_total` counts refusals of both kinds, cache hits
included.

## How it is enforced

| Where                                        | Mechanism                                                                                                                                                                |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `handlers::stream_response`, buffered branch | `parse_usage` right after `translate_response`; a guardrail or plugin block returns through `withheld_response`                                                          |
| `handlers::proxy`, cacheable miss            | same, before the output guard; the entry is still stored as the upstream sent it                                                                                         |
| `handlers::buffered_response`                | takes the pre-policy usage and hands it to the stream through `with_billed_usage`, so a delivered-but-rewritten body is billed from the provider's figures               |
| `handlers::withheld_response`                | builds a `UsageLoggingStream` over an empty body with the billed usage, then calls `withhold(status, reason)`, which marks the row and drops the stream (one `finalize`) |
| `handlers::cached_response`                  | a blocked hit logs a zero-cost `withheld` row                                                                                                                            |
| `logging::UsageLoggingStream::finalize`      | prefers the billed usage over the buffer and sets `usage_unknown` from `Usage::reported`                                                                                 |
| `clickhouse/011_withheld_usage.sql`          | adds `withheld` and `usage_unknown`, both `UInt8 default 0`                                                                                                              |

A live SSE stream never reaches a post-call block. Output guardrails either
refuse a streamed request before any upstream call (`streaming_post_call =
"reject"`, the default) or let it through unmasked (`"passthrough"`), and
post-response plugins are skipped on streams. A stream buffered for the response
cache is the exception, and it takes the cacheable-miss path above with its SSE
usage frames parsed.

## Tests

`crates/rolter-gateway/tests/withheld_usage.rs` drives the gateway over HTTP
against mock upstreams and a stand-in ClickHouse. It covers redacted versus
blocked output, a blocking plugin, a plugin that rewrites the usage object, an
upstream that reports no usage, and a retried request. The budget-counter and
response-cache cases need Redis. They read `ROLTER_TEST_REDIS_URL`, skip when
it is unset, and isolate themselves by a unique org id and cache namespace. The
nextest job in `quality.yml` runs a Redis service for them. To run them
locally:

```bash
docker run -d --rm --name rolter-test-redis -p 127.0.0.1:26379:6379 redis:8-alpine
ROLTER_TEST_REDIS_URL=redis://127.0.0.1:26379/ \
  cargo test -p rolter-gateway --test withheld_usage
docker rm -f rolter-test-redis
```
