# Performance

Goal: beat the reference Python proxy (LiteLLM cites ~8ms P95 added latency at 1k RPS) with a much smaller per-request overhead in Rust.

## Hot-path principles

- **Lock-free config reads** — the routing table is an `ArcSwap<Snapshot>`; readers never block, even during a hot reload.
- **Minimal-copy streaming** — upstream responses are piped to the client as a `Body::from_stream` over the `reqwest` byte stream; rolter does not buffer whole responses.
- **Connection reuse** — pooled `reqwest` clients with HTTP/2 keep-alive and `tcp_nodelay`; one client per egress-proxy target, cached.
- **Cheap auth** — virtual-key lookup is an O(1) hash-map hit on the in-memory snapshot.
- **Avoid full deserialization** — only the fields needed for routing (`model`, `stream`) are read; the body is forwarded as raw bytes.
- **Logging off the hot path** — usage/cost rows are batched and written to ClickHouse asynchronously.
- **Release profile** — `lto = "thin"`, `codegen-units = 1`, `strip = true`.

## Things to watch

- The approximate cache-aware trie is per-route in-memory state; bound its size with eviction before it grows large.
- Per-request JSON parse for `model` is small but measurable; consider a fast path / partial parse for very high RPS.
- Prefer `bytes::Bytes` (ref-counted) over `Vec<u8>` copies when rewriting the model field.

## Benchmarking

Three layers. Only the third is on the per-PR gate:

- **Micro-benches** (`just bench`) — criterion, covering balancer `pick` across
  every strategy and the prefix trie. Compiled on every PR via clippy so they
  cannot bit-rot, but not run (timings are noisy on shared runners).
- **End-to-end load** (`just bench-sim` / `bench-vllm` / `bench-sglang`) —
  `integration/engines/bench.py` against a real engine, measuring rolter's added
  latency directly and its behaviour under sustained concurrency.
- **Allocation counting** (`cargo test -p rolter-gateway --test
  hot_path_allocations`) — asserts that the per-attempt admission checks make
  **zero** allocations. This one *is* a merge gate, because it measures a
  property rather than a duration.

### Why allocation counting is a test and not a benchmark

"Keep the hot path allocation-light" is a rule this document states and
AGENTS.md repeats, but timings alone cannot enforce it: a per-request
allocation costs tens of nanoseconds, which is well inside the noise of a
shared CI runner. That is why the criterion benches are compiled and not run —
and it means a regression like the `(String, usize)` key that `Breaker::allows`
and `Cooldowns::is_parked` once built could come back without anything failing.

`crates/rolter-gateway/tests/hot_path_allocations.rs` installs a counting
`#[global_allocator]` and asserts an exact **zero** on the steady-state paths.
Zero is deliberate: an allocation *budget* would be a portability trap across
allocators and toolchains, whereas "this path does not allocate at all" is both
stable and the property actually wanted. A count is exact and reproducible in a
way a duration is not, so this can gate where the benches cannot.

Two details worth knowing before extending it:

- **The counter is per-thread, not global.** Cargo runs a binary's tests on
  parallel threads, so a process-wide counter attributes a sibling test's
  ordinary allocations to whichever measurement happens to be open, and fails
  it for something it never did. Arming and counting are both thread-local, and
  a test asserts that property directly.
- **It is verified against an injected regression, not just observed passing.**
  A guard that cannot fail is worse than none, since it reads as evidence. Put
  a `model.to_string()` back into `Breaker::allows` and exactly the two breaker
  tests fail, with one allocation per candidate target; the cooldown tests,
  which touch none of the changed code, keep passing.

To extend it to another hot-path component, add a test that warms any lazily
built internals first — otherwise you measure first-call setup — then wraps the
steady-state operation in `counting(...)` and asserts zero.

### Tool decision (#847, closing #455)

The load harness **extends `bench.py`** rather than adopting
[GuideLLM](https://github.com/vllm-project/guidellm) or driving load with
`oha`/`k6`. Recorded here because #847 required settling it before writing code.

- **Air-gapped operation is a hard requirement for rolter.** `bench.py` is
  stdlib-only — no install step, no wheels or binaries to vendor. GuideLLM
  brings a substantial Python dependency tree; `oha`/`k6` are external binaries.
  A benchmark you cannot run on the disconnected machine you are tuning is not
  much of a benchmark.
- **The direct-vs-rolter delta is the entire point of this harness**, and no
  general-purpose load generator produces it. They measure one endpoint;
  `added_latency_p50_ms` needs both driven identically in the same run under the
  same conditions. Adopting one of them means keeping `bench.py` anyway for the
  delta — two harnesses instead of one.
- **ITL needs SSE token boundaries.** `oha` and `k6` measure bytes and requests,
  not tokens, so they cannot produce inter-token latency at all. GuideLLM does
  understand tokens, but the two points above still stand.
- **Reproducible without credentials** — the default `sim` engine
  (`llm-d-inference-sim`) needs no API key, so anyone can reproduce a number.

The cost is carrying ~250 lines of stdlib Python for concurrency and knee
detection. That was judged cheaper than a dependency tree plus a second harness.
Revisit if the harness starts needing tokenizer-aware workload shaping, which is
where GuideLLM genuinely earns its footprint.

### What it measures

`bench.py` drives the engine directly and through rolter in the same run:

| Metric | Notes |
|---|---|
| `ttft_p50/p95_ms` | time to first byte |
| `itl_p50/p95_ms` | inter-token latency, streaming only, needs `--max-tokens > 1` |
| `latency_p50/p95/p99_ms` | end to end |
| `requests_per_second` | achieved, not offered |
| `error_rate` | non-2xx and transport failures, which is what saturation looks like |
| `added_latency_p50_ms` | rolter minus direct — the headline overhead number |

Profiles: `--concurrency` holds closed-loop steady state after a warmup, and
`--sweep 1,2,4,8,16` walks concurrency upward — a ramp/burst profile — reporting
**max sustainable RPS**: the highest achieved throughput whose p99 stayed inside
`--knee-factor` (default 2×) of the lowest-concurrency baseline *and* which was
not erroring.

That bound matters. Throughput usually keeps climbing well past the point where
latency collapses, so a peak RPS with no latency condition attached describes a
system nobody would actually run. The report also names the concurrency level
where the knee was crossed, so a reader can see where it went rather than only
that it did.

```bash
just bench-sim     # sequential: per-request added latency (fast, unchanged)
just load-sim      # concurrency sweep: max sustainable RPS, ITL, error rate
just test-bench    # harness unit tests — stdlib only, no engine or network
```

`load-*` is deliberately a separate recipe: a sweep across five concurrency
levels takes minutes, and silently making the added-latency run that much slower
would be a bad trade. Tune with `LOAD_LEVELS`, `LOAD_REQUESTS`, `LOAD_WARMUP`
and `LOAD_MAX_TOKENS`.

Two measurement rules worth knowing when reading the JSON:

- **Failed requests are counted, never timed.** A connection refused in 0.1 ms
  is not a fast request; letting it into the percentiles would make a saturated
  system look quicker than a healthy one. They appear in `error_rate` instead.
- **ITL keys are absent, not zero, when nothing streamed.** A reported ITL of 0
  would read as "instant tokens" rather than "not measured".

### Baseline

No baseline snapshot is recorded yet. It needs a run on fixed, documented
hardware, and a number taken on a laptop under thermal throttling would be worse
than none — an unreproducible baseline invites false regressions. Record
hardware, engine version and rolter version alongside the numbers when one is
taken.

## Inference engines

The ROL-238 suite checks rolter against OpenAI-compatible engine servers.
Output is intentionally meaningless; this validates the HTTP, OpenAI JSON, and
SSE contracts rather than model quality.

The default engine, `sim`, is [llm-d-inference-sim](https://github.com/llm-d/llm-d-inference-sim):
a ~30MB multi-arch vLLM API simulator that needs no model downloads and boots
in milliseconds, which makes the suite cheap enough to run as a regular PR
check. The real CPU vLLM and SGLang profiles remain for on-demand runs; they
use `trl-internal-testing/tiny-random-LlamaForCausalLM` only for its
configuration/tokenizer and initialize random weights with
`--load-format dummy` (with a `head_dim=64` override, since the CPU attention
kernels reject the model's native `head_dim=4`). The CPU vLLM profile uses
eager execution to avoid expensive compilation warm-up during CI smoke runs.

It runs on CPU in Docker and therefore works on GitHub-hosted runners. Each
engine profile starts two independent dummy upstreams so the gateway exercises
a real target pool. Run one engine locally:

```sh
just integration-sim
just integration-vllm
just integration-sglang
```

Each command boots routes for every balancing strategy (round-robin, random,
power-of-two, consistent-hash, cache-aware, weighted, pipeline, cheapest, and
fastest). It verifies `/v1/models`, non-streaming chat, and SSE both directly
and through rolter, and explicitly confirms round-robin reaches both targets.
Logs are kept in `artifacts/engines/<engine>/`.

### Local end-to-end run

`just integration-vllm` and `just integration-sglang` start two CPU
dummy-weight servers, render the gateway configuration, and run the OpenAI JSON
and SSE assertions. The runner cleans up all containers and child processes on
exit and preserves the combined engine log plus the gateway log under
`artifacts/engines/<engine>/`.

For manual inspection, start the selected two-server pool and leave it running:

```sh
docker compose -f docker/docker-compose.engines.yml --profile vllm up -d
# use profile sglang for ports 30000 and 30001
```

Render the gateway configuration in another terminal and start rolter:

```sh
config=$(mktemp)
sed \
  -e 's/__ROLTER_PORT__/4010/g' \
  -e 's/__ENGINE_1_PORT__/8000/g' \
  -e 's/__ENGINE_2_PORT__/8001/g' \
  integration/engines/rolter-dummy.toml.in >"$config"
cargo run -p rolter-gateway -- --config "$config"
```

Verify non-streaming JSON and streaming SSE through the gateway:

```sh
curl -i http://127.0.0.1:4010/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"dummy-round-robin","messages":[{"role":"user","content":"Reply with one token."}],"max_tokens":1,"temperature":0}'

curl -N http://127.0.0.1:4010/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"dummy-round-robin","stream":true,"messages":[{"role":"user","content":"Reply with one token."}],"max_tokens":1,"temperature":0}'
```

Exercise all configured strategies; each request must return a non-empty
OpenAI `choices` array:

```sh
for model in dummy-round-robin dummy-random dummy-power-of-two \
  dummy-consistent-hash dummy-cache-aware dummy-weighted dummy-pipeline \
  dummy-cheapest dummy-fastest; do
  curl -fsS http://127.0.0.1:4010/v1/chat/completions \
    -H 'content-type: application/json' \
    -d "{\"model\":\"$model\",\"messages\":[{\"role\":\"user\",\"content\":\"ping\"}],\"max_tokens\":1}" \
    | jq -e '.choices | length > 0' >/dev/null
done
```

Clean up the pool and temporary gateway configuration:

```sh
docker compose -f docker/docker-compose.engines.yml --profile vllm down --volumes
rm -f "$config"
```

For non-gating direct-versus-gateway samples, run `just bench-vllm` or `just
bench-sglang`. They record non-streaming and streaming p50/p95/p99 latency and
streaming first-byte time in JSON. Results only compare runs on the same host,
CPU image, engine versions, and host configuration; throughput thresholds are
deliberately not merge gates.

The `engine integration` workflow runs the `sim` smoke as a regular check on
pull requests that touch engine paths. Dispatch it manually with
`engine=vllm` to smoke the real CPU engine (Actions tab, or
`gh workflow run "engine integration" -f engine=vllm`). SGLang remains
available through the local `just integration-sglang` command, but its
source-built CPU image is currently too heavy for the shared CI gate. This
suite is for compatibility, not a performance gate.
When [ROL-67](https://linear.app/rolter/issue/ROL-67/openaianthropic-requestresponse-translation-streaming)
lands, add the equivalent `/v1/messages` assertion through the gateway.
