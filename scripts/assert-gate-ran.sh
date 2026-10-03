#!/usr/bin/env bash
# the `edited` fast path of ci.yml's `ci-ok`: exits 0 only when a ci.yml run on
# this pull request's head sha recorded a passing `gate-ok` job, and no other
# run on that sha still has a gate verdict to deliver.
#
#   REPO=rolter-ai/rolter HEAD_SHA=<40-hex sha> SELF_RUN_ID=<run id> GH_TOKEN=... \
#     bash scripts/assert-gate-ran.sh
#
# a title or body edit starts an `edited` run that skips the heavy gate, so its
# `ci-ok` can only vouch for a gate some other run already finished (#1328). it
# used to decline at once when that gate was still running, on the theory that
# the gate run's newer `ci-ok` would supersede the red one. github does not
# work that way: the commit's status rollup keeps both `ci-ok` check-runs, the
# red one included, and the pull request stays blocked until somebody re-runs
# the edited run by hand (#2391). so instead of declining, this waits for the
# in-flight gate to deliver its verdict and then reports that verdict.
#
# what it waits for is the `gate-ok` job, not the run. `gate-ok` concludes as
# soon as `quality` and `codeql` do — `success` when both passed, `skipped`
# otherwise — so there is nothing left to learn about the gate once it has a
# conclusion, even while that run's own `ci-ok` is still going. an `edited`
# run's `gate-ok` is skipped within seconds of the run starting, which is what
# keeps two concurrent edits from waiting on each other until the deadline.
#
# fails closed: an api call that still fails after three attempts, a gate that
# has not delivered a verdict after the deadline, and a sha with no passing
# `gate-ok` all exit 1. the deadline is sized from the gate (median ~29 min, max
# 61 min, see scripts/wait-for-ci-gate.sh), and `ci-ok`'s `timeout-minutes`
# sits above it.
#
# bash 3.2 compatible, so the fixture tests (scripts/test-assert-gate-ran.sh)
# run on a stock mac too.
set -euo pipefail

# one poll a minute keeps a long wait to ~2 api calls a minute per in-flight
# run, well inside the GITHUB_TOKEN budget even with several edits waiting
poll_seconds=60
max_polls=90

: "${REPO:?REPO must name the repository, e.g. rolter-ai/rolter}"
: "${HEAD_SHA:?HEAD_SHA must be the pull request head sha}"
: "${SELF_RUN_ID:?SELF_RUN_ID must be this run id, so it can be left out}"
if ! printf '%s' "$HEAD_SHA" | grep -Eq '^[0-9a-f]{40}$'; then
  echo "::error::HEAD_SHA '$HEAD_SHA' is not a full commit sha; refusing to report green without a gate result"
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# api GET into a file, three attempts. no pipeline on purpose: under pipefail a
# `| head` or `| grep -q` short-circuit fails for the wrong reason (#1291,
# #1306), so the body goes to a file and jq reads it
api() {
  local path=$1 out=$2 attempt=1
  while ! gh api "$path" >"$out"; do
    if [ "$attempt" -ge 3 ]; then
      # a flaking query must never resolve to green: no answer is not an answer
      echo "::error::GET $path failed $attempt times; refusing to report green without a gate result"
      return 1
    fi
    attempt=$((attempt + 1))
    sleep 5
  done
}

# gate_verdict RUN_ID: sets `verdict` to the conclusion of the run's `gate-ok`
# job, or to nothing while it has none. the jobs listing is of the latest
# attempt, so a gate re-run that is in flight reads as undecided again. it sets
# a variable rather than printing because a command substitution runs with
# errexit cleared, and its stdout would swallow the `::error::` lines
verdict=""
gate_verdict() {
  local jobs="$work/jobs-$1.json"
  verdict=""
  api "repos/${REPO}/actions/runs/$1/jobs?per_page=100" "$jobs" || return 1
  if ! jq -r '[.jobs[] | select(.name == "gate-ok") | .conclusion // empty] | first // ""' \
    "$jobs" >"$work/verdict"; then
    echo "::error::the jobs listing for ci run $1 was not the expected json; refusing to report green without a gate result"
    return 1
  fi
  verdict=$(cat "$work/verdict")
}

# scoped to ci.yml by filename: a run of any other workflow on this sha says
# nothing about this gate
runs_path="repos/${REPO}/actions/workflows/ci.yml/runs?head_sha=${HEAD_SHA}&per_page=100&exclude_pull_requests=true"
runs="$work/runs.json"

poll=0
while :; do
  api "$runs_path" "$runs"
  # this listing contains the current run too — drop it by id, never by name,
  # since every run of this workflow shares the name
  if ! jq -r --arg self "$SELF_RUN_ID" \
    '.workflow_runs[] | select((.id | tostring) != $self) | "\(.id) \(.status)"' \
    "$runs" >"$work/others.txt"; then
    echo "::error::the ci runs listing for ${HEAD_SHA} was not the expected json; refusing to report green without a gate result"
    exit 1
  fi

  # a run still going but whose `gate-ok` already concluded has nothing more to
  # say about the gate, so only an undecided one is worth waiting for
  pending=""
  while read -r run_id status; do
    [ -n "$run_id" ] || continue
    [ "$status" != completed ] || continue
    gate_verdict "$run_id"
    if [ -z "$verdict" ]; then
      pending="${pending} ${run_id}"
    fi
  done <"$work/others.txt"

  if [ -z "$pending" ]; then
    break
  fi
  if [ "$poll" -ge "$max_polls" ]; then
    echo "::error::gate still running on ${HEAD_SHA} after $((max_polls * poll_seconds / 60))m (run(s):${pending}): declining to report green over an unfinished gate. once that gate finishes, re-run this job: gh run rerun ${SELF_RUN_ID} --failed"
    exit 1
  fi
  if [ "$poll" -eq 0 ]; then
    echo "::notice::the gate on ${HEAD_SHA} is still running (run(s):${pending}). this metadata-only run waits for its verdict (up to $((max_polls * poll_seconds / 60))m) and then reports it, so no action is needed"
  else
    echo "still waiting for the gate on ${HEAD_SHA} (run(s):${pending})"
  fi
  poll=$((poll + 1))
  sleep "$poll_seconds"
done

# every other run now has a gate verdict or is finished. one counts as a gate
# pass when its `gate-ok` job concluded success — not when the run as a whole
# did. the run's conclusion folds in `ci-ok`'s pr title and pr body steps,
# neither of which says anything about whether the code was gated, and a
# failed body check on a frozen dirty body used to strand the sha forever
# (#1522). a cancelled run has no successful `gate-ok`, so it counts as no run
# at all rather than as a pass
echo "ci runs on ${HEAD_SHA}:"
passed=0
while read -r run_id status; do
  [ -n "$run_id" ] || continue
  gate_verdict "$run_id"
  echo "  run ${run_id}: ${status}, gate-ok ${verdict:--}"
  if [ "$verdict" = success ]; then
    passed=$((passed + 1))
  fi
done <"$work/others.txt"

if [ "$passed" -eq 0 ]; then
  echo "::error::no ci run on ${HEAD_SHA} recorded a passing gate-ok job (a cancelled run counts as none); the heavy gate failed, was cancelled, or never ran here — push a fix or re-run the gate instead of editing the title to go green, then re-run this job too: gh run rerun ${SELF_RUN_ID} --failed"
  exit 1
fi
echo "gate already passed for ${HEAD_SHA} in ${passed} run(s)"
