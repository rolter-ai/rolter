#!/usr/bin/env python3
"""Offline recorded-fixture tests for the specification drift reporter."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.spec_drift import FetchError, LiveSource

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "spec_drift.py"
FIXTURES = ROOT / "scripts" / "fixtures" / "spec-drift"


class SpecDriftTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.directory = Path(self.temp.name)
        self.report = self.directory / "report.json"
        self.summary = self.directory / "summary.md"
        self.state = self.directory / "state.json"

    def tearDown(self):
        self.temp.cleanup()

    def run_check(self):
        subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "check",
                "--registry",
                str(FIXTURES / "registry.toml"),
                "--responses",
                str(FIXTURES / "responses.json"),
                "--state",
                str(self.state),
                "--report",
                str(self.report),
                "--summary",
                str(self.summary),
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads(self.report.read_text())

    def test_recorded_signals_cover_stable_source_shapes(self):
        report = self.run_check()
        results = {result["id"]: result for result in report["results"]}

        expected = {
            "pinned-spec": "b" * 40,
            "dated-spec": "2026-02-03",
            "changelog-spec": "2026-02-04",
            "rfc-spec": "Obsoleted by: RFC 9999; Updated by: RFC 8888, RFC 7777",
        }
        for spec_id, new_signal in expected.items():
            with self.subTest(spec_id=spec_id):
                self.assertEqual(results[spec_id]["status"], "drift")
                self.assertEqual(results[spec_id]["new_signal"], new_signal)

        self.assertEqual(results["unavailable-spec"]["status"], "unavailable")
        self.assertEqual(results["forwarded-spec"]["status"], "unchanged")
        self.assertEqual(results["current-rfc-spec"]["status"], "unchanged")
        self.assertEqual(results["living-spec"]["status"], "pending")
        self.assertEqual(report["drift_count"], 4)

        summary = self.summary.read_text()
        self.assertIn("POST /v1/pinned", summary)
        self.assertIn("crates/example/src/pinned.rs", summary)
        self.assertIn("crates/example/src/pinned.rs::works", summary)
        self.assertIn("https://example.test/pinned", summary)

    def test_rfc_header_is_not_reported_again_without_a_new_change(self):
        first = self.run_check()
        rfc = next(item for item in first["results"] if item["id"] == "rfc-spec")
        self.assertEqual(rfc["status"], "drift")

        second = self.run_check()
        rfc = next(item for item in second["results"] if item["id"] == "rfc-spec")
        self.assertEqual(rfc["status"], "unchanged")

    def test_living_page_change_must_persist_for_two_runs(self):
        first = self.run_check()
        living = next(item for item in first["results"] if item["id"] == "living-spec")
        self.assertEqual(living["status"], "pending")

        second = self.run_check()
        living = next(item for item in second["results"] if item["id"] == "living-spec")
        self.assertEqual(living["status"], "drift")
        self.assertEqual(living["old_signal"], living["pending_since_previous_run"])

        third = self.run_check()
        living = next(item for item in third["results"] if item["id"] == "living-spec")
        self.assertEqual(living["status"], "unchanged")

    def test_git_timeout_becomes_a_source_error(self):
        def timeout():
            raise subprocess.TimeoutExpired("git", 30)

        with mock.patch("scripts.spec_drift.time.sleep"), self.assertRaises(FetchError):
            LiveSource._retry(timeout)

    def test_issue_sync_creates_then_comments_on_the_fixed_title(self):
        self.run_check()
        fake_log = self.directory / "gh.log"
        environment = os.environ.copy()
        environment.update(
            {
                "PATH": f"{FIXTURES}{os.pathsep}{environment['PATH']}",
                "FAKE_GH_LOG": str(fake_log),
                "FAKE_GH_ISSUES": str(FIXTURES / "no-issues.json"),
            }
        )
        command = [
            sys.executable,
            str(SCRIPT),
            "sync-issues",
            "--report",
            str(self.report),
            "--repo",
            "example/rolter",
        ]
        subprocess.run(command, cwd=ROOT, env=environment, check=True, capture_output=True)
        created = fake_log.read_text()
        self.assertIn("issue create", created)
        self.assertIn("--title spec drift: pinned-spec", created)
        self.assertNotIn("issue comment", created)

        environment["FAKE_GH_ISSUES"] = str(FIXTURES / "open-issues.json")
        subprocess.run(command, cwd=ROOT, env=environment, check=True, capture_output=True)
        commented = fake_log.read_text().splitlines()
        self.assertTrue(any(line.startswith("issue comment 42 ") for line in commented))
        self.assertEqual(sum("issue create" in line for line in commented), 4)


if __name__ == "__main__":
    unittest.main()
