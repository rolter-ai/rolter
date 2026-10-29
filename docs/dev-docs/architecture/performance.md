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
  every strategy and the prefix trie, the breaker and cooldown registries, and
  the hot-path bench below. Compiled on every PR via clippy so they cannot
  bit-rot, but not run (timings are noisy on shared runners).
- **End-to-end load** (`just bench-sim` / `bench-vllm` / `bench-sglang`) —
  `integration/engines/bench.py` against a real engine, measuring rolter's added
  latency directly and its behaviour under sustained concurrency.
- **Allocation counting** (`cargo test -p rolter-gateway --test
hot_path_allocations`) — asserts that the per-attempt admission checks make
  **zero** allocations. This one _is_ a merge gate, because it measures a
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
Zero is deliberate: an allocation _budget_ would be a portability trap across
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

### Hot-path bench (#1054)

`crates/rolter-gateway/benches/hot_path.rs` (`just bench-hot-path`) measures the
path a request takes to a target, and prints an **allocs/op table before the
timings**. It installs the same per-thread counting allocator as
`hot_path_allocations.rs`, but only for the bench binary, so it reports counts
for paths too large to assert zero on (a pick over 128 targets, a pool past the
one-word eligibility mask) and a regression shows up as a number that moved.
A pipeline pick up to 64 targets allocates nothing since #3249 and is asserted
in `hot_path_allocations.rs`, beside the key-pool mask read. It covers:

- `admission` — `Breaker::allows`, `Cooldowns::is_parked` and `Health::is_healthy`
  for an 8-target route, all healthy.
- `eligible_pick/<strategy>/<variant>/<targets>` — one `pick_eligible` the way
  `TargetPool::pick` makes it, over 4, 24, 64 and 128 targets, with one target
  parked, one unhealthy, both at load 0 so a strategy that ignores eligibility
  reads them as the emptiest.
- `snapshot_read` — `AppState.snapshot.load()` plus the route lookup, over 10 and
  1000 routes.
- `key_pool` — a multi-key provider's key pick asking which of its 8 keys are
  parked, once per key and as one mask read (#3249).

The load harness is not duplicated: `just load-sim` (above) already drives
`fake-llm`-style traffic through `/v1/*` for throughput and p50/p99, and is the
place to look at end-to-end numbers. The bench here is for the pieces that run
inside one request.

**Regression signal.** Nothing gates on timings. To compare a branch with
`master`, run the `bench-hot-path` workflow (Actions, _Run workflow_): it is
`workflow_dispatch`-only and not part of `ci-ok`, it benches the base ref with
`--save-baseline base` and then the dispatched ref with `--baseline base`, and
prints criterion's change report and the allocs/op table in the job summary.
Locally, the same two commands on two checkouts do it. Read the allocs/op column
first: a count is exact where a shared-runner duration swings by tens of
percent.

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

| Metric                   | Notes                                                               |
| ------------------------ | ------------------------------------------------------------------- |
| `ttft_p50/p95_ms`        | time to first byte                                                  |
| `itl_p50/p95_ms`         | inter-token latency, streaming only, needs `--max-tokens > 1`       |
| `latency_p50/p95/p99_ms` | end to end                                                          |
| `requests_per_second`    | achieved, not offered                                               |
| `error_rate`             | non-2xx and transport failures, which is what saturation looks like |
| `added_latency_p50_ms`   | rolter minus direct — the headline overhead number                  |

Profiles: `--concurrency` holds closed-loop steady state after a warmup, and
`--sweep 1,2,4,8,16` walks concurrency upward — a ramp/burst profile — reporting
**max sustainable RPS**: the highest achieved throughput whose p99 stayed inside
`--knee-factor` (default 2×) of the lowest-concurrency baseline _and_ which was
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

The benchmark uses the public `rolter-dummy` route, pinned to `engine-1`, so
direct and gateway phases hit the same upstream. The `dummy-<strategy>` routes
remain two-target pools for the compatibility smoke. A harness regression test
checks that the model sent by `bench.py` names exactly that single-target route;
otherwise the smoke can pass while the measured gateway requests all return 404.

Two measurement rules worth knowing when reading the JSON:

- **Failed requests are counted, never timed.** A connection refused in 0.1 ms
  is not a fast request; letting it into the percentiles would make a saturated
  system look quicker than a healthy one. They appear in `error_rate` instead.
- **ITL keys are absent, not zero, when nothing streamed.** A reported ITL of 0
  would read as "instant tokens" rather than "not measured".

### Baseline

**Hot-path bench, a development VM (not the fixed-hardware baseline).** Measured
on a cloud VM: 4 vCPU Intel Xeon @ 2.8 GHz, 16 GB, shared host, `release`
profile, `--warm-up-time 1 --measurement-time 2`, one run. Treat the timings as
an order of magnitude; the allocation counts are exact. The numbers below are the
state before the eligibility change of #1942; the same bench after it, with
the per-target and snapshot variants side by side, is in
[Load balancing](load-balancing.md#eligibility-cost).

| path                                                   | time (median)           | allocs/op    |
| ------------------------------------------------------ | ----------------------- | ------------ |
| `admission`, 8 targets, three registries               | 0.99 µs                 | 0            |
| `snapshot_read`, 1000 routes                           | 37 ns                   | 0            |
| `eligible_pick/round_robin`, 4 to 128 targets          | 18 to 19 ns             | 0            |
| `eligible_pick/power_of_two`, 4 to 128 targets         | 19 to 20 ns             | 0            |
| `eligible_pick/cache_aware`, 4 targets / 24 / 64 / 128 | 7.7 / 82 / 288 / 578 µs | 0, 0, 0, 1   |
| `eligible_pick/pipeline`, 4 targets / 24 / 64 / 128    | 15 / 95 / 292 / 572 µs  | 6, 9, 10, 11 |

`cache_aware` and `pipeline` are dominated by their own work, not the
eligibility closure: the bench has every target hold the same prompt, so the
prefix trie match runs in full per target. Both scale linearly with the pool.

#### Fixed-hardware load snapshot (#894)

Recorded on **rtx**, October 28, 2026. These are observations of the default
development-profile harness, **not release-build capacity or a regression gate**.

- **Hardware:** bare-metal AMD Ryzen 9 5900X, 12 physical cores / 24 threads,
  62.71 GiB RAM; NVIDIA RTX 3090 at 0% compute utilization (desktop display
  allocations remained). CPU governor `performance`; no other builds or inference
  engines running. The existing idle Postgres container was left untouched.
- **Host:** CachyOS Linux `7.3.0-4-cachyos`, Docker Engine `29.9.2`,
  Python `3.14.8`, Rust `1.99.1`.
- **Engine:** `ghcr.io/llm-d/llm-d-inference-sim:v0.12.0`, image digest
  `sha256:c6838d8616c9818f4cdb0e9ef7597e483a71c081b869d34c7eafac1481d76c60`,
  default `--mode random`. Two simulator containers run; the benchmark route
  deliberately uses only `engine-1` so both phases measure that same upstream.
- **Code:** rolter `1.0.0`, clean checkout
  `d3565b3ff658b675b70349a5cdecfa14a7c4f747` (base
  `9bf2d713de9ab25f07382e8c21bdff8d79815f95` plus the #3400 route correction).
  `just load-sim` builds `rolter-gateway` in the **dev** profile, not `--release`.
- **Default sweep:** `LOAD_LEVELS=1,2,4,8,16`, `LOAD_REQUESTS=200`,
  `LOAD_WARMUP=10`, `LOAD_MAX_TOKENS=16` for streaming. Non-streaming requests
  use the harness default of **1 token**. The p99 knee factor is 2.

Both complete default runs had **zero errors** at every level, directly and
through rolter. The added median latency below is the gateway p50 minus the
direct p50 at concurrency 1; sweep JSON does not itself emit an
`added_latency_p50_ms` key. ITL is also reported at concurrency 1, in milliseconds.

| Run | Mode          | `added_latency_p50_ms` | Max sustainable RPS, direct | Max sustainable RPS, rolter | `sustainable_rps_retained` | ITL p50, direct / rolter |
| --- | ------------- | ---------------------- | --------------------------- | --------------------------- | -------------------------- | ------------------------ |
| 1   | non-streaming | 0.902                  | 2395.440                    | 1237.896                    | 0.5168                     | not measured             |
| 2   | non-streaming | 0.938                  | 2690.994                    | 1730.669                    | 0.6431                     | not measured             |
| 1   | streaming     | 0.800                  | 2530.995                    | 1887.192                    | 0.7456                     | 0.008 / 0.022            |
| 2   | streaming     | 0.790                  | 2177.161                    | 1307.300                    | 0.6005                     | 0.008 / 0.023            |

Raw artifacts, with every concurrency level retained:

- Run 1: [non-streaming](load-baseline-rtx/run-1/load-non-streaming.json),
  [streaming](load-baseline-rtx/run-1/load-streaming.json).
- Run 2: [non-streaming](load-baseline-rtx/run-2/load-non-streaming.json),
  [streaming](load-baseline-rtx/run-2/load-streaming.json).

**Why the throughput figures differ by more than 5%.** The default sample is
short: 200 successful requests and sub-millisecond direct responses. The p99
ceiling is twice the concurrency-1 p99, and a small tail change can admit or
reject an entire concurrency level. In the non-streaming runs the gateway's
winning level moved from 2 to 4; for streaming it moved from 4 to 2. This is
not evidence of changing engine capacity. The Python stdlib client and
simulator share the host with a dev-profile gateway, so the figures describe
this harness as well as the gateway.

To check sampling sensitivity rather than select a favourable default run,
two additional sweeps used `LOAD_REQUESTS=2000`, with every other setting
unchanged and no competing builds or engines. These ran on the same base with
the identical #3400 route correction before it was committed; their tree was
not clean, unlike the two default runs above. Both longer sweeps selected
concurrency 2 directly and 4 through rolter, for both modes:

| Diagnostic | Mode          | Added p50 ms | Direct RPS | Rolter RPS | Retained | ITL p50, direct / rolter |
| ---------- | ------------- | ------------ | ---------- | ---------- | -------- | ------------------------ |
| 1          | non-streaming | 0.857        | 2576.701   | 1882.290   | 0.7305   | not measured             |
| 2          | non-streaming | 0.853        | 2643.711   | 1855.856   | 0.7020   | not measured             |
| 1          | streaming     | 0.756        | 2652.206   | 1910.481   | 0.7203   | 0.008 / 0.021            |
| 2          | streaming     | 0.760        | 2514.988   | 1902.405   | 0.7564   | 0.009 / 0.021            |

- Diagnostic 1: [non-streaming](load-baseline-rtx/diagnostic-1/load-non-streaming.json),
  [streaming](load-baseline-rtx/diagnostic-1/load-streaming.json).
- Diagnostic 2: [non-streaming](load-baseline-rtx/diagnostic-2/load-non-streaming.json),
  [streaming](load-baseline-rtx/diagnostic-2/load-streaming.json).

This supports a sampling-sensitive knee, not a 5%-stable capacity claim.
Do not compare a future single default run to one selected number here as a
regression threshold; keep repeated runs, error rates and the accepted
concurrency level together.

**Dispatch recommendation:** use an opt-in `workflow_dispatch` on an isolated,
fixed-hardware runner with exclusive concurrency, pinned code/image provenance
and repeated artifacts. Keep it outside `ci-ok`; shared hosted runners are
useful for harness smoke tests, not absolute capacity comparisons. Track the
job and its sampling policy in #3401 rather than adding infrastructure here.

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

The SGLang profile in `docker/docker-compose.engines.yml` is a **Xeon CPU**
build, not a portable x86 smoke. It remains intentionally pinned to v0.5.21:
the v0.5.22 kernels target AVX-512/BF16 and AMX, which the Ryzen 5900X on rtx
does not expose. No SGLang runtime success on that host is claimed, for either
version. #3414 tracks validation and the next bump on compatible hardware.

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
pull requests that touch engine paths. A newer push to the same pull request
cancels the smoke still running for the older commit; a manual dispatch is never
cancelled by another run. Dispatch it manually with
`engine=vllm` to smoke the real CPU engine (Actions tab, or
`gh workflow run "engine integration" -f engine=vllm`). SGLang remains
available through the local `just integration-sglang` command, but its
source-built CPU image is currently too heavy for the shared CI gate. This
suite is for compatibility, not a performance gate.
When [ROL-67](https://linear.app/rolter/issue/ROL-67/openaianthropic-requestresponse-translation-streaming)
lands, add the equivalent `/v1/messages` assertion through the gateway.
