#!/usr/bin/env bash
# runs scripts/fleet/{land,claim,board}.sh against a fake gh (and a fake wt for
# the worktree step, over a throwaway git repo), and checks their verdicts,
# exit codes and the commands they would have run. it also drives the
# SessionStart hook .claude/hooks/start-storybook.sh against a stub storybook on
# a free port.
#
# nothing here talks to GitHub or to the real repository, and the hook test
# never touches port 6006. it runs as a prek hook (`fleet-scripts`).
#
# bash 3.2 compatible, so it runs on a stock mac too.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
hook="$root/.claude/hooks/start-storybook.sh"
land="$root/scripts/fleet/land.sh"
claim="$root/scripts/fleet/claim.sh"
board="$root/scripts/fleet/board.sh"

# a git or prek hook runs this with GIT_DIR, GIT_INDEX_FILE and the like pointing
# at the real repository, and `git init`, `git remote add` and `git commit`
# below would then act on it instead of on the throwaway repository (an unset
# GIT_DIR made `git init --bare` flip core.bare in the real repository once)
for var in $(git rev-parse --local-env-vars); do
  unset "$var"
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export FAKE_DIR="$work/fake"
mkdir -p "$FAKE_DIR" "$work/bin"
export PATH="$work/bin:$PATH"
export BOARD_RETRY_SLEEP=0
export LAND_VERIFY_SLEEP=0
# the throwaway repo below needs an identity; these never reach the real one
export GIT_AUTHOR_NAME="fleet test" GIT_AUTHOR_EMAIL="fleet-test@example.invalid"
export GIT_COMMITTER_NAME="fleet test" GIT_COMMITTER_EMAIL="fleet-test@example.invalid"

failures=0
pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1" >&2
  failures=$((failures + 1))
}

# ---- fake gh -----------------------------------------------------------------
cat >"$work/bin/gh" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
d=$FAKE_DIR
echo "gh $*" >>"$d/calls.log"
jqexpr=""
prev=""
for a in "$@"; do
  [ "$prev" = --jq ] && jqexpr=$a
  prev=$a
done
emit() { if [ -n "$jqexpr" ]; then jq -r "$jqexpr" "$1"; else cat "$1"; fi; }
case "$1 $2" in
  "pr view") emit "$d/pr.json" ;;
  "pr list") emit "$d/children.json" ;;
  "pr checks")
    cat "$d/checks.json"
    exit "$(cat "$d/checks.rc")"
    ;;
  "pr merge")
    jq '.state = "MERGED" | .mergeCommit = {oid: "feedface0000000000000000000000000000aaaa"}' "$d/pr.json" >"$d/pr.tmp"
    mv "$d/pr.tmp" "$d/pr.json"
    ;;
  "run list") emit "$d/runs.json" ;;
  "repo view") echo master ;;
  "issue view") emit "$d/issue-$3.json" ;;
  "issue close")
    jq '.state = "CLOSED"' "$d/issue-$3.json" >"$d/issue.tmp"
    mv "$d/issue.tmp" "$d/issue-$3.json"
    ;;
  "issue edit")
    label=""
    prev=""
    for a in "$@"; do
      [ "$prev" = --add-label ] && label=$a
      prev=$a
    done
    jq --arg l "$label" '.labels += [{name: $l}]' "$d/issue-$3.json" >"$d/issue.tmp"
    mv "$d/issue.tmp" "$d/issue-$3.json"
    ;;
  "project field-list") cat "$d/fields.json" ;;
  "project view") emit "$d/project.json" ;;
  "project item-add") echo '{"id":"ITEM1"}' | jq -r "${jqexpr:-.}" ;;
  "project item-edit")
    field="" opt="" prev=""
    for a in "$@"; do
      [ "$prev" = --field-id ] && field=$a
      [ "$prev" = --single-select-option-id ] && opt=$a
      prev=$a
    done
    jq -r --arg f "$field" --arg o "$opt" \
      '.fields[] | select(.id == $f) | "\(.name)=\([.options[] | select(.id == $o) | .name] | first)"' \
      "$d/fields.json" >>"$d/values.new"
    ;;
  "api graphql")
    if printf '%s' "$*" | grep -q reviewThreads; then
      cat "$d/threads.json"
    else
      # board.sh read-back: the last value written per field, or Status=Todo
      # once when the "race" flag is set (the board automation overwriting)
      if [ -f "$d/race" ]; then
        rm -f "$d/race"
        sed -i.bak 's/^Status=.*/Status=Todo/' "$d/values.new" && rm -f "$d/values.new.bak"
      fi
      jq -Rn '{data: {node: {fieldValues: {nodes: [inputs | split("=") | {name: .[1], field: {name: .[0]}}]}}}}' <"$d/values.new"
    fi
    ;;
  "api -X")
    # DELETE repos/<owner>/<repo>/git/refs/heads/<branch>
    ref=${4#repos/*/*/git/}
    git -C "$d/origin.git" update-ref -d "$ref"
    ;;
  "api repos"*) emit "$d/compare.json" ;;
  *)
    echo "fake gh: unhandled: $*" >&2
    exit 99
    ;;
esac
FAKE
chmod +x "$work/bin/gh"

# ---- fake wt: removes the worktree, keeps the branch (as wt does for a squash) --
cat >"$work/bin/wt" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail
echo "wt $*" >>"$FAKE_DIR/calls.log"
main=$2
branch=${@: -1}
path=$(git -C "$main" worktree list --porcelain | awk -v ref="refs/heads/$branch" '/^worktree /{p=substr($0,10)} $0=="branch " ref{print p; exit}')
git -C "$main" worktree remove "$path"
FAKE
chmod +x "$work/bin/wt"

# ---- fixtures ----------------------------------------------------------------
sha=0123456789abcdef0123456789abcdef01234567

reset() {
  : >"$FAKE_DIR/calls.log"
  : >"$FAKE_DIR/values.new"
  rm -f "$FAKE_DIR/race"
  cat >"$FAKE_DIR/pr.json" <<JSON
{"number":7,"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"CLEAN",
 "baseRefName":"master","headRefName":"feat/7-thing","headRefOid":"$sha","title":"feat(ui): a thing [#11]",
 "body":"does it\n\nCloses #11 and refs #2049","url":"https://github.com/rolter-ai/rolter/pull/7",
 "closingIssuesReferences":[{"number":11}],"mergedAt":null,"mergeCommit":null,"autoMergeRequest":null}
JSON
  echo '[]' >"$FAKE_DIR/children.json"
  echo '[{"name":"ci-ok","bucket":"pass"}]' >"$FAKE_DIR/checks.json"
  echo 0 >"$FAKE_DIR/checks.rc"
  echo '[]' >"$FAKE_DIR/runs.json"
  echo '{"behind_by":0}' >"$FAKE_DIR/compare.json"
  echo '{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[]}}}}}' >"$FAKE_DIR/threads.json"
  echo '{"number":11,"state":"OPEN","url":"https://github.com/rolter-ai/rolter/issues/11","title":"t","projectItems":[{"title":"rolter","status":{"name":"Todo"}}]}' >"$FAKE_DIR/issue-11.json"
  cat >"$FAKE_DIR/fields.json" <<'JSON'
{"fields":[
 {"name":"Title","id":"F0"},
 {"name":"Status","id":"FS","options":[{"name":"Backlog","id":"s1"},{"name":"In Progress","id":"s2"}]},
 {"name":"Priority","id":"FP","options":[{"name":"High","id":"p1"},{"name":"Low","id":"p2"}]},
 {"name":"Effort","id":"FE","options":[{"name":"S","id":"e1"},{"name":"M","id":"e2"}]},
 {"name":"Area","id":"FA","options":[{"name":"ui","id":"a1"},{"name":"ci","id":"a2"}]}]}
JSON
  echo '{"id":"PROJ1"}' >"$FAKE_DIR/project.json"
}

# check DESCRIPTION EXPECTED-RC EXPECTED-STDOUT-SUBSTRING -- COMMAND...
check() {
  local desc=$1 want_rc=$2 want_out=$3 out rc=0
  shift 4
  out=$("$@" 2>"$work/stderr") || rc=$?
  if [ "$rc" -ne "$want_rc" ]; then
    fail "$desc: exit $rc, wanted $want_rc (stdout: $out)"
  elif [ -n "$want_out" ] && ! printf '%s' "$out" | grep -qF -- "$want_out"; then
    fail "$desc: stdout '$out' lacks '$want_out'"
  else
    pass "$desc"
  fi
}

# ok_if DESCRIPTION COMMAND... passes when the command succeeds
ok_if() {
  local desc=$1
  shift
  if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}
# not_if DESCRIPTION COMMAND... passes when the command fails
not_if() {
  local desc=$1
  shift
  if "$@" >/dev/null 2>&1; then fail "$desc"; else pass "$desc"; fi
}

# edit FILE JQ-FILTER
edit() {
  jq "$2" "$FAKE_DIR/$1" >"$FAKE_DIR/edit.tmp"
  mv "$FAKE_DIR/edit.tmp" "$FAKE_DIR/$1"
}

called() { grep -qF -- "$1" "$FAKE_DIR/calls.log"; }

# ---- land.sh: the verdict ------------------------------------------------------
reset
check "ready" 0 "PR #7 READY: up to date, 0 open threads, ci-ok green, CLEAN" -- "$land" 7
check "accepts a url" 0 "PR #7 READY" -- "$land" https://github.com/rolter-ai/rolter/pull/7
check "never merges without --merge" 0 "READY" -- "$land" 7
not_if "no merge call without --merge" called "pr merge"

edit pr.json '.isDraft = true'
check "draft blocks" 1 "NOT READY: draft" -- "$land" 7
reset
edit pr.json '.baseRefName = "feat/6-parent"'
check "stacked child is not landed here" 1 "stacked on feat/6-parent" -- "$land" 7
reset
echo '[{"number":8}]' | jq -c 'map({number})' >"$FAKE_DIR/children.json"
check "stack parent is not landed here" 1 "stack parent: #8" -- "$land" 7
reset
echo '{"behind_by":3}' >"$FAKE_DIR/compare.json"
check "behind is a note by default" 0 "READY: behind 3" -- "$land" 7
check "behind blocks with --require-up-to-date" 1 "behind master by 3" -- "$land" --require-up-to-date 7
reset
edit pr.json '.mergeable = "CONFLICTING" | .mergeStateStatus = "DIRTY"'
check "conflicts block" 1 "merge conflicts" -- "$land" 7
reset
cat >"$FAKE_DIR/threads.json" <<'JSON'
{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
 {"isResolved":true,"path":"a.rs","comments":{"nodes":[{"author":{"login":"x"},"url":"u1"}]}},
 {"isResolved":false,"path":"b.rs","comments":{"nodes":[{"author":{"login":"github-advanced-security"},"url":"u2"}]}}]}}}}}
JSON
check "an unresolved thread blocks" 1 "NOT READY: 1 unresolved review thread(s)" -- "$land" 7
ok_if "the thread is named on stderr" grep -q "b.rs by github-advanced-security u2" "$work/stderr"
reset
echo '[{"name":"ci-ok","bucket":"pass"},{"name":"ci-ok","bucket":"fail"}]' >"$FAKE_DIR/checks.json"
echo 1 >"$FAKE_DIR/checks.rc"
check "any red ci-ok blocks, even next to a green one" 1 "required check failed (ci-ok)" -- "$land" 7
echo '[{"name":"ci-ok","bucket":"pending"}]' >"$FAKE_DIR/checks.json"
echo 8 >"$FAKE_DIR/checks.rc"
check "a pending ci-ok waits" 2 "PENDING: required check pending (ci-ok)" -- "$land" 7
: >"$FAKE_DIR/checks.json"
check "no reported check waits" 2 "no required check reported yet" -- "$land" 7
reset
echo '[{"databaseId":42,"status":"in_progress"},{"databaseId":41,"status":"completed"}]' >"$FAKE_DIR/runs.json"
check "a green ci-ok over a running gate waits (#1328)" 2 "ci run(s) 42 still in progress" -- "$land" 7
reset
edit pr.json '.mergeStateStatus = "BLOCKED"'
check "BLOCKED with nothing to blame is reported" 1 "BLOCKED by branch protection" -- "$land" 7
reset
edit pr.json '.mergeStateStatus = "UNKNOWN"'
check "UNKNOWN mergeability waits" 2 "mergeability still being computed" -- "$land" 7
reset
edit pr.json '.state = "CLOSED"'
check "closed without merging" 1 "closed without merging" -- "$land" 7

# ---- land.sh --merge, over a throwaway repository ------------------------------
reset
# the repositories below must be the throwaway ones: refuse to go on if git
# still resolves to something outside $work
git init -q --bare "$FAKE_DIR/origin.git"
if [ "$(git -C "$FAKE_DIR/origin.git" rev-parse --absolute-git-dir)" != "$(cd "$FAKE_DIR/origin.git" && pwd -P)" ]; then
  echo "refusing to run: git does not resolve to the throwaway repository" >&2
  exit 1
fi
git init -q -b master "$work/main"
git -C "$work/main" remote add origin "$FAKE_DIR/origin.git"
git -C "$work/main" commit -q --no-gpg-sign --allow-empty -m "init"
git -C "$work/main" push -q origin master
git -C "$work/main" worktree add -q -b feat/7-thing "$work/wt-7"
git -C "$work/wt-7" commit -q --no-gpg-sign --allow-empty -m "work"
git -C "$work/wt-7" push -q origin feat/7-thing
tip=$(git -C "$work/wt-7" rev-parse HEAD)
edit pr.json ".headRefOid = \"$tip\""

cd "$work/wt-7"
check "--merge --dry-run changes nothing" 0 "PR #7 MERGED" -- "$land" --merge --dry-run 7 || true
not_if "--dry-run does not merge" called "pr merge"
ok_if "--dry-run keeps the branch" git -C "$work/main" show-ref --verify --quiet refs/heads/feat/7-thing

# the merge, run from inside the worktree it removes
check "--merge lands, closes, cleans" 0 "PR #7 MERGED feedfac | closed #11; removed worktree" -- "$land" --merge 7
ok_if "squash merge, no --delete-branch" called "pr merge 7 -R rolter-ai/rolter --squash"
not_if "never passes --delete-branch" called "delete-branch"
ok_if "closed the leftover issue" called "issue close 11"
ok_if "issue #11 is closed" grep -q '"CLOSED"' "$FAKE_DIR/issue-11.json"
not_if "posts no comment" called "issue comment"
not_if "worktree removed" [ -d "$work/wt-7" ]
not_if "local branch deleted" git -C "$work/main" show-ref --verify --quiet refs/heads/feat/7-thing
not_if "remote branch deleted" git -C "$FAKE_DIR/origin.git" show-ref --verify --quiet refs/heads/feat/7-thing

# running it again on the merged PR finishes nothing twice
: >"$FAKE_DIR/calls.log"
cd "$work/main"
check "--merge on a merged PR is a no-op" 0 "PR #7 MERGED feedfac" -- "$land" --merge 7
not_if "no second merge" called "pr merge"
not_if "no second close" called "issue close"

# a worktree with a commit the PR did not carry keeps its branch
reset
git -C "$work/main" worktree add -q -b feat/7-thing "$work/wt-7b" master
git -C "$work/wt-7b" commit -q --no-gpg-sign --allow-empty -m "extra"
edit pr.json ".headRefOid = \"$sha\""
cd "$work/main"
check "--merge keeps a branch whose tip is not the merged head" 0 "PR #7 MERGED" -- "$land" --merge 7
ok_if "kept the diverged branch" git -C "$work/main" show-ref --verify --quiet refs/heads/feat/7-thing

# a failing verdict stops the merge
reset
echo '[{"name":"ci-ok","bucket":"pending"}]' >"$FAKE_DIR/checks.json"
check "--merge refuses a PR that is not ready" 2 "PENDING" -- "$land" --merge 7
not_if "no merge when pending" called "pr merge"

# ---- usage ---------------------------------------------------------------------
cd "$root"
check "land --help" 0 "usage: land.sh" -- "$land" --help
check "land without a pr" 3 "" -- "$land"
check "land rejects a word" 3 "" -- "$land" nope
check "claim --help" 0 "usage: claim.sh" -- "$claim" --help
check "board --help" 0 "usage: board.sh" -- "$board" --help

# ---- board.sh ------------------------------------------------------------------
reset
url=https://github.com/rolter-ai/rolter/issues/11
check "board writes and verifies" 0 "board ok: $url In Progress/High/M/ui" -- "$board" "$url" high m ui "in progress"
ok_if "status option id came from the live field list" called "item-edit --id ITEM1 --project-id PROJ1 --field-id FS --single-select-option-id s2"
check "board rejects an unknown option" 3 "" -- "$board" "$url" urgent m ui
ok_if "board names the field it rejected" grep -q "is not a Priority option" "$work/stderr"
check "board rejects a non-url" 3 "" -- "$board" 11 high m ui
reset
touch "$FAKE_DIR/race"
BOARD_RETRIES=3 check "board rewrites a field the automation overwrote" 0 "board ok" -- "$board" "$url" high m ui "in progress"
ok_if "status was written twice (second time after the race)" [ "$(grep -c 'item-edit.*--field-id FS' "$FAKE_DIR/calls.log")" -eq 2 ]
reset
touch "$FAKE_DIR/race"
BOARD_RETRIES=1 check "board gives up after its retries" 1 "" -- "$board" "$url" high m ui "in progress"
reset
check "board --dry-run writes nothing" 0 "board dry-run" -- "$board" --dry-run "$url" high m ui
not_if "dry-run does not touch the board" called "item-add"

# ---- claim.sh ------------------------------------------------------------------
reset
check "claim puts the issue In Progress" 0 "claimed #11" -- "$claim" 11 high m ui
ok_if "status set to In Progress" called "field-id FS --single-select-option-id s2"
ok_if "board item added" called "item-add"
edit issue-11.json '.projectItems[0].status.name = "In Progress"'
check "claim is idempotent" 0 "claimed #11" -- "$claim" 11 high m ui
ok_if "claim says it was already claimed" grep -q "already In Progress" "$work/stderr"
reset
edit issue-11.json '.state = "CLOSED"'
check "claim refuses a closed issue" 1 "" -- "$claim" 11 high m ui
not_if "a closed issue is not boarded" called "item-add"
reset
edit issue-11.json '.url = "https://github.com/rolter-ai/rolter/pull/11"'
check "claim refuses a pull request" 3 "" -- "$claim" 11 high m ui
reset
check "claim --dry-run writes nothing" 0 "board dry-run" -- "$claim" --dry-run 11 high m ui
not_if "dry-run does not touch the board" called "item-add"
check "claim rejects a non-number" 3 "" -- "$claim" abc high m ui
check "claim rejects a bad priority" 3 "" -- "$claim" 11 urgentish m ui

# ---- the SessionStart storybook hook --------------------------------------------
free_port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
listening() { (exec 3<>"/dev/tcp/127.0.0.1/$free_port") 2>/dev/null; }
stop_stub() {
  local pid
  pid=$(lsof -nP -iTCP:"$free_port" -sTCP:LISTEN -t 2>/dev/null || true)
  [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
}
trap 'stop_stub; rm -rf "$work"' EXIT

mkdir -p "$work/bare" "$work/proj/ui/node_modules/.bin"
cat >"$work/proj/ui/node_modules/.bin/storybook" <<'STUB'
#!/usr/bin/env bash
echo "$*" >"$STUB_ARGS"
exec python3 -m http.server "$3" --bind 127.0.0.1
STUB
chmod +x "$work/proj/ui/node_modules/.bin/storybook"
export STUB_ARGS="$work/stub-args"

run_hook() { # run_hook PROJECT-DIR -> output in $work/hook.out, seconds in $hook_secs
  local start=$SECONDS
  CLAUDE_PROJECT_DIR=$1 ROLTER_STORYBOOK_PORT=$free_port "$hook" >"$work/hook.out" 2>&1 </dev/null
  hook_rc=$?
  hook_secs=$((SECONDS - start))
}

run_hook "$work/bare"
if [ "$hook_rc" -eq 0 ] && [ ! -s "$work/hook.out" ] && ! listening; then pass "hook: no node_modules, silent no-op"; else fail "hook: no node_modules case"; fi

run_hook "$work/proj"
if [ "$hook_rc" -eq 0 ] && [ ! -s "$work/hook.out" ] && [ "$hook_secs" -le 2 ]; then pass "hook: returns at once and prints nothing"; else fail "hook: slow or noisy start (rc $hook_rc, ${hook_secs}s)"; fi
tries=0
until listening || [ "$tries" -ge 50 ]; do
  tries=$((tries + 1))
  sleep 0.1
done
if listening; then pass "hook: the server is listening after the hook returned"; else fail "hook: nothing listening"; fi
if [ "$(cat "$STUB_ARGS" 2>/dev/null)" = "dev -p $free_port --ci --no-open --exact-port" ]; then pass "hook: storybook started with --ci --exact-port"; else fail "hook: wrong storybook arguments: $(cat "$STUB_ARGS" 2>/dev/null)"; fi
if [ -f "$work/proj/.claude/storybook-session.log" ]; then pass "hook: log under .claude/"; else fail "hook: no log file"; fi

rm -f "$STUB_ARGS"
run_hook "$work/proj"
sleep 0.3
if [ "$hook_rc" -eq 0 ] && [ ! -s "$work/hook.out" ] && [ ! -e "$STUB_ARGS" ]; then pass "hook: server already up, no second start and no output"; else fail "hook: started a second server"; fi
stop_stub

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "all fleet script checks passed"
