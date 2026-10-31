#!/usr/bin/env bash
# opens, or comments on, the tracking issue of a workflow whose nightly run
# failed. the one implementation behind .github/workflows/report-failure.yml,
# which extended.yml, ui-e2e.yml and sso-e2e.yml call (#2682).
#
#   GH_TOKEN=... GH_REPO=rolter-ai/rolter RUN_URL=<url> REF=master SHA=<sha> \
#   TITLE='extended.yml: nightly checks failing' AREA=ci EFFORT=XS \
#   RESULTS=$'macos check|failure\nmsrv build|success' bash scripts/report-failure.sh
#
# RESULTS is one `label|result` per line, rendered as the issue's table. NOTE
# (optional) is a sentence appended under it; MILESTONE defaults to
# "Maintenance, CI & DX".
#
# the first failure opens an issue titled TITLE, and every failure while it
# stays open comments on it, so a red night is seen rather than left in the
# Actions tab. close the issue once the fix lands; the next failure opens a
# fresh one.
#
# an issue this token opens raises no `issues` event, so project-automation never
# sees it (#2201). a new issue is therefore triaged here: the milestone is set
# and project-automation.yml is dispatched, which is the one event the token may
# start, for the board item and its fields. both are best-effort and only warn:
# the issue is what matters, and a renamed milestone must not cost it.
#
# every value arrives through env and is passed to `gh` as an argument, never
# interpolated into a command line. bash 3.2 compatible, so the fake-gh test
# (scripts/test-report-failure.sh) runs on a stock mac too.
set -euo pipefail

: "${GH_REPO:?GH_REPO must name the repository, e.g. rolter-ai/rolter}"
: "${RUN_URL:?RUN_URL must be the url of the failing run}"
: "${REF:?REF must be the ref the run was on}"
: "${SHA:?SHA must be the commit the run was on}"
: "${TITLE:?TITLE must be the tracking issue title}"
: "${RESULTS:?RESULTS must hold one 'label|result' line per job}"
: "${AREA:?AREA must be the project-automation area, e.g. ci}"
: "${EFFORT:?EFFORT must be the project-automation effort, e.g. XS}"
milestone="${MILESTONE:-Maintenance, CI & DX}"
note="${NOTE:-}"

table="| job | result |"$'\n'"| --- | --- |"
while IFS= read -r row; do
  [ -n "$row" ] || continue
  table="${table}"$'\n'"| ${row%%|*} | ${row#*|} |"
done <<<"$RESULTS"

body="$(printf '%s\n' \
  "[Run]($RUN_URL) on \`$REF\` at \`$SHA\`:" \
  "" \
  "$table")"
if [ -n "$note" ]; then
  body="$(printf '%s\n\n%s\n' "$body" "$note")"
fi

# the list and the jq filter are two commands, not a pipeline, so a failing
# `gh issue list` is a failed run and never "no issue open yet" (which would
# open a duplicate on every flaky listing)
open_issues="$(gh issue list --state open --label ci --limit 200 --json number,title)"
number="$(jq -r --arg t "$TITLE" 'map(select(.title == $t)) | .[0].number // empty' <<<"$open_issues")"

summary="${GITHUB_STEP_SUMMARY:-/dev/null}"
if [ -n "$number" ]; then
  gh issue comment "$number" --body "$body"
  echo "commented on #$number" >>"$summary"
  exit 0
fi

url="$(gh issue create --title "$TITLE" --label ci \
  --body "$(printf '%s\n\n%s\n' "$body" "Opened by the \`report failure\` job. Close it once the failing checks are fixed; the next failure opens a new one.")")"
echo "opened $url" >>"$summary"
number="${url##*/}"
if ! gh issue edit "$number" --milestone "$milestone"; then
  echo "::warning title=report failure::could not set the milestone of #$number to $milestone; set it by hand"
fi
if ! gh workflow run project-automation.yml --ref "$REF" \
  -f number="$number" -f area="$AREA" -f effort="$EFFORT"; then
  echo "::warning title=report failure::could not dispatch project-automation for #$number; add it by hand: gh workflow run project-automation.yml -f number=$number -f area=$AREA -f effort=$EFFORT"
fi
