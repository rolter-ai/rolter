#!/usr/bin/env python3
"""Tests for the load harness (#847).

Stdlib `unittest` and a stdlib HTTP server, for the same reason `bench.py`
itself is stdlib-only: this has to run on a disconnected machine.

    python3 -m unittest discover -s integration/engines -v
"""

from __future__ import annotations

import json
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

from bench import (
    Sample,
    max_sustainable_rps,
    metrics,
    parse_levels,
    percentile,
    run_phase,
    sample,
)


class PercentileTests(unittest.TestCase):
    def test_empty_input_does_not_raise(self):
        # a phase where every request failed has no latencies to rank
        self.assertEqual(percentile([], 0.99), 0.0)

    def test_single_value_is_every_percentile(self):
        self.assertEqual(percentile([7.0], 0.50), 7.0)
        self.assertEqual(percentile([7.0], 0.99), 7.0)

    def test_ranks_ascending_regardless_of_input_order(self):
        values = [5.0, 1.0, 4.0, 2.0, 3.0]
        self.assertEqual(percentile(values, 0.0), 1.0)
        self.assertEqual(percentile(values, 1.0), 5.0)
        self.assertEqual(percentile(values, 0.50), 3.0)


class MetricsTests(unittest.TestCase):
    def test_failed_requests_are_counted_but_never_timed(self):
        # a connection refused in 0.1ms is not a fast request; letting it into
        # the percentiles would make a saturated system look quicker than a
        # healthy one
        samples = [
            Sample(ttft_ms=10, total_ms=100, ok=True),
            Sample(ttft_ms=10, total_ms=100, ok=True),
            Sample(ok=False),
            Sample(ok=False),
        ]
        out = metrics(samples, elapsed_seconds=1.0)
        self.assertEqual(out["count"], 4)
        self.assertEqual(out["ok_count"], 2)
        self.assertEqual(out["error_rate"], 0.5)
        self.assertEqual(out["latency_p50_ms"], 100)
        self.assertEqual(out["latency_mean_ms"], 100)

    def test_throughput_counts_only_successful_requests(self):
        samples = [Sample(total_ms=1, ok=True), Sample(ok=False)]
        self.assertEqual(metrics(samples, 1.0)["requests_per_second"], 1.0)

    def test_a_fully_failed_phase_reports_zeroes_rather_than_raising(self):
        out = metrics([Sample(ok=False)], 1.0)
        self.assertEqual(out["error_rate"], 1.0)
        self.assertEqual(out["latency_p99_ms"], 0.0)
        self.assertEqual(out["requests_per_second"], 0.0)
        self.assertNotIn("itl_p50_ms", out)

    def test_inter_token_latency_is_pooled_across_requests(self):
        samples = [
            Sample(total_ms=10, itl_ms=[1.0, 3.0], ok=True),
            Sample(total_ms=10, itl_ms=[5.0], ok=True),
        ]
        out = metrics(samples, 1.0)
        self.assertEqual(out["itl_samples"], 3)
        self.assertEqual(out["itl_p50_ms"], 3.0)
        self.assertEqual(out["itl_mean_ms"], 3.0)

    def test_itl_keys_are_absent_when_nothing_streamed(self):
        # non-streaming runs must not publish an ITL of 0, which would read as
        # "instant tokens" rather than "not measured"
        out = metrics([Sample(total_ms=10, ok=True)], 1.0)
        self.assertNotIn("itl_p50_ms", out)
        self.assertNotIn("itl_samples", out)


class MaxSustainableRpsTests(unittest.TestCase):
    @staticmethod
    def level(concurrency, rps, p99, error_rate=0.0):
        return {
            "concurrency": concurrency,
            "requests_per_second": rps,
            "latency_p99_ms": p99,
            "error_rate": error_rate,
        }

    def test_picks_the_best_throughput_below_the_knee(self):
        levels = [
            self.level(1, 10.0, 100.0),   # baseline p99, ceiling = 200
            self.level(2, 18.0, 120.0),
            self.level(4, 30.0, 190.0),   # still healthy
            self.level(8, 40.0, 400.0),   # past the knee: more RPS, unusable p99
        ]
        summary = max_sustainable_rps(levels, knee_factor=2.0)
        self.assertEqual(summary["max_sustainable_rps"], 30.0)
        self.assertEqual(summary["at_concurrency"], 4)
        self.assertEqual(summary["p99_ceiling_ms"], 200.0)

    def test_peak_throughput_alone_is_not_the_answer(self):
        # the whole point: level 8 has the highest RPS and must not be reported
        levels = [
            self.level(1, 10.0, 100.0),
            self.level(8, 999.0, 5000.0),
        ]
        self.assertEqual(max_sustainable_rps(levels, 2.0)["max_sustainable_rps"], 10.0)

    def test_an_erroring_level_is_not_sustainable_however_fast(self):
        levels = [
            self.level(1, 10.0, 100.0),
            self.level(4, 50.0, 110.0, error_rate=0.02),
        ]
        summary = max_sustainable_rps(levels, 2.0)
        self.assertEqual(summary["max_sustainable_rps"], 10.0)
        self.assertEqual(summary["knee_at_concurrency"], 4)

    def test_reports_where_the_knee_was_crossed(self):
        levels = [
            self.level(1, 10.0, 100.0),
            self.level(2, 15.0, 150.0),
            self.level(4, 20.0, 900.0),
        ]
        self.assertEqual(max_sustainable_rps(levels, 2.0)["knee_at_concurrency"], 4)

    def test_no_knee_reached_reports_none(self):
        levels = [self.level(1, 10.0, 100.0), self.level(2, 20.0, 110.0)]
        summary = max_sustainable_rps(levels, 2.0)
        self.assertIsNone(summary["knee_at_concurrency"])
        self.assertEqual(summary["max_sustainable_rps"], 20.0)

    def test_empty_sweep_is_not_an_error(self):
        self.assertEqual(max_sustainable_rps([], 2.0), {})


class ParseLevelsTests(unittest.TestCase):
    def test_levels_are_sorted_so_the_baseline_is_least_loaded(self):
        # the knee is measured against levels[0]; an unsorted list would
        # silently compare against whatever the operator typed first
        self.assertEqual(parse_levels("8,1,4,2"), [1, 2, 4, 8])

    def test_rejects_non_positive_and_empty(self):
        for bad in ("0,1", "-2", "", ",,"):
            with self.assertRaises(ValueError):
                parse_levels(bad)


class _StreamingHandler(BaseHTTPRequestHandler):
    """Minimal OpenAI-shaped SSE endpoint with a measurable gap between tokens."""

    gap_seconds = 0.02

    def do_POST(self):  # noqa: N802 - BaseHTTPRequestHandler's interface
        length = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(length) or b"{}")
        self.send_response(200)
        if not body.get("stream"):
            payload = json.dumps(
                {"choices": [{"message": {"role": "assistant", "content": "ok"}}]}
            ).encode()
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        self.send_header("content-type", "text/event-stream")
        self.end_headers()

        def frame(delta):
            self.wfile.write(b"data: " + json.dumps({"choices": [{"delta": delta}]}).encode() + b"\n\n")
            self.wfile.flush()

        # a role-only frame carries no text; it must not count as a token
        frame({"role": "assistant"})
        for token in ("a", "b", "c"):
            frame({"content": token})
            time.sleep(self.gap_seconds)
        # nor does a finish-reason frame
        self.wfile.write(b'data: {"choices":[{"delta":{},"finish_reason":"stop"}]}\n\n')
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, *_args):
        pass


class StreamParsingTests(unittest.TestCase):
    """The SSE parsing is what makes ITL measurable, so it is tested against a
    real socket rather than a stubbed reader."""

    @classmethod
    def setUpClass(cls):
        cls.server = HTTPServer(("127.0.0.1", 0), _StreamingHandler)
        cls.url = f"http://127.0.0.1:{cls.server.server_port}"
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def test_only_content_frames_count_as_token_boundaries(self):
        result = sample(self.url, stream=True, max_tokens=3)
        self.assertTrue(result.ok)
        # three content tokens -> two gaps. Counting the role and finish frames
        # would give four and halve the reported ITL
        self.assertEqual(len(result.itl_ms), 2)

    def test_inter_token_latency_reflects_the_real_gap(self):
        result = sample(self.url, stream=True, max_tokens=3)
        for gap in result.itl_ms:
            self.assertGreater(gap, _StreamingHandler.gap_seconds * 1000 * 0.5)

    def test_ttft_precedes_total(self):
        result = sample(self.url, stream=True, max_tokens=3)
        self.assertLessEqual(result.ttft_ms, result.total_ms)

    def test_non_streaming_records_no_inter_token_latency(self):
        result = sample(self.url, stream=False)
        self.assertTrue(result.ok)
        self.assertEqual(result.itl_ms, [])

    def test_a_refused_connection_is_a_failed_sample_not_an_exception(self):
        # saturation must be measurable, so transport failure is data
        result = sample("http://127.0.0.1:1", stream=False, timeout=1)
        self.assertFalse(result.ok)

    def test_concurrent_phase_runs_every_request(self):
        out = run_phase(self.url, stream=False, requests=8, concurrency=4, max_tokens=1, timeout=30)
        self.assertEqual(out["count"], 8)
        self.assertEqual(out["error_rate"], 0.0)


if __name__ == "__main__":
    unittest.main()
