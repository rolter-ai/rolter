#!/usr/bin/env bash
# runs the two scripts behind release-plz.yml's `release-gate` job against a
# fake gh, curl, cargo and clock, and checks what they decided.
#
# scripts/wait-for-ci-gate.sh holds the crates.io publish until the ci.yml push
# run on the commit has a green `ci-ok`; scripts/unpublished-crates.sh decides
# whether there is anything to publish at all. both run only on a push to
# master, and only a release push exercises the wait, so a regression would
# show up as a release that publishes unverified or never publishes (#2025).
# this is the test. it runs as a step of quality.yml's `static checks` job and
# as a prek hook.
#
# bash 3.2 compatible, so it runs on a stock mac too.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
wait_script="$root/scripts/wait-for-ci-gate.sh"
list_script="$root/scripts/unpublished-crates.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

sha=0123456789abcdef0123456789abcdef01234567
other_sha=fedcba9876543210fedcba9876543210fedcba98

# ── fakes ─────────────────────────────────────────────────────────────────────
bin="$work/bin"
mkdir -p "$bin"

# clock: an epoch in $FAKE/clock that only `sleep` moves
cat >"$bin/date" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do last=$arg; done
if [ "${last:-}" = "+%s" ]; then cat "$FAKE/clock"; exit 0; fi
echo "fake date: unexpected call: $*" >&2
exit 2
EOF

cat >"$bin/sleep" <<'EOF'
#!/usr/bin/env bash
echo $(( $(cat "$FAKE/clock") + $1 )) >"$FAKE/clock"
echo "$1" >>"$FAKE/slept"
EOF

# gh: answers the two GETs the wait makes. $FAKE/runs is a list of runs, each
# with `appears` and `finish` in seconds since the case started, so a run shows
# up, runs and finishes as the fake clock moves. the fake applies none of the
# query's filters, so a run from another event, branch or sha reaches the
# script exactly as a misbehaving api would hand it over. $FAKE/faults holds
# one "<call> <fault>" per line; the first line naming a call is consumed by
# that call's next attempt
cat >"$bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ $# -ne 2 ] || [ "$1" != api ]; then
  echo "fake gh: unexpected call: $*" >&2; exit 2
fi
path=$2
echo "$path" >>"$FAKE/calls"
case $path in
  */actions/workflows/ci.yml/runs\?*) call=runs ;;
  */actions/runs/*/jobs\?*) call=jobs ;;
  *) echo "fake gh: unexpected path: $path" >&2; exit 2 ;;
esac
fault=""
if line=$(grep -n -m1 "^$call " "$FAKE/faults"); then
  fault=${line#*:}; fault=${fault#* }
  sed -i.bak "${line%%:*}d" "$FAKE/faults"
fi
case $fault in
  "") ;;
  bad-gateway) echo "HTTP 502: Bad Gateway (https://api.github.com/$path)" >&2; exit 1 ;;
  not-json) echo '<html>unicorn</html>'; exit 0 ;;
  *) echo "fake gh: unknown fault $fault" >&2; exit 2 ;;
esac
now=$(( $(cat "$FAKE/clock") - $(cat "$FAKE/start") ))
case $call in
  runs)
    jq -c --argjson now "$now" '{workflow_runs: [.[] | select(.appears <= $now) | {
      id, head_sha, event, head_branch, created_at,
      html_url: "https://github.com/rolter-ai/rolter/actions/runs/\(.id)",
      status: (if $now >= .finish then "completed" else "in_progress" end),
      conclusion: (if $now >= .finish then .conclusion else null end)}]}' "$FAKE/runs" ;;
  jobs)
    id=${path#*/actions/runs/}; id=${id%%/*}
    jq -c --arg id "$id" '(.[$id] // []) as $jobs
      | {total_count: ($jobs | length),
         jobs: [$jobs[] | {name, status: "completed", conclusion}]}' "$FAKE/jobs" ;;
esac
EOF

# cargo: `cargo metadata` answers from $FAKE/metadata, and fails like cargo
# does outside a workspace when that file is empty
cat >"$bin/cargo" <<'EOF'
#!/usr/bin/env bash
if [ "$*" != "metadata --no-deps --format-version 1" ]; then
  echo "fake cargo: unexpected call: $*" >&2; exit 2
fi
if [ ! -s "$FAKE/metadata" ]; then
  echo "error: could not find \`Cargo.toml\` in \`$PWD\` or any parent directory" >&2; exit 101
fi
cat "$FAKE/metadata"
EOF

# curl: answers the crates.io version lookup from $FAKE/published (crate ->
# versions). $FAKE/curl_faults maps a crate to an http code to answer with, to
# `timeout`, or to `other-version` for a 200 about a different version
cat >"$bin/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
out="" agent="" url=""
while [ $# -gt 0 ]; do
  case $1 in
    -o) out=$2; shift 2 ;;
    -A) agent=$2; shift 2 ;;
    -w | --max-time | --retry) shift 2 ;;
    -*) shift ;;
    *) url=$1; shift ;;
  esac
done
echo "$agent|$url" >>"$FAKE/curl_calls"
path=${url#https://crates.io/api/v1/crates/}
if [ "$path" = "$url" ] || [ -z "$out" ]; then
  echo "fake curl: unexpected call for $url" >&2; exit 2
fi
name=${path%%/*}; version=${path#*/}
fault=$(jq -r --arg n "$name" '.[$n] // ""' "$FAKE/curl_faults")
case $fault in
  "") ;;
  timeout) echo "curl: (28) Operation timed out after 20001 milliseconds" >&2; printf 000; exit 28 ;;
  other-version)
    jq -cn --arg n "$name" '{version: {crate: $n, num: "0.0.1"}}' >"$out"; printf 200; exit 0 ;;
  *) echo '{"errors":[{"detail":"upstream trouble"}]}' >"$out"; printf '%s' "$fault"; exit 0 ;;
esac
if jq -e --arg n "$name" --arg v "$version" '(.[$n] // []) | index($v) != null' "$FAKE/published" >/dev/null; then
  jq -cn --arg n "$name" --arg v "$version" '{version: {crate: $n, num: $v}}' >"$out"; printf 200
else
  echo "{\"errors\":[{\"detail\":\"crate \`$name\` does not have a version \`$version\`\"}]}" >"$out"; printf 404
fi
EOF
chmod +x "$bin/date" "$bin/sleep" "$bin/gh" "$bin/cargo" "$bin/curl"

# ── harness ───────────────────────────────────────────────────────────────────
failures=0
failures_before=0
case_name=""
case_dir=""

start_case() {
  case_name=$1
  case_dir="$work/$(echo "$1" | tr -c 'a-zA-Z0-9\n' '-')"
  mkdir -p "$case_dir"
  echo 2000000 >"$case_dir/clock"
  echo 2000000 >"$case_dir/start"
  echo '[]' >"$case_dir/runs"
  echo '{}' >"$case_dir/jobs"
  : >"$case_dir/faults"
  : >"$case_dir/slept"
  : >"$case_dir/calls"
  : >"$case_dir/curl_calls"
  echo '{}' >"$case_dir/curl_faults"
  echo '{}' >"$case_dir/published"
  : >"$case_dir/metadata"
}

# add_run ID EVENT BRANCH APPEARS FINISH CONCLUSION [SHA]: a ci.yml run that shows
# up APPEARS seconds into the case and completes FINISH seconds in
add_run() {
  jq -c --argjson id "$1" --arg event "$2" --arg branch "$3" \
    --argjson appears "$4" --argjson finish "$5" --arg conclusion "$6" --arg sha "${7:-$sha}" \
    --arg created "$(printf '2026-09-29T01:%02d:00Z' "$1")" \
    '. + [{id: $id, head_sha: $sha, event: $event, head_branch: $branch,
           created_at: $created, appears: $appears, finish: $finish, conclusion: $conclusion}]' \
    "$case_dir/runs" >"$case_dir/runs.new"
  mv "$case_dir/runs.new" "$case_dir/runs"
}

# set_jobs ID "name=conclusion ...": the jobs of run ID's latest attempt
set_jobs() {
  local id=$1 spec=$2 list='[]' pair
  for pair in $spec; do
    list=$(jq -c --arg n "${pair%%=*}" --arg c "${pair#*=}" '. + [{name: $n, conclusion: $c}]' <<<"$list")
  done
  jq -c --arg id "$id" --argjson list "$list" '.[$id] = $list' "$case_dir/jobs" >"$case_dir/jobs.new"
  mv "$case_dir/jobs.new" "$case_dir/jobs"
}

# metadata "name@version@publish ...": a cargo metadata document, where publish
# is `null` (publishable) or `[]` (publish = false)
metadata() {
  local list='[]' entry name version publish
  for entry in "$@"; do
    name=${entry%%@*}; version=${entry#*@}; publish=${version#*@}; version=${version%%@*}
    list=$(jq -c --arg n "$name" --arg v "$version" --argjson p "$publish" \
      '. + [{name: $n, version: $v, publish: $p}]' <<<"$list")
  done
  jq -c --argjson list "$list" -n '{packages: $list, workspace_members: [], version: 1}' >"$case_dir/metadata"
}

run_env() {
  env -i \
    PATH="$bin:$PATH" HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" FAKE="$case_dir" \
    GH_TOKEN=fake-token REPO=rolter-ai/rolter GITHUB_RUN_ID=77 \
    GITHUB_STEP_SUMMARY="$case_dir/summary" "$@"
}

# run_wait [sha]. the script writes `verified=true` to $GITHUB_OUTPUT as its
# last action and release-plz-release runs on nothing else, so every case also
# checks the output file holds exactly that when the script exits 0 and stays
# empty otherwise
run_wait() {
  local rc=0 want=""
  : >"$case_dir/github_output"
  (cd "$case_dir" && run_env SHA="${1:-$sha}" GITHUB_OUTPUT="$case_dir/github_output" \
    bash --noprofile --norc "$wait_script") >"$case_dir/out" 2>&1 || rc=$?
  echo "$rc" >"$case_dir/rc"
  [ "$rc" -eq 0 ] && want="verified=true"
  check "GITHUB_OUTPUT" "$(cat "$case_dir/github_output")" "$want"
}

run_list() {
  local rc=0
  (cd "$case_dir" && run_env bash --noprofile --norc "$list_script") \
    >"$case_dir/stdout" 2>"$case_dir/out" || rc=$?
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
expect_slept() { check "total wait" "$(awk '{ s += $1 } END { print s + 0 }' "$case_dir/slept")" "$1"; }
expect_stdout() { check "stdout" "$(tr '\n' ' ' <"$case_dir/stdout" | sed 's/ $//')" "$1"; }
expect_output() {
  if ! grep -qF -- "$1" "$case_dir/out"; then
    echo "FAIL [$case_name] output lacks: $1" >&2
    failures=$((failures + 1))
  fi
}
expect_no_output() {
  if grep -qF -- "$1" "$case_dir/out"; then
    echo "FAIL [$case_name] output unexpectedly has: $1" >&2
    failures=$((failures + 1))
  fi
}
finish_case() {
  if [ "$failures" -ne "$failures_before" ]; then
    echo "---- output of [$case_name] ----" >&2
    cat "$case_dir/out" >&2
  fi
  failures_before=$failures
}

# ── wait-for-ci-gate.sh ───────────────────────────────────────────────────────
start_case "a finished green push run passes at once"
add_run 1 push master 0 0 success
set_jobs 1 "gate-ok=success ci-ok=success"
run_wait
expect_rc 0
expect_slept 0
expect_output "ci-ok succeeded on ci.yml push run 1 for $sha"
check "summary" "$(grep -c 'ci-ok succeeded' "$case_dir/summary")" 1
finish_case

start_case "the query names ci.yml push runs on master for this sha"
add_run 1 push master 0 0 success
set_jobs 1 "ci-ok=success"
run_wait
expect_rc 0
check "runs query" "$(sed -n 1p "$case_dir/calls")" \
  "repos/rolter-ai/rolter/actions/workflows/ci.yml/runs?head_sha=$sha&event=push&branch=master&per_page=100"
check "jobs query" "$(sed -n 2p "$case_dir/calls")" "repos/rolter-ai/rolter/actions/runs/1/jobs?per_page=100"
finish_case

start_case "a push run still going is waited out"
add_run 1 push master 0 600 success
set_jobs 1 "ci-ok=success"
run_wait
expect_rc 0
expect_slept 600
expect_output "1 ci.yml push run(s) on $sha still running"
finish_case

start_case "a push run that has not started yet is waited for"
add_run 1 push master 90 400 success
set_jobs 1 "ci-ok=success"
run_wait
expect_rc 0
expect_slept 420
expect_output "no ci.yml push run on $sha yet"
finish_case

start_case "a red ci-ok fails at once"
add_run 1 push master 0 0 failure
set_jobs 1 "quality=failure ci-ok=failure"
run_wait
expect_rc 1
expect_slept 0
expect_output "ci-ok on ci.yml run 1 concluded 'failure', not success"
finish_case

start_case "a cancelled ci-ok fails"
add_run 1 push master 0 0 cancelled
set_jobs 1 "ci-ok=cancelled"
run_wait
expect_rc 1
expect_output "concluded 'cancelled'"
finish_case

start_case "a run that finished without a ci-ok job fails closed"
add_run 1 push master 0 0 cancelled
set_jobs 1 "quality=cancelled"
run_wait
expect_rc 1
expect_output "finished without a ci-ok job"
finish_case

start_case "a green run as a whole does not stand in for ci-ok"
add_run 1 push master 0 0 success
set_jobs 1 "gate-ok=success"
run_wait
expect_rc 1
expect_output "finished without a ci-ok job"
finish_case

start_case "a second ci-ok job that failed is not outvoted"
add_run 1 push master 0 0 failure
set_jobs 1 "ci-ok=success ci-ok=failure"
run_wait
expect_rc 1
expect_output "concluded 'failure'"
finish_case

# the case that makes a check-run name worthless as a gate: a pull_request run
# from master into another branch reports its ci-ok on master's head sha
start_case "green runs from another event, branch or sha never count"
add_run 1 pull_request master 0 0 success
add_run 2 workflow_dispatch master 0 0 success
add_run 3 push main 0 0 success
add_run 4 push master 0 0 success "$other_sha"
set_jobs 1 "ci-ok=success"
set_jobs 2 "ci-ok=success"
set_jobs 3 "ci-ok=success"
set_jobs 4 "ci-ok=success"
run_wait
expect_rc 1
expect_slept 5400
expect_output "no finished ci.yml push run on $sha after 90m (runs: 0, unfinished: 0)"
expect_output "gh run rerun 77 --failed"
expect_no_output "ci-ok succeeded"
finish_case

start_case "a push run that never finishes times out red"
add_run 1 push master 0 99999 success
set_jobs 1 "ci-ok=success"
run_wait
expect_rc 1
expect_slept 5400
expect_output "after 90m (runs: 1, unfinished: 1)"
check "summary" "$(grep -c 'timed out after 90m' "$case_dir/summary")" 1
finish_case

start_case "a run that finishes on the last poll still counts"
add_run 1 push master 0 5400 success
set_jobs 1 "ci-ok=success"
run_wait
expect_rc 0
expect_slept 5400
finish_case

start_case "the newest finished push run decides, green over an older red"
add_run 1 push master 0 0 failure
add_run 2 push master 0 0 success
set_jobs 1 "ci-ok=failure"
set_jobs 2 "ci-ok=success"
run_wait
expect_rc 0
expect_output "ci-ok succeeded on ci.yml push run 2"
finish_case

start_case "the newest finished push run decides, red over an older green"
add_run 1 push master 0 0 success
add_run 2 push master 0 0 failure
set_jobs 1 "ci-ok=success"
set_jobs 2 "ci-ok=failure"
run_wait
expect_rc 1
expect_output "ci-ok on ci.yml run 2 concluded 'failure'"
finish_case

start_case "a second push run still going holds the verdict"
add_run 1 push master 0 0 success
add_run 2 push master 0 300 failure
set_jobs 1 "ci-ok=success"
set_jobs 2 "ci-ok=failure"
run_wait
expect_rc 1
expect_slept 300
expect_output "ci-ok on ci.yml run 2 concluded 'failure'"
finish_case

start_case "a flaky runs listing is retried"
add_run 1 push master 0 0 success
set_jobs 1 "ci-ok=success"
printf 'runs bad-gateway\nruns bad-gateway\n' >"$case_dir/faults"
run_wait
expect_rc 0
expect_slept 10
finish_case

start_case "a runs listing that keeps failing fails closed"
add_run 1 push master 0 0 success
set_jobs 1 "ci-ok=success"
printf 'runs bad-gateway\nruns bad-gateway\nruns bad-gateway\n' >"$case_dir/faults"
run_wait
expect_rc 1
expect_output "failed 3 times; refusing to publish without a gate result"
finish_case

start_case "a jobs listing that keeps failing fails closed"
add_run 1 push master 0 0 success
set_jobs 1 "ci-ok=success"
printf 'jobs bad-gateway\njobs bad-gateway\njobs bad-gateway\n' >"$case_dir/faults"
run_wait
expect_rc 1
expect_output "failed 3 times"
expect_no_output "ci-ok succeeded"
finish_case

start_case "an answer that is not json fails closed"
add_run 1 push master 0 0 success
set_jobs 1 "ci-ok=success"
echo "runs not-json" >"$case_dir/faults"
run_wait
expect_rc 1
expect_no_output "ci-ok succeeded"
finish_case

start_case "a green run outside actions exits 0 with no output file to write"
add_run 1 push master 0 0 success
set_jobs 1 "ci-ok=success"
rc=0
(cd "$case_dir" && run_env SHA="$sha" bash --noprofile --norc "$wait_script") \
  >"$case_dir/out" 2>&1 || rc=$?
echo "$rc" >"$case_dir/rc"
expect_rc 0
expect_output "ci-ok succeeded on ci.yml push run 1 for $sha"
finish_case

start_case "a sha that is not a full commit sha is refused before any call"
run_wait "master"
expect_rc 1
expect_output "is not a full commit sha"
check "calls" "$(cat "$case_dir/calls")" ""
finish_case

# ── unpublished-crates.sh ─────────────────────────────────────────────────────
start_case "nothing to publish prints nothing"
metadata "rolter-core@0.2.0@null" "rolter@0.2.0@null" "rolter-ui@0.2.0@[]"
echo '{"rolter-core":["0.1.0","0.2.0"],"rolter":["0.2.0"]}' >"$case_dir/published"
run_list
expect_rc 0
expect_stdout ""
expect_output "rolter-core 0.2.0: on crates.io"
finish_case

start_case "a version missing from crates.io is listed"
metadata "rolter-core@0.3.0@null" "rolter@0.3.0@null"
echo '{"rolter-core":["0.3.0"],"rolter":["0.2.0"]}' >"$case_dir/published"
run_list
expect_rc 0
expect_stdout "rolter@0.3.0"
expect_output "rolter 0.3.0: not on crates.io"
finish_case

start_case "a crate marked publish = false is never looked up"
metadata "rolter-core@0.2.0@null" "rolter-ui@0.2.0@[]"
echo '{"rolter-core":["0.2.0"]}' >"$case_dir/published"
run_list
expect_rc 0
expect_stdout ""
check "lookups" "$(cut -d'|' -f2 "$case_dir/curl_calls" | tr '\n' ' ' | sed 's/ $//')" \
  "https://crates.io/api/v1/crates/rolter-core/0.2.0"
finish_case

start_case "a crate with an explicit registry list is still checked"
metadata 'rolter-core@0.2.0@["crates-io"]'
run_list
expect_rc 0
expect_stdout "rolter-core@0.2.0"
finish_case

start_case "a failed lookup counts as unpublished"
metadata "rolter-core@0.2.0@null" "rolter-proxy@0.2.0@null" "rolter@0.2.0@null"
echo '{"rolter-core":["0.2.0"],"rolter-proxy":["0.2.0"],"rolter":["0.2.0"]}' >"$case_dir/published"
echo '{"rolter-core":"503","rolter-proxy":"timeout"}' >"$case_dir/curl_faults"
run_list
expect_rc 0
expect_stdout "rolter-core@0.2.0 rolter-proxy@0.2.0"
expect_output "rolter-core 0.2.0: lookup failed (http 503); counting it as unpublished"
expect_output "rolter-proxy 0.2.0: lookup failed (http 000)"
finish_case

start_case "a 200 about another version does not count"
metadata "rolter@0.2.0@null"
echo '{"rolter":"other-version"}' >"$case_dir/curl_faults"
run_list
expect_rc 0
expect_stdout "rolter@0.2.0"
finish_case

start_case "crates.io is told who is asking"
metadata "rolter@0.2.0@null"
echo '{"rolter":["0.2.0"]}' >"$case_dir/published"
run_list
expect_rc 0
check "user agent" "$(cut -d'|' -f1 "$case_dir/curl_calls")" \
  "rolter-release-gate (github.com/rolter-ai/rolter)"
finish_case

start_case "cargo metadata failing is an error, never an empty list"
run_list
check "exit code is non-zero" "$([ "$(cat "$case_dir/rc")" -ne 0 ] && echo yes || echo no)" yes
expect_stdout ""
finish_case

start_case "a workspace with no publishable crate is an error"
metadata "rolter-ui@0.2.0@[]"
run_list
expect_rc 1
expect_output "listed no publishable crate"
finish_case

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "release-gate: every case passed"
