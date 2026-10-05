#!/usr/bin/env bash
# claim an issue before branching: put it on the board and set Status to In
# Progress together with its Priority, Effort and Area, so nobody else picks
# the same issue up (the board is the only place work in flight is visible).
#
#   scripts/fleet/claim.sh [--dry-run] <issue> <Priority> <Effort> <Area>
#
# Refuses a closed issue and a pull request number. Safe to re-run: claiming an
# issue that is already In Progress re-applies the same fields and says so on
# stderr. Field values are documented in board.sh --help, which does the work.
#
# needs gh (authenticated, `project` scope) and jq. bash 3.2 compatible.
set -euo pipefail

repo=${ROLTER_REPO:-rolter-ai/rolter}
board_title=${ROLTER_BOARD_TITLE:-rolter}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

usage() {
  cat <<'USAGE'
usage: claim.sh [--dry-run] <issue> <Priority> <Effort> <Area>

  issue     issue number (not a pull request)
  Priority  Urgent | High | Medium | Low
  Effort    XS | S | M | L | XL
  Area      gateway | control | ui | proxy | balancer | store | auth | core |
            docs | ci | infra | cross-cutting

--dry-run  read-only: check the issue and print what would be written
exit codes: 0 claimed, 1 refused (closed), 3 usage
USAGE
}

dry_run=0
args=()
for arg in "$@"; do
  case "$arg" in
    -h | --help)
      usage
      exit 0
      ;;
    --dry-run) dry_run=1 ;;
    *) args+=("$arg") ;;
  esac
done
if [ "${#args[@]}" -ne 4 ]; then
  usage >&2
  exit 3
fi
issue=${args[0]}
case "$issue" in
  '' | *[!0-9]*)
    echo "claim.sh: '$issue' is not an issue number" >&2
    exit 3
    ;;
esac

view=$(gh issue view "$issue" -R "$repo" --json number,state,url,projectItems </dev/null)
state=$(jq -r .state <<<"$view")
url=$(jq -r .url <<<"$view")
status=$(jq -r --arg t "$board_title" '[.projectItems[]? | select(.title == $t) | .status.name] | first // ""' <<<"$view")

case "$url" in
  */issues/*) ;;
  *)
    echo "claim.sh: #$issue is not an issue ($url)" >&2
    exit 3
    ;;
esac
if [ "$state" != OPEN ]; then
  echo "claim.sh: #$issue is $state, nothing to claim" >&2
  exit 1
fi
if [ "$status" = "In Progress" ]; then
  echo "claim.sh: #$issue is already In Progress; re-applying the fields" >&2
fi

if [ "$dry_run" -eq 1 ]; then
  "$here/board.sh" --dry-run "$url" "${args[1]}" "${args[2]}" "${args[3]}" "In Progress"
  exit 0
fi

"$here/board.sh" "$url" "${args[1]}" "${args[2]}" "${args[3]}" "In Progress"
echo "claimed #$issue"
