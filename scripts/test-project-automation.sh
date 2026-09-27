#!/usr/bin/env bash
# runs the `add to project board` step of .github/workflows/project-automation.yml
# against a fake `gh`, a fake clock and a fake board, and checks what it did.
#
# the step decides which GitHub API failures to wait out and which to report,
# and the order of its patterns is part of that decision: a secondary rate limit
# also says "rate limit", and a throttled call also carries an http 403. none of
# it runs anywhere but on an opened issue or PR on master, where a regression
# shows up only as items missing from the board (#1718), so this is the test.
# it also pins the #1469 contract: triage that lands while a seed write is
# backing off must survive the retry.
#
# the script under test is extracted from the workflow rather than kept in a
# file of its own, because the workflow runs on pull_request_target and does
# not check the repository out.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
workflow="$root/.github/workflows/project-automation.yml"
step_name="add item to project and seed default Status and Priority"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# the step's run: block, de-indented. keyed on the step name so a second step
# with its own run: block cannot be picked up by mistake
awk -v step="- name: $step_name" '
  index($0, step) { in_step = 1; next }
  in_step && /^        run: \|$/ { grab = 1; next }
  grab && /^$/ { print ""; next }
  grab && /^          / { print substr($0, 11); next }
  grab { exit }
' "$workflow" >"$work/step.sh"
for marker in 'gh_graphql_once()' 'set_field()' 'read_values()'; do
  if ! grep -qF "$marker" "$work/step.sh"; then
    echo "error: could not extract the step's run: block from $workflow ($marker missing)" >&2
    exit 1
  fi
done

# ── fakes ─────────────────────────────────────────────────────────────────────
bin="$work/bin"
mkdir -p "$bin"

# clock: an epoch in $FAKE/clock that only `sleep` moves
cat >"$bin/date" <<'EOF'
#!/usr/bin/env bash
if [ "$*" = "+%s" ]; then cat "$FAKE/clock"; exit 0; fi
# `date -u -d @N FORMAT`: the tests never read the formatted time
for arg in "$@"; do case $arg in @*) echo "t${arg#@}" ;; esac; done
EOF

cat >"$bin/sleep" <<'EOF'
#!/usr/bin/env bash
echo $(( $(cat "$FAKE/clock") + $1 )) >"$FAKE/clock"
echo "$1" >>"$FAKE/slept"
# a board edit queued to land during the first backoff, the way a human or an
# agent triaging the item would
if [ -s "$FAKE/during_sleep" ]; then
  jq -c --argjson patch "$(cat "$FAKE/during_sleep")" '. + $patch' "$FAKE/board" >"$FAKE/board.new"
  mv "$FAKE/board.new" "$FAKE/board"
  : >"$FAKE/during_sleep"
fi
EOF

# gh: answers the four graphql calls the step makes plus `gh api rate_limit`.
# $FAKE/faults holds one "<call> <fault>" per line; the first line naming a call
# is consumed by that call's next attempt
cat >"$bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "$1 $2" = "api rate_limit" ]; then cat "$FAKE/rate_limit"; exit 0; fi
[ "$1 $2" = "api graphql" ] || { echo "fake gh: unexpected call: $*" >&2; exit 2; }
shift 2
query="" field="" opt=""
while [ $# -gt 0 ]; do
  case $1 in
    -f)
      case $2 in
        query=*) query=${2#query=} ;;
        field=*) field=${2#field=} ;;
        opt=*) opt=${2#opt=} ;;
      esac
      shift 2 ;;
    *) shift ;;
  esac
done
case $query in
  *addProjectV2ItemById*) call=add ;;
  *updateProjectV2ItemFieldValue*) call=update ;;
  *fieldValues*) call=read ;;
  *fields*) call=fields ;;
  *) echo "fake gh: unknown query" >&2; exit 2 ;;
esac
echo "$call" >>"$FAKE/calls"

write() {
  local name=${field#F:} value=${opt#O:*:}
  jq -c --arg f "$name" --arg v "$value" '.[$f] = $v' "$FAKE/board" >"$FAKE/board.new"
  mv "$FAKE/board.new" "$FAKE/board"
  echo "$name=$value" >>"$FAKE/writes"
}

fault=""
if line=$(grep -n -m1 "^$call " "$FAKE/faults"); then
  fault=${line#*:}; fault=${fault#* }
  sed -i.bak "${line%%:*}d" "$FAKE/faults"
fi
case $fault in
  "" | ok) ;;
  secondary)
    echo 'HTTP 403: You have exceeded a secondary rate limit. Please wait a few minutes before you try again. (https://api.github.com/graphql)' >&2
    exit 1 ;;
  primary)
    echo '{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded for user ID 1."}]}'
    echo 'gh: API rate limit exceeded for user ID 1.' >&2
    exit 1 ;;
  bad-gateway)
    echo 'HTTP 502: Bad Gateway (https://api.github.com/graphql)' >&2
    exit 1 ;;
  eof)
    echo 'Post "https://api.github.com/graphql": EOF' >&2
    exit 1 ;;
  refused)
    echo 'Post "https://api.github.com/graphql": dial tcp 140.82.112.6:443: connect: connection refused' >&2
    exit 1 ;;
  bad-credentials)
    echo 'HTTP 401: Bad credentials (https://api.github.com/graphql)' >&2
    exit 1 ;;
  unknown)
    echo 'gh: something this job has never seen' >&2
    exit 1 ;;
  null-node)
    echo '{"data":{"node":null}}'
    exit 0 ;;
  lost-response)
    # the write lands, then the connection drops before the answer arrives
    [ "$call" = update ] && write
    echo 'HTTP 502: Bad Gateway (https://api.github.com/graphql)' >&2
    exit 1 ;;
  *) echo "fake gh: unknown fault $fault" >&2; exit 2 ;;
esac

case $call in
  add) echo '{"data":{"addProjectV2ItemById":{"item":{"id":"ITEM_1"}}}}' ;;
  fields)
    jq -cn '{data:{node:{fields:{nodes:[
      {},
      {id:"F:Status",name:"Status",options:([
        "Backlog","Todo","In Progress","In Review","Done"] | map({id:("O:Status:"+.),name:.}))},
      {id:"F:Priority",name:"Priority",options:([
        "Urgent","High","Medium","Low"] | map({id:("O:Priority:"+.),name:.}))}
    ]}}}}' ;;
  read)
    jq -c '{data:{node:{fieldValues:{nodes:([{}] + [to_entries[]
      | select(.value != null) | {name:.value, field:{name:.key}}])}}}}' "$FAKE/board" ;;
  update)
    write
    echo '{"data":{"updateProjectV2ItemFieldValue":{"projectV2Item":{"id":"ITEM_1"}}}}' ;;
esac
EOF
chmod +x "$bin/date" "$bin/sleep" "$bin/gh"

# ── harness ───────────────────────────────────────────────────────────────────
failures=0
case_name=""
case_dir=""

# start_case NAME: a fresh fake board, clock and fault list in $case_dir, for
# the caller to adjust before run_step
start_case() {
  case_name=$1
  case_dir="$work/$(echo "$1" | tr -c 'a-zA-Z0-9\n' '-')"
  mkdir -p "$case_dir"
  echo 1000000 >"$case_dir/clock"
  echo '{}' >"$case_dir/board"
  : >"$case_dir/faults"
  : >"$case_dir/during_sleep"
  : >"$case_dir/slept"
  : >"$case_dir/writes"
  echo "4999 1003600 5000" >"$case_dir/rate_limit"
}

# run_step [is_pr] [token]: runs the step against $case_dir, capturing its
# exit code and output
run_step() {
  local is_pr=${1:-false} token=${2-fake-token} rc=0
  (
    cd "$case_dir"
    env -i \
      PATH="$bin:$PATH" HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" FAKE="$case_dir" \
      GH_TOKEN="$token" PROJECT_ID=PVT_test CONTENT_ID=I_test \
      CONTENT_URL=https://github.com/rolter-ai/rolter/issues/1 \
      IS_PR="$is_pr" RETRY_WINDOW=900 GITHUB_RUN_ID=42 \
      bash --noprofile --norc -eo pipefail "$work/step.sh"
  ) >"$case_dir/out" 2>&1 || rc=$?
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
expect_board() { check "board" "$(jq -cS . "$case_dir/board")" "$(jq -cS . <<<"$1")"; }
expect_slept() { check "total wait" "$(awk '{ s += $1 } END { print s + 0 }' "$case_dir/slept")" "$1"; }
expect_writes() { check "writes" "$(tr '\n' ' ' <"$case_dir/writes" | sed 's/ $//')" "$1"; }
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
  if [ "$failures" -ne "${failures_before:-0}" ]; then
    echo "---- output of [$case_name] ----" >&2
    cat "$case_dir/out" >&2
  fi
  failures_before=$failures
}

# ── cases ─────────────────────────────────────────────────────────────────────
start_case "issue, happy path"
run_step
expect_rc 0
expect_board '{"Status":"Todo","Priority":"Medium"}'
expect_slept 0
finish_case

start_case "pr, happy path"
run_step true
expect_rc 0
expect_board '{"Status":"In Review"}'
finish_case

start_case "triage already applied is kept"
echo '{"Status":"Backlog","Priority":"High"}' >"$case_dir/board"
run_step
expect_rc 0
expect_board '{"Status":"Backlog","Priority":"High"}'
expect_writes ""
expect_output "Status is already 'Backlog'; leaving it alone"
finish_case

# #1469: the seed write is throttled, the item is triaged during the backoff,
# and the retry must see that triage rather than write over it
start_case "triage landing during a write backoff wins"
echo "update secondary" >"$case_dir/faults"
echo '{"Status":"Backlog","Priority":"High"}' >"$case_dir/during_sleep"
run_step
expect_rc 0
expect_board '{"Status":"Backlog","Priority":"High"}'
expect_writes ""
expect_slept 60
finish_case

# the same while the second write waits for the hourly budget to refill: Status
# is seeded before the wait, Priority is triaged during it and must stay
start_case "triage landing during a budget wait wins"
printf 'update ok\nupdate primary\n' >"$case_dir/faults"
echo "0 1000120 5000" >"$case_dir/rate_limit"
echo '{"Priority":"High"}' >"$case_dir/during_sleep"
run_step
expect_rc 0
expect_board '{"Status":"Todo","Priority":"High"}'
expect_writes "Status=Todo"
expect_slept 125
finish_case

start_case "a write that landed before its answer was lost is not repeated"
echo "update lost-response" >"$case_dir/faults"
run_step
expect_rc 0
expect_board '{"Status":"Todo","Priority":"Medium"}'
expect_writes "Status=Todo Priority=Medium"
expect_slept 5
finish_case

start_case "secondary limit twice, then through"
printf 'add secondary\nadd secondary\n' >"$case_dir/faults"
run_step
expect_rc 0
expect_board '{"Status":"Todo","Priority":"Medium"}'
expect_slept 180
finish_case

start_case "secondary limit that outlasts the window"
for _ in 1 2 3 4 5 6; do echo "add secondary"; done >"$case_dir/faults"
run_step
expect_rc 1
expect_output "title=GitHub API secondary rate limit"
expect_output "nothing is misconfigured"
expect_no_output "project board token misconfigured"
expect_board '{}'
finish_case

start_case "budget refilling inside the window is waited out"
echo "fields primary" >"$case_dir/faults"
echo "0 1000120 5000" >"$case_dir/rate_limit"
run_step
expect_rc 0
expect_slept 125
expect_board '{"Status":"Todo","Priority":"Medium"}'
finish_case

start_case "budget refilling after the window fails at once"
echo "add primary" >"$case_dir/faults"
echo "0 1003000 5000" >"$case_dir/rate_limit"
run_step
expect_rc 1
expect_slept 0
expect_output "title=GitHub API budget exhausted"
expect_output "gh run rerun 42 --failed"
finish_case

start_case "rate limit with points already back backs off briefly"
printf 'add primary\nadd primary\n' >"$case_dir/faults"
run_step
expect_rc 0
expect_slept 15
finish_case

start_case "server errors are retried"
printf 'add bad-gateway\nread bad-gateway\n' >"$case_dir/faults"
run_step
expect_rc 0
expect_slept 10
expect_board '{"Status":"Todo","Priority":"Medium"}'
finish_case

start_case "a dropped connection is retried"
printf 'add eof\nfields refused\n' >"$case_dir/faults"
run_step
expect_rc 0
expect_slept 10
finish_case

start_case "an outage that outlasts six attempts"
for _ in 1 2 3 4 5 6; do echo "add bad-gateway"; done >"$case_dir/faults"
run_step
expect_rc 1
expect_slept 155
expect_output "title=GitHub API unavailable"
finish_case

start_case "bad credentials fail at once"
echo "add bad-credentials" >"$case_dir/faults"
run_step
expect_rc 1
expect_slept 0
expect_output "title=project board token misconfigured"
expect_output "this is not a rate limit"
finish_case

start_case "an unrecognised error fails at once with the way back"
echo "add unknown" >"$case_dir/faults"
run_step
expect_rc 1
expect_slept 0
expect_output "title=GitHub API request failed"
expect_output "gh run rerun 42 --failed"
finish_case

start_case "an unreadable item is never seeded"
echo "read null-node" >"$case_dir/faults"
run_step
expect_rc 1
expect_writes ""
expect_output "refusing to seed defaults"
finish_case

start_case "an empty token fails before any call"
run_step false ""
expect_rc 1
expect_output "title=project board token missing"
check "calls" "$(cat "$case_dir/calls" 2>/dev/null || true)" ""
finish_case

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "project-automation: every case passed"
