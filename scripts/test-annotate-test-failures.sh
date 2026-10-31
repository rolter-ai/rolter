#!/usr/bin/env bash
# runs .github/scripts/annotate-test-failures.sh over a nextest log and a libtest
# log and checks the `::error` annotations it prints (#2771).
#
# a red `nextest / doctests` job used to annotate only "Process completed with
# exit code 100", so anyone who could read annotations but not the raw log had to
# rebuild the workspace to learn which test failed. the annotator is the only
# thing that puts the test's name and panic in an annotation, and its parsing is
# tied to the exact shape of cargo's and nextest's output, so a nextest release
# that changes the shape fails here instead of silently annotating nothing. it
# runs as a step of quality.yml's `static checks` job and as a prek hook.
#
# needs bash 4 (the annotator uses `mapfile`), as CI's runner has.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
annotator="$root/.github/scripts/annotate-test-failures.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
fail() {
  echo "FAIL [$1] $2" >&2
  failures=$((failures + 1))
}

# annotate NAME LOG: the annotations for the log, in $work/NAME.out
annotate() {
  bash "$annotator" "$2" "tests failed" >"$work/$1.out"
}
has() {
  # has NAME NEEDLE
  grep -qF -- "$2" "$work/$1.out" || fail "$1" "annotations lack: $2 (got: $(cat "$work/$1.out"))"
}
lacks() {
  if grep -qF -- "$2" "$work/$1.out"; then
    fail "$1" "annotations have: $2"
  fi
}
count() {
  # count NAME N: the number of ::error lines
  local got
  got=$(grep -c '^::error ' "$work/$1.out" || true)
  [ "$got" = "$2" ] || fail "$1" "expected $2 annotation(s), got $got"
}

# ── nextest, as `cargo nextest run --color never` prints it ───────────────────
cat >"$work/nextest.log" <<'LOG'
   Compiling demo v0.1.0
────────────
 Nextest run ID 49df3b67-90d1-45a9-bf33-5c0770631955 with nextest profile: default
    Starting 5 tests across 2 binaries
        PASS [   0.006s] (1/5) demo tests::passes
        FAIL [   0.239s] (2/5) demo tests::fails_with_assert
  stdout ───

    running 1 test
    test tests::fails_with_assert ... FAILED

    failures:

    failures:
        tests::fails_with_assert

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.23s

  stderr ───

    thread 'tests::fails_with_assert' (32499) panicked at src/lib.rs:7:9:
    assertion `left == right` failed: math is hard
      left: 2
     right: 3
    stack backtrace:
       0: __rustc::rust_begin_unwind
                 at /rustc/2d8144b/library/std/src/panicking.rs:689:5
    note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.

        FAIL [   0.244s] (3/5) demo::it integration_fails
  stdout ───

    running 1 test
    test integration_fails ... FAILED

  stderr ───

    thread 'integration_fails' (32500) panicked at tests/it.rs:4:7:
    no value
    stack backtrace:
       0: __rustc::rust_begin_unwind

     TIMEOUT [  60.001s] (4/5) demo tests::hangs
  stdout ───

    running 1 test

        SLOW [> 60.000s] (5/5) demo tests::slow_but_fine
────────────
     Summary [  61.255s] 5 tests run: 1 passed, 3 failed, 0 skipped
        FAIL [   0.239s] (2/5) demo tests::fails_with_assert
        FAIL [   0.244s] (3/5) demo::it integration_fails
     TIMEOUT [  60.001s] (4/5) demo tests::hangs
error: test run failed
LOG
annotate nextest "$work/nextest.log"
count nextest 3
has nextest "demo tests::fails_with_assert%0Athread 'tests::fails_with_assert' (32499) panicked at src/lib.rs:7:9:"
has nextest "assertion \`left == right\` failed: math is hard%0A  left: 2%0A right: 3"
has nextest "demo::it integration_fails%0Athread 'integration_fails' (32500) panicked at tests/it.rs:4:7:%0Ano value"
# a test that hung has no panic to quote, but is still named
has nextest "::error title=tests failed::demo tests::hangs"
lacks nextest "backtrace"
lacks nextest "passes"
lacks nextest "slow_but_fine"
# the recap repeats each failure; it must not repeat the annotation
count nextest 3

# ── libtest, as the coverage step prints it ───────────────────────────────────
cat >"$work/libtest.log" <<'LOG'
running 3 tests
test tests::passes ... ok
test tests::fails_with_assert ... FAILED
test tests::fails_multiline ... FAILED

failures:

---- tests::fails_with_assert stdout ----

thread 'tests::fails_with_assert' (32560) panicked at src/lib.rs:7:9:
assertion `left == right` failed: math is hard
stack backtrace:

---- tests::fails_multiline stdout ----

thread 'tests::fails_multiline' (32559) panicked at src/lib.rs:11:9:
first line
stack backtrace:

failures:
    tests::fails_multiline
    tests::fails_with_assert

test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
LOG
annotate libtest "$work/libtest.log"
count libtest 2
has libtest "tests::fails_with_assert%0Athread 'tests::fails_with_assert' (32560) panicked at src/lib.rs:7:9:%0Aassertion"
has libtest "tests::fails_multiline%0Athread 'tests::fails_multiline' (32559) panicked at src/lib.rs:11:9:%0Afirst line"

# ── a log that names no failing test ──────────────────────────────────────────
cat >"$work/broken.log" <<'LOG'
error[E0308]: mismatched types
error: could not compile `demo` (lib test) due to 1 previous error
LOG
annotate broken "$work/broken.log"
count broken 1
has broken "no failing test was named in the output"

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "annotate-test-failures: every case passed"
