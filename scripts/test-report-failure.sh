#!/usr/bin/env bash
# runs scripts/report-failure.sh, the logic behind the `report failure` job of
# extended.yml, ui-e2e.yml and sso-e2e.yml, against a fake gh, and checks the
# workflows that call it (#2682).
#
# the three nightly suites gate nothing, so this job is the only thing that
# makes a red night visible, and it only ever runs when something is already
# broken. a regression in it is found weeks later, by an issue that never
# opened. this is the test. it runs as a step of quality.yml's `static checks`
# job and as a prek hook.
#
# bash 3.2 compatible, so it runs on a stock mac too.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
script="$root/scripts/report-failure.sh"
workflows="$root/.github/workflows"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
fail() {
  echo "FAIL [$1] $2" >&2
  failures=$((failures + 1))
}

# ── workflow wiring ───────────────────────────────────────────────────────────
# every suite calls the shared workflow with an issue title of its own, and none
# keeps a copy of the logic
reusable="$workflows/report-failure.yml"
[ -f "$reusable" ] || fail "wiring" "$reusable does not exist"
grep -qE '^  workflow_call:' "$reusable" || fail "wiring" "report-failure.yml is not a workflow_call workflow"
grep -qF 'run: bash scripts/report-failure.sh' "$reusable" || fail "wiring" "report-failure.yml does not run the script under test"
# the write scopes belong to the job, not the workflow (zizmor rates them high there)
workflow_level=$(awk '/^permissions:/ { on = 1; next } on && /^[a-z]/ { exit } on { print }' "$reusable")
case $workflow_level in
  *write*) fail "wiring" "report-failure.yml grants a write scope at workflow level: $workflow_level" ;;
esac

titles=""
for caller in extended ui-e2e sso-e2e; do
  file="$workflows/$caller.yml"
  grep -qF 'uses: ./.github/workflows/report-failure.yml' "$file" ||
    fail "wiring" "$caller.yml does not call report-failure.yml"
  if grep -qE 'gh issue (create|comment)' "$file"; then
    fail "wiring" "$caller.yml keeps its own copy of the issue logic"
  fi
  # the job's own permissions: the called job needs both, the caller has to grant them
  job=$(awk '/^  report-failure:/ { on = 1; next } on && /^  [a-z]/ { exit } on { print }' "$file")
  for scope in "contents: read" "issues: write" "actions: write"; do
    case $job in
      *"$scope"*) ;;
      *) fail "wiring" "$caller.yml's report-failure job must grant $scope" ;;
    esac
  done
  title=$(printf '%s\n' "$job" | sed -nE 's/^ +title: "(.*)"$/\1/p')
  [ -n "$title" ] || fail "wiring" "$caller.yml passes no issue title"
  case "
$titles
" in
    *"
$title
"*) fail "wiring" "$caller.yml reuses the issue title '$title', which another suite already has" ;;
  esac
  titles="$titles
$title"
  # a master-only report
  case $job in
    *"github.ref == 'refs/heads/master'"*) ;;
    *) fail "wiring" "$caller.yml's report-failure job must report on master only" ;;
  esac
done
# extended.yml's table lists every job the report waits on, ui-audit included (#1930)
extended_job=$(awk '/^  report-failure:/ { on = 1; next } on && /^  [a-z]/ { exit } on { print }' "$workflows/extended.yml")
for needed in cross-platform compose-smoke msrv coverage bun-pin ui-audit; do
  case $extended_job in
    *"needs.$needed.result"*) ;;
    *) fail "wiring" "extended.yml's report lacks the $needed job" ;;
  esac
done
# quality.yml stays secret-free (#734)
if grep -vE '^[[:space:]]*#' "$workflows/quality.yml" | grep -qE 'secrets\.|uses: .*report-failure'; then
  fail "wiring" "quality.yml references secrets or calls the report-failure workflow"
fi

# ── fakes ─────────────────────────────────────────────────────────────────────
bin="$work/bin"
mkdir -p "$bin"

# gh: records every call as a directory $FAKE/calls/<n>-<verb> holding one file
# per argument, since an argument (the issue body) is several lines long.
# `issue list` answers $FAKE/issues; `issue create` prints a url. an existing
# $FAKE/fail-<verb> makes that verb exit 1 (e.g. fail-issue-edit)
cat >"$bin/gh" <<'EOF2'
#!/usr/bin/env bash
set -euo pipefail
verb="$1-$2"
shift 2
n=$(( $(ls "$FAKE/calls" | wc -l) + 1 ))
dir="$FAKE/calls/$(printf '%02d' "$n")-$verb"
mkdir -p "$dir"
k=0
for a in "$@"; do
  k=$((k + 1))
  printf '%s' "$a" >"$dir/$(printf '%02d' "$k")"
done
if [ -e "$FAKE/fail-$verb" ]; then
  echo "fake gh: $verb failed" >&2
  exit 1
fi
case $verb in
  issue-list) cat "$FAKE/issues" ;;
  issue-create) echo "https://github.com/rolter-ai/rolter/issues/4242" ;;
  issue-comment | issue-edit | workflow-run) ;;
  *) echo "fake gh: unexpected call: $verb" >&2; exit 2 ;;
esac
EOF2
chmod +x "$bin/gh"

case_name=""
case_dir=""
start_case() {
  case_name=$1
  case_dir="$work/$(echo "$1" | tr -c 'a-zA-Z0-9\n' '-')"
  mkdir -p "$case_dir"
  mkdir -p "$case_dir/calls"
  echo '[]' >"$case_dir/issues"
}

run_script() {
  # run_script [VAR=value ...]: the environment a caller passes, minus what is overridden
  local rc=0
  env -i PATH="$bin:$PATH" HOME="$HOME" TMPDIR="${TMPDIR:-/tmp}" FAKE="$case_dir" \
    GITHUB_STEP_SUMMARY="$case_dir/summary" \
    GH_TOKEN=fake GH_REPO=rolter-ai/rolter RUN_URL=https://github.com/rolter-ai/rolter/actions/runs/1 \
    REF=master SHA=0123456789abcdef0123456789abcdef01234567 \
    TITLE="demo.yml: nightly failing" AREA=ci EFFORT=XS \
    RESULTS=$'macos check|failure\nmsrv build|success' \
    "$@" \
    bash --noprofile --norc "$script" >"$case_dir/out" 2>&1 || rc=$?
  echo "$rc" >"$case_dir/rc"
}

check() {
  # check DESCRIPTION ACTUAL EXPECTED
  if [ "$2" != "$3" ]; then
    fail "$case_name" "$1: expected '$3', got '$2'"
  fi
}
expect_rc() { check "exit code" "$(cat "$case_dir/rc")" "$1"; }
# verbs: the calls made, in order, by verb alone
expect_verbs() {
  local got=""
  for d in "$case_dir"/calls/*-*; do
    [ -d "$d" ] || continue
    got="$got$(basename "$d" | sed 's/^[0-9]*-//') "
  done
  check "gh calls" "$got" "$1"
}
call_dir() { ls -d "$case_dir"/calls/*-"$1" 2>/dev/null | head -n1; }
expect_arg() {
  # expect_arg VERB NEEDLE: an argument of the call contains NEEDLE
  local dir
  dir=$(call_dir "$1")
  if [ -z "$dir" ] || ! cat "$dir"/* 2>/dev/null | grep -qF -- "$2"; then
    fail "$case_name" "no $1 argument contains: $2"
  fi
}
expect_output() {
  grep -qF -- "$1" "$case_dir/out" || fail "$case_name" "output lacks: $1 (got: $(cat "$case_dir/out"))"
}

# ── a first failure opens the issue ───────────────────────────────────────────
start_case "a first failure opens an issue and triages it"
run_script
expect_rc 0
expect_verbs "issue-list issue-create issue-edit workflow-run "
expect_arg issue-create "demo.yml: nightly failing"
expect_arg issue-create "| macos check | failure |"
expect_arg issue-create "| msrv build | success |"
expect_arg issue-create "[Run](https://github.com/rolter-ai/rolter/actions/runs/1) on \`master\` at \`0123456789abcdef0123456789abcdef01234567\`"
expect_arg issue-create "Opened by the \`report failure\` job"
check "label" "$(cat "$(call_dir issue-create)/04")" ci
expect_arg issue-edit "4242"
expect_arg issue-edit "Maintenance, CI & DX"
expect_arg workflow-run "project-automation.yml"
expect_arg workflow-run "number=4242"
expect_arg workflow-run "area=ci"
expect_arg workflow-run "effort=XS"
check "summary" "$(cat "$case_dir/summary")" "opened https://github.com/rolter-ai/rolter/issues/4242"

# ── a failure while the issue is open comments on it ──────────────────────────
start_case "a failure while the issue is open comments on it"
echo '[{"number": 7, "title": "another.yml: failing"}, {"number": 9, "title": "demo.yml: nightly failing"}]' >"$case_dir/issues"
run_script
expect_rc 0
expect_verbs "issue-list issue-comment "
expect_arg issue-comment "9"
expect_arg issue-comment "| macos check | failure |"
check "summary" "$(cat "$case_dir/summary")" "commented on #9"

start_case "an issue of another suite is not this suite's issue"
echo '[{"number": 7, "title": "another.yml: failing"}]' >"$case_dir/issues"
run_script
expect_rc 0
expect_verbs "issue-list issue-create issue-edit workflow-run "

# ── the note, the title with metacharacters, a custom milestone ───────────────
start_case "a note is added under the table and a title is passed verbatim"
run_script 'TITLE=a "quoted" $(whoami) `title`' 'NOTE=See the `playwright-report` artifact.' 'MILESTONE=Other milestone'
expect_rc 0
expect_arg issue-create 'a "quoted" $(whoami) `title`'
expect_arg issue-create 'See the `playwright-report` artifact.'
expect_arg issue-edit "Other milestone"

# ── best-effort triage never costs the issue ──────────────────────────────────
start_case "a milestone that cannot be set only warns"
: >"$case_dir/fail-issue-edit"
run_script
expect_rc 0
expect_verbs "issue-list issue-create issue-edit workflow-run "
expect_output "::warning title=report failure::could not set the milestone of #4242"

start_case "a dispatch that cannot start only warns"
: >"$case_dir/fail-workflow-run"
run_script
expect_rc 0
expect_output "::warning title=report failure::could not dispatch project-automation for #4242"
expect_output "gh workflow run project-automation.yml -f number=4242 -f area=ci -f effort=XS"

# ── a listing that fails must not open a duplicate ────────────────────────────
start_case "a failing issue listing is a failed run, not an empty list"
: >"$case_dir/fail-issue-list"
run_script
check "exit code is nonzero" "$([ "$(cat "$case_dir/rc")" != 0 ] && echo yes)" yes
expect_verbs "issue-list "

# ── required input ────────────────────────────────────────────────────────────
start_case "a missing title fails before any call"
run_script TITLE=
check "exit code is nonzero" "$([ "$(cat "$case_dir/rc")" != 0 ] && echo yes)" yes
expect_verbs ""

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "report-failure: every case passed"
