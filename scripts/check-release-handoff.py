#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = ["pyyaml==6.0.3"]
# ///
"""Assert the release pipeline still hands off, and is still gated, as designed.

The release pipeline hands off in two hops, and both are easy to delete by
accident because neither has a test behind it:

    release-plz.yml  --workflow_dispatch-->  release.yml  --> wheels/pypi/ghcr
    release-plz.yml  --workflow_dispatch-->  ci.yml       --> ci-ok on the release pr

and it publishes to crates.io only behind a gate that is just as easy to cut:

    release-gate  --scripts/wait-for-ci-gate.sh-->  ci-ok of this sha's ci.yml push run
    release-plz-release  runs only when release-gate reports it verified

release-plz tags with the repo GITHUB_TOKEN, and GitHub suppresses downstream
events for token-created refs, so release.yml's `push: tags` trigger never fires
for a real release. `workflow_dispatch` is the documented exception: it always
creates a run. Drop that dispatch and releases keep going out with a GitHub
release and crates.io but no wheel, silently and forever. That is what happened
to v0.0.6-v0.0.10 while PyPI sat on 0.0.5 (#903).

Every assertion reads the workflows as parsed YAML: a job exists, its `needs`
*set* holds the required ids, a trigger or input is declared. The few that are
text by nature (a command inside a `run:` block, an option string passed to an
action, the api query in the gate's wait script) search only the job or script
they belong to, and read its shell with the comments taken out, so a command
that survives only as a comment does not count.
A job or step switched off with `if: false` counts as missing. So the check
passes however a workflow is laid out, reflowed by prettier included, and fails
when the wiring itself is wrong (#1723).

    uv run --script scripts/check-release-handoff.py              # check the workflows
    uv run --script scripts/check-release-handoff.py --self-test  # prove each check can fail

`--self-test` breaks the parsed workflows one invariant at a time and fails
unless the matching check catches every break. It then rewrites correct wiring
in other forms (other YAML layouts, reordered `needs`, a one-item list, extra
needs) and fails unless every check still passes.
"""

from __future__ import annotations

import argparse
import copy
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterator, Optional

import yaml

ROOT = Path(__file__).resolve().parent.parent
PLZ = ".github/workflows/release-plz.yml"
REL = ".github/workflows/release.yml"
CI = ".github/workflows/ci.yml"
WAIT = "scripts/wait-for-ci-gate.sh"
FILES = (PLZ, REL, CI)
SCRIPTS = (WAIT,)

Workflow = dict
Tree = dict  # workflow path -> parsed workflow, script path -> its text


# ── reading a workflow the way the runner does ──────────────────────────────


class WorkflowLoader(yaml.SafeLoader):
    """SafeLoader with GitHub's scalar rules and no silent duplicate keys."""


# yaml 1.1 resolves on/off/yes/no to booleans, so a plain SafeLoader turns the
# `on:` trigger key into True. actions reads yaml 1.2, where only true and false
# are booleans, so resolve exactly those and leave every other word a string
WorkflowLoader.yaml_implicit_resolvers = {
    first: [(tag, rx) for tag, rx in resolvers if tag != "tag:yaml.org,2002:bool"]
    for first, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
WorkflowLoader.add_implicit_resolver(
    "tag:yaml.org,2002:bool",
    re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$"),
    list("tTfF"),
)


def _mapping_without_duplicates(loader: WorkflowLoader, node: yaml.MappingNode) -> dict:
    # pyyaml keeps the last of two equal keys, so a second `needs:` or a second
    # job with the same id would let this check read one value while actions
    # rejects the file outright. refuse it the way actions does
    loader.flatten_mapping(node)
    seen: set = set()
    for key_node, _ in node.value:
        key = loader.construct_object(key_node, deep=True)
        if key in seen:
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping",
                node.start_mark,
                f"found duplicate key {key!r}",
                key_node.start_mark,
            )
        seen.add(key)
    return loader.construct_mapping(node, deep=True)


WorkflowLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _mapping_without_duplicates
)


def parse(text: str) -> Workflow:
    # WorkflowLoader is a SafeLoader, so no tag can construct a python object
    data = yaml.load(text, Loader=WorkflowLoader)
    if not isinstance(data, dict):
        raise yaml.YAMLError("a workflow must be a mapping at the top level")
    return data


def load(root: Path) -> tuple[Tree, list[str]]:
    tree: Tree = {}
    errors: list[str] = []
    for rel in FILES:
        path = root / rel
        if not path.is_file():
            errors.append(f"{rel} is missing; the release pipeline needs every one of them")
            continue
        try:
            tree[rel] = parse(path.read_text(encoding="utf-8"))
        except yaml.YAMLError as err:
            errors.append(f"{rel}: not a readable workflow: {err}")
    for rel in SCRIPTS:
        path = root / rel
        if not path.is_file():
            errors.append(f"{rel} is missing; the release gate runs it")
            continue
        tree[rel] = path.read_text(encoding="utf-8")
    return tree, errors


# ── reading the parsed structure ────────────────────────────────────────────


def disabled(node: dict) -> bool:
    # `if: false` and `if: ${{ false }}` switch a job or step off for good; one
    # that never runs wires nothing, so the checks treat it as absent
    cond = node.get("if")
    return cond is False or (isinstance(cond, str) and expression(cond) == "false")


def job(wf: Workflow, name: str) -> Optional[dict]:
    found = (wf.get("jobs") or {}).get(name)
    return found if isinstance(found, dict) and not disabled(found) else None


def needs(j: Optional[dict]) -> set:
    # `needs: a`, `needs: [a, b]` and a block sequence are the same to actions
    value = (j or {}).get("needs") or []
    return {value} if isinstance(value, str) else set(value)


def steps(j: Optional[dict]) -> list:
    return [s for s in (j or {}).get("steps") or [] if isinstance(s, dict) and not disabled(s)]


def strip_shell_comments(script: str) -> str:
    # a `#` that starts a word outside quotes comments out the rest of its line.
    # quoting is tracked across lines, so a multi-line jq program keeps its text,
    # and `$(` opens a fresh unquoted context even inside double quotes, as it
    # does in bash, so `runs="$(gh api \⏎ # ...` is a comment. a `#` inside a
    # word (`${REF#v}`, `$#`) or inside quotes is not
    out: list[str] = []
    stack = [""]  # "" unquoted, "'" or '"' quoted, "(" an open $( or ( group
    i = 0
    while i < len(script):
        c, top = script[i], stack[-1]
        if top == "'":
            if c == "'":
                stack.pop()
        elif c == "\\":
            out.append(script[i : i + 2])
            i += 2
            continue
        elif c == "$" and script[i + 1 : i + 2] == "(":
            stack.append("(")
            out.append("$(")
            i += 2
            continue
        elif top == '"':
            if c == '"':
                stack.pop()
        elif c in "'\"":
            stack.append(c)
        elif c == "(":
            stack.append("(")
        elif c == ")" and top == "(":
            stack.pop()
        elif c == "#" and (i == 0 or script[i - 1] in " \t\n;&|()"):
            end = script.find("\n", i)
            i = len(script) if end == -1 else end
            continue
        out.append(c)
        i += 1
    return "".join(out)


def shell_commands(script: str) -> Iterator[str]:
    # one logical command per line: comments dropped first, as the shell does,
    # so a comment ending in a backslash continues nothing, then continuations joined
    for line in re.sub(r"\\\n\s*", " ", strip_shell_comments(script)).splitlines():
        if line.strip():
            yield line


def run_text(j: Optional[dict]) -> str:
    # the shell every step of one job runs, comments removed; textual checks look
    # here and only here. each step is its own script, so a quote left open in
    # one never swallows the next
    return "\n".join(
        command
        for s in steps(j)
        if isinstance(s.get("run"), str)
        for command in shell_commands(s["run"])
    )


def all_run_text(wf: Workflow) -> str:
    return "\n".join(run_text(j) for j in (wf.get("jobs") or {}).values() if isinstance(j, dict))


def strings(node: Any) -> Iterator[str]:
    if isinstance(node, str):
        yield node
    elif isinstance(node, dict):
        for value in node.values():
            yield from strings(value)
    elif isinstance(node, list):
        for value in node:
            yield from strings(value)


def all_strings(wf: Workflow) -> str:
    # every value in the workflow: run blocks, env and action inputs alike. a
    # forbidden pattern stays forbidden wherever it moves, and a yaml comment that
    # names one is not part of the parse, so it never trips the check
    return "\n".join(strings(wf))


def triggers(wf: Workflow) -> dict:
    # `on: push`, `on: [push, workflow_dispatch]` and the mapping form all declare triggers
    on = wf.get("on")
    if isinstance(on, str):
        return {on: None}
    if isinstance(on, list):
        return {t: None for t in on}
    return on if isinstance(on, dict) else {}


def can_dispatch(wf: Workflow, name: str) -> bool:
    j = job(wf, name)
    if j is None:
        return False
    # a job without its own block inherits the workflow's
    grant = j["permissions"] if "permissions" in j else wf.get("permissions")
    if grant == "write-all":
        return True
    return isinstance(grant, dict) and grant.get("actions") == "write"


def step_with(j: Optional[dict], key: str, value: str) -> bool:
    return any((s.get("with") or {}).get(key) == value for s in steps(j))


def expression(value: Any) -> str:
    # `${{ a }}`, `${{a}}` and a bare `if:` expression all compare equal
    text = str(value or "").strip()
    match = re.fullmatch(r"\$\{\{(.*)\}\}", text, re.DOTALL)
    return re.sub(r"\s+", "", match.group(1) if match else text)


def required_checks_default(j: Optional[dict]) -> set:
    # the fallback list in `vars.RELEASE_REQUIRED_CHECKS || '<names>'`
    for text in strings(j):
        match = re.search(r"vars\.RELEASE_REQUIRED_CHECKS\s*\|\|\s*'([^']*)'", text)
        if match:
            return {name.strip() for name in match.group(1).split(",")}
    return set()


# ── the checks ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Check:
    id: str
    file: str
    message: str
    holds: Callable[[Any], bool]  # a parsed workflow, or a script's text


CHECKS: list[Check] = []


def check(file: str, message: str) -> Callable:
    def register(fn: Callable[[Any], bool]) -> Callable[[Any], bool]:
        CHECKS.append(Check(fn.__name__, file, message, fn))
        return fn

    return register


# the handoff itself


@check(PLZ, "no 'dispatch-artifact-release' job, so release.yml will never run")
def handoff_job(wf: Workflow) -> bool:
    return job(wf, "dispatch-artifact-release") is not None


@check(PLZ, "the handoff job no longer dispatches release.yml")
def handoff_dispatches_release(wf: Workflow) -> bool:
    return bool(
        re.search(
            r"\bgh workflow run release\.yml\b", run_text(job(wf, "dispatch-artifact-release"))
        )
    )


@check(PLZ, "the handoff job needs 'actions: write' to dispatch")
def handoff_can_dispatch(wf: Workflow) -> bool:
    return can_dispatch(wf, "dispatch-artifact-release")


@check(PLZ, "the handoff must run after release-plz-release, on its tag output")
def handoff_after_release(wf: Workflow) -> bool:
    return "release-plz-release" in needs(job(wf, "dispatch-artifact-release"))


@check(PLZ, "release-plz-release must expose the resolved tag as a job output")
def release_exposes_tag(wf: Workflow) -> bool:
    j = job(wf, "release-plz-release") or {}
    output = (j.get("outputs") or {}).get("tag")
    return expression(output) == "steps.tag.outputs.tag" and any(
        s.get("id") == "tag" for s in steps(j)
    )


# the tag the handoff carries. release-plz emits a `tag` for every published
# crate, not just the one with git_tag_enable: the crates.io-only members get a
# derived `<crate>-v<version>` string that was never pushed. picking the first
# tagged entry resolved `rolter-core-v0.0.11`, release.yml checked out a ref that
# does not exist, and v0.0.11 shipped to crates.io with no wheel (#1026). select
# by package name and refuse anything that is not a vX.Y.Z tag


@check(PLZ, "the tag must be selected by package_name, not by array position")
def tag_selected_by_name(wf: Workflow) -> bool:
    return bool(
        re.search(
            r'select\(\s*\.package_name\s*==\s*"rolter-gateway"\s*\)',
            run_text(job(wf, "release-plz-release")),
        )
    )


@check(PLZ, "the resolved tag must be validated as a vX.Y.Z release tag")
def tag_validated(wf: Workflow) -> bool:
    return "grep -Eq '^v[0-9]" in run_text(job(wf, "release-plz-release"))


@check(PLZ, "do not take the first tagged release; crates.io-only tags are never pushed")
def no_first_tagged_release(wf: Workflow) -> bool:
    first_tagged = (
        r'map\(\s*select\(\s*\.tag\s*!=\s*null\s+and\s+\.tag\s*!=\s*""\s*\)\s*\)\s*\|\s*\.\[0\]'
    )
    return not re.search(first_tagged, all_strings(wf))


# `gh api` switches to POST as soon as any -f/-F is present, which 404s the
# GET-only workflow-runs endpoint and aborted the confirmation loop (#1026).
# `event=` was the field that did it, and it stays in the path whatever the
# method: that is the form the #1026 fix settled on and the form the two
# confirmation checks below read. any other field is fine once -X GET is pinned
FIELD_FLAG = re.compile(r"(?<!\S)(?:-[fF]|--(?:raw-)?field)")
EVENT_FIELD = re.compile(r"(?<!\S)(?:-[fF]|--(?:raw-)?field)(?:\s+|=)?event=")
GET_METHOD = re.compile(r"(?<!\S)(?:-X|--method)(?:\s+|=)?GET(?!\S)")


@check(PLZ, "event=workflow_dispatch must stay in the runs query path, never a -f/-F field")
def runs_event_in_path(wf: Workflow) -> bool:
    return not EVENT_FIELD.search(all_strings(wf))


@check(PLZ, "a -f/-F field on a gh api runs query needs -X GET, or gh api POSTs and 404s")
def runs_query_is_get(wf: Workflow) -> bool:
    for command in all_run_text(wf).splitlines():
        _, api, call = command.partition("gh api ")
        if api and "/runs" in call and FIELD_FLAG.search(call) and not GET_METHOD.search(call):
            return False
    return True


@check(PLZ, "the handoff must confirm a release run actually started")
def handoff_confirms_run(wf: Workflow) -> bool:
    return "actions/workflows/release.yml/runs?event=workflow_dispatch" in run_text(
        job(wf, "dispatch-artifact-release")
    )


# gating the release pr. the same suppression hides the release *pr* from
# ci.yml: `ci-ok` is the only required check on master and nothing ever reported
# it there, so every release merged through an admin bypass (#1025). drop this
# dispatch and releases go back to needing one, quietly


@check(PLZ, "no 'dispatch-release-pr-ci' job, so the release pr will never get ci-ok")
def pr_gate_job(wf: Workflow) -> bool:
    return job(wf, "dispatch-release-pr-ci") is not None


@check(PLZ, "the release-pr gate no longer dispatches ci.yml")
def pr_gate_dispatches_ci(wf: Workflow) -> bool:
    return bool(
        re.search(r"\bgh workflow run ci\.yml\b", run_text(job(wf, "dispatch-release-pr-ci")))
    )


@check(PLZ, "the release-pr gate needs 'actions: write' to dispatch")
def pr_gate_can_dispatch(wf: Workflow) -> bool:
    return can_dispatch(wf, "dispatch-release-pr-ci")


@check(PLZ, "the release-pr gate must run after release-plz-pr")
def pr_gate_after_release_pr(wf: Workflow) -> bool:
    return "release-plz-pr" in needs(job(wf, "dispatch-release-pr-ci"))


@check(PLZ, "the release-pr gate must confirm a ci run actually started")
def pr_gate_confirms_run(wf: Workflow) -> bool:
    return "actions/workflows/ci.yml/runs?event=workflow_dispatch" in run_text(
        job(wf, "dispatch-release-pr-ci")
    )


@check(CI, "ci.yml dropped its workflow_dispatch trigger; the release pr cannot be gated")
def ci_dispatchable(wf: Workflow) -> bool:
    return "workflow_dispatch" in triggers(wf)


# the required status check is the check-run *name*, and the release gate below
# looks it up by that name too
@check(CI, "ci.yml must still aggregate everything into a check named ci-ok")
def ci_ok_job(wf: Workflow) -> bool:
    j = job(wf, "ci-ok")
    return j is not None and j.get("name", "ci-ok") == "ci-ok"


# gating the crates.io publish. release-plz-release runs cargo publish with the
# persisted contents: write token, so it must never start on a commit nothing
# verified (ROL-103). it used to need a job that re-ran all of quality.yml on
# the merge commit; it now needs release-gate, which waits for ci-ok on this
# sha's ci.yml push run (#2025). either wiring passes. what must hold is that
# the publish is bound to the gate itself: release-gate skips the wait when
# nothing is pending, and a detector that says so by mistake must leave the
# release skipped, never ungated. the checks above only notice a job switched
# off with a literal `if: false`, so the binding is asserted in its own right

VERIFIED = {
    "needs.release-gate.outputs.verified=='true'",
    "'true'==needs.release-gate.outputs.verified",
}
# a status function replaces the implicit success() that keeps a failed or
# cancelled gate from reaching the publish
OVERRIDES_SUCCESS = re.compile(r"\b(?:always|failure|cancelled)\(\)")


def widened(cond: Any) -> bool:
    # a `||` or a status function lets the job run past a gate that did not pass
    expr = expression(cond)
    return "||" in expr or bool(OVERRIDES_SUCCESS.search(expr))


def runs_on_verified(cond: Any) -> bool:
    # one `&&` conjunct is exactly the verified comparison
    return any(term.strip("()") in VERIFIED for term in expression(cond).split("&&"))


def runs_quality(j: Optional[dict]) -> bool:
    return bool(
        re.search(r"\.github/workflows/quality\.ya?ml(?:@|$)", str((j or {}).get("uses", "")))
    )


@check(
    PLZ,
    "release-plz-release must need release-gate and run only when "
    "needs.release-gate.outputs.verified == 'true' (or need a job that runs quality.yml)",
)
def publish_gated(wf: Workflow) -> bool:
    j = job(wf, "release-plz-release")
    if j is None or widened(j.get("if")):
        return False
    if any(runs_quality(job(wf, need)) for need in needs(j)):
        return True
    return (
        "release-gate" in needs(j)
        and job(wf, "release-gate") is not None
        and runs_on_verified(j.get("if"))
    )


# the step is this one bare command and nothing else. the script writes
# `verified=true` itself as its last action, so no shell option, `||`, pipe,
# function wrapper or write of the step's own can set the output after a red
# gate, and the step's exit status is the script's, so a red gate is a red job
WAIT_COMMAND = re.compile(r"(?:bash\s+)?(?:\./)?scripts/wait-for-ci-gate\.sh")


@check(
    PLZ,
    "release-gate must take verified from a step that runs only "
    "scripts/wait-for-ci-gate.sh, and never from a check-run lookup",
)
def gate_waits_on_push_run(wf: Workflow) -> bool:
    j = job(wf, "release-gate")
    if j is None or "check-runs" in run_text(j):
        return False
    source = re.fullmatch(
        r"steps\.([\w-]+)\.outputs\.verified", expression((j.get("outputs") or {}).get("verified"))
    )
    step = next((s for s in steps(j) if source and s.get("id") == source.group(1)), None)
    if step is None or not isinstance(step.get("run"), str):
        return False
    # continue-on-error would turn a red gate into a green job that skips the
    # publish without anyone seeing why
    if any(node.get("continue-on-error") not in (None, False) for node in (j, step)):
        return False
    commands = [c.strip() for c in shell_commands(step["run"])]
    return len(commands) == 1 and bool(WAIT_COMMAND.fullmatch(commands[0]))


# what makes the bare call above enough: the script's one write of the output is
# its final command, so the output exists only when every check before it passed
@check(
    WAIT,
    "wait-for-ci-gate.sh must write verified=true to $GITHUB_OUTPUT once, as its last command",
)
def wait_writes_verified_last(script: str) -> bool:
    commands = [c.strip() for c in shell_commands(script)]
    writes = [i for i, c in enumerate(commands) if "verified=" in c]
    last = commands[-1] if commands else ""
    return (
        writes == [len(commands) - 1]
        and "verified=true" in last
        and re.search(r">>\s*\"?\$\{?GITHUB_OUTPUT\b", last) is not None
    )


@check(
    WAIT,
    "wait-for-ci-gate.sh must query ci.yml push runs on master for the sha and require "
    "their ci-ok job, never a check-run name",
)
def wait_binds_push_run(script: str) -> bool:
    text = "\n".join(shell_commands(script))
    queries = re.findall(r"actions/workflows/ci\.yml/runs\?(\S*)", text)
    bound = any(all(p in q for p in ("head_sha=", "event=push", "branch=master")) for q in queries)
    reads_jobs = re.search(r"actions/runs/\S+/jobs", text)
    ci_ok_job = re.search(r'select\(\s*\.name\s*==\s*"ci-ok"\s*\)', text)
    return bound and bool(reads_jobs and ci_ok_job) and "check-runs" not in text


# the receiving end


@check(REL, "release.yml dropped its workflow_dispatch trigger")
def release_dispatchable(wf: Workflow) -> bool:
    return "workflow_dispatch" in triggers(wf)


@check(REL, "release.yml dropped the 'tag' dispatch input")
def release_tag_input(wf: Workflow) -> bool:
    dispatch = triggers(wf).get("workflow_dispatch") or {}
    return "tag" in (dispatch.get("inputs") or {})


@check(REL, "release.yml no longer has a publish-pypi job")
def publish_pypi_job(wf: Workflow) -> bool:
    return job(wf, "publish-pypi") is not None


# the gate is asserted, not re-run. calling quality.yml from here would check out
# the *caller's* ref (master on a dispatch, not the tag being packaged), so the
# gate would verify one tree while the wheels came from another (#988). worse,
# threading the tag in instead makes the shared workflow check out a
# dispatch-supplied ref in a default-branch context, which is cache poisoning
@check(REL, "release.yml must not re-run quality.yml; assert ci-ok for the tagged sha instead")
def gate_not_rerun(wf: Workflow) -> bool:
    calls = [str(j.get("uses", "")) for j in (wf.get("jobs") or {}).values() if isinstance(j, dict)]
    return not any(re.search(r"\.github/workflows/quality\.ya?ml(?:@|$)", c) for c in calls)


@check(REL, "the release gate must require ci-ok for the tagged commit")
def gate_requires_ci_ok(wf: Workflow) -> bool:
    return "ci-ok" in required_checks_default(job(wf, "verify-external-checks"))


# the parity gate. without it a skipped publish drags the run to green instead of red

PARITY_NEEDS = {
    "verify-external-checks",
    "build-wheels",
    "build-image",
    "smoke-wheels",
    "smoke-image",
    "publish-pypi",
    "publish-docker",
}


@check(REL, "release.yml dropped the verify-parity gate")
def parity_job(wf: Workflow) -> bool:
    return job(wf, "verify-parity") is not None


@check(
    REL,
    "verify-parity must observe every build, smoke and publish job to catch a skipped publish "
    f"(needs {', '.join(sorted(PARITY_NEEDS))})",
)
def parity_observes_publish(wf: Workflow) -> bool:
    return PARITY_NEEDS <= needs(job(wf, "verify-parity"))


@check(REL, "verify-parity must run with always() or a skipped publish stays invisible")
def parity_always_runs(wf: Workflow) -> bool:
    return "always()" in expression((job(wf, "verify-parity") or {}).get("if"))


# the artifact set. `macos-latest` is arm64, so without an explicit x86_64 target
# intel macs get no wheel; without an sdist they get no candidate at all and the
# install fails outright rather than building from source (#989)


@check(REL, "release.yml must cross-build the macos x86_64 wheel")
def macos_x86_64_wheel(wf: Workflow) -> bool:
    return step_with(job(wf, "build-wheels"), "target", "x86_64-apple-darwin")


@check(REL, "release.yml must build an sdist as the source fallback")
def sdist_built(wf: Workflow) -> bool:
    return step_with(job(wf, "build-wheels"), "command", "sdist")


@check(REL, "verify-parity must assert the published artifact set")
def parity_expects_macos_x86_64(wf: Workflow) -> bool:
    return 'expect "macos x86_64 wheel"' in run_text(job(wf, "verify-parity"))


@check(REL, "verify-parity must assert an sdist was published")
def parity_expects_sdist(wf: Workflow) -> bool:
    return 'expect "sdist"' in run_text(job(wf, "verify-parity"))


# the publish barrier. stage 1 builds, stage 2 smoke-tests, stage 3 publishes.
# if a publish job stops depending on every build and smoke job, a failed wheel
# can leave images public against a release with nothing on pypi (#992)

BARRIER = {"verify-external-checks", "build-wheels", "build-image", "smoke-wheels", "smoke-image"}


@check(REL, "release.yml must build images per arch in their own stage")
def build_image_job(wf: Workflow) -> bool:
    return job(wf, "build-image") is not None


@check(REL, "stage 1 must push untagged digests, not tags")
def image_pushed_by_digest(wf: Workflow) -> bool:
    return any("push-by-digest=true" in s for s in strings(steps(job(wf, "build-image"))))


@check(
    REL,
    f"publish-pypi must wait for every build and smoke job (needs {', '.join(sorted(BARRIER))})",
)
def pypi_waits_for_barrier(wf: Workflow) -> bool:
    return BARRIER <= needs(job(wf, "publish-pypi"))


@check(
    REL,
    f"publish-docker must wait for every build and smoke job (needs {', '.join(sorted(BARRIER))})",
)
def docker_waits_for_barrier(wf: Workflow) -> bool:
    return BARRIER <= needs(job(wf, "publish-docker"))


# an artifact that was never run is not a verified artifact


@check(REL, "release.yml must install and run the built wheel before publishing")
def smoke_wheels_job(wf: Workflow) -> bool:
    return job(wf, "smoke-wheels") is not None


@check(REL, "release.yml must run the built image before publishing")
def smoke_image_job(wf: Workflow) -> bool:
    return job(wf, "smoke-image") is not None


@check(REL, "the wheel smoke test must resolve only from dist/, never pypi")
def wheel_smoke_offline(wf: Workflow) -> bool:
    return "--no-index" in run_text(job(wf, "smoke-wheels"))


def failures(tree: Tree) -> list[Check]:
    return [c for c in CHECKS if c.file in tree and not c.holds(tree[c.file])]


FOOTER = """
the release handoff is broken. see docs/dev-docs/development/packaging.md ("Release
pipeline"); a release that loses this wiring publishes a github release and
crates.io but never a pypi wheel, leaves the release pr unable to reach a
green ci-ok, or publishes to crates.io from a commit nothing verified, and
nothing goes red. a job or step disabled with `if: false` counts as missing,
and so does a command left only in a shell comment."""


def run(root: Path) -> int:
    tree, errors = load(root)
    for message in errors:
        print(f"error: {message}", file=sys.stderr)
    if errors:
        return 1
    failed = failures(tree)
    for c in failed:
        print(f"error: {c.file}: {c.message}", file=sys.stderr)
    if failed:
        print(FOOTER, file=sys.stderr)
        return 1
    print(f"release handoff is wired: ok ({len(CHECKS)} checks)")
    return 0


# ── self-test: every check must be able to fail ─────────────────────────────


class NoOp(Exception):
    """A fixture edit that found nothing to break proves nothing."""


def jobs_of(tree: Tree, file: str) -> dict:
    return tree[file]["jobs"]


def drop_job(file: str, name: str) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        if jobs_of(tree, file).pop(name, None) is None:
            raise NoOp(f"{file} has no job {name}")

    return mutate


def rename_job(file: str, name: str, to: str) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        jobs = jobs_of(tree, file)
        if name not in jobs:
            raise NoOp(f"{file} has no job {name}")
        jobs[to] = jobs.pop(name)

    return mutate


def drop_need(file: str, name: str, need: str) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        j = jobs_of(tree, file)[name]
        current = needs(j)
        if need not in current:
            raise NoOp(f"{file}: {name} does not need {need}")
        j["needs"] = sorted(current - {need})

    return mutate


def edit_job(file: str, name: str, edit: Callable[[dict], None]) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        edit(jobs_of(tree, file)[name])

    return mutate


def replace_in_job(file: str, name: str, old: str, new: str) -> Callable[[Tree], None]:
    # rewrites every string in the job, run: blocks and action inputs alike
    def rewrite(node: Any) -> Any:
        if isinstance(node, str):
            return node.replace(old, new)
        if isinstance(node, dict):
            return {k: rewrite(v) for k, v in node.items()}
        if isinstance(node, list):
            return [rewrite(v) for v in node]
        return node

    def mutate(tree: Tree) -> None:
        jobs = jobs_of(tree, file)
        before = jobs[name]
        jobs[name] = rewrite(before)
        if jobs[name] == before:
            raise NoOp(f"{file}: {name} never mentions {old!r}")

    return mutate


def append_run(file: str, name: str, line: str) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        jobs_of(tree, file)[name].setdefault("steps", []).append({"run": line})

    return mutate


def comment_out(
    file: str, name: str, needle: str, whole_step: bool = False
) -> Callable[[Tree], None]:
    # the edit someone makes to pause a command: `# ` in front of the line, or in
    # front of every line of the step when the command spans several
    def mutate(tree: Tree) -> None:
        hit = False
        for s in steps(jobs_of(tree, file)[name]):
            script = s.get("run")
            if not isinstance(script, str) or needle not in script:
                continue
            s["run"] = "\n".join(
                f"# {line}" if whole_step or needle in line else line for line in script.split("\n")
            )
            hit = True
        if not hit:
            raise NoOp(f"{file}: {name} runs nothing mentioning {needle!r}")

    return mutate


def disable(file: str, name: str, step_mentioning: Optional[str] = None) -> Callable[[Tree], None]:
    # `if: false` on the job, or on the step whose shell mentions the given text
    def mutate(tree: Tree) -> None:
        j = jobs_of(tree, file)[name]
        if step_mentioning is None:
            j["if"] = False
            return
        for s in steps(j):
            if step_mentioning in str(s.get("run", "")):
                s["if"] = "${{ false }}"
                return
        raise NoOp(f"{file}: {name} has no step running {step_mentioning!r}")

    return mutate


def replace_in_script(file: str, old: str, new: str) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        if old not in tree[file]:
            raise NoOp(f"{file} never mentions {old!r}")
        tree[file] = tree[file].replace(old, new)

    return mutate


def drop_steps(file: str, name: str, match: Callable[[dict], bool]) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        j = jobs_of(tree, file)[name]
        kept = [s for s in j.get("steps") or [] if not match(s)]
        if len(kept) == len(j.get("steps") or []):
            raise NoOp(f"{file}: {name} has no such step")
        j["steps"] = kept

    return mutate


def chain(*mutations: Callable[[Tree], None]) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        for m in mutations:
            m(tree)

    return mutate


def set_key(key: str, value: Any) -> Callable[[dict], None]:
    def edit(node: dict) -> None:
        node[key] = value

    return edit


def drop_trigger(file: str, trigger: str) -> Callable[[Tree], None]:
    def mutate(tree: Tree) -> None:
        if trigger not in triggers(tree[file]):
            raise NoOp(f"{file} has no {trigger} trigger")
        tree[file]["on"] = {k: v for k, v in triggers(tree[file]).items() if k != trigger}

    return mutate


def drop_tag_input(tree: Tree) -> None:
    inputs = tree[REL]["on"]["workflow_dispatch"]["inputs"]
    inputs["version"] = inputs.pop("tag")


def drop_tag_step_id(tree: Tree) -> None:
    for s in steps(jobs_of(tree, PLZ)["release-plz-release"]):
        if s.get("id") == "tag":
            s["id"] = "resolve"
            return
    raise NoOp("release-plz-release has no step with id tag")


# (check id, what the fixture breaks, the break). every check needs at least one
BREAKS: list[tuple[str, str, Callable[[Tree], None]]] = [
    ("handoff_job", "handoff job deleted", drop_job(PLZ, "dispatch-artifact-release")),
    (
        "handoff_job",
        "handoff job renamed",
        rename_job(PLZ, "dispatch-artifact-release", "dispatch"),
    ),
    (
        "handoff_job",
        "handoff job disabled with if: false",
        disable(PLZ, "dispatch-artifact-release"),
    ),
    (
        "handoff_dispatches_release",
        "dispatch removed from the handoff",
        replace_in_job(PLZ, "dispatch-artifact-release", "gh workflow run release.yml", "true"),
    ),
    (
        "handoff_dispatches_release",
        "dispatch commented out",
        comment_out(PLZ, "dispatch-artifact-release", "gh workflow run release.yml"),
    ),
    (
        "handoff_dispatches_release",
        "dispatch left only in a trailing comment",
        replace_in_job(
            PLZ,
            "dispatch-artifact-release",
            "gh workflow run release.yml",
            "true  # gh workflow run release.yml",
        ),
    ),
    (
        "handoff_dispatches_release",
        "dispatch step disabled with if: false",
        disable(PLZ, "dispatch-artifact-release", "gh workflow run release.yml"),
    ),
    (
        "handoff_dispatches_release",
        "dispatch moved to another job",
        chain(
            replace_in_job(PLZ, "dispatch-artifact-release", "gh workflow run release.yml", "true"),
            append_run(PLZ, "release-plz-pr", "gh workflow run release.yml -f tag=v1.2.3"),
        ),
    ),
    (
        "handoff_can_dispatch",
        "actions: read instead of write",
        edit_job(PLZ, "dispatch-artifact-release", set_key("permissions", {"actions": "read"})),
    ),
    (
        "handoff_can_dispatch",
        "job block dropped, workflow default is contents: read",
        edit_job(PLZ, "dispatch-artifact-release", lambda j: j.pop("permissions")),
    ),
    (
        "handoff_after_release",
        "handoff no longer needs release-plz-release",
        drop_need(PLZ, "dispatch-artifact-release", "release-plz-release"),
    ),
    (
        "release_exposes_tag",
        "tag output dropped",
        edit_job(PLZ, "release-plz-release", lambda j: j.pop("outputs")),
    ),
    (
        "release_exposes_tag",
        "tag output reads the wrong step",
        edit_job(
            PLZ,
            "release-plz-release",
            set_key("outputs", {"tag": "${{ steps.release.outputs.tag }}"}),
        ),
    ),
    ("release_exposes_tag", "step id tag renamed", drop_tag_step_id),
    (
        "tag_selected_by_name",
        "select by package name dropped",
        replace_in_job(
            PLZ, "release-plz-release", 'select(.package_name == "rolter-gateway")', "select(.tag)"
        ),
    ),
    (
        "tag_selected_by_name",
        "tag resolution commented out",
        comment_out(
            PLZ, "release-plz-release", 'select(.package_name == "rolter-gateway")', whole_step=True
        ),
    ),
    (
        "tag_validated",
        "vX.Y.Z validation dropped",
        replace_in_job(PLZ, "release-plz-release", "grep -Eq '^v[0-9]", "grep -Eq '^"),
    ),
    (
        "tag_validated",
        "vX.Y.Z validation commented out",
        comment_out(PLZ, "release-plz-release", "grep -Eq '^v[0-9]"),
    ),
    (
        "no_first_tagged_release",
        "first tagged entry taken again",
        append_run(
            PLZ,
            "release-plz-release",
            "jq -r 'map(select(.tag != null and .tag != \"\")) | .[0].tag'",
        ),
    ),
    (
        "runs_event_in_path",
        "query moved to -f on its own line",
        append_run(
            PLZ,
            "dispatch-artifact-release",
            'gh api \\\n  "repos/$REPO/actions/workflows/release.yml/runs" \\\n'
            "  -f event=workflow_dispatch",
        ),
    ),
    (
        "runs_event_in_path",
        "query moved to -F inline",
        append_run(PLZ, "dispatch-release-pr-ci", 'gh api "$url" -F event=workflow_dispatch'),
    ),
    (
        "runs_event_in_path",
        "event sent as a field even with -X GET",
        replace_in_job(
            PLZ,
            "dispatch-artifact-release",
            'runs?event=workflow_dispatch&per_page=20"',
            'runs?per_page=20" -X GET -f event=workflow_dispatch',
        ),
    ),
    (
        "runs_query_is_get",
        "another field moved out of the runs query path",
        replace_in_job(
            PLZ, "dispatch-release-pr-ci", '&branch=$BRANCH&per_page=20"', '" -f branch="$BRANCH"'
        ),
    ),
    (
        "runs_query_is_get",
        "-X GET named only in a trailing comment",
        replace_in_job(
            PLZ,
            "dispatch-release-pr-ci",
            '&branch=$BRANCH&per_page=20"',
            '" -f branch="$BRANCH"  # -X GET',
        ),
    ),
    (
        "handoff_confirms_run",
        "confirmation query removed",
        replace_in_job(
            PLZ,
            "dispatch-artifact-release",
            "actions/workflows/release.yml/runs?event=workflow_dispatch",
            "actions/runs?event=workflow_dispatch",
        ),
    ),
    (
        "handoff_confirms_run",
        "confirmation query commented out",
        comment_out(PLZ, "dispatch-artifact-release", "actions/workflows/release.yml/runs?event="),
    ),
    ("pr_gate_job", "release-pr gate deleted", drop_job(PLZ, "dispatch-release-pr-ci")),
    (
        "pr_gate_dispatches_ci",
        "ci dispatch removed from the gate",
        replace_in_job(PLZ, "dispatch-release-pr-ci", "gh workflow run ci.yml", "true"),
    ),
    (
        "pr_gate_dispatches_ci",
        "ci dispatch commented out",
        comment_out(PLZ, "dispatch-release-pr-ci", "gh workflow run ci.yml"),
    ),
    (
        "pr_gate_can_dispatch",
        "actions: write dropped from the gate",
        edit_job(PLZ, "dispatch-release-pr-ci", set_key("permissions", {"pull-requests": "read"})),
    ),
    (
        "pr_gate_after_release_pr",
        "gate no longer needs release-plz-pr",
        drop_need(PLZ, "dispatch-release-pr-ci", "release-plz-pr"),
    ),
    (
        "pr_gate_confirms_run",
        "ci confirmation query removed",
        replace_in_job(
            PLZ,
            "dispatch-release-pr-ci",
            "actions/workflows/ci.yml/runs?event=workflow_dispatch",
            "actions/runs?event=workflow_dispatch",
        ),
    ),
    (
        "pr_gate_confirms_run",
        "ci confirmation query commented out",
        comment_out(PLZ, "dispatch-release-pr-ci", "actions/workflows/ci.yml/runs?event="),
    ),
    ("ci_dispatchable", "ci.yml workflow_dispatch removed", drop_trigger(CI, "workflow_dispatch")),
    (
        "publish_gated",
        "release-plz-release no longer needs release-gate",
        drop_need(PLZ, "release-plz-release", "release-gate"),
    ),
    (
        "publish_gated",
        "the verified condition dropped from release-plz-release",
        edit_job(PLZ, "release-plz-release", lambda j: j.pop("if")),
    ),
    (
        "publish_gated",
        "the verified condition widened with always()",
        edit_job(
            PLZ,
            "release-plz-release",
            set_key("if", "${{ always() && needs.release-gate.outputs.verified == 'true' }}"),
        ),
    ),
    (
        "publish_gated",
        "the verified condition or-ed with another",
        edit_job(
            PLZ,
            "release-plz-release",
            set_key(
                "if", "needs.release-gate.outputs.verified == 'true' || github.event_name == 'push'"
            ),
        ),
    ),
    (
        "publish_gated",
        "release-plz-release keyed on the pending detector instead",
        edit_job(
            PLZ,
            "release-plz-release",
            set_key("if", "needs.release-gate.outputs.pending == 'true'"),
        ),
    ),
    ("publish_gated", "release-gate disabled with if: false", disable(PLZ, "release-gate")),
    (
        "publish_gated",
        "a quality.yml gate run past with always()",
        chain(
            lambda t: jobs_of(t, PLZ).__setitem__(
                "verify", {"uses": "./.github/workflows/quality.yml"}
            ),
            edit_job(PLZ, "release-plz-release", set_key("needs", ["verify"])),
            edit_job(PLZ, "release-plz-release", set_key("if", "${{ always() }}")),
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait replaced by a check-runs name lookup",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            'gh api "repos/$REPO/commits/$SHA/check-runs" '
            "-q '.check_runs[] | select(.name == \"ci-ok\") | .conclusion' | grep -qx success",
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait commented out",
        comment_out(PLZ, "release-gate", "scripts/wait-for-ci-gate.sh"),
    ),
    (
        "gate_waits_on_push_run",
        "the wait made non-fatal",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            "bash scripts/wait-for-ci-gate.sh || true",
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait sent to the background",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            "bash scripts/wait-for-ci-gate.sh &",
        ),
    ),
    (
        "gate_waits_on_push_run",
        "verified written by the step before the wait",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            'echo "verified=true" >> "$GITHUB_OUTPUT"\nbash scripts/wait-for-ci-gate.sh',
        ),
    ),
    (
        "gate_waits_on_push_run",
        "verified written by the step after the wait, with errexit off",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            'set +e\nbash scripts/wait-for-ci-gate.sh\necho "verified=true" >> "$GITHUB_OUTPUT"',
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait piped into tee, which hides its exit status",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            'bash scripts/wait-for-ci-gate.sh | tee "$RUNNER_TEMP/gate.log"',
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait wrapped in a function that is excused with ||",
        replace_in_job(
            PLZ,
            "release-gate",
            "bash scripts/wait-for-ci-gate.sh",
            "gate() {\n  bash scripts/wait-for-ci-gate.sh\n}\ngate || true",
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait step allowed to fail",
        edit_job(
            PLZ,
            "release-gate",
            lambda j: [s.__setitem__("continue-on-error", True) for s in j["steps"]],
        ),
    ),
    (
        "gate_waits_on_push_run",
        "verified read from the pending detector",
        edit_job(
            PLZ,
            "release-gate",
            set_key("outputs", {"verified": "${{ steps.pending.outputs.pending }}"}),
        ),
    ),
    (
        "gate_waits_on_push_run",
        "the wait step disabled with if: false",
        disable(PLZ, "release-gate", "scripts/wait-for-ci-gate.sh"),
    ),
    (
        "wait_writes_verified_last",
        "the wait script no longer writes verified",
        replace_in_script(WAIT, 'echo "verified=true" >>"${GITHUB_OUTPUT:-/dev/null}"', "true"),
    ),
    (
        "wait_writes_verified_last",
        "verified also written before the verdict",
        replace_in_script(
            WAIT,
            "set -euo pipefail\n",
            'set -euo pipefail\necho "verified=true" >>"${GITHUB_OUTPUT:-/dev/null}"\n',
        ),
    ),
    (
        "wait_writes_verified_last",
        "verified written only to the log",
        replace_in_script(
            WAIT, 'echo "verified=true" >>"${GITHUB_OUTPUT:-/dev/null}"', 'echo "verified=true"'
        ),
    ),
    (
        "wait_writes_verified_last",
        "the write left only in a comment",
        replace_in_script(WAIT, 'echo "verified=true"', '# echo "verified=true"'),
    ),
    (
        "wait_binds_push_run",
        "runs of any event accepted",
        replace_in_script(WAIT, "&event=push", ""),
    ),
    (
        "wait_binds_push_run",
        "runs of any branch accepted",
        replace_in_script(WAIT, "&branch=master", ""),
    ),
    (
        "wait_binds_push_run",
        "the wait moved onto check-runs by name",
        replace_in_script(
            WAIT,
            "actions/workflows/ci.yml/runs?head_sha=${SHA}&event=push&branch=master",
            "commits/${SHA}/check-runs?check_name=ci-ok",
        ),
    ),
    (
        "wait_binds_push_run",
        "the ci-ok job no longer required",
        replace_in_script(WAIT, 'select(.name == "ci-ok")', 'select(.name == "gate-ok")'),
    ),
    (
        "wait_binds_push_run",
        "the push-run query left only in a comment",
        replace_in_script(WAIT, 'runs_path="repos/', '# runs_path="repos/'),
    ),
    ("ci_ok_job", "ci-ok job deleted", drop_job(CI, "ci-ok")),
    ("ci_ok_job", "ci-ok check renamed", edit_job(CI, "ci-ok", set_key("name", "all green"))),
    (
        "release_dispatchable",
        "release.yml workflow_dispatch removed",
        drop_trigger(REL, "workflow_dispatch"),
    ),
    ("release_tag_input", "tag input renamed", drop_tag_input),
    ("publish_pypi_job", "publish-pypi deleted", drop_job(REL, "publish-pypi")),
    (
        "gate_not_rerun",
        "quality.yml called again",
        lambda t: jobs_of(t, REL).__setitem__(
            "verify", {"uses": "./.github/workflows/quality.yml"}
        ),
    ),
    (
        "gate_not_rerun",
        "quality.yml called by full path and ref",
        lambda t: jobs_of(t, REL).__setitem__(
            "verify", {"uses": "rolter-ai/rolter/.github/workflows/quality.yml@master"}
        ),
    ),
    (
        "gate_requires_ci_ok",
        "ci-ok dropped from the default required checks",
        replace_in_job(REL, "verify-external-checks", "'ci-ok,", "'"),
    ),
    (
        "gate_requires_ci_ok",
        "required checks no longer defaulted",
        replace_in_job(REL, "verify-external-checks", "RELEASE_REQUIRED_CHECKS ||", "REQUIRED ||"),
    ),
    ("parity_job", "verify-parity deleted", drop_job(REL, "verify-parity")),
    (
        "parity_job",
        "verify-parity disabled with if: false",
        edit_job(REL, "verify-parity", set_key("if", "${{ false }}")),
    ),
    (
        "parity_observes_publish",
        "verify-parity stops observing publish-pypi",
        drop_need(REL, "verify-parity", "publish-pypi"),
    ),
    (
        "parity_observes_publish",
        "verify-parity stops observing publish-docker",
        drop_need(REL, "verify-parity", "publish-docker"),
    ),
    (
        "parity_observes_publish",
        "verify-parity stops observing smoke-wheels",
        drop_need(REL, "verify-parity", "smoke-wheels"),
    ),
    (
        "parity_always_runs",
        "always() dropped",
        edit_job(REL, "verify-parity", lambda j: j.pop("if")),
    ),
    (
        "parity_always_runs",
        "always() swapped for success()",
        edit_job(REL, "verify-parity", set_key("if", "${{ success() }}")),
    ),
    (
        "macos_x86_64_wheel",
        "x86_64 macos target dropped",
        replace_in_job(REL, "build-wheels", "x86_64-apple-darwin", "aarch64-apple-darwin"),
    ),
    (
        "sdist_built",
        "sdist build dropped",
        drop_steps(REL, "build-wheels", lambda s: (s.get("with") or {}).get("command") == "sdist"),
    ),
    (
        "parity_expects_macos_x86_64",
        "macos x86_64 expectation dropped",
        replace_in_job(REL, "verify-parity", 'expect "macos x86_64 wheel"', "true"),
    ),
    (
        "parity_expects_macos_x86_64",
        "macos x86_64 expectation commented out",
        comment_out(REL, "verify-parity", 'expect "macos x86_64 wheel"'),
    ),
    (
        "parity_expects_sdist",
        "sdist expectation dropped",
        replace_in_job(REL, "verify-parity", 'expect "sdist"', "true"),
    ),
    (
        "parity_expects_sdist",
        "sdist expectation commented out",
        comment_out(REL, "verify-parity", 'expect "sdist"'),
    ),
    ("build_image_job", "build-image deleted", drop_job(REL, "build-image")),
    (
        "image_pushed_by_digest",
        "images pushed by tag",
        replace_in_job(REL, "build-image", "push-by-digest=true", "push-by-digest=false"),
    ),
    (
        "pypi_waits_for_barrier",
        "publish-pypi stops waiting for smoke-image",
        drop_need(REL, "publish-pypi", "smoke-image"),
    ),
    (
        "pypi_waits_for_barrier",
        "publish-pypi stops waiting for build-wheels",
        drop_need(REL, "publish-pypi", "build-wheels"),
    ),
    (
        "docker_waits_for_barrier",
        "publish-docker stops waiting for smoke-wheels",
        drop_need(REL, "publish-docker", "smoke-wheels"),
    ),
    ("docker_waits_for_barrier", "publish-docker deleted", drop_job(REL, "publish-docker")),
    ("smoke_wheels_job", "smoke-wheels deleted", drop_job(REL, "smoke-wheels")),
    ("smoke_image_job", "smoke-image deleted", drop_job(REL, "smoke-image")),
    (
        "wheel_smoke_offline",
        "wheel smoke may resolve from pypi",
        replace_in_job(REL, "smoke-wheels", "--no-index ", ""),
    ),
    (
        "wheel_smoke_offline",
        "--no-index left only in a comment",
        chain(
            replace_in_job(REL, "smoke-wheels", "--no-index ", ""),
            append_run(REL, "smoke-wheels", "# keep --no-index here"),
        ),
    ),
    (
        "wheel_smoke_offline",
        "--no-index left only in a trailing comment",
        replace_in_job(
            REL,
            "smoke-wheels",
            '--no-index --find-links dist "rolter==$VERSION"',
            '--find-links dist "rolter==$VERSION"  # --no-index',
        ),
    ),
    (
        "wheel_smoke_offline",
        "wheel install step disabled with if: false",
        disable(REL, "smoke-wheels", "--no-index"),
    ),
]


# (what the fixture changes, the change). correct wiring written another way:
# every check must stay green through each of these
BENIGN: list[tuple[str, Callable[[Tree], None]]] = [
    (
        "runs query fields sent as a GET",
        replace_in_job(
            PLZ,
            "dispatch-release-pr-ci",
            '&branch=$BRANCH&per_page=20"',
            '" -X GET -f branch="$BRANCH" -f per_page=20',
        ),
    ),
    (
        "single need written as a one-item list",
        edit_job(PLZ, "dispatch-artifact-release", set_key("needs", ["release-plz-release"])),
    ),
    (
        "extra need on the parity gate",
        edit_job(
            REL,
            "verify-parity",
            lambda j: j.__setitem__("needs", sorted(needs(j) | {"publish-release-notes"})),
        ),
    ),
    (
        "always() written as an expression",
        edit_job(REL, "verify-parity", set_key("if", "${{ always() }}")),
    ),
    (
        "triggers written as a list",
        lambda t: t[CI].__setitem__("on", sorted(triggers(t[CI]))),
    ),
    (
        "a trailing comment after the dispatch",
        replace_in_job(
            PLZ,
            "dispatch-artifact-release",
            '-f tag="$TAG"',
            '-f tag="$TAG"  # hand the tag to release.yml',
        ),
    ),
    (
        "a # inside quotes or a parameter expansion before the command",
        replace_in_job(
            REL,
            "smoke-wheels",
            "python -m pip install",
            ': "${REF#v}" \'#\' "#"; python -m pip install',
        ),
    ),
    (
        "a comment line inside a multi-line quoted string",
        replace_in_job(
            PLZ,
            "release-plz-release",
            'map(select(.package_name == "rolter-gateway"))',
            '# keep the order\n           map(select(.package_name == "rolter-gateway"))',
        ),
    ),
    (
        "the verified condition beside another, as an expression",
        edit_job(
            PLZ,
            "release-plz-release",
            set_key(
                "if",
                "${{ github.repository == 'rolter-ai/rolter' && "
                "needs.release-gate.outputs.verified == 'true' }}",
            ),
        ),
    ),
    (
        "the verified condition in parentheses, compared the other way round",
        edit_job(
            PLZ,
            "release-plz-release",
            set_key("if", "${{ ('true' == needs.release-gate.outputs.verified) }}"),
        ),
    ),
    (
        "release-plz-release gated by a job that re-runs quality.yml instead",
        chain(
            lambda t: jobs_of(t, PLZ).__setitem__(
                "verify", {"uses": "./.github/workflows/quality.yml"}
            ),
            edit_job(PLZ, "release-plz-release", set_key("needs", ["verify"])),
            edit_job(PLZ, "release-plz-release", lambda j: j.pop("if")),
        ),
    ),
    (
        "the wait step under a shell without errexit",
        edit_job(
            PLZ,
            "release-gate",
            lambda j: [
                s.__setitem__("shell", "bash {0}") for s in j["steps"] if s.get("id") == "wait"
            ],
        ),
    ),
    (
        "the wait script run by path",
        replace_in_job(
            PLZ, "release-gate", "bash scripts/wait-for-ci-gate.sh", "./scripts/wait-for-ci-gate.sh"
        ),
    ),
    (
        "a job or step with a real condition",
        chain(
            edit_job(
                REL,
                "smoke-wheels",
                lambda j: [s.__setitem__("if", "${{ !cancelled() }}") for s in j["steps"]],
            ),
            edit_job(PLZ, "dispatch-artifact-release", set_key("if", "${{ needs.x.outputs.y }}")),
        ),
    ),
]


def reversed_needs(node: Any) -> Any:
    if isinstance(node, dict):
        out = {k: reversed_needs(v) for k, v in node.items()}
        if isinstance(out.get("needs"), list):
            out["needs"] = list(reversed(out["needs"]))
        return out
    if isinstance(node, list):
        return [reversed_needs(v) for v in node]
    return node


def relayouts(tree: Tree) -> Iterator[tuple[str, Tree]]:
    # the same workflows written the ways #1723 was about: one-line flow lists,
    # block sequences, reordered members. none of it changes what actions runs
    for style, flow in (("block", False), ("flow", True)):
        yield (
            style,
            {
                f: (
                    parse(
                        yaml.safe_dump(reversed_needs(wf), default_flow_style=flow, sort_keys=True)
                    )
                    if isinstance(wf, dict)
                    else wf
                )
                for f, wf in tree.items()
            },
        )


def self_test(root: Path) -> int:
    problems: list[str] = []
    tree, errors = load(root)
    if errors:
        for message in errors:
            print(f"error: {message}", file=sys.stderr)
        return 1

    # the real workflows must pass, or nothing below means anything
    problems += [f"the committed workflows fail {c.id}" for c in failures(tree)]

    for style, relaid in relayouts(tree):
        problems += [
            f"{c.id} fails once the workflows are laid out as {style}" for c in failures(relaid)
        ]

    for what, change in BENIGN:
        benign = copy.deepcopy(tree)
        try:
            change(benign)
        except (NoOp, KeyError, IndexError) as err:
            problems.append(f"benign change {what!r} did not apply, so it proves nothing: {err}")
            continue
        problems += [f"{c.id} fails on correct wiring ({what})" for c in failures(benign)]

    known = {c.id for c in CHECKS}
    for check_id, what, mutate in BREAKS:
        if check_id not in known:
            problems.append(f"break {what!r} names an unknown check {check_id}")
            continue
        broken = copy.deepcopy(tree)
        try:
            mutate(broken)
        except (NoOp, KeyError, IndexError) as err:
            problems.append(f"break {what!r} did not apply, so it proves nothing: {err}")
            continue
        if check_id not in {c.id for c in failures(broken)}:
            problems.append(f"{check_id} stayed green with {what}")
    problems += [
        f"{c.id} has no broken fixture proving it can fail"
        for c in CHECKS
        if c.id not in {b[0] for b in BREAKS}
    ]

    # the loader must read `on:` as a key and refuse what actions refuses
    if "on" not in parse("on: push\njobs: {}\n"):
        problems.append("`on:` did not parse as the string key 'on'")
    try:
        parse("jobs:\n  a: {}\n  a: {}\n")
        problems.append("a duplicate job id parsed without an error")
    except yaml.YAMLError:
        # the refusal is the expected outcome, so there is nothing to record
        pass

    # and end to end: a broken tree on disk is a failing run
    with tempfile.TemporaryDirectory() as tmp:
        broken = copy.deepcopy(tree)
        drop_need(REL, "verify-parity", "publish-pypi")(broken)
        for rel, wf in broken.items():
            (Path(tmp) / rel).parent.mkdir(parents=True, exist_ok=True)
            text = yaml.safe_dump(wf) if isinstance(wf, dict) else wf
            (Path(tmp) / rel).write_text(text, encoding="utf-8")
        if [c.id for c in failures(load(Path(tmp))[0])] != ["parity_observes_publish"]:
            problems.append(
                "a workflow broken on disk did not fail exactly parity_observes_publish"
            )

    for problem in problems:
        print(f"error: self-test: {problem}", file=sys.stderr)
    if problems:
        return 1
    print(
        f"self-test: ok ({len(CHECKS)} checks, {len(BREAKS)} broken fixtures all caught, "
        f"{len(BENIGN)} rewritten-but-correct fixtures and the block and flow layouts pass)"
    )
    return 0


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="break each invariant in a copy of the workflows and fail unless its check catches it",
    )
    args = parser.parse_args(argv)
    return self_test(ROOT) if args.self_test else run(ROOT)


if __name__ == "__main__":
    sys.exit(main())
