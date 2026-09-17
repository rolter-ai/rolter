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
    # check the body of the open pr whose head is REF. used by the dispatched
    # ci run, which carries no pr in its event payload and has to look one up
    # (#1523). the resolution lives here rather than inline in the workflow so
    # it can be exercised against a fixture: set ROLTER_PULLS_JSON to a file of
    # the same shape the api returns and no network call is made.
    --pr-for-ref)
      repo="$2"
      ref="$3"
      pulls="${ROLTER_PULLS_JSON:-}"
      if [ -z "$pulls" ]; then
        pulls="$(mktemp)"
        # no pipeline: under `pipefail` a short-circuiting reader would fail
        # this for the wrong reason (#1291, #1306)
        if ! gh api "repos/${repo}/pulls?state=open&per_page=100" > "$pulls"; then
          echo "::error::could not list open pull requests for ${repo}; refusing to report this body as checked" >&2
          exit 1
        fi
      fi
      matched="$(jq --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | length' "$pulls")"
      if [ "$matched" -eq 0 ]; then
        echo "::notice::no open pull request has ${ref} as its head; there is no body to check"
      else
        body="$(jq -r --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | .[0].body // ""' "$pulls")"
        check_text "pr body for ${ref}" "$body"
      fi
      shift 3
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
