#!/usr/bin/env bash
# turns a cargo test or cargo nextest log into one ::error annotation per
# failing test, carrying the panic location and message, so a red job names its
# test without the raw log. usage: annotate-test-failures.sh <log> <title>
#
# nextest prints `FAIL [ 0.2s] (2/4) <binary> <test>` for each failure and, under
# it, the test's stderr; libtest prints `test <name> ... FAILED` and a
# `---- <name> stdout ----` section. a nextest log carries the libtest lines too
# (it shows each test's stdout), so nextest's are looked for first
set -euo pipefail

log="$1"
title="$2"

# workflow-command data escapes: % first, then newlines
escape() {
  local s="$1"
  s="${s//'%'/'%25'}"
  s="${s//$'\r'/'%0D'}"
  s="${s//$'\n'/'%0A'}"
  printf '%s' "${s}"
}

# the statuses nextest gives a test that did not pass. a `TRY n FAIL` retry line
# does not match: the test's final status is printed as well
nextest_status='(FAIL|TIMEOUT|SIG[A-Z]+|EXEC-FAIL|LEAK-FAIL)'
mapfile -t nextest_failed < <(
  sed -nE "s/^[[:space:]]+${nextest_status}[[:space:]]+\[[[:space:]]*[0-9.]+s\][[:space:]]+(\([0-9]+\/[0-9]+\)[[:space:]]+)?(.+)\$/\3/p" "${log}" |
    awk '!seen[$0]++'
)
if [ "${#nextest_failed[@]}" -gt 0 ]; then
  for name in "${nextest_failed[@]}"; do
    # the live section under the status line holds the test's output; take the
    # panic and the lines of its message, up to the backtrace
    detail="$(awk -v t="${name}" '
      function strip(l) { sub(/^    /, "", l); return l }
      function status_line(l) { return l ~ /^[[:space:]]+[A-Z][A-Za-z0-9 -]* +\[ *[0-9.]+s\]/ || l ~ /^[[:space:]]+SLOW +\[/ }
      function test_of(l) { sub(/^[^]]*\][[:space:]]+(\([0-9]+\/[0-9]+\)[[:space:]]+)?/, "", l); return l }
      !on && status_line($0) && $0 !~ /^[[:space:]]+(PASS|SKIP|SLOW) / && test_of($0) == t { on = 1; next }
      on && status_line($0) { exit }
      on && /^─/ { exit }
      on && !hit && /panicked at/ { hit = 1; print strip($0); n = 0; next }
      hit {
        if ($0 ~ /^[[:space:]]*$/ || $0 ~ /stack backtrace:/ || $0 ~ /^[[:space:]]*note:/ || ++n > 3) exit
        print strip($0)
      }
    ' "${log}")"
    echo "::error title=$(escape "${title}")::$(escape "${name}${detail:+$'\n'}${detail}")"
  done
  exit 0
fi

mapfile -t failed < <(sed -nE 's/^test (.+) \.\.\. FAILED$/\1/p' "${log}" | sort -u)
if [ "${#failed[@]}" -eq 0 ]; then
  echo "::error title=$(escape "${title}")::no failing test was named in the output; the build or a test binary failed before any test reported"
  exit 0
fi

for name in "${failed[@]}"; do
  # the `---- <name> stdout ----` section holds the panic for that test
  detail="$(awk -v t="---- ${name} stdout ----" '
    $0 == t { on = 1; next }
    on && /^---- / { exit }
    on && /panicked at/ { print; getline; print; exit }
  ' "${log}")"
  echo "::error title=$(escape "${title}")::$(escape "${name}${detail:+$'\n'}${detail}")"
done
