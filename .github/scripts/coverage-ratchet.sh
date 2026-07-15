#!/usr/bin/env bash
# coverage ratchet: compare the current workspace line-coverage percentage
# against a committed baseline so new code cannot silently drop coverage.
#
# policy (ROL-246): the job stays informational (continue-on-error in ci) until
# the baseline is trusted. this script therefore *reports* a regression with a
# non-zero exit, but the workflow decides whether that blocks. once the baseline
# is stable, drop continue-on-error to promote it to a required check.
#
# inputs:
#   $1  path to the cargo-llvm-cov json summary (default: coverage.json)
#   $2  path to the baseline file             (default: .github/coverage-baseline.txt)
# env:
#   COVERAGE_TOLERANCE  allowed downward drift in percentage points (default: 0.5)
set -euo pipefail

json="${1:-coverage.json}"
baseline_file="${2:-.github/coverage-baseline.txt}"
tolerance="${COVERAGE_TOLERANCE:-0.5}"

if [[ ! -f "$json" ]]; then
  echo "coverage json not found: $json" >&2
  exit 2
fi

# cargo-llvm-cov json: .data[0].totals.lines.percent is the workspace line %
current="$(jq -r '.data[0].totals.lines.percent' "$json")"
if [[ -z "$current" || "$current" == "null" ]]; then
  echo "could not read line coverage from $json" >&2
  exit 2
fi
current="$(printf '%.2f' "$current")"

summary="${GITHUB_STEP_SUMMARY:-/dev/stdout}"

if [[ ! -f "$baseline_file" ]]; then
  # no baseline yet: establish one from the current run and ask a human to commit it
  echo "no baseline at $baseline_file — current line coverage is ${current}%"
  {
    echo "### Coverage ratchet"
    echo
    echo "No baseline committed yet. Current workspace line coverage: **${current}%**."
    echo
    echo "Commit this value to \`$baseline_file\` to start ratcheting."
  } >> "$summary"
  exit 0
fi

# ignore comment (#) and blank lines, then take the first number
baseline_raw="$(grep -vE '^\s*(#|$)' "$baseline_file" | grep -Eo '[0-9]+(\.[0-9]+)?' | head -1)"
if [[ -z "$baseline_raw" ]]; then
  echo "no numeric baseline found in $baseline_file" >&2
  exit 2
fi
baseline="$(printf '%.2f' "$baseline_raw")"
floor="$(echo "$baseline - $tolerance" | bc -l)"

# bc returns 1 when the comparison is true
regressed="$(echo "$current < $floor" | bc -l)"

{
  echo "### Coverage ratchet"
  echo
  echo "| metric | value |"
  echo "| --- | --- |"
  echo "| current line coverage | ${current}% |"
  echo "| committed baseline | ${baseline}% |"
  echo "| tolerance | ${tolerance} pp |"
  echo "| floor (baseline − tolerance) | ${floor}% |"
} >> "$summary"

if [[ "$regressed" == "1" ]]; then
  {
    echo
    echo "⚠️ **Coverage regressed**: ${current}% is below the floor of ${floor}%."
    echo "Add tests or, if intentional, lower \`$baseline_file\` in the same PR."
  } >> "$summary"
  echo "coverage regressed: ${current}% < floor ${floor}% (baseline ${baseline}%)" >&2
  exit 1
fi

# ratchet up: if current meaningfully beats the baseline, nudge to raise it
raise="$(echo "$current > $baseline + $tolerance" | bc -l)"
if [[ "$raise" == "1" ]]; then
  {
    echo
    echo "✅ Coverage is **${current}%**, above the baseline of ${baseline}%. "
    echo "Consider raising \`$baseline_file\` to lock in the gain."
  } >> "$summary"
else
  echo "" >> "$summary"
  echo "✅ Coverage ${current}% is within tolerance of the ${baseline}% baseline." >> "$summary"
fi
