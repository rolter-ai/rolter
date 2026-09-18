#!/usr/bin/env bash
# Prepare and prove the dashboard UX capture before a dogfood week (#1728).
#
# The capture is the point of the week: screen views, time-to-interactive, form
# abandons and error states, written to ClickHouse by the dashboard itself. Its
# failure mode is that there is no failure mode — `ui/src/lib/ux.ts` swallows
# every error by design, so a stack where the table is missing, the session has
# lapsed or ClickHouse is refusing connections looks *exactly* like a stack
# where nobody clicked anything. Nothing in the dashboard says otherwise.
#
# So the week does not start on a hope. This script does two things, and either
# one alone is worth running:
#
#   apply-schema  apply clickhouse/*.sql to the running server
#   verify        sign in, post a probe event through the real endpoint, and
#                 read it back out of ClickHouse
#
# `apply-schema` is not belt-and-braces. docker-compose mounts `clickhouse/`
# into ClickHouse's `docker-entrypoint-initdb.d`, and those scripts run **only
# when the data directory is first created**. Any machine whose `chdata` volume
# predates a migration has never seen it — a stack created before #805 landed
# has no `ui_events` table at all, so every batch is a 500 and the whole week
# captures nothing. Every file there is `create ... if not exists`, so applying
# them to a live server is a no-op wherever they already ran.
#
#   ./integration/dogfood/ux-capture.sh              # both, the normal call
#   ./integration/dogfood/ux-capture.sh apply-schema # just the schema
#   ./integration/dogfood/ux-capture.sh verify       # just the proof
#
# Reads CLICKHOUSE_URL and ROLTER_CONTROL_URL when set; loopback defaults
# otherwise. Credentials come from creds.env, like everything else here.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"

ch="${CLICKHOUSE_URL:-http://127.0.0.1:8123}"
ch="${ch%/}"
control="${ROLTER_CONTROL_URL:-http://127.0.0.1:4001}"
control="${control%/}"

set -a
# shellcheck source=/dev/null
. "$here/creds.env"
set +a

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
fail() {
  printf '\033[31m✗ %s\033[0m\n' "$1" >&2
  exit 1
}
ok() { printf '\033[32m✓ %s\033[0m\n' "$1"; }

apply_schema() {
  bold "[ux] applying clickhouse/*.sql to $ch"
  curl -fsS "$ch/ping" >/dev/null 2>&1 ||
    fail "no ClickHouse at $ch — start it with: docker compose -f docker/docker-compose.yml up -d clickhouse"
  # split on `;` before posting: ClickHouse's HTTP interface takes one
  # statement per request ("Multi-statements are not allowed"), while the init
  # entrypoint these files are written for runs them through clickhouse-client,
  # which does not care. several of them hold two `alter table` statements
  for sql in "$repo"/clickhouse/*.sql; do
    python3 - "$ch" "$sql" <<'PYEOF' || fail "$(basename "$sql") was refused; see the error above"
import re, sys, urllib.error, urllib.request

base, path = sys.argv[1], sys.argv[2]
text = re.sub(r"--[^\n]*", "", open(path).read())
for statement in (s.strip() for s in text.split(";")):
    if not statement:
        continue
    try:
        urllib.request.urlopen(urllib.request.Request(f"{base}/", data=statement.encode()))
    except urllib.error.HTTPError as err:
        print(f"{path}: {err.read().decode(errors='replace').strip()}", file=sys.stderr)
        sys.exit(1)
PYEOF
    printf '  %s\n' "$(basename "$sql")"
  done
  ok "schema applied"
}

verify() {
  bold "[ux] proving the pipeline end to end"

  local token
  token=$(curl -fsS "$control/api/v1/auth/login" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$DEV_EMAIL\",\"password\":\"$DEV_PASSWORD\"}" |
    python3 -c 'import json,sys; print(json.load(sys.stdin).get("token",""))') ||
    fail "could not sign in to $control — is the control plane up?"
  [ -n "$token" ] ||
    fail "login returned no token (an MFA challenge? this account must have no second factor)"
  ok "signed in as $DEV_EMAIL"

  # a session id nothing else will ever use, so the read-back cannot pick up a
  # real interaction and call it a pass
  local session ts
  session="uxprobe-$(date +%s)-$RANDOM"
  ts=$(python3 -c 'import datetime; print(datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="milliseconds").replace("+00:00","Z"))')

  local status
  status=$(curl -s -o /tmp/ux-probe.out -w '%{http_code}' "$control/api/v1/ui-events" \
    -H "authorization: Bearer $token" -H 'content-type: application/json' \
    -d "{\"events\":[{\"event_id\":\"$session\",\"ts\":\"$ts\",\"screen\":\"ux-preflight\",\"action\":\"screen_view\",\"session_id\":\"$session\"}]}")

  case "$status" in
  202) ok "the endpoint accepted the probe batch" ;;
  401 | 403)
    fail "the endpoint answered $status — the dashboard would disable its UX stream for the life of the tab"
    ;;
  404 | 405)
    fail "the endpoint answered $status — this control plane does not serve /api/v1/ui-events, and the dashboard would stop sending after one request"
    ;;
  *) fail "the endpoint answered $status: $(cat /tmp/ux-probe.out)" ;;
  esac

  local rows
  rows=$(curl -fsS "$ch/" --data-binary \
    "select count() from ui_events where session_id = '$session' FORMAT TSV") ||
    fail "could not read ui_events back from $ch"
  [ "$rows" = "1" ] ||
    fail "the batch was accepted but $rows rows arrived — the endpoint answered 202 and the row is not in ClickHouse"
  ok "the probe row is in ClickHouse, with its own screen, action and ts"

  local total
  total=$(curl -fsS "$ch/" --data-binary "select count() from ui_events FORMAT TSV")
  printf '\n  ui_events holds %s rows. watch the capture with:\n\n' "$total"
  printf "    curl -s '%s/' --data-binary \\\\\n" "$ch"
  printf "      \"select screen, action, count() c from ui_events \\\\\n"
  printf "        where ts > now() - interval 1 hour group by screen, action order by c desc FORMAT PrettyCompact\"\n\n"
}

case "${1:-all}" in
apply-schema) apply_schema ;;
verify) verify ;;
all)
  apply_schema
  verify
  ;;
*)
  echo "usage: $(basename "$0") [apply-schema|verify|all]" >&2
  exit 2
  ;;
esac
