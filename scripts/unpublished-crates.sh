#!/usr/bin/env bash
# prints `<crate>@<version>` for every publishable workspace crate whose version
# is not confirmed on crates.io, one per line, and nothing when all of them are.
#
#   bash scripts/unpublished-crates.sh
#
# release-plz.yml's `release-gate` job uses it to decide whether this push has
# anything to publish (#2025). only a crate with nothing pending lets the job
# skip the wait for ci-ok, and a skip only ever skips the publish, so every
# doubt resolves to "pending": a lookup that fails for any reason counts as
# unpublished. the crate list is read from `cargo metadata` (every package whose
# `publish` is not `[]`, i.e. not `publish = false`) rather than written out,
# so a crate added to the workspace can never fall through. an empty or
# unreadable list is an error (exit 1), which the caller treats as pending too.
#
# bash 3.2 compatible, so the fixture tests (scripts/test-release-gate.sh) run
# on a stock mac too.
set -euo pipefail

api=https://crates.io/api/v1/crates
# crates.io asks automated clients to identify themselves
agent="rolter-release-gate (github.com/rolter-ai/rolter)"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

if ! cargo metadata --no-deps --format-version 1 >"$work/metadata.json"; then
  echo "error: cargo metadata failed; cannot tell what is released" >&2
  exit 1
fi
if ! jq -r '.packages[] | select(.publish != []) | "\(.name) \(.version)"' \
  "$work/metadata.json" >"$work/crates.txt"; then
  echo "error: cargo metadata answered something that is not a package list" >&2
  exit 1
fi
if [ ! -s "$work/crates.txt" ]; then
  echo "error: cargo metadata listed no publishable crate; cannot tell what is released" >&2
  exit 1
fi

while read -r name version; do
  code=$(curl -sS --max-time 20 --retry 2 -A "$agent" \
    -o "$work/body.json" -w '%{http_code}' "$api/$name/$version" </dev/null || true)
  if [ "$code" = 200 ]; then
    if jq -e --arg n "$name" --arg v "$version" \
      '.version.crate == $n and .version.num == $v' "$work/body.json" >/dev/null 2>&1; then
      echo "$name $version: on crates.io" >&2
      continue
    fi
    echo "$name $version: crates.io answered 200 about something else; counting it as unpublished" >&2
  elif [ "$code" = 404 ]; then
    echo "$name $version: not on crates.io" >&2
  else
    echo "$name $version: lookup failed (http ${code:-none}); counting it as unpublished" >&2
  fi
  echo "$name@$version"
done <"$work/crates.txt"
