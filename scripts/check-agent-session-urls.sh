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
# and two that resolve an open pr from the api by its head ref, for the
# dispatched ci run whose event payload carries no pull request (#1523, #1562):
#
#   check-agent-session-urls.sh --pr-for-ref REPO REF            # that pr's body
#   check-agent-session-urls.sh --commit-range-for-ref REPO REF  # base..head messages
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

# writes the open-pull-request listing for REPO to stdout as a file path.
# shared by the two --*-for-ref modes below so the lookup, its failure rule and
# its fixture override exist once rather than twice. set ROLTER_PULLS_JSON to a
# file of the same shape the api returns and no network call is made, which is
# what makes both modes runnable outside ci.
open_pulls_file() {
  local repo="$1"
  if [ -n "${ROLTER_PULLS_JSON:-}" ]; then
    printf '%s' "${ROLTER_PULLS_JSON}"
    return 0
  fi
  local pulls
  pulls="$(mktemp)"
  # no pipeline: under `pipefail` a short-circuiting reader would fail this for
  # the wrong reason (#1291, #1306)
  if ! gh api "repos/${repo}/pulls?state=open&per_page=100" > "$pulls"; then
    echo "::error::could not list open pull requests for ${repo}; refusing to report this as checked" >&2
    return 1
  fi
  printf '%s' "$pulls"
}

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
      pulls="$(open_pulls_file "$repo")" || exit 1
      matched="$(jq --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | length' "$pulls")"
      if [ "$matched" -eq 0 ]; then
        echo "::notice::no open pull request has ${ref} as its head; there is no body to check"
      else
        body="$(jq -r --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | .[0].body // ""' "$pulls")"
        check_text "pr body for ${ref}" "$body"
      fi
      shift 3
      ;;
    # the commit-message half of the same problem (#1562). quality.yml's job
    # read base/head out of `github.event.pull_request`, so on a dispatched run
    # it was skipped outright — and a skipped job inside a reusable workflow
    # does not fail it, so `ci-ok` went green having never read the commits.
    # that is the path the release pr takes by design (#1025) and the
    # documented recovery for a stranded sha (#1522), so it is the wrong place
    # to have no verdict. resolve the same pr by head ref and take the range
    # from it
    --commit-range-for-ref)
      repo="$2"
      ref="$3"
      pulls="$(open_pulls_file "$repo")" || exit 1
      matched="$(jq --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | length' "$pulls")"
      if [ "$matched" -eq 0 ]; then
        echo "::notice::no open pull request has ${ref} as its head; there are no commits to check"
      else
        base="$(jq -r --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | .[0].base.sha // ""' "$pulls")"
        head="$(jq -r --arg ref "$ref" '[.[] | select(.head.ref == $ref)] | .[0].head.sha // ""' "$pulls")"
        if [ -z "$base" ] || [ -z "$head" ]; then
          echo "::error::the open pull request for ${ref} carries no base/head sha; refusing to report its commits as checked" >&2
          exit 1
        fi
        # the range needs both endpoints in the local clone; the caller is
        # responsible for a full-history checkout, and a missing object must
        # fail rather than silently check nothing
        if ! git rev-parse --verify --quiet "${base}^{commit}" > /dev/null \
          || ! git rev-parse --verify --quiet "${head}^{commit}" > /dev/null; then
          echo "::error::${base}..${head} is not fully present in this clone; check out with fetch-depth 0 before checking commit messages" >&2
          exit 1
        fi
        while IFS= read -r sha; do
          check_text "commit $sha" "$(git log -1 --format=%B "$sha")"
        done < <(git rev-list "${base}..${head}")
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
