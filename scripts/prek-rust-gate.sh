#!/usr/bin/env bash
# runs the given command only when the pushed range touches a rust input, so a
# dashboard- or docs-only push does not wait on a cold `--all-features` build of
# the whole workspace (#1486)
#
#     bash scripts/prek-rust-gate.sh bash scripts/prek-rust-tests.sh
#     bash scripts/prek-rust-gate.sh --staged cargo fmt --all -- --check
#
# prek exports the push range as PRE_COMMIT_FROM_REF / PRE_COMMIT_TO_REF. every
# case this script cannot reason about runs the command: no range (a push with
# no remote ancestor, `prek run --all-files`), an unresolvable ref, a failing
# diff, or ROLTER_PREK_RUST_ALWAYS=1. skipping is only ever the proven case.
# hosted `ci-ok` stays the authoritative gate either way
#
# this is deliberately not a `files =` filter on the hook: prek drops paths that
# no longer exist before matching, so a push that only deletes a `.rs` file
# would skip the suite it can break
#
# --staged is for commit-stage hooks, where prek hands over no range: the index
# (`git diff --cached`) stands in for it, deletions included (#1526). an empty
# index means a manual `prek run`, so the command runs
set -euo pipefail

staged=0
if [[ "${1:-}" == "--staged" ]]; then
    staged=1
    shift
fi

if [[ $# -eq 0 ]]; then
    echo "usage: prek-rust-gate.sh [--staged] <command> [args...]" >&2
    exit 2
fi

from="${PRE_COMMIT_FROM_REF:-}"
to="${PRE_COMMIT_TO_REF:-}"

run() {
    exec "$@"
}

if [[ "${ROLTER_PREK_RUST_ALWAYS:-}" == "1" ]]; then
    run "$@"
fi

if [[ -z "$from" || -z "$to" ]]; then
    if [[ "$staged" == "0" ]]; then
        run "$@"
    fi
    if ! changed=$(git diff --cached --name-only --no-renames) || [[ -z "$changed" ]]; then
        run "$@"
    fi
    range="the index"
fi

# three-dot, as prek diffs it, so only what the pushed commits changed counts
# and never what the remote gained since. --no-renames lists both sides of a
# rename, so moving a file out of crates/ still counts
if [[ -z "${range:-}" ]]; then
    if ! git rev-parse --verify --quiet "$from^{commit}" >/dev/null ||
        ! git rev-parse --verify --quiet "$to^{commit}" >/dev/null; then
        run "$@"
    fi
    if ! changed=$(git diff --name-only --no-renames "$from...$to"); then
        run "$@"
    fi
    range="$from...$to"
fi

# every path whose change can move a rust build, test or dependency-policy
# result. most are obvious; the tail is files a test reads from outside its
# crate at run time — when a test starts reading a new one, add it here
rust_inputs='^(crates/'
rust_inputs+='|.*\.rs$'
rust_inputs+='|(.*/)?Cargo\.(toml|lock)$'
rust_inputs+='|rust-toolchain(\.toml)?$'
rust_inputs+='|\.cargo/'
rust_inputs+='|\.config/(deny|nextest)\.toml$'
rust_inputs+='|prek\.toml$'
rust_inputs+='|scripts/prek-rust-(gate|tests)\.sh$'
rust_inputs+='|rolter\.example\.toml$'
rust_inputs+='|docs/dev-docs/development/stability-markers\.md$'
rust_inputs+='|ui/src/lib/nav\.tsx$'
rust_inputs+=')'

if printf '%s\n' "$changed" | grep -Eq -- "$rust_inputs"; then
    run "$@"
fi

echo "no rust inputs changed in $range; skipping: $*"
