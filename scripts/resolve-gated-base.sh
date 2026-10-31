#!/usr/bin/env bash
# prints the base sha a dispatched ci run gated: the base of the open pull
# request whose head is the dispatched ref, or `none`.
#
#   REPO=rolter-ai/rolter REF_NAME=release-plz-2026-10-27 GH_TOKEN=... \
#     bash scripts/resolve-gated-base.sh
#
# `gate-ok` records the base it gated in its step name, and the `edited` fast
# path of `ci-ok` accepts only a pass against the pull request's current base
# (#2649). a pull_request run reads that base from its payload. a
# `workflow_dispatch` run, which is how the release PR is gated (#1025), has no
# pull request in its payload, so it used to record `none` and a human edit to
# the release PR's title or body went red until a new commit landed (#2657).
# this resolves the pull request from the api instead, by its head ref.
#
# never fails the run: the base is a record, not a gate. when it cannot be
# resolved (no open pull request for the ref, or an api that keeps failing) it
# prints `none`, which the fast path never accepts, so a lookup that went wrong
# costs a re-dispatch and never a hollow green.
#
# bash 3.2 compatible, so scripts/test-assert-gate-ran.sh runs on a stock mac too.
set -euo pipefail

: "${REPO:?REPO must name the repository, e.g. rolter-ai/rolter}"
: "${REF_NAME:?REF_NAME must be the dispatched branch}"

owner=${REPO%%/*}
path="repos/${REPO}/pulls?state=open&head=${owner}:${REF_NAME}&per_page=100"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# three attempts, no pipeline: see assert-gate-ran.sh for why the body goes to a
# file and not through `| jq`
attempt=1
while ! gh api "$path" >"$work/pulls.json"; do
  if [ "$attempt" -ge 3 ]; then
    echo "::warning::GET $path failed $attempt times; recording no base for this gate" >&2
    echo none
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 5
done

# the `head` filter already narrows the listing; the select keeps a fork's
# branch of the same name from being taken for this one
if ! jq -r --arg ref "$REF_NAME" --arg owner "$owner" \
  '[.[] | select(.head.ref == $ref and .head.repo.owner.login == $owner)] | first // {} |
     .base.sha // "none"' "$work/pulls.json" >"$work/base"; then
  echo "::warning::the pull request listing for ${REF_NAME} was not the expected json; recording no base for this gate" >&2
  echo none
  exit 0
fi
base=$(cat "$work/base")
if ! printf '%s' "$base" | grep -Eq '^[0-9a-f]{40}$'; then
  echo "::notice::no open pull request has ${REF_NAME} as its head; recording no base for this gate" >&2
  echo none
  exit 0
fi
echo "$base"
