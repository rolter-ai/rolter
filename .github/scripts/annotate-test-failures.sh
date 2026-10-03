#!/usr/bin/env bash
# turns a cargo test log into one ::error annotation per failing test, carrying
# the panic location and message, so a red job names its test without the raw
# log. usage: annotate-test-failures.sh <log> <title>
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
