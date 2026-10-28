# Semantic guard policies on streamed responses: refuse or buffer, never bypass

**Status:** Proposed · **Date:** 27 Oct 2026 · **Issues:** [#459](https://github.com/rolter-ai/rolter/issues/459)

## Context

#459 turns the single deployment-wide guardrail webhook into scoped semantic
guard policies: several guardrail providers can be enabled, a policy scoped to
the deployment, an organization, a project, a route or a virtual key picks the
providers that apply to a request, and each provider runs at its own stage —
`pre_call` on the request body, `post_call` on the response body. A `post_call`
provider judges a whole completion: "is this answer toxic", "does it leak the
system prompt", "is this a refusal the caller should not see". Until #459 the
`post_call` stage was stored and validated but never run.

A streamed response is never a whole completion while it is being delivered.
The gateway forwards Server-Sent Events as they arrive, so by the time the last
event exists every earlier one has already reached the caller. The other
whole-body controls already settled what happens then, each the same way:

- built-in output guardrails (#591): `streaming_post_call = "reject"` by
  default, `"allow"` as an explicit, documented opt-out;
- `post_response` plugins (#1776): a fail-closed plugin refuses a stream, a
  fail-open one keeps streaming because it already chose delivery without its
  verdict;
- the PII sanitizer's response leg (#848, ADR-0036): refused by default,
  `streaming = "passthrough"` as an explicit opt-out.

The common rule is that a client's `"stream": true` never switches off a control
the operator turned on. The semantic stage needs a strategy that keeps that rule
and is tested, because a silent bypass here is the sharpest correctness risk the
issue names: a policy that blocks toxic answers on `stream: false` and lets the
same answer through on `stream: true` is a policy any caller can turn off.

## Options

### A. Preflight only

Run only the `pre_call` providers on a streamed request and skip `post_call`.
This is what the gateway did before #459, and it is exactly the silent bypass
the issue rules out: nothing tells the operator their response policy did not
run. Rejected as a default and as an option. An explicit opt-out in the style of
`streaming_post_call = "allow"` was considered and left out too — an operator
who does not want a response check on streams can scope the `post_call` provider
away from the routes that stream, which is visible in the policy itself rather
than in a flag.

### B. Refuse the stream

Answer a streamed request whose resolved policy has a `post_call` provider with
`400 guardrail_streaming_unsupported` (the code the built-in output rules
already use), before any upstream call, so nothing is billed. The caller can
retry without `"stream": true`. Cheap, obviously correct, and consistent with
every other whole-body control. Its cost is that the policy turns streaming off
for the scope it covers.

### C. Buffer the final result

Let the upstream stream, but hold it in the gateway: read every event (after
dialect translation, so it is the client's dialect), assemble the completion's
text from the deltas, consult the `post_call` providers once on the assembled
text, and then either replay the buffered events to the caller unchanged or
answer `403 guardrail_blocked` in their place. The caller keeps its SSE client
and its streaming code path; what it loses is incremental delivery — time to
first token becomes time to last token.

Details that make it safe rather than merely possible:

- **A bound.** The buffer is capped (16 MiB). A stream that outgrows it is
  withheld, not truncated and not delivered unchecked.
- **Transforms.** A provider's `transform` cannot be re-encoded into another
  dialect's event sequence without rewriting usage, ids and finish reasons. A
  `transform` verdict on a buffered stream withholds the response rather than
  delivering the untransformed one.
- **Failure modes still apply.** A provider that times out, is overloaded
  (non-2xx, including `429` and `503`) or answers something that is not a
  decision resolves through its own `fail_open` / `fail_closed`, exactly as on a
  non-streamed body.
- **Billing.** The upstream call happened; a withheld stream is logged and
  billed like a withheld non-streamed completion (#1478).
- **The response cache.** A cached stream replays through the same delivery
  policy as a cached body, so a hit is judged by the request it answers.

### D. Per-event checks

Consult the providers on every event, or on a sliding window, and cut the
stream when one objects. Rejected: a classifier sees a fragment, so its verdict
on "is this toxic" or "does this leak the prompt" is weaker than on the whole
answer (a false negative can straddle any window boundary), a provider call per
event multiplies the provider's load by the token count, and by the time an
event is judged the earlier ones are already delivered, so a block can only cut
the stream short — the caller has already seen part of what the policy forbids.
It would also need a new streaming contract for providers. A future windowed
mode can still be added as a third value of the same setting.

## Decision

Implement **B as the default and C as an opt-in**, per policy:

```toml
[[semantic_guards.policies]]
name = "support-bots"
scope = "project"
scope_id = "…"
providers = ["toxicity", "prompt-injection"]
streaming = "reject"   # default; or "buffer"
```

There is no value that serves a stream with the `post_call` providers skipped,
so neither strategy can bypass the policy silently. A policy with no `post_call`
provider leaves streaming untouched: `pre_call` providers run on the request
before any streaming starts, whichever strategy is set.

Every refused stream counts in `rolter_guardrail_stream_rejections_total`, and
every buffered one in `rolter_guardrail_streams_buffered_total`, so an operator
can see both the cost of B and how often C is in use.

## Consequences

- A scope whose policy has a `post_call` provider either cannot stream (B) or
  streams with no incremental delivery (C). The policy says which, so the cost
  is visible where the policy is written.
- Under C a long generation sends nothing until it finishes. A client or proxy
  with an idle timeout shorter than the generation time can give up first; the
  user docs say so.
- The buffered stream is held in gateway memory up to the cap per request.
- Providers receive a streamed completion as `{"stream": true, "text": "…"}`
  under `content` (`streamed: true` on the envelope) rather than as the event
  sequence, so a provider needs no SSE parser.

## Open questions

- Whether a windowed per-event mode (D) is worth a later opt-in for providers
  that are built for fragments, such as a fast keyword classifier.
- Whether C should send SSE comment heartbeats while buffering, to keep idle
  timeouts at bay. It would commit the response status (`200`) before the
  verdict exists, so a block could no longer be a `403`; left out for now.
