#!/usr/bin/env bash
# rejects a coding-agent session or remote-connection url in a commit message
# or pr body. these are ephemeral, sometimes-private links (see the 8 sep
# commit on feat/1385-stability-marker, preserved at
# archive/1385-stability-marker-prior) that must never ship into history.
#
# usage:
#   check-agent-session-urls.sh --file PATH           # one file's content (commit-msg hook)
#   check-agent-session-urls.sh --text TEXT           # one literal string (pr body)
#   check-agent-session-urls.sh --commit-range A..B   # each commit message in a range
#
# flags combine; the script fails if any check finds a match.
set -euo pipefail

# claude.ai/code/session covers a local claude code session and a remote
# control session tied to it - same url shape. the trailer pattern is
# vendor-agnostic: any coding agent's own "<Name>-Session:" git trailer
# carrying a url, so a future agent that adopts the same convention is
# caught without hardcoding its domain.
PATTERN='claude\.ai/code/session|[A-Za-z][A-Za-z0-9_-]*-Session:[[:space:]]*https?://'

fail=0

check_text() {
  local label="$1" text="$2"
  local hits
  if hits=$(printf '%s' "$text" | grep -inE "$PATTERN" || true) && [ -n "$hits" ]; then
    echo "::error::agent session or remote-connection url found in ${label}"
    echo "$hits"
    fail=1
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --file)
      check_text "$2" "$(cat "$2")"
      shift 2
      ;;
    --text)
      check_text "pr body" "$2"
      shift 2
      ;;
    --commit-range)
      while IFS= read -r sha; do
        check_text "commit $sha" "$(git log -1 --format=%B "$sha")"
      done < <(git rev-list "$2")
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [ "$fail" -ne 0 ]; then
  echo "::error::remove the session/remote-connection url and rewrite the commit or pr body"
  echo "::error::a pr-authoring tool may have appended this line after your own body; strip just that line with a direct PATCH to the pr, which does not re-trigger the injection - see docs/dev-docs/development/ci-gating.md#agent-session-urls"
  exit 1
fi
