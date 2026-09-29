#!/usr/bin/env bash
# waits for the ci.yml *push* run on one master commit to finish, then exits 0
# only if that run's `ci-ok` job concluded success. its last action is writing
# `verified=true` to $GITHUB_OUTPUT, so reaching the end of this script is the
# only way the step that runs it can report a verified commit.
#
#   REPO=rolter-ai/rolter SHA=<40-hex sha> GH_TOKEN=... bash scripts/wait-for-ci-gate.sh
#
# release-plz.yml's `release-gate` job runs this before the crates.io publish
# (#2025). it is bound to a run rather than to a check-run name on purpose: any
# workflow can post a check-run called `ci-ok` on a master sha (a pull_request
# run from master into another branch reports on master's head, and workflows
# fired by outsiders write check-runs there too), while a ci.yml push run on
# a sha can only come from ci.yml at that sha. release.yml's
# `verify-external-checks` still matches check-run names; the two are
# different mechanisms, not copies of each other.
#
# fails closed: an api call that still fails after three attempts, a completed
# run with no `ci-ok` job, any conclusion other than success, and no verdict
# after 90 minutes all exit 1. the deadline is sized from the push gate (median
# ~29 min, max 61 min over 26 runs), and the calling job's timeout sits above it.
# recovery for a timeout or a red run is in docs/dev-docs/development/packaging.md
# ("The crates.io publish waits for the push run").
#
# bash 3.2 compatible, so the fixture tests (scripts/test-release-gate.sh) run
# on a stock mac too.
set -euo pipefail

deadline_minutes=90
poll_seconds=30

: "${REPO:?REPO must name the repository, e.g. rolter-ai/rolter}"
: "${SHA:?SHA must be the commit to gate}"
if ! printf '%s' "$SHA" | grep -Eq '^[0-9a-f]{40}$'; then
  echo "::error::SHA '$SHA' is not a full commit sha; refusing to gate on it"
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# api GET into a file, three attempts. no pipeline on purpose: under pipefail a
# `| head` or `| grep -q` short-circuit fails for the wrong reason (#1306)
api() {
  local path=$1 out=$2 attempt=1
  while ! gh api "$path" >"$out"; do
    if [ "$attempt" -ge 3 ]; then
      echo "::error::GET $path failed $attempt times; refusing to publish without a gate result"
      return 1
    fi
    attempt=$((attempt + 1))
    sleep 5
  done
}

summary() {
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s\n' "$1" >>"$GITHUB_STEP_SUMMARY"
  fi
}

runs_path="repos/${REPO}/actions/workflows/ci.yml/runs?head_sha=${SHA}&event=push&branch=master&per_page=100"
deadline=$(($(date -u +%s) + deadline_minutes * 60))
echo "waiting for the ci.yml push run on ${SHA} to finish (up to ${deadline_minutes}m)"

while :; do
  api "$runs_path" "$work/runs.json"
  # the query already filters on all three; checking them again here means a
  # filter the api stopped honouring reads as "no run", never as someone
  # else's run
  if ! jq --arg sha "$SHA" \
    '[.workflow_runs[] | select(.head_sha == $sha and .event == "push" and .head_branch == "master")]' \
    "$work/runs.json" >"$work/matched.json"; then
    echo "::error::the ci.yml runs listing for ${SHA} was not the expected json; refusing to publish without a gate result"
    exit 1
  fi
  total=$(jq 'length' "$work/matched.json")
  unfinished=$(jq '[.[] | select(.status != "completed")] | length' "$work/matched.json")

  if [ "$total" -gt 0 ] && [ "$unfinished" -eq 0 ]; then
    break
  fi
  if [ "$(date -u +%s)" -ge "$deadline" ]; then
    echo "::error::no finished ci.yml push run on ${SHA} after ${deadline_minutes}m (runs: ${total}, unfinished: ${unfinished}); refusing to publish. once ci-ok is green on that run, re-run this job with: gh run rerun ${GITHUB_RUN_ID:-<release-plz run id>} --failed"
    summary "release gate: timed out after ${deadline_minutes}m waiting for the ci.yml push run on \`${SHA}\`"
    exit 1
  fi
  if [ "$total" -eq 0 ]; then
    echo "no ci.yml push run on ${SHA} yet"
  else
    echo "${unfinished} ci.yml push run(s) on ${SHA} still running"
  fi
  sleep "$poll_seconds"
done

# the newest by creation time, then id. a push run is never cancelled by a
# later push (ci.yml gives master pushes a per-run concurrency group), so there
# is normally exactly one
run_id=$(jq -r 'sort_by(.created_at, .id) | last | .id' "$work/matched.json")
run_url=$(jq -r 'sort_by(.created_at, .id) | last | .html_url // ""' "$work/matched.json")
echo "ci.yml push run on ${SHA}: ${run_id} ${run_url}"

# the jobs of the run's latest attempt, so a re-run that went green counts.
# ci.yml has far fewer than 100 jobs; were ci-ok ever pushed onto a second
# page, the lookup below would find none and fail closed
api "repos/${REPO}/actions/runs/${run_id}/jobs?per_page=100" "$work/jobs.json"
if ! jq -r '.jobs[] | "  \(.name): \(.status) \(.conclusion // "-")"' "$work/jobs.json"; then
  echo "::error::the jobs listing for ci.yml run ${run_id} was not the expected json; refusing to publish without a gate result"
  exit 1
fi
found=$(jq '[.jobs[] | select(.name == "ci-ok")] | length' "$work/jobs.json")
if [ "$found" -eq 0 ]; then
  echo "::error::ci.yml run ${run_id} finished without a ci-ok job (cancelled before it ran?); refusing to publish"
  summary "release gate: ci.yml run ${run_id} on \`${SHA}\` has no ci-ok job"
  exit 1
fi
# every ci-ok job in the attempt must be green, so a second one cannot outvote
# the first
bad=$(jq -r '[.jobs[] | select(.name == "ci-ok") | select(.conclusion != "success") | (.conclusion // .status)] | join(",")' "$work/jobs.json")
if [ -n "$bad" ]; then
  echo "::error::ci-ok on ci.yml run ${run_id} concluded '${bad}', not success; refusing to publish ${SHA}. if the failure was a flake, re-run that ci run, and once its ci-ok is green: gh run rerun ${GITHUB_RUN_ID:-<release-plz run id>} --failed"
  summary "release gate: ci-ok on ci.yml run ${run_id} concluded \`${bad}\` for \`${SHA}\`"
  exit 1
fi

echo "ci-ok succeeded on ci.yml push run ${run_id} for ${SHA}"
summary "release gate: ci-ok succeeded on the ci.yml push run for \`${SHA}\` (${run_url:-run ${run_id}})"

# the only write of the step output, and the last command, so every failure
# above leaves it unset whatever shell options the calling step runs under.
# outside actions there is no output file and nothing to write
echo "verified=true" >>"${GITHUB_OUTPUT:-/dev/null}"
