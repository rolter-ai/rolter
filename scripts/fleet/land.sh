#!/usr/bin/env bash
# the land-a-PR loop in one command: is this pull request ready to merge, and
# optionally merge it and clean up after it.
#
#   scripts/fleet/land.sh [--merge] [--dry-run] [--require-up-to-date] <pr>
#
# Without --merge it only reads and prints one verdict line on stdout (details
# go to stderr), so it is safe to run on any pull request at any time:
#
#   PR #2417 READY: ci-ok green, 0 open threads, behind 3, CLEAN
#   PR #2417 PENDING: ci-ok pending
#   PR #2417 NOT READY: 2 unresolved review thread(s)
#
# exit codes: 0 ready (or already merged), 1 not ready, 2 pending, 3 usage or
# tool error. What it checks, in order:
#   - state and draft: a draft reports BLOCKED however green its checks are
#   - stack: a PR that targets another branch, or that other PRs target, is
#     merged with `merge-async` by hand (AGENTS.md), never from here
#   - behind: commits master has that the head lacks. master requires no
#     up-to-date branch, and it moves every few minutes, so this is reported
#     but does not block unless --require-up-to-date is given
#   - unresolved review threads (GraphQL): master requires every conversation
#     resolved, so one open CodeQL thread keeps a green PR BLOCKED
#   - the required check (`ci-ok`): any failing entry blocks, any pending one
#     waits, and so does any ci.yml run still in flight on the head sha, since
#     a title or body edit can leave a hollow green next to a running gate
#
# --merge squash-merges when the verdict is READY (it never waits), then
#   - closes the issues the PR body says it closes if the merge left them open
#     (no comment is posted; comments go out under Ilya's account)
#   - removes the PR's worktree with `wt remove` (no --force; a dirty worktree
#     stays) and deletes the local and remote branch, the local one only when
#     its tip is the merged head
# Every step checks state first, so re-running after a partial failure or on an
# already merged PR finishes the leftovers and changes nothing else. It never
# passes --delete-branch to the merge, so it cannot orphan a stacked child.
# --dry-run with --merge prints the steps and performs none of them.
#
# needs gh (authenticated), jq and git; wt for the worktree step.
# bash 3.2 compatible.
set -euo pipefail

repo=${ROLTER_REPO:-rolter-ai/rolter}
owner=${repo%%/*}
name=${repo##*/}
verify_tries=${LAND_VERIFY_TRIES:-5}
verify_sleep=${LAND_VERIFY_SLEEP:-2}

usage() {
  cat <<'USAGE'
usage: land.sh [--merge] [--dry-run] [--require-up-to-date] <pr>

  <pr>                    pull request number (or its url)
  --merge                 squash-merge when READY, close the issues it closes,
                          remove the worktree, delete the branch
  --dry-run               with --merge: print the steps, change nothing
  --require-up-to-date    a head behind master blocks instead of being noted

stdout: one verdict line. exit: 0 ready/merged, 1 not ready, 2 pending, 3 error.
USAGE
}

do_merge=0
dry_run=0
strict_behind=0
pr=""
for arg in "$@"; do
  case "$arg" in
    -h | --help)
      usage
      exit 0
      ;;
    --merge) do_merge=1 ;;
    --dry-run) dry_run=1 ;;
    --require-up-to-date) strict_behind=1 ;;
    -*)
      echo "land.sh: unknown option $arg" >&2
      usage >&2
      exit 3
      ;;
    *)
      [ -z "$pr" ] || {
        usage >&2
        exit 3
      }
      pr=${arg%/}
      pr=${pr##*/}
      pr=${pr#\#}
      ;;
  esac
done
case "$pr" in
  '' | *[!0-9]*)
    usage >&2
    exit 3
    ;;
esac

for tool in gh jq git; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "land.sh: $tool is required" >&2
    exit 3
  }
done

log() { printf 'land: %s\n' "$*" >&2; }

view_fields=number,state,isDraft,mergeable,mergeStateStatus,baseRefName,headRefName,headRefOid
view_fields=$view_fields,title,body,url,closingIssuesReferences,mergedAt,mergeCommit,autoMergeRequest
pr_json=$(gh pr view "$pr" -R "$repo" --json "$view_fields" </dev/null)
field() { jq -r "$1" <<<"$pr_json"; }

state=$(field .state)
head_ref=$(field .headRefName)
head_oid=$(field .headRefOid)
base_ref=$(field .baseRefName)
short=${head_oid:0:7}

blockers=""
waits=""
notes=""
block() { blockers="${blockers:+$blockers; }$1"; }
hold() { waits="${waits:+$waits; }$1"; }
note() { notes="${notes:+$notes, }$1"; }

# the issues the pull request closes: GitHub's own link list plus the keywords
# in the body, one number per line, sorted and unique
closing_issues() {
  {
    jq -r '.closingIssuesReferences[]?.number' <<<"$pr_json"
    jq -r '.body // ""' <<<"$pr_json" |
      grep -Eio '(close[sd]?|fix(e[sd])?|resolve[sd]?):? +#[0-9]+' |
      grep -Eo '[0-9]+$' || true
  } | sort -un
}

# ---- verdict ---------------------------------------------------------------
evaluate() {
  case "$state" in
    CLOSED)
      block "closed without merging"
      return
      ;;
    MERGED) return ;;
  esac

  if [ "$(field .isDraft)" = true ]; then
    block "draft (gh pr ready $pr)"
  fi

  default_branch=$(gh repo view "$repo" --json defaultBranchRef --jq .defaultBranchRef.name </dev/null)
  if [ "$base_ref" != "$default_branch" ]; then
    block "stacked on $base_ref: merge the parent first, then use merge-async (AGENTS.md)"
  fi
  children=$(gh pr list -R "$repo" --state open --base "$head_ref" --json number --jq 'map("#\(.number)") | join(" ")' </dev/null)
  if [ -n "$children" ]; then
    block "stack parent: $children target this branch, merge with merge-async by hand"
  fi

  # behind master. compared by sha so a fork head or a deleted branch still works
  behind=$(gh api "repos/$repo/compare/${default_branch}...${head_oid}" --jq .behind_by </dev/null)
  if [ "$behind" -gt 0 ]; then
    if [ "$strict_behind" -eq 1 ]; then
      block "behind $default_branch by $behind (rebase and push)"
    else
      note "behind $behind"
    fi
  else
    note "up to date"
  fi

  if [ "$(field .mergeable)" = CONFLICTING ] || [ "$(field .mergeStateStatus)" = DIRTY ]; then
    block "merge conflicts with $default_branch"
  fi

  # unresolved review threads, all pages
  # shellcheck disable=SC2016 # `$owner` and friends are graphql variables
  threads=$(gh api graphql --paginate -f query='
    query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
      repository(owner: $owner, name: $name) {
        pullRequest(number: $number) {
          reviewThreads(first: 100, after: $endCursor) {
            pageInfo { hasNextPage endCursor }
            nodes {
              isResolved
              path
              comments(first: 1) { nodes { author { login } url } }
            }
          }
        }
      }
    }' -f owner="$owner" -f name="$name" -F number="$pr" </dev/null |
    jq -s '[.[].data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved | not)]')
  open_threads=$(jq 'length' <<<"$threads")
  if [ "$open_threads" -gt 0 ]; then
    block "$open_threads unresolved review thread(s)"
    jq -r '.[] | "  thread: \(.path // "(no file)") by \(.comments.nodes[0].author.login // "?") \(.comments.nodes[0].url // "")"' <<<"$threads" >&2
  else
    note "0 open threads"
  fi

  # the required check. gh exits non-zero on a failing or pending check but
  # still prints the json, so the exit code is ignored on purpose
  checks=$(gh pr checks "$pr" -R "$repo" --required --json name,bucket </dev/null 2>/dev/null || true)
  if ! jq -e 'type == "array" and length > 0' >/dev/null 2>&1 <<<"$checks"; then
    hold "no required check reported yet"
  else
    failed=$(jq -r '[.[] | select(.bucket == "fail" or .bucket == "cancel") | .name] | unique | join(", ")' <<<"$checks")
    pending=$(jq -r '[.[] | select(.bucket == "pending") | .name] | unique | join(", ")' <<<"$checks")
    if [ -n "$failed" ]; then
      block "required check failed ($failed)"
    elif [ -n "$pending" ]; then
      hold "required check pending ($pending)"
    else
      note "ci-ok green"
    fi
  fi

  # a ci.yml run still going on this exact sha. an `edited` run waits for the
  # gate, and a green entry next to a running gate is the hollow green of #1328
  running=$(gh run list -R "$repo" --workflow ci.yml --commit "$head_oid" --limit 30 \
    --json databaseId,status --jq '[.[] | select(.status != "completed") | .databaseId] | join(" ")' </dev/null)
  if [ -n "$running" ]; then
    hold "ci run(s) $running still in progress on $short"
  fi

  if [ -z "$blockers" ] && [ -z "$waits" ]; then
    case "$(field .mergeStateStatus)" in
      CLEAN | UNSTABLE | HAS_HOOKS | BEHIND) note "$(field .mergeStateStatus)" ;;
      UNKNOWN) hold "mergeability still being computed" ;;
      *) block "$(field .mergeStateStatus) by branch protection with no thread or check to blame; see gh pr view $pr" ;;
    esac
  fi
}

verdict_line() {
  if [ "$state" = MERGED ]; then
    echo "PR #$pr MERGED $(jq -r '.mergeCommit.oid // "" | .[0:7]' <<<"$pr_json")"
    return 0
  fi
  if [ -n "$blockers" ]; then
    echo "PR #$pr NOT READY: $blockers${waits:+; waiting: $waits}"
    return 1
  fi
  if [ -n "$waits" ]; then
    echo "PR #$pr PENDING: $waits"
    return 2
  fi
  echo "PR #$pr READY: $notes"
  return 0
}

evaluate

if [ "$do_merge" -eq 0 ]; then
  rc=0
  verdict_line || rc=$?
  exit "$rc"
fi

# ---- merge and clean up ----------------------------------------------------
if [ "$state" != MERGED ]; then
  rc=0
  line=$(verdict_line) || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "$line"
    log "not merging: $line"
    exit "$rc"
  fi
  if [ "$dry_run" -eq 1 ]; then
    log "would squash-merge #$pr ($line)"
  else
    gh pr merge "$pr" -R "$repo" --squash </dev/null >&2
    tries=0
    while :; do
      state=$(gh pr view "$pr" -R "$repo" --json state --jq .state </dev/null)
      [ "$state" = MERGED ] && break
      tries=$((tries + 1))
      if [ "$tries" -ge "$verify_tries" ]; then
        echo "PR #$pr ENQUEUED: merge requested but the PR is still $state (merge queue or auto-merge); re-run to finish cleanup"
        exit 2
      fi
      sleep "$verify_sleep"
    done
    pr_json=$(gh pr view "$pr" -R "$repo" --json "$view_fields" </dev/null)
    log "merged #$pr"
  fi
fi

summary=""

# close what the pull request says it closes, if the merge left it open
for issue in $(closing_issues); do
  issue_state=$(gh issue view "$issue" -R "$repo" --json state --jq .state </dev/null 2>/dev/null || echo UNKNOWN)
  if [ "$issue_state" = OPEN ]; then
    if [ "$dry_run" -eq 1 ]; then
      log "would close #$issue"
    else
      gh issue close "$issue" -R "$repo" --reason completed >/dev/null </dev/null
      log "closed #$issue"
    fi
    summary="$summary closed #$issue;"
  fi
done

# the worktree and the branch. the checkout this script runs in may be the
# worktree being removed, so everything below runs from the main checkout
common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
main=""
[ -z "$common" ] || main=$(dirname "$common")
if [ -z "$main" ] || [ ! -d "$main" ]; then
  log "not inside a git checkout of the repository: skipping worktree and branch cleanup"
else
  wt_path=$(git -C "$main" worktree list --porcelain | awk -v ref="refs/heads/$head_ref" '
    /^worktree / { path = substr($0, 10) }
    $0 == "branch " ref { print path; exit }')
  if [ -n "$wt_path" ] && [ "$wt_path" != "$main" ]; then
    if [ "$dry_run" -eq 1 ]; then
      log "would remove worktree $wt_path"
    else
      removed=0
      if command -v wt >/dev/null 2>&1; then
        wt -C "$main" remove --foreground "$head_ref" >&2 && removed=1
      else
        git -C "$main" worktree remove "$wt_path" >&2 && removed=1
      fi
      if [ "$removed" -eq 1 ]; then
        summary="$summary removed worktree $wt_path;"
      else
        log "worktree $wt_path not removed (uncommitted changes?); left in place"
      fi
    fi
  fi

  if git -C "$main" show-ref --verify --quiet "refs/heads/$head_ref"; then
    local_tip=$(git -C "$main" rev-parse "refs/heads/$head_ref")
    if [ "$local_tip" != "$head_oid" ]; then
      log "local branch $head_ref is at ${local_tip:0:7}, not the merged head $short: kept"
    elif [ "$dry_run" -eq 1 ]; then
      log "would delete local branch $head_ref"
    elif git -C "$main" branch -D "$head_ref" >&2; then
      summary="$summary deleted local branch;"
    else
      log "local branch $head_ref is still checked out somewhere: kept"
    fi
  fi

  # the repo deletes the head branch on merge; this only catches what that missed
  if git -C "$main" ls-remote --exit-code --heads origin "$head_ref" >/dev/null 2>&1; then
    if [ "$dry_run" -eq 1 ]; then
      log "would delete remote branch $head_ref"
    elif gh api -X DELETE "repos/$repo/git/refs/heads/$head_ref" >/dev/null </dev/null; then
      summary="$summary deleted remote branch;"
    fi
  fi
  git -C "$main" fetch origin "$base_ref" --quiet >&2 || true
fi

echo "PR #$pr MERGED $(jq -r '.mergeCommit.oid // "" | .[0:7]' <<<"$pr_json")${summary:+ |${summary%;}}"
