#!/usr/bin/env python3
"""Report upstream specification drift; recorded fixtures keep tests offline."""

import argparse
import hashlib
import json
import re
import subprocess
import time
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path
from urllib.error import URLError
from urllib.parse import urlparse
from urllib.request import Request, urlopen

import tomllib

DATE = re.compile(r"\b20\d\d-\d\d-\d\d\b")
HEX_40 = re.compile(r"\b[0-9a-f]{40}\b")


class FetchError(RuntimeError):
    pass


class MainContent(HTMLParser):
    def __init__(self):
        super().__init__()
        self.depth = 0
        self.main_depth = 0
        self.parts = []

    def handle_starttag(self, tag, attrs):
        if tag == "main":
            self.main_depth += 1
        if self.main_depth and tag in {"script", "style", "nav", "time"}:
            self.depth += 1

    def handle_endtag(self, tag):
        if self.main_depth and tag in {"script", "style", "nav", "time"} and self.depth:
            self.depth -= 1
        if tag == "main" and self.main_depth:
            self.main_depth -= 1

    def handle_data(self, data):
        if self.main_depth and not self.depth:
            self.parts.append(data)

    def text(self, document):
        self.feed(document)
        return " ".join(" ".join(self.parts).split())


class LiveSource:
    def get(self, url):
        return self._retry(lambda: self._get_once(url))

    def git(self, url):
        return self._retry(lambda: self._git_once(url))

    @staticmethod
    def _retry(operation):
        for attempt in range(3):
            try:
                return operation()
            except (FetchError, URLError, OSError, subprocess.TimeoutExpired) as error:
                if attempt == 2:
                    raise FetchError(str(error)) from error
                time.sleep(2**attempt)
        raise AssertionError("unreachable")

    @staticmethod
    def _get_once(url):
        request = Request(url, headers={"User-Agent": "rolter-spec-drift/1"})
        try:
            with urlopen(request, timeout=20) as response:
                return {
                    "body": response.read().decode("utf-8", errors="replace"),
                    "etag": response.headers.get("ETag", ""),
                    "last_modified": response.headers.get("Last-Modified", ""),
                }
        except URLError as error:
            raise FetchError(str(error)) from error

    @staticmethod
    def _git_once(url):
        completed = subprocess.run(
            ["git", "ls-remote", url, "HEAD"],
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
        if completed.returncode:
            raise FetchError(completed.stderr.strip() or "git ls-remote failed")
        match = HEX_40.search(completed.stdout)
        if not match:
            raise FetchError("git ls-remote returned no HEAD commit")
        return match.group(0)


class RecordedSource:
    def __init__(self, path):
        self.responses = json.loads(Path(path).read_text())

    def get(self, url):
        response = self.responses.get(url)
        if response is None:
            raise FetchError(f"no recorded response for {url}")
        if "error" in response:
            raise FetchError(response["error"])
        return {
            "body": response.get("body", ""),
            "etag": response.get("etag", ""),
            "last_modified": response.get("last_modified", ""),
        }

    def git(self, url):
        response = self.responses.get(url)
        if response is None or "git" not in response:
            raise FetchError(f"no recorded git response for {url}")
        match = HEX_40.search(response["git"])
        if not match:
            raise FetchError("recorded git response has no HEAD commit")
        return match.group(0)


def source_url(record, keyword):
    for url in record.get("related", []):
        if keyword in url:
            return url
    return record["url"]


def repository_url(record):
    urls = list(record.get("related", [])) + [record["url"]]
    for url in reversed(urls):
        parsed = urlparse(url)
        if parsed.netloc == "github.com":
            owner, repository, *_ = parsed.path.strip("/").split("/")
            return f"https://github.com/{owner}/{repository.removesuffix('.git')}"
    raise FetchError("a pinned record has no GitHub repository source")


def rfc_signal(document):
    obsolete = re.search(r"Obsoleted by:\s*([0-9,\s]+)", document, re.IGNORECASE)
    updated = re.search(r"Updated by:\s*([0-9,\s]+)", document, re.IGNORECASE)
    parts = []
    for label, match in (("Obsoleted by", obsolete), ("Updated by", updated)):
        if match:
            numbers = re.findall(r"\d+", match.group(1))
            parts.append(f"{label}: " + ", ".join(f"RFC {number}" for number in numbers))
    return "; ".join(parts)


def base_result(record, signal_kind, old_signal):
    return {
        "id": record["id"],
        "url": record["url"],
        "signal_kind": signal_kind,
        "old_signal": old_signal,
        "new_signal": "",
        "status": "unchanged",
        "gateway_endpoints": record.get("gateway_endpoints", []),
        "control_endpoints": record.get("control_endpoints", []),
        "modules": record.get("modules", []),
        "tests": record.get("tests", []),
    }


def check_record(record, source, living_state, rfc_state):
    if record.get("pinned"):
        result = base_result(record, "commit", record["pinned"])
        result["new_signal"] = source.git(repository_url(record))
    elif record["contract"] == "rfc":
        previous = rfc_state.get(record["id"], "")
        result = base_result(record, "rfc", previous)
        result["new_signal"] = rfc_signal(source.get(record["url"])["body"])
        rfc_state[record["id"]] = result["new_signal"]
    elif record["contract"] == "dated":
        result = base_result(record, "version", record["version"])
        response = source.get(source_url(record, "versioning"))
        dates = DATE.findall(response["body"])
        if not dates:
            raise FetchError("versioning source lists no dated revision")
        result["new_signal"] = max(dates)
        if not DATE.fullmatch(record["version"]):
            return result
    elif any("changelog" in url for url in record.get("related", [])):
        result = base_result(record, "reviewed_at", record["reviewed_at"])
        response = source.get(source_url(record, "changelog"))
        dates = DATE.findall(response["body"])
        if not dates:
            raise FetchError("changelog lists no entry date")
        result["new_signal"] = max(dates)
    else:
        response = source.get(record["url"])
        signal = response["etag"] or response["last_modified"]
        kind = "etag" if response["etag"] else "last-modified"
        if not signal:
            content = MainContent().text(response["body"])
            if not content:
                raise FetchError("living page has no extractable main content")
            signal = "sha256:" + hashlib.sha256(content.encode()).hexdigest()
            kind = "content-digest"
        previous = living_state.get(record["id"], {})
        accepted = previous.get("accepted", "")
        result = base_result(record, kind, accepted)
        result["new_signal"] = signal
        if signal == accepted:
            living_state[record["id"]] = {"accepted": accepted}
            return result
        pending = previous.get("pending")
        if pending == signal:
            result["status"] = "drift"
            result["old_signal"] = pending
            result["pending_since_previous_run"] = pending
            living_state[record["id"]] = {"accepted": signal}
        else:
            result["status"] = "pending"
            result["pending_since_previous_run"] = pending or ""
            living_state[record["id"]] = {"accepted": accepted, "pending": signal}
        return result

    if result["new_signal"] != result["old_signal"]:
        result["status"] = "drift"
    return result


def markdown_cell(value):
    return str(value).replace("|", "\\|").replace("\n", " ")


def write_summary(report, path):
    lines = [
        "# Specification drift",
        "",
        "Informational only; upstream outages and changes never gate pull requests.",
        "",
        "| record | status | old signal | new signal | primary URL | endpoints | modules | tests |",
        "| --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    selected = [item for item in report["results"] if item["status"] != "unchanged"]
    if not selected:
        lines.append("| none | unchanged |  |  |  |  |  |  |")
    for item in selected:
        endpoints = item["gateway_endpoints"] + item["control_endpoints"]
        values = [
            item["id"],
            item["status"],
            item["old_signal"],
            item["new_signal"],
            item["url"],
            ", ".join(endpoints),
            ", ".join(item["modules"]),
            ", ".join(item["tests"]),
        ]
        if item["status"] == "unavailable":
            values[3] = item["error"]
        lines.append("| " + " | ".join(markdown_cell(value) for value in values) + " |")
    Path(path).write_text("\n".join(lines) + "\n")


def check(args):
    registry = tomllib.loads(Path(args.registry).read_text())
    source = RecordedSource(args.responses) if args.responses else LiveSource()
    state = json.loads(Path(args.state).read_text()) if Path(args.state).exists() else {}
    living_state = state.setdefault("living", {})
    rfc_state = state.setdefault("rfc", {})
    results = []
    for record in registry["spec"]:
        try:
            result = check_record(record, source, living_state, rfc_state)
        except (FetchError, ValueError) as error:
            result = base_result(record, "unavailable", "")
            result["status"] = "unavailable"
            result["error"] = str(error)
        results.append(result)
    report = {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "results": results,
        "drift_count": sum(item["status"] == "drift" for item in results),
        "unavailable_count": sum(item["status"] == "unavailable" for item in results),
    }
    Path(args.report).write_text(json.dumps(report, indent=2) + "\n")
    Path(args.state).write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")
    write_summary(report, args.summary)


def issue_body(item, report):
    endpoints = item["gateway_endpoints"] + item["control_endpoints"]
    return "\n".join(
        [
            f"Upstream signal changed for `{item['id']}` at {report['generated_at']}.",
            "",
            f"- Signal: `{item['signal_kind']}`",
            f"- Previous: `{item['old_signal'] or 'none'}`",
            f"- Current: `{item['new_signal']}`",
            f"- Primary source: {item['url']}",
            f"- Endpoints: {', '.join(endpoints) or 'none'}",
            f"- Modules: {', '.join(item['modules']) or 'none'}",
            f"- Tests: {', '.join(item['tests']) or 'none'}",
            "",
            (
                "Review the source, update the registry and its tests where applicable, then "
                "close this issue. A defer note belongs here; the issue stays open while deferred."
            ),
        ]
    )


def gh(arguments, input_text=None):
    return subprocess.run(
        ["gh", *arguments],
        input=input_text,
        check=True,
        capture_output=True,
        text=True,
    )


def sync_issues(args):
    report = json.loads(Path(args.report).read_text())
    for item in report["results"]:
        if item["status"] != "drift":
            continue
        title = f"spec drift: {item['id']}"
        listing = gh(
            [
                "issue",
                "list",
                "--repo",
                args.repo,
                "--state",
                "all",
                "--limit",
                "100",
                "--search",
                f'in:title "{title}"',
                "--json",
                "number,title,state,url",
            ]
        )
        issues = [issue for issue in json.loads(listing.stdout) if issue["title"] == title]
        body = issue_body(item, report)
        if not issues:
            gh(
                ["issue", "create", "--repo", args.repo, "--title", title, "--body-file", "-"],
                body,
            )
            continue
        issue = min(issues, key=lambda candidate: candidate["number"])
        if issue["state"].upper() != "OPEN":
            gh(["issue", "reopen", str(issue["number"]), "--repo", args.repo])
        gh(
            ["issue", "comment", str(issue["number"]), "--repo", args.repo, "--body-file", "-"],
            body,
        )


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    check_parser = commands.add_parser("check")
    check_parser.add_argument("--registry", default="crates/rolter-core/specifications.toml")
    check_parser.add_argument("--responses")
    check_parser.add_argument("--state", required=True)
    check_parser.add_argument("--report", required=True)
    check_parser.add_argument("--summary", required=True)
    issue_parser = commands.add_parser("sync-issues")
    issue_parser.add_argument("--report", required=True)
    issue_parser.add_argument("--repo", required=True)
    args = parser.parse_args()
    if args.command == "check":
        check(args)
    else:
        sync_issues(args)


if __name__ == "__main__":
    main()
