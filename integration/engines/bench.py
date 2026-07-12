#!/usr/bin/env python3
"""Emit comparable direct-engine and rolter latency samples as JSON."""

from __future__ import annotations

import argparse
import json
import statistics
import time
import urllib.request
from typing import Dict, List, Tuple


def percentile(values: List[float], value: float) -> float:
    return sorted(values)[max(0, min(len(values) - 1, round((len(values) - 1) * value)))]


def sample(base_url: str, stream: bool) -> Tuple[float, float]:
    payload = json.dumps({
        "model": "rolter-dummy",
        "messages": [{"role": "user", "content": "Reply with one token."}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": stream,
    }).encode()
    request = urllib.request.Request(
        f"{base_url}/v1/chat/completions", data=payload, headers={"content-type": "application/json"}
    )
    started = time.perf_counter()
    with urllib.request.urlopen(request, timeout=90) as response:
        first_byte = None
        while True:
            chunk = response.read(1)
            if not chunk:
                break
            if first_byte is None:
                first_byte = time.perf_counter()
    finished = time.perf_counter()
    return ((first_byte or finished) - started) * 1_000, (finished - started) * 1_000


def metrics(samples: List[Tuple[float, float]], elapsed_seconds: float) -> Dict[str, float]:
    ttft, total = map(list, zip(*samples))
    return {
        "count": len(samples),
        "ttft_p50_ms": round(percentile(ttft, 0.50), 3),
        "ttft_p95_ms": round(percentile(ttft, 0.95), 3),
        "latency_p50_ms": round(percentile(total, 0.50), 3),
        "latency_p95_ms": round(percentile(total, 0.95), 3),
        "latency_p99_ms": round(percentile(total, 0.99), 3),
        "latency_mean_ms": round(statistics.mean(total), 3),
        "requests_per_second": round(len(samples) / elapsed_seconds, 3),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine-url", required=True)
    parser.add_argument("--gateway-url", required=True)
    parser.add_argument("--requests", type=int, default=10)
    parser.add_argument("--stream", action="store_true")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.requests < 1:
        parser.error("--requests must be >= 1")

    # One warmup request per endpoint removes initialization noise from samples.
    sample(args.engine_url, args.stream)
    sample(args.gateway_url, args.stream)
    direct_started = time.perf_counter()
    direct = [sample(args.engine_url, args.stream) for _ in range(args.requests)]
    direct_elapsed = time.perf_counter() - direct_started
    gateway_started = time.perf_counter()
    gateway = [sample(args.gateway_url, args.stream) for _ in range(args.requests)]
    gateway_elapsed = time.perf_counter() - gateway_started
    result = {
        "stream": args.stream,
        "requests": args.requests,
        "direct": metrics(direct, direct_elapsed),
        "through_rolter": metrics(gateway, gateway_elapsed),
    }
    result["added_latency_p50_ms"] = round(
        result["through_rolter"]["latency_p50_ms"] - result["direct"]["latency_p50_ms"], 3
    )
    with open(args.output, "w", encoding="utf-8") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
