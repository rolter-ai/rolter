#!/usr/bin/env bash
# runs scripts/assert-gate-ran.sh, the `edited` fast path of ci.yml's `ci-ok`,
# against a fake gh and a fake clock, and checks its verdict.
#
# the fast path only ever runs on a pull request whose title or body was
# edited, so a regression shows up either as a hollow green over a gate that
# has not passed (#1328) or as a red `ci-ok` that keeps a pull request blocked
# after its gate went green (#2391). this is the test. it runs as a step of
# quality.yml's `static checks` job and as a prek hook.
#
# bash 3.2 compatible, so it runs on a stock mac too.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
script="$root/scripts/assert-gate-ran.sh"
workflow="$root/.github/workflows/ci.yml"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

sha=0123456789abcdef0123456789abcdef01234567
self=900

failures=0

# ── workflow wiring ───────────────────────────────────────────────────────────
# the step, from its `- name:` line to the first line indented less than its keys
awk '
  $0 == "      - name: assert the gate already ran for this commit" { in_step = 1; print; next }
  in_step && NF && !/^        / { exit }
  in_step { print }
' "$workflow" >"$work/step.yml"

static_check() {
  # static_check DESCRIPTION TEXT
  if ! grep -qF -- "$2" "$work/step.yml"; then
    echo "FAIL [workflow wiring] $1" >&2
    failures=$((failures + 1))
  fi
}
static_check "the step runs this script" 'run: bash scripts/assert-gate-ran.sh'
# shellcheck disable=SC2016 # the workflow text itself, not a shell expansion
static_check "the step keys on the pr head, not the merge commit" \
  'HEAD_SHA: ${{ github.event.pull_request.head.sha }}'
# shellcheck disable=SC2016
static_check "the step leaves its own run out" 'SELF_RUN_ID: ${{ github.run_id }}'

# the job timeout has to sit above the script's own deadline, or a bare
# timeout with no explanation is what a long gate produces
wait_minutes=$(awk -F= '
  /^poll_seconds=/ { p = $2 } /^max_polls=/ { m = $2 }
  END { if (p && m) print int(p * m / 60) }
' "$script")
job_timeout=$(awk '
  $0 == "  ci-ok:" { in_job = 1; next }
  in_job && /^  [a-z]/ { exit }
  in_job && /^    timeout-minutes: [0-9]+$/ { print $2; exit }
' "$workflow")
if [ -z "$wait_minutes" ] || [ -z "$job_timeout" ] || [ "$job_timeout" -le "$wait_minutes" ]; then
  echo "FAIL [workflow wiring] ci-ok's timeout-minutes (${job_timeout:-none}) must exceed the fast path's wait (${wait_minutes:-unknown}m)" >&2
  failures=$((failures + 1))
fi

# ── fakes ─────────────────────────────────────────────────────────────────────
bin="$work/bin"
mkdir -p "$bin"

# clock: seconds since the case started, moved only by `sleep`
cat >"$bin/sleep" <<'EOF'
#!/usr/bin/env bash
echo $(( $(cat "$FAKE/clock") + $1 )) >"$FAKE/clock"
echo "$1" >>"$FAKE/slept"
EOF

# gh: answers the runs listing and each run's jobs listing out of $FAKE/runs,
# a list of {id, done, gate_at, gate, listed}. a run is `completed` from second
# `done` on (never, when null); its `gate-ok` job has conclusion `gate` from
# second `gate_at` on (never, when null), and before that is queued with no
# conclusion, or missing from the listing altogether when `listed` is false.
# $FAKE/faults holds how many of the next calls fail with a 502, and a
# non-empty $FAKE/respond replaces the answer to every call that gets through
cat >"$bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = api ] && [ "$#" -eq 2 ] || { echo "fake gh: unexpected call: $*" >&2; exit 2; }
path=$2
echo "$path" >>"$FAKE/calls"
faults=$(cat "$FAKE/faults")
if [ "$faults" -gt 0 ]; then
  echo $((faults - 1)) >"$FAKE/faults"
  echo "HTTP 502: Bad Gateway (https://api.github.com/$path)" >&2
  exit 1
fi
if [ -s "$FAKE/respond" ]; then
  cat "$FAKE/respond"
  exit 0
fi
now=$(cat "$FAKE/clock")
case $path in
  "repos/rolter-ai/rolter/actions/workflows/ci.yml/runs?head_sha=0123456789abcdef0123456789abcdef01234567&per_page=100&exclude_pull_requests=true")
    jq --argjson now "$now" '{workflow_runs: [.[] | {id,
      status: (if .done != null and $now >= .done then "completed" else "in_progress" end)}]}' \
      "$FAKE/runs" ;;
  repos/rolter-ai/rolter/actions/runs/*/jobs?per_page=100)
    id=${path#repos/rolter-ai/rolter/actions/runs/}
    id=${id%%/*}
    jq --argjson now "$now" --argjson id "$id" '[.[] | select(.id == $id)] | first |
      if . == null then error("no such run") else . end |
      {jobs: ([{name: "ci-ok", status: "in_progress", conclusion: null}]
        + (if .gate_at != null and $now >= .gate_at
           then [{name: "gate-ok", status: "completed", conclusion: .gate}]
           elif .listed == false then []
           else [{name: "gate-ok", status: "queued", conclusion: null}] end))}' \
      "$FAKE/runs" ;;
  *) echo "fake gh: unexpected path: $path" >&2; exit 2 ;;
esac
EOF
chmod +x "$bin/sleep" "$bin/gh"

# ── harness ───────────────────────────────────────────────────────────────────
case_name=""
case_dir=""

# start_case NAME RUNS_JSON: this run (id 900, unfinished, gate-ok skipped as
# on every `edited` run) is always in the listing, as it is on github
start_case() {
  case_name=$1
  case_dir="$work/$(echo "$1" | tr -c 'a-zA-Z0-9\n' '-')"
  mkdir -p "$case_dir"
  echo 0 >"$case_dir/clock"
  echo 0 >"$case_dir/faults"
  : >"$case_dir/respond"
  : >"$case_dir/calls"
  : >"$case_dir/slept"
  jq --argjson self "$self" '[{id: $self, done: null, gate_at: 0, gate: "skipped"}] + .' \
    <<<"$2" >"$case_dir/runs"
}

run_script() {
  local rc=0
  env -i \
    PATH="$bin:$PATH" HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" FAKE="$case_dir" \
    GH_TOKEN=fake-token REPO=rolter-ai/rolter HEAD_SHA="$sha" SELF_RUN_ID="$self" \
    bash --noprofile --norc "$script" >"$case_dir/out" 2>&1 || rc=$?
  echo "$rc" >"$case_dir/rc"
}

check() {
  # check DESCRIPTION ACTUAL EXPECTED
  if [ "$2" != "$3" ]; then
    echo "FAIL [$case_name] $1: expected '$3', got '$2'" >&2
    failures=$((failures + 1))
  fi
}
expect_rc() { check "exit code" "$(cat "$case_dir/rc")" "$1"; }
# the total time slept, since a 90-minute wait is ninety lines of `60`
expect_waited() { check "seconds waited" "$(cat "$case_dir/clock")" "$1"; }
expect_output() {
  if ! grep -qF -- "$1" "$case_dir/out"; then
    echo "FAIL [$case_name] output lacks: $1" >&2
    failures=$((failures + 1))
  fi
}
finish_case() {
  if [ "$failures" -ne "${failures_before:-0}" ]; then
    echo "---- output of [$case_name] ----" >&2
    cat "$case_dir/out" >&2
  fi
  failures_before=$failures
}

passed_run='{"id": 100, "done": 0, "gate_at": 0, "gate": "success"}'

# ── a gate that already finished ──────────────────────────────────────────────
start_case "a finished passing gate is green at once" "[$passed_run]"
run_script
expect_rc 0
expect_waited 0
expect_output "gate already passed for $sha in 1 run(s)"
finish_case

start_case "a finished failing gate is red at once" \
  '[{"id": 100, "done": 0, "gate_at": 0, "gate": "skipped"}]'
run_script
expect_rc 1
expect_waited 0
expect_output "no ci run on $sha recorded a passing gate-ok job"
finish_case

start_case "a cancelled run with no gate-ok counts as none" \
  '[{"id": 100, "done": 0, "gate_at": null, "listed": false}]'
run_script
expect_rc 1
expect_output "no ci run on $sha recorded a passing gate-ok job"
finish_case

start_case "no other run at all is red" '[]'
run_script
expect_rc 1
expect_output "no ci run on $sha recorded a passing gate-ok job"
finish_case

# a failed gate next to a passing one on the same sha passes, as it always has:
# the question is whether the gate passed on this sha, and a re-run that went
# green answers it
start_case "a passing gate beside a failed one is green" \
  "[{\"id\": 99, \"done\": 0, \"gate_at\": 0, \"gate\": \"skipped\"}, $passed_run]"
run_script
expect_rc 0
finish_case

# ── #2391: a gate still in flight ─────────────────────────────────────────────
# the edit landed eight minutes into a gate that takes thirty. this run used to
# go red here, and that red kept the pull request blocked after the gate passed
start_case "an in-flight gate that passes is waited for and green" \
  '[{"id": 100, "done": 1560, "gate_at": 1500, "gate": "success"}]'
run_script
expect_rc 0
expect_waited 1500
expect_output "this metadata-only run waits for its verdict"
expect_output "gate already passed for $sha in 1 run(s)"
finish_case

# the wait must still never turn into a hollow green (#1328)
start_case "an in-flight gate that fails is waited for and red" \
  '[{"id": 100, "done": 1260, "gate_at": 1200, "gate": "skipped"}]'
run_script
expect_rc 1
expect_waited 1200
expect_output "no ci run on $sha recorded a passing gate-ok job"
finish_case

start_case "an in-flight gate cancelled under the wait is red" \
  '[{"id": 100, "done": 600, "gate_at": null, "listed": false}]'
run_script
expect_rc 1
expect_waited 600
expect_output "no ci run on $sha recorded a passing gate-ok job"
finish_case

# an older pass does not let the fast path skip past a gate still in flight
start_case "a passing gate does not cut short a second one in flight" \
  "[$passed_run, {\"id\": 101, \"done\": null, \"gate_at\": 300, \"gate\": \"success\"}]"
run_script
expect_rc 0
expect_waited 300
expect_output "gate already passed for $sha in 2 run(s)"
finish_case

start_case "a gate that never finishes is red at the deadline" \
  '[{"id": 100, "done": null, "gate_at": null}]'
run_script
expect_rc 1
expect_waited 5400
expect_output "gate still running on $sha after 90m (run(s): 100)"
expect_output "gh run rerun $self --failed"
finish_case

# the gate is decided once `gate-ok` concludes; the rest of that run is its
# own pr-title and pr-body steps, which say nothing about the gate
start_case "a decided gate-ok is enough while its run is still going" \
  '[{"id": 100, "done": null, "gate_at": 0, "gate": "success"}]'
run_script
expect_rc 0
expect_waited 0
finish_case

start_case "a gate-ok not yet listed counts as undecided" \
  '[{"id": 100, "done": 200, "gate_at": 120, "gate": "success", "listed": false}]'
run_script
expect_rc 0
expect_waited 120
finish_case

# ── other edits ───────────────────────────────────────────────────────────────
# a second edit's run has skipped its gate-ok within seconds, so the two never
# wait on each other until the deadline
start_case "a concurrent edited run is not waited for" \
  "[$passed_run, {\"id\": 950, \"done\": null, \"gate_at\": 0, \"gate\": \"skipped\"}]"
run_script
expect_rc 0
expect_waited 0
finish_case

start_case "an edited run in its first seconds is waited for briefly" \
  "[$passed_run, {\"id\": 950, \"done\": null, \"gate_at\": 30, \"gate\": \"skipped\"}]"
run_script
expect_rc 0
expect_waited 60
finish_case

# ── the api ───────────────────────────────────────────────────────────────────
start_case "a flaking api is retried" "[$passed_run]"
echo 2 >"$case_dir/faults"
run_script
expect_rc 0
check "retry waits" "$(tr '\n' ' ' <"$case_dir/slept" | sed 's/ $//')" "5 5"
finish_case

start_case "an api that stays down fails closed" "[$passed_run]"
echo 3 >"$case_dir/faults"
run_script
expect_rc 1
expect_output "failed 3 times; refusing to report green without a gate result"
finish_case

start_case "a jobs listing that stays down fails closed" "[$passed_run]"
# the runs listing gets through, then every jobs call fails
run_script_jobs_down() {
  local rc=0
  cat >"$bin/gh-real" <"$bin/gh"
  cat >"$bin/gh" <<'EOF'
#!/usr/bin/env bash
case $2 in
  */jobs*) echo "$2" >>"$FAKE/calls"; echo "HTTP 502" >&2; exit 1 ;;
esac
exec "$(dirname "$0")/gh-real" "$@"
EOF
  chmod +x "$bin/gh" "$bin/gh-real"
  run_script
  mv "$bin/gh-real" "$bin/gh"
}
run_script_jobs_down
expect_rc 1
expect_output "/jobs?per_page=100 failed 3 times"
finish_case

start_case "an answer that is not json fails closed" "[$passed_run]"
echo '<html>unicorn</html>' >"$case_dir/respond"
run_script
expect_rc 1
expect_output "was not the expected json"
finish_case

start_case "a head sha that is not a sha fails before any call" "[$passed_run]"
rc=0
env -i PATH="$bin:$PATH" HOME="$HOME" FAKE="$case_dir" REPO=rolter-ai/rolter \
  HEAD_SHA=refs/heads/master SELF_RUN_ID="$self" \
  bash --noprofile --norc "$script" >"$case_dir/out" 2>&1 || rc=$?
check "exit code" "$rc" 1
check "api calls" "$(cat "$case_dir/calls")" ""
finish_case

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "assert-gate-ran: every case passed"
