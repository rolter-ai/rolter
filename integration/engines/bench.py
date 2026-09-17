#!/usr/bin/env python3
"""Emit comparable direct-engine and rolter latency samples as JSON.

Drives an engine directly and through rolter in the same run, under the same
conditions, so `added_latency_p50_ms` is a like-for-like number rather than two
runs subtracted.

Sequential mode measures per-request overhead. `--concurrency` holds a
closed-loop steady state, `--sweep` finds the maximum sustainable RPS before the
p99 knee, and `--ramp` walks concurrency upward for a burst profile (#847).

Deliberately stdlib-only: rolter must be operable air-gapped, and a benchmark
you cannot run on the disconnected machine you are tuning is not much of a
benchmark. See docs/dev-docs/architecture/performance.md for why this was extended
rather than replaced with GuideLLM or oha/k6.
"""

from __future__ import annotations

import argparse
import json
import statistics
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Dict, List, Optional


@dataclass
class Sample:
    """One request's timings. `ok=False` means it failed; a failed request has
    no meaningful latency and is counted, not timed."""

    ttft_ms: float = 0.0
    total_ms: float = 0.0
    # gaps between consecutive streamed tokens; empty for non-streaming or a
    # single-token response, where there is no gap to measure
    itl_ms: List[float] = field(default_factory=list)
    ok: bool = True


def percentile(values: List[float], value: float) -> float:
    if not values:
        return 0.0
    return sorted(values)[max(0, min(len(values) - 1, round((len(values) - 1) * value)))]


def sample(base_url: str, stream: bool, max_tokens: int = 1, timeout: float = 90) -> Sample:
    """Issue one request and time it.

    For streaming, each SSE `data:` frame carrying a content delta is treated as
    a token boundary, which is what makes inter-token latency measurable at all.
    """
    payload = json.dumps({
        "model": "rolter-dummy",
        "messages": [{"role": "user", "content": "Reply with one token."}],
        "max_tokens": max_tokens,
        "temperature": 0,
        "stream": stream,
    }).encode()
    request = urllib.request.Request(
        f"{base_url}/v1/chat/completions", data=payload, headers={"content-type": "application/json"}
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            first_byte: Optional[float] = None
            token_times: List[float] = []
            if stream:
                for raw in response:
                    now = time.perf_counter()
                    if first_byte is None:
                        first_byte = now
                    line = raw.strip()
                    if not line.startswith(b"data:"):
                        continue
                    body = line[5:].strip()
                    if body == b"[DONE]":
                        break
                    # only frames that actually carry text are token boundaries;
                    # role-only and finish-reason frames would inflate the count
                    # and depress ITL toward zero
                    try:
                        delta = json.loads(body)["choices"][0].get("delta", {})
                    except (ValueError, KeyError, IndexError):
                        continue
                    if delta.get("content"):
                        token_times.append(now)
            else:
                while True:
                    chunk = response.read(1)
                    if not chunk:
                        break
                    if first_byte is None:
                        first_byte = time.perf_counter()
    except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, OSError):
        # saturation shows up as errors, so they are a result, not a crash
        return Sample(ok=False)
    finished = time.perf_counter()
    return Sample(
        ttft_ms=((first_byte or finished) - started) * 1_000,
        total_ms=(finished - started) * 1_000,
        itl_ms=[(b - a) * 1_000 for a, b in zip(token_times, token_times[1:])],
        ok=True,
    )


def metrics(samples: List[Sample], elapsed_seconds: float) -> Dict[str, float]:
    """Summarise a phase. Latency percentiles cover successful requests only —
    a connection refused in 0.1ms is not a fast request, and averaging it in
    would make a saturated system look quicker than a healthy one."""
    ok = [s for s in samples if s.ok]
    ttft = [s.ttft_ms for s in ok]
    total = [s.total_ms for s in ok]
    itl = [gap for s in ok for gap in s.itl_ms]
    out = {
        "count": len(samples),
        "ok_count": len(ok),
        "error_rate": round((len(samples) - len(ok)) / len(samples), 4) if samples else 0.0,
        "ttft_p50_ms": round(percentile(ttft, 0.50), 3),
        "ttft_p95_ms": round(percentile(ttft, 0.95), 3),
        "latency_p50_ms": round(percentile(total, 0.50), 3),
        "latency_p95_ms": round(percentile(total, 0.95), 3),
        "latency_p99_ms": round(percentile(total, 0.99), 3),
        "latency_mean_ms": round(statistics.mean(total), 3) if total else 0.0,
        "requests_per_second": round(len(ok) / elapsed_seconds, 3) if elapsed_seconds > 0 else 0.0,
    }
    if itl:
        out["itl_p50_ms"] = round(percentile(itl, 0.50), 3)
        out["itl_p95_ms"] = round(percentile(itl, 0.95), 3)
        out["itl_mean_ms"] = round(statistics.mean(itl), 3)
        out["itl_samples"] = len(itl)
    return out


def run_phase(
    base_url: str,
    stream: bool,
    requests: int,
    concurrency: int,
    max_tokens: int,
    timeout: float,
) -> Dict[str, float]:
    """Run `requests` requests at `concurrency` in flight, closed-loop.

    Elapsed time is measured around the whole phase, so `requests_per_second` is
    throughput actually achieved rather than a rate that was offered.
    """
    started = time.perf_counter()
    if concurrency <= 1:
        samples = [sample(base_url, stream, max_tokens, timeout) for _ in range(requests)]
    else:
        with ThreadPoolExecutor(max_workers=concurrency) as pool:
            samples = list(
                pool.map(lambda _: sample(base_url, stream, max_tokens, timeout), range(requests))
            )
    return metrics(samples, time.perf_counter() - started)


def warmup(base_url: str, stream: bool, count: int, max_tokens: int, timeout: float) -> None:
    """Absorb connection setup, pool fill and engine JIT before sampling. These
    land in the first requests and would otherwise skew the tail of a short run."""
    for _ in range(max(0, count)):
        sample(base_url, stream, max_tokens, timeout)


def max_sustainable_rps(levels: List[Dict[str, float]], knee_factor: float) -> Dict[str, float]:
    """Highest achieved RPS whose p99 stayed inside `knee_factor` of the
    lowest-concurrency baseline, and which was not erroring.

    "Max RPS" without a latency bound is a meaningless number: throughput
    usually keeps climbing while latency collapses, so the peak alone describes
    a system nobody would run. The knee is where the useful capacity ends.
    """
    if not levels:
        return {}
    baseline = levels[0]["latency_p99_ms"]
    ceiling = baseline * knee_factor
    healthy = [
        level
        for level in levels
        if level["latency_p99_ms"] <= ceiling and level["error_rate"] == 0.0
    ]
    best = max(healthy, key=lambda level: level["requests_per_second"], default=None)
    return {
        "baseline_p99_ms": round(baseline, 3),
        "knee_factor": knee_factor,
        "p99_ceiling_ms": round(ceiling, 3),
        "max_sustainable_rps": best["requests_per_second"] if best else 0.0,
        "at_concurrency": best["concurrency"] if best else 0,
        # the first level that broke the bound, so a reader can see where it
        # went rather than only that it did
        "knee_at_concurrency": next(
            (
                level["concurrency"]
                for level in levels
                if level["latency_p99_ms"] > ceiling or level["error_rate"] > 0.0
            ),
            None,
        ),
    }


def sweep(
    base_url: str,
    stream: bool,
    requests: int,
    concurrency_levels: List[int],
    max_tokens: int,
    timeout: float,
    knee_factor: float,
    warmup_count: int,
) -> Dict[str, object]:
    levels = []
    for concurrency in concurrency_levels:
        warmup(base_url, stream, warmup_count, max_tokens, timeout)
        level = run_phase(base_url, stream, requests, concurrency, max_tokens, timeout)
        level["concurrency"] = concurrency
        levels.append(level)
    return {"levels": levels, "summary": max_sustainable_rps(levels, knee_factor)}


def parse_levels(raw: str) -> List[int]:
    levels = [int(part) for part in raw.split(",") if part.strip()]
    if not levels or any(level < 1 for level in levels):
        raise ValueError("concurrency levels must be positive integers")
    # ascending, so the first level is the least-loaded baseline the knee is
    # measured against
    return sorted(levels)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine-url", required=True)
    parser.add_argument("--gateway-url", required=True)
    parser.add_argument("--requests", type=int, default=10, help="requests per phase")
    parser.add_argument("--concurrency", type=int, default=1, help="in-flight requests")
    parser.add_argument("--stream", action="store_true")
    parser.add_argument(
        "--max-tokens",
        type=int,
        default=1,
        help="tokens to request; must exceed 1 for inter-token latency to exist",
    )
    parser.add_argument("--warmup", type=int, default=1, help="unsampled requests before each phase")
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument(
        "--sweep",
        metavar="LEVELS",
        help="comma-separated concurrency levels, e.g. 1,2,4,8,16 — reports max sustainable RPS",
    )
    parser.add_argument(
        "--knee-factor",
        type=float,
        default=2.0,
        help="p99 multiple of the baseline that counts as the latency knee",
    )
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.requests < 1:
        parser.error("--requests must be >= 1")
    if args.concurrency < 1:
        parser.error("--concurrency must be >= 1")
    if args.knee_factor <= 1:
        parser.error("--knee-factor must be > 1")
    if args.stream and args.max_tokens < 2:
        # not fatal: streaming latency is still worth measuring, but say so
        # rather than emit an ITL-less streaming report that looks broken
        print("note: --max-tokens is 1, so no inter-token latency will be recorded")

    result: Dict[str, object] = {
        "stream": args.stream,
        "requests": args.requests,
        "max_tokens": args.max_tokens,
    }

    if args.sweep:
        levels = parse_levels(args.sweep)
        result["mode"] = "sweep"
        result["concurrency_levels"] = levels
        result["direct"] = sweep(
            args.engine_url, args.stream, args.requests, levels,
            args.max_tokens, args.timeout, args.knee_factor, args.warmup,
        )
        result["through_rolter"] = sweep(
            args.gateway_url, args.stream, args.requests, levels,
            args.max_tokens, args.timeout, args.knee_factor, args.warmup,
        )
        direct_rps = result["direct"]["summary"].get("max_sustainable_rps", 0.0)
        rolter_rps = result["through_rolter"]["summary"].get("max_sustainable_rps", 0.0)
        result["sustainable_rps_retained"] = (
            round(rolter_rps / direct_rps, 4) if direct_rps else 0.0
        )
    else:
        result["mode"] = "fixed"
        result["concurrency"] = args.concurrency
        warmup(args.engine_url, args.stream, args.warmup, args.max_tokens, args.timeout)
        result["direct"] = run_phase(
            args.engine_url, args.stream, args.requests, args.concurrency,
            args.max_tokens, args.timeout,
        )
        warmup(args.gateway_url, args.stream, args.warmup, args.max_tokens, args.timeout)
        result["through_rolter"] = run_phase(
            args.gateway_url, args.stream, args.requests, args.concurrency,
            args.max_tokens, args.timeout,
        )
        result["added_latency_p50_ms"] = round(
            result["through_rolter"]["latency_p50_ms"] - result["direct"]["latency_p50_ms"], 3
        )

    with open(args.output, "w", encoding="utf-8") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
