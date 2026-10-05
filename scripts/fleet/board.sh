#!/usr/bin/env bash
# put an issue or pull request on the rolter board (org project #1) and set its
# Status, Priority, Effort and Area fields in one call.
#
#   scripts/fleet/board.sh <issue-or-pr-url> <Priority> <Effort> <Area> [Status]
#
# Status defaults to Backlog. Field and option ids are looked up by name from
# the live project, so a renamed or re-created option never leaves a stale id
# behind; an unknown value fails with the list of valid ones. Safe to re-run:
# `gh project item-add` returns the existing item for an issue already on the
# board, and setting a field to the value it already has changes nothing.
#
# the board automation seeds Todo/Medium a few seconds after an issue is
# created and can overwrite an edit made too early (see issue-tracking.md), so
# the fields are read back after writing and rewritten a few times if they did
# not stick.
#
# needs gh (authenticated, `project` scope) and jq. bash 3.2 compatible.
set -euo pipefail

owner=${ROLTER_BOARD_OWNER:-rolter-ai}
project=${ROLTER_BOARD_NUMBER:-1}
retries=${BOARD_RETRIES:-3}
retry_sleep=${BOARD_RETRY_SLEEP:-10}

usage() {
  cat <<'USAGE'
usage: board.sh [--dry-run] <issue-or-pr-url> <Priority> <Effort> <Area> [Status]

  Priority  Urgent | High | Medium | Low
  Effort    XS | S | M | L | XL
  Area      gateway | control | ui | proxy | balancer | store | auth | core |
            docs | ci | infra | cross-cutting
  Status    Backlog (default) | Todo | In Progress | In Review | Done | Canceled

values are matched case-insensitively against the live board options.
--dry-run only reads (field list, project id) and prints what it would write.
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
if [ "${#args[@]}" -lt 4 ] || [ "${#args[@]}" -gt 5 ]; then
  usage >&2
  exit 3
fi
url=${args[0]}
want_priority=${args[1]}
want_effort=${args[2]}
want_area=${args[3]}
want_status=${args[4]:-Backlog}

case "$url" in
  https://github.com/*/issues/* | https://github.com/*/pull/*) ;;
  *)
    echo "board.sh: '$url' is not an issue or pull request url" >&2
    exit 3
    ;;
esac

for tool in gh jq; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "board.sh: $tool is required" >&2
    exit 3
  }
done

fields=$(gh project field-list "$project" --owner "$owner" --limit 100 --format json </dev/null)

# resolve FIELD OPTION: prints "<field-id> <option-id> <canonical option name>"
resolve() {
  local field=$1 option=$2 line
  line=$(jq -r --arg f "$field" --arg o "$option" '
    [.fields[] | select(.name == $f) | . as $fld | .options[]?
      | select((.name | ascii_downcase) == ($o | ascii_downcase))
      | "\($fld.id) \(.id) \(.name)"] | first // empty' <<<"$fields")
  if [ -z "$line" ]; then
    local valid
    valid=$(jq -r --arg f "$field" '[.fields[] | select(.name == $f) | .options[]?.name] | join(", ")' <<<"$fields")
    echo "board.sh: '$option' is not a $field option (valid: ${valid:-none, field missing})" >&2
    exit 3
  fi
  printf '%s\n' "$line"
}

# a plain assignment keeps errexit, so an unknown value stops the script here
# (`read <<<"$(resolve ...)"` would swallow the failed substitution)
resolved=$(resolve Status "$want_status")
read -r status_field status_opt status_name <<<"$resolved"
resolved=$(resolve Priority "$want_priority")
read -r priority_field priority_opt priority_name <<<"$resolved"
resolved=$(resolve Effort "$want_effort")
read -r effort_field effort_opt effort_name <<<"$resolved"
resolved=$(resolve Area "$want_area")
read -r area_field area_opt area_name <<<"$resolved"

project_id=$(gh project view "$project" --owner "$owner" --format json --jq .id </dev/null)

if [ "$dry_run" -eq 1 ]; then
  echo "board dry-run: $url -> $status_name/$priority_name/$effort_name/$area_name (project $project_id)"
  exit 0
fi

item=$(gh project item-add "$project" --owner "$owner" --url "$url" --format json --jq .id </dev/null)

set_field() {
  gh project item-edit --id "$item" --project-id "$project_id" \
    --field-id "$1" --single-select-option-id "$2" >/dev/null </dev/null
}

write_all() {
  set_field "$status_field" "$status_opt"
  set_field "$priority_field" "$priority_opt"
  set_field "$effort_field" "$effort_opt"
  set_field "$area_field" "$area_opt"
}

# the item's current single-select values as "Field=Value" lines
read_back() {
  # shellcheck disable=SC2016 # `$id` is a graphql variable, not a shell expansion
  gh api graphql -f query='
    query($id: ID!) {
      node(id: $id) {
        ... on ProjectV2Item {
          fieldValues(first: 30) {
            nodes {
              ... on ProjectV2ItemFieldSingleSelectValue {
                name
                field { ... on ProjectV2FieldCommon { name } }
              }
            }
          }
        }
      }
    }' -f id="$item" </dev/null |
    jq -r '.data.node.fieldValues.nodes[] | select(.field != null) | "\(.field.name)=\(.name)"'
}

attempt=1
while :; do
  write_all
  current=$(read_back)
  missing=""
  for pair in "Status=$status_name" "Priority=$priority_name" "Effort=$effort_name" "Area=$area_name"; do
    printf '%s\n' "$current" | grep -Fxq -- "$pair" || missing="$missing $pair"
  done
  [ -z "$missing" ] && break
  if [ "$attempt" -ge "$retries" ]; then
    echo "board.sh: fields did not stick after $attempt attempts, still wrong:$missing" >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep "$retry_sleep"
done

echo "board ok: $url $status_name/$priority_name/$effort_name/$area_name"
