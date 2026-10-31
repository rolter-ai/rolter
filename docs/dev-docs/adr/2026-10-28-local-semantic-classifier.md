# An in-process local classifier for semantic guard policies

**Status:** Accepted · **Date:** 28 Oct 2026 · **Issues:** [#3329](https://github.com/rolter-ai/rolter/issues/3329), [#459](https://github.com/rolter-ai/rolter/issues/459)

## Context

#459 made semantic guard providers scoped and plural (ADR-0037 covers their
streams), but every provider is an external HTTP service spoken to over the
guardrail webhook contract. A deployment that wants a small prompt-injection or
toxicity classifier has to run a sidecar, and pays a network round trip per
request and per stage, plus the operational cost of one more service that can
be down.

The policy model does not care where a verdict comes from. A provider answers a
`ScoredDecision` (`allow`/`block`/`transform` plus `categories` scored in
`[0, 1]`); the policy holds the scores to its thresholds, and a call that
produced no verdict resolves through the provider's `failure_mode`. A provider
that runs in the gateway process can plug into exactly that seam, as long as it:

- answers the same `ScoredDecision`, so policies, thresholds, metrics and
  failure modes apply unchanged;
- cannot starve the request path of CPU, which a network call never could;
- reaches air-gapped deployments without the gateway downloading anything;
- costs nothing in a gateway that does not use it.

This ships after 1.0.0 as an additive provider kind, so the recommendation is
deliberately conservative: the smallest runtime that is useful, with the door
open to a heavier one.

## Options

### Runtime

| Runtime                            | Binary / build cost                                                                                                                                                                              | CPU and platform                                                                                  | Licence, maintenance                                                                                  | What it can run                                           |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| ONNX Runtime via `ort`             | links a prebuilt `libonnxruntime` (tens of MB) or builds it from C++ source (tens of minutes); `download-binaries` fetches at build time, which the distroless and offline builds cannot rely on | prebuilt binaries assume AVX on x86-64; a shared library must be copied into the distroless image | MIT/Apache; `ort` 2.x was in release candidates for most of its life and tracks ONNX Runtime releases | any exported transformer (DeBERTa, DistilBERT, MiniLM)    |
| `candle` (pure Rust)               | no C++ toolchain, but a large dependency graph (`candle-core`, `candle-nn`, `candle-transformers`, `tokenizers`, `safetensors`); minutes of extra build time and several MB of binary            | portable; SIMD where the compiler finds it; slower than ONNX Runtime on CPU                       | MIT/Apache, actively maintained, API still moving between minor releases                              | the architectures candle implements                       |
| tokenizer + linear head (this ADR) | a few hundred lines in `rolter-gateway`, no new dependency (`sha2` and `serde_json` are already in the tree)                                                                                     | portable, no SIMD needed; microseconds per KiB of text                                            | ours                                                                                                  | hashed n-gram logistic regression, one score per category |

A transformer is the better classifier for subtle prompt injection; a hashed
n-gram logistic head is the classic baseline for toxicity, spam and keyword-ish
injection ("ignore all previous instructions") and is what a team can train on
a laptop from a labelled CSV. The transformer runtimes cost every gateway build
and image, and `cargo hack --each-feature` has to keep each of them compiling
warning-free; the linear head costs nothing measurable.

### Opt-in

A cargo feature buys nothing for the linear head: it adds no dependency, and a
feature-gated kind would give a snapshot-fed gateway a provider it can parse and
not run. A feature is the right gate for a heavy runtime, which is where the
binary-size and build-time cost lives.

### Model shipping

The gateway runs from a distroless image, often with no egress at all. A model
can reach it as:

1. a file path in config, the file delivered by a mounted volume (a Kubernetes
   `ConfigMap`/`Secret` for a small model, a `PersistentVolume` or an init
   container for a larger one, a bind mount under Docker);
2. a URL the gateway downloads at start;
3. bytes inlined into the config or the snapshot.

(2) is a runtime dependency on a model host and a supply-chain path the
operator did not ask for; (3) puts megabytes into every snapshot poll.

## Decision

**Runtime.** Ship a dependency-free classifier in `rolter-gateway`: a word
tokenizer, hashed word n-gram features and one logistic head per category, read
from a JSON weights file in the format `rolter-linear/1`. The tokenizer
lowercases, splits on every character that is not alphanumeric, forms word
n-grams of length 1 to `ngrams` joined by one space, hashes each with 64-bit
FNV-1a into `buckets` and marks the bucket present. A category's score is
`sigmoid(bias + Σ weight[bucket])` over the present buckets. The format is
simple enough to produce from scikit-learn (the user docs carry a training
script) and to read in a page of Rust. ONNX through `ort` is the follow-up
runtime, behind an `onnx` cargo feature off by default, tracked in
[#3443](https://github.com/rolter-ai/rolter/issues/3443); nothing in this design has to change for it but a second `format`.

**The provider kind.** A semantic guard provider gains `kind`: `webhook` (the
default, every existing provider) or `local`. A local provider names
`model_path` and `model_sha256` instead of `url` and `auth`; `stage`,
`timeout_ms`, `failure_mode` and `max_body_bytes` keep their meaning, and
`max_retries` does not apply (a local model gives the same answer twice). It
answers a `ScoredDecision` with its categories and `allow`, so blocking is the
policy's thresholds' job, exactly as for a pure webhook classifier. The text it
classifies is every string in the body the stage judges, except a fixed set of
structural fields (`model`, `role`, `type`, `id` and the like), bounded to
`max_body_bytes` (since #3513, scored in overlapping `max_body_bytes` windows up
to `max_scored_bytes`).

**Budgets.**

| Budget             | Limit                                                                                                                                                                                                                                                                              |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| model file         | at most 64 MiB on disk                                                                                                                                                                                                                                                             |
| model in memory    | `buckets × categories` dense `f32` weights, at most 2²² of them (16 MiB); `buckets` ≤ 2²⁰, ≤ 32 categories                                                                                                                                                                         |
| `ngrams`           | 1 to 3                                                                                                                                                                                                                                                                             |
| text per call      | the provider's `max_body_bytes` (64 KiB by default); since #3513 a longer text is scored in overlapping windows of that size up to `max_scored_bytes` (8 windows, 512 KiB, by default; at most 16 MiB and 64 windows)                                                              |
| latency per call   | about 1 ms per 64 KiB window on one core (release build, the test fixture model; measured for #3513), so about 10 ms for a text that fills the default budget; the provider's `timeout_ms` bounds the whole call, queueing and every window included, and must cover a full budget |
| per request        | one call per local provider per stage, as for a webhook                                                                                                                                                                                                                            |
| memory per gateway | the sum of the distinct models loaded (one copy per `sha256`, however many providers or paths share it), plus the previous apply's until the next; parsing a file peaks at roughly three times its size, transiently                                                               |

**Model shipping.** `model_path` is a path on the gateway's file system and
`model_sha256` its required SHA-256 pin. The gateway never downloads a model.
It loads the file when it applies a configuration (at start and on every hot
reload), keeps a loaded model across reloads while its path and pin are
unchanged, and refuses a file whose digest does not match the pin. Requiring
the pin means a swapped file on a shared volume cannot silently change what a
policy blocks: replacing a model is a configuration change, recorded like one.
Files are read on the blocking pool before a reload is applied, never on a
request worker, and only regular files are opened. The models of the previous
apply stay reachable until the next one, so a request still on the previous
snapshot keeps its model. A model that cannot be loaded (missing, not a regular
file, over the size cap, wrong digest, malformed) is logged once per apply and its provider produces no verdict, so it
resolves through its `failure_mode` like an unreachable webhook.

**CPU bound.** Inference runs on Tokio's blocking pool, never on a request
worker, behind one semaphore per gateway shared by every local provider.
`[semantic_guards] local_concurrency` sets its size; unset, it is half the
CPUs the process may use (at least one), since the blocking pool also serves
the rest of the gateway. Under a control plane the value comes from the
control plane's own config file, through the snapshot. A call waits for a permit at most its
provider's `timeout_ms`; a call that gets none is the **overload** case and
resolves through the provider's `failure_mode`, exactly like a webhook that
answered `429`/`503`. A call whose inference outlives the rest of `timeout_ms`
is a timeout. The cap is per gateway replica, like route concurrency, so a
fleet scales it with its replicas.

**Opt-in.** No cargo feature for the linear head; the `onnx` follow-up is the
one that gets a feature. A gateway with no local provider loads nothing and
holds an empty registry.

**Snapshot.** `kind`, `model_path` and `model_sha256` travel in
`/internal/snapshot` inside format 1. The kind enum has an `unknown` fallback
(`#[serde(other)]`) that fails closed: a provider of a kind this gateway does
not know blocks the stage whatever its `failure_mode`, so a future kind is
additive under ADR-0038. It joins the other fallback enums of #3347:
`GatewayConfig::fail_closed_unknown_values` names it in the gateway's warn line
(and leaves the value in place, since no known kind is a safe reading of it),
and a config file spelling an unknown kind is refused with the accepted values.
No released gateway predates the kind, since 1.0.0 serves format 1 with it.

**Control plane.** Dashboard-registered providers store `kind`,
`model_path` and `model_sha256` (migration 0190, relaxing the `url` check for
local rows). The provider trial endpoint stays webhook-only: the model lives
on the gateways' file system, not the control plane's.

## Consequences

- A prompt-injection or toxicity check needs no sidecar and no network hop, at
  the cost of a classifier far weaker than a transformer. The docs say so, and
  that a webhook in front of a real model is still the way to run one.
- Gateway CPU becomes part of the guard's budget. The semaphore keeps it
  bounded, and an exhausted cap is visible as `error_open`/`error_closed`
  outcomes in `rolter_guardrail_policy_decisions_total`.
- Every gateway that runs a local provider needs the same file at the same
  path; the pin makes a gateway with a different file fail its provider rather
  than judge differently.
- The weights format is ours to keep: `rolter-linear/1` is never changed in
  place, a new layout is a new `format` string.
- The ONNX runtime remains a follow-up behind an `onnx` cargo feature (#3443).

## Rejected

- **`ort` now.** The best classifier, but the native library, its build and
  its platform assumptions are the wrong first step for an additive kind in a
  distroless image; it stays the follow-up.
- **`candle` now.** No C++, but a large, moving dependency graph for every
  build, and slower than ONNX Runtime on CPU, so it is neither the light nor the
  fast option.
- **Downloading models at start.** A runtime dependency and a supply-chain path
  nobody opted into; an operator who wants it can run an init container.
- **An optional checksum.** A model on a shared volume changing under a running
  policy is exactly the change that must be visible.
- **Running inference on the request worker.** A long body would stall every
  other request on that worker.
