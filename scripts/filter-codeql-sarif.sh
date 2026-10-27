#!/usr/bin/env bash
# drop code-scanning results that sit under a test directory from a sarif file
#
# codeql's rust extractor analyses every file the build sees, and
# `paths-ignore` in the codeql config does not stop it for a compiled language,
# so test fixtures (a deliberately wrong nonce, a loopback http url) raise
# alerts on every pull request that rewrites a large test file (#2749). this
# filters the sarif before upload instead. production code under
# `crates/*/src/` is never touched, and a result is dropped only when its
# primary location is under `crates/<crate>/tests/`
#
# usage: filter-codeql-sarif.sh <in.sarif> <out.sarif>
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <in.sarif> <out.sarif>" >&2
    exit 2
fi

in="$1"
out="$2"

if [[ ! -f "$in" ]]; then
    echo "filter-codeql-sarif: no such file '$in'" >&2
    exit 1
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

jq '
  def in_tests:
    (.locations // [])
    | any(.[]; (.physicalLocation.artifactLocation.uri // "")
        | test("(^|/)crates/[^/]+/tests/"));
  .runs |= map(.results |= ((. // []) | map(select(in_tests | not))))
' "$in" > "$tmp"

before=$(jq '[.runs[].results | length] | add // 0' "$in")
after=$(jq '[.runs[].results | length] | add // 0' "$tmp")
echo "filter-codeql-sarif: kept ${after} of ${before} results (dropped $((before - after)) under crates/*/tests/)"
mv "$tmp" "$out"
