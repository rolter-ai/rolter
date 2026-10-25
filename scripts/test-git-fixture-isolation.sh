#!/usr/bin/env bash
# runs every scripts/test-*.sh that builds a throwaway git repository under the
# environment a git or prek hook hands it, and checks that the repository the
# hook belongs to comes out untouched: same config, HEAD, refs and index.
#
# a pre-commit hook runs with GIT_DIR, GIT_INDEX_FILE and the like pointing at
# the real repository, and every git command inherits them, `git -C <fixture>`
# included. a fixture script that does not clear them commits its fixture
# history onto the real HEAD, writes its fixture identity into the real config,
# and once left core.bare=true there (#3196). the fix is the loop at the top of
# each fixture script that unsets every name `git rev-parse --local-env-vars`
# prints; this checks it statically, then proves it against a sentinel
# repository standing in for the real one.
#
# the scripts are found by grepping for git, not listed, so a fixture script
# added later is covered the day it is written. bash 3.2 compatible, so it runs
# under macos's /bin/bash as well as in ci.
set -euo pipefail

# this script builds its own sentinel with git, so it must not inherit a hook's
# environment either
for var in $(git rev-parse --local-env-vars); do
  unset "$var"
done

root=$(cd "$(dirname "$0")/.." && pwd)
self=$(basename "$0")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# a script that runs `git init`, or git against a directory it names, builds a
# fixture repository
scripts=""
for f in "$root"/scripts/test-*.sh; do
  [ "$(basename "$f")" = "$self" ] && continue
  if grep -qE '(^|[^[:alnum:]_-])git (init|-C) ' "$f"; then
    scripts="$scripts $f"
  fi
done
if [ -z "$scripts" ]; then
  fail "found no fixture script to check; the grep above no longer matches them"
fi

# ── statically: each one clears the hook environment ─────────────────────────
for f in $scripts; do
  if ! grep -q 'git rev-parse --local-env-vars' "$f"; then
    fail "scripts/$(basename "$f") builds a git fixture but never unsets \`git rev-parse --local-env-vars\`"
  fi
done

# ── dynamically: run each against a sentinel standing in for the real repo ───
sentinel="$work/sentinel"
main="$sentinel/main"
wt="$sentinel/wt"
# a main checkout and a linked worktree (wt's layout), each with a commit and a
# staged file, so config, HEAD, refs and both indexes are all something a stray
# command could change
make_sentinel() {
  rm -rf "$sentinel"
  git init -q -b master "$main"
  git -C "$main" -c user.name=sentinel -c user.email=sentinel@example.com \
    commit -q --no-gpg-sign --allow-empty -m "sentinel"
  git -C "$main" worktree add -q -b feat/1-sentinel "$wt"
  : >"$main/tracked"
  git -C "$main" add tracked
  : >"$wt/tracked"
  git -C "$wt" add tracked
}

# snapshot: what a stray fixture command could change in the real repository
snapshot() {
  {
    echo "== config"
    cat "$main/.git/config"
    echo "== HEAD (main, worktree)"
    cat "$main/.git/HEAD" "$main/.git/worktrees/wt/HEAD"
    echo "== refs"
    git -C "$main" for-each-ref --format='%(refname) %(objectname)'
    echo "== index (main)"
    git -C "$main" ls-files --stage
    echo "== index (worktree)"
    git -C "$wt" ls-files --stage
  } 2>&1
}

# run_hooked LABEL SCRIPT DIR VAR=VALUE...: SCRIPT from DIR under the variables
# git exports to a pre-commit hook there, then compare the sentinel
run_hooked() {
  local label=$1 script=$2 dir=$3 name rc=0
  shift 3
  name="scripts/$(basename "$script")"
  make_sentinel
  snapshot >"$work/before"
  (cd "$dir" && env "$@" bash "$script") >"$work/out" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ]; then
    fail "$name exited $rc under a hook in $label:"
    sed 's/^/    /' "$work/out" >&2
  fi
  snapshot >"$work/after"
  if ! diff -u "$work/before" "$work/after" >"$work/diff"; then
    fail "$name changed the repository of a hook in $label:"
    sed 's/^/    /' "$work/diff" >&2
  else
    echo "ok: $name leaves a hook's repository alone ($label)"
  fi
}

for f in $scripts; do
  run_hooked "the main checkout" "$f" "$main" \
    GIT_DIR="$main/.git" GIT_WORK_TREE="$main" GIT_INDEX_FILE="$main/.git/index"
  # the case that left core.bare=true behind: from a linked worktree, GIT_DIR
  # names its private directory, and a `git init` there rewrites the shared
  # config as a bare repository's
  run_hooked "a linked worktree" "$f" "$wt" \
    GIT_DIR="$main/.git/worktrees/wt" GIT_INDEX_FILE="$main/.git/worktrees/wt/index"
done

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "git-fixture-isolation: every fixture script is isolated"
