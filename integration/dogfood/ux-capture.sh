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
# The dogfood stack is the default: ClickHouse is published on loopback and the
# account is the checked-in one. Neither holds for the compose team shape
# (docker-compose.yml + docker-compose.team.yml, #1890), so three things bend
# (#2794):
#
#   account     DEV_EMAIL / DEV_PASSWORD from the environment win. creds.env
#               only fills what the environment left unset, so the dogfood
#               stack needs no setup and a team stack takes your own login.
#               DEV_TOTP is a current code (or an unspent recovery code) for an
#               account that has a second factor
#   clickhouse  `--team` (or UX_COMPOSE, the docker compose command that
#               reaches the stack) runs every statement through
#               `docker compose exec clickhouse clickhouse-client`, which
#               authenticates with the container's own credentials, so nothing
#               needs to be published and no password passes through here.
#               Without it, CLICKHOUSE_URL is read over HTTP and may carry
#               credentials (`http://<user>:<password>@host:8123`)
#   control     ROLTER_CONTROL_URL, or in `--team` mode the address the env
#               file publishes the control plane on
#
#   DEV_EMAIL=you@example.com DEV_PASSWORD=... ./integration/dogfood/ux-capture.sh --team
#
# `--team` reads the stack's secrets from ROLTER_ENV_FILE (default: `.env` at the
# repository root), the same file `docker compose --env-file` is handed. Any
# other layout — a project name, an extra overlay — is UX_COMPOSE spelled out:
#
#   UX_COMPOSE='docker compose -p mine -f docker/docker-compose.yml' ./ux-capture.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"

usage() {
  cat <<EOF
usage: $(basename "$0") [--team] [apply-schema|verify|all]

  apply-schema  apply clickhouse/*.sql to the running server
  verify        prove the UX capture end to end with a probe event
  all           both, in that order (the default)

  --team        the stack is the compose team shape: ClickHouse is not
                published, so statements run through
                \`docker compose exec clickhouse clickhouse-client\`

environment: DEV_EMAIL, DEV_PASSWORD, DEV_TOTP, CLICKHOUSE_URL,
ROLTER_CONTROL_URL, ROLTER_ENV_FILE, UX_COMPOSE (see the top of this file)
EOF
}

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
fail() {
  printf '\033[31m✗ %s\033[0m\n' "$1" >&2
  exit 1
}
ok() { printf '\033[32m✓ %s\033[0m\n' "$1"; }

team=0
cmd=""
for arg in "$@"; do
  case "$arg" in
  --team) team=1 ;;
  -h | --help)
    usage
    exit 0
    ;;
  apply-schema | verify | all) cmd="$arg" ;;
  *)
    usage >&2
    exit 2
    ;;
  esac
done
cmd="${cmd:-all}"

# scratch for request bodies and curl configs. they hold the operator's password
# and session token, so they live in a 0700 directory instead of on argv, where
# any user of the host could read them out of `ps`
tmp="$(mktemp -d)"
cleanup() {
  # a session is a credential: end the one this run opened rather than leave the
  # operator's own account signed in for its full lifetime
  if [ -f "$tmp/auth.cfg" ]; then
    curl -s --max-time 5 -o /dev/null -X POST -K "$tmp/auth.cfg" \
      "${control:-}/api/v1/auth/logout" || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

# ---- where things are ------------------------------------------------------

# the docker compose command that reaches the stack, as an array. empty means
# ClickHouse is read over HTTP
compose=()
env_file=""
if [ -n "${UX_COMPOSE:-}" ]; then
  read -r -a compose <<<"$UX_COMPOSE"
elif [ "$team" = 1 ]; then
  env_file="${ROLTER_ENV_FILE:-$repo/.env}"
  [ -f "$env_file" ] ||
    fail "no env file at $env_file — the team stack's secrets live there. point ROLTER_ENV_FILE at the file you gave \`docker compose --env-file\`"
  compose=(docker compose
    -f "$repo/docker/docker-compose.yml"
    -f "$repo/docker/docker-compose.team.yml"
    --env-file "$env_file")
fi
if [ "${#compose[@]}" -gt 0 ]; then
  command -v "${compose[0]}" >/dev/null 2>&1 ||
    fail "${compose[0]} is not installed, and --team / UX_COMPOSE need it to reach ClickHouse"
fi

ch="${CLICKHOUSE_URL:-http://127.0.0.1:8123}"
ch="${ch%/}"
# the url as it may be printed: the credentials in it are a password
ch_shown="$(printf '%s' "$ch" | sed -E 's#(://)[^/@]*@#\1***@#')"

control="${ROLTER_CONTROL_URL:-}"
if [ -z "$control" ] && [ -n "$env_file" ]; then
  # the team shape publishes the control plane on ROLTER_CONTROL_HOST, which is
  # loopback unless the env file says otherwise. a bind-all address is not one
  # a client can dial
  host="$(sed -n 's/^ROLTER_CONTROL_HOST=//p' "$env_file" | tail -n 1 | tr -d "\"'")"
  case "$host" in
  "" | 0.0.0.0 | ::) host=127.0.0.1 ;;
  *:*) host="[$host]" ;;
  esac
  control="http://$host:4001"
fi
control="${control:-http://127.0.0.1:4001}"
control="${control%/}"

# ---- who signs in ----------------------------------------------------------

# creds.env is the dogfood stack's account, so it is a default and nothing more:
# whatever the caller already put in the environment is what they meant. it is
# sourced first and then overridden, never the other way round, because a plain
# `source` would overwrite the caller's value. per variable, so that a changed
# DEV_PASSWORD still pairs with the stack's own address
env_email="${DEV_EMAIL:-}"
env_password="${DEV_PASSWORD:-}"
# shellcheck source=/dev/null
. "$here/creds.env"
email_from=creds.env
password_from=creds.env
[ -n "$env_email" ] && email_from=the\ environment
[ -n "$env_password" ] && password_from=the\ environment
DEV_EMAIL="${env_email:-$DEV_EMAIL}"
DEV_PASSWORD="${env_password:-$DEV_PASSWORD}"
DEV_TOTP="${DEV_TOTP:-}"

# ---- clickhouse ------------------------------------------------------------

# one request to ClickHouse's HTTP interface: `ch_http <path> [curl args]`.
# the url, credentials included, goes in as a config on stdin so it is on neither
# argv nor a screen. a refusal prints ClickHouse's own words, because "returned
# error: 403" does not say the password was wrong
ch_http() {
  local path="$1" status
  shift
  status=$(curl -sS --max-time 30 -o "$tmp/ch.out" -w '%{http_code}' -K - "$@" \
    <<<"url = \"$ch$path\"") || return 1
  case "$status" in
  2??) cat "$tmp/ch.out" ;;
  *)
    printf 'ClickHouse answered %s: %s\n' "$status" "$(head -c 300 "$tmp/ch.out")" >&2
    return 1
    ;;
  esac
}

# `clickhouse-client` inside the service. in the team shape the container is
# started with CLICKHOUSE_USER and CLICKHOUSE_PASSWORD, which the client reads,
# so it signs in as the stack's own user without this host ever holding the
# password. the open stack sets neither and gets the passwordless default user
ch_client() {
  "${compose[@]}" exec -T clickhouse clickhouse-client "$@"
}

# one statement in, its tab-separated answer out
ch_sql() {
  if [ "${#compose[@]}" -gt 0 ]; then
    ch_client --query "$1"
  else
    ch_http / --data-binary "$1"
  fi
}

ch_describe() {
  if [ "${#compose[@]}" -gt 0 ]; then
    printf 'docker compose exec clickhouse'
  else
    printf '%s' "$ch_shown"
  fi
}

# what ClickHouse itself logged about refused ui_events inserts. the ingest's
# 500 body is deliberately generic (#1747), so the reason is only here, in
# system.query_log, which the compose stack keeps on purpose (#2795)
insert_errors() {
  local found
  ch_sql "system flush logs" >/dev/null 2>&1 || true
  found=$(ch_sql "select toString(event_time), exception from system.query_log \
where type in ('ExceptionBeforeStart', 'ExceptionWhileProcessing') \
and query_kind = 'Insert' and query like '%ui_events%' \
and event_time > now() - interval 5 minute order by event_time desc limit 3 FORMAT TSV" 2>&1) ||
    found="system.query_log could not be read: $found"
  [ -n "$found" ] || found="none logged in the last five minutes: the insert never reached ClickHouse, so look at the connection from the control plane (its CLICKHOUSE_URL)"
  printf '%s\n' "$found"
}

apply_schema() {
  bold "[ux] applying clickhouse/*.sql to $(ch_describe)"
  if [ "${#compose[@]}" -gt 0 ]; then
    ch_sql 'select 1' >/dev/null 2>&1 ||
      fail "\`${compose[*]} exec clickhouse\` did not answer — is the stack up? start it with: ${compose[*]} up -d clickhouse"
    # clickhouse-client takes a whole file, which is what the init entrypoint
    # these files are written for does too
    for sql in "$repo"/clickhouse/*.sql; do
      ch_client --multiquery <"$sql" ||
        fail "$(basename "$sql") was refused; see the error above"
      printf '  %s\n' "$(basename "$sql")"
    done
  else
    ch_http /ping >/dev/null 2>&1 ||
      fail "no ClickHouse at $ch_shown — start it with: docker compose -f docker/docker-compose.yml up -d clickhouse. on the compose team stack ClickHouse is not published: run this with --team"
    # split on `;` before posting: ClickHouse's HTTP interface takes one
    # statement per request ("Multi-statements are not allowed"), while the init
    # entrypoint these files are written for runs them through clickhouse-client,
    # which does not care. several of them hold two `alter table` statements
    for sql in "$repo"/clickhouse/*.sql; do
      CH_URL="$ch" python3 - "$sql" <<'PYEOF' || fail "$(basename "$sql") was refused; see the error above"
import base64, os, re, sys, urllib.error, urllib.parse, urllib.request

path = sys.argv[1]
# urllib does not read credentials out of a url the way curl does: split them
# off and send them as the header they are
parts = urllib.parse.urlsplit(os.environ["CH_URL"])
base = urllib.parse.urlunsplit(
    (parts.scheme, parts.netloc.rpartition("@")[2], parts.path.rstrip("/"), "", "")
)
headers = {}
if parts.username is not None:
    user = urllib.parse.unquote(parts.username)
    password = urllib.parse.unquote(parts.password or "")
    headers["Authorization"] = "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()

text = re.sub(r"--[^\n]*", "", open(path).read())
for statement in (s.strip() for s in text.split(";")):
    if not statement:
        continue
    try:
        urllib.request.urlopen(
            urllib.request.Request(f"{base}/", data=statement.encode(), headers=headers)
        )
    except urllib.error.HTTPError as err:
        print(f"{path}: {err.read().decode(errors='replace').strip()}", file=sys.stderr)
        sys.exit(1)
PYEOF
      printf '  %s\n' "$(basename "$sql")"
    done
  fi
  ok "schema applied"
}

# ---- the control plane -----------------------------------------------------

# `api_status <outfile> <curl args>`: the http status of a control-plane call,
# its body in <outfile>. the exit code is curl's, so a refused connection is
# told apart from an answer
api_status() {
  local out="$1"
  shift
  curl -sS --max-time 20 -o "$out" -w '%{http_code}' "$@"
}

# `kind secret` out of a login or step-up answer, without ever printing a body
# that holds a token
login_kind() {
  python3 - "$1" <<'PYEOF' 2>/dev/null || echo "unknown -"
import json, sys

body = json.load(open(sys.argv[1]))
if body.get("token"):
    print("session", body["token"])
elif body.get("mfa_required"):
    print("challenge", body.get("mfa_token", "-"))
elif body.get("mfa_enrolment_required"):
    print("enrol", "-")
else:
    print("unknown", "-")
PYEOF
}

sign_in() {
  local status answer kind secret hint
  # json from the environment rather than from string interpolation: a real
  # password holds quotes and backslashes, and neither belongs on argv
  DEV_EMAIL="$DEV_EMAIL" DEV_PASSWORD="$DEV_PASSWORD" python3 -c '
import json, os
print(json.dumps({"email": os.environ["DEV_EMAIL"], "password": os.environ["DEV_PASSWORD"]}))
' >"$tmp/login.json"

  status=$(api_status "$tmp/login.out" "$control/api/v1/auth/login" \
    -H 'content-type: application/json' --data-binary "@$tmp/login.json") ||
    fail "could not reach $control — is the control plane up? on the team stack it is published on ROLTER_CONTROL_HOST:4001; set ROLTER_CONTROL_URL if yours is elsewhere"
  case "$status" in
  200) ;;
  401)
    # creds.env is the dogfood stack's own account, so a refusal that leaned on it
    # is most likely a different stack, and a refusal that did not is a typo
    hint=""
    if [ "$email_from" = creds.env ] || [ "$password_from" = creds.env ]; then
      hint=" — creds.env is the dogfood stack's account; on any other stack pass your own: DEV_EMAIL=... DEV_PASSWORD=..."
    fi
    fail "$control refused $DEV_EMAIL (address from $email_from, password from $password_from)$hint"
    ;;
  429) fail "$control is throttling sign-ins for $DEV_EMAIL after failed attempts; wait and run it again" ;;
  *) fail "$control answered $status to the sign-in: $(head -c 300 "$tmp/login.out")" ;;
  esac

  answer=$(login_kind "$tmp/login.out")
  kind="${answer%% *}"
  secret="${answer#* }"
  case "$kind" in
  session) ;;
  challenge)
    [ -n "$DEV_TOTP" ] ||
      fail "$DEV_EMAIL has a second factor. pass a current code as DEV_TOTP=123456 (an authenticator code is accepted once, so a dashboard sign-in right after has to wait for the next one)"
    printf '{"mfa_token":"%s","code":"%s"}' "$secret" "$DEV_TOTP" >"$tmp/mfa.json"
    status=$(api_status "$tmp/login.out" "$control/api/v1/auth/mfa/verify" \
      -H 'content-type: application/json' --data-binary "@$tmp/mfa.json") ||
      fail "could not reach $control for the second-factor step"
    [ "$status" = 200 ] ||
      fail "$control refused the DEV_TOTP code ($status) — it is accepted once, and only inside its 30 second window"
    answer=$(login_kind "$tmp/login.out")
    secret="${answer#* }"
    [ "${answer%% *}" = session ] || fail "the second-factor step returned no session"
    ;;
  enrol)
    fail "$DEV_EMAIL has to enrol a second factor before it can sign in (the org requires one): do it once in the dashboard, then pass its code as DEV_TOTP"
    ;;
  *) fail "the sign-in answered 200 with no session in it" ;;
  esac

  # a curl config rather than a header on argv: this is the operator's own
  # session, and it is revoked again when the script exits
  printf 'header = "authorization: Bearer %s"\n' "$secret" >"$tmp/auth.cfg"
  ok "signed in as $DEV_EMAIL"
}

verify() {
  bold "[ux] proving the pipeline end to end ($control -> $(ch_describe))"
  printf '  account: %s (address from %s, password from %s)\n' \
    "$DEV_EMAIL" "$email_from" "$password_from"

  sign_in

  # a session id nothing else will ever use, so the read-back cannot pick up a
  # real interaction and call it a pass
  local session ts
  session="uxprobe-$(date +%s)-$RANDOM"
  ts=$(python3 -c 'import datetime; print(datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="milliseconds").replace("+00:00","Z"))')

  local status
  printf '{"events":[{"event_id":"%s","ts":"%s","screen":"ux-preflight","action":"screen_view","session_id":"%s"}]}' \
    "$session" "$ts" "$session" >"$tmp/probe.json"
  status=$(api_status "$tmp/probe.out" "$control/api/v1/ui-events" \
    -K "$tmp/auth.cfg" -H 'content-type: application/json' --data-binary "@$tmp/probe.json") ||
    fail "could not post the probe batch to $control"

  case "$status" in
  202) ok "the endpoint accepted the probe batch" ;;
  401 | 403)
    fail "the endpoint answered $status — the dashboard would disable its UX stream for the life of the tab"
    ;;
  404 | 405)
    fail "the endpoint answered $status — this control plane does not serve /api/v1/ui-events, and the dashboard would stop sending after one request"
    ;;
  500)
    printf '  clickhouse insert errors, last five minutes:\n' >&2
    insert_errors | sed 's/^/    /' >&2
    fail "the endpoint answered 500: $(cat "$tmp/probe.out")"
    ;;
  *) fail "the endpoint answered $status: $(cat "$tmp/probe.out")" ;;
  esac

  local rows
  rows=$(ch_sql "select count() from ui_events where session_id = '$session' FORMAT TSV") ||
    fail "could not read ui_events back through $(ch_describe)"
  if [ "$rows" != "1" ]; then
    # the endpoint answers 202 when UX events are switched off, too, and that is
    # the likelier cause on a stack someone has been tuning, so rule it out
    local switch
    switch=$(api_status "$tmp/logging.out" "$control/api/v1/logging-settings" \
      -K "$tmp/auth.cfg" 2>/dev/null) && [ "$switch" = 200 ] &&
      python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("ui_events") is False else 1)' \
        "$tmp/logging.out" 2>/dev/null &&
      fail "the batch was accepted and stored nothing: dashboard usage events are switched off. turn on Logs Settings -> Dashboard Usage Events (logging_settings.ui_events), then run this again"
    fail "the batch was accepted but $rows rows arrived — the endpoint answered 202 and the row is not in ClickHouse. is the control plane's CLICKHOUSE_URL the server read here?"
  fi
  ok "the probe row is in ClickHouse, with its own screen, action and ts"

  local total
  total=$(ch_sql "select count() from ui_events FORMAT TSV")
  printf '\n  ui_events holds %s rows. watch the capture with:\n\n' "$total"
  if [ "${#compose[@]}" -gt 0 ]; then
    printf '    %s exec clickhouse clickhouse-client --query \\\n' "${compose[*]}"
  else
    printf "    curl -s '%s/' --data-binary \\\\\n" "$ch_shown"
  fi
  printf "      \"select screen, action, count() c from ui_events \\\\\n"
  printf "        where ts > now() - interval 1 hour group by screen, action order by c desc FORMAT PrettyCompact\"\n\n"
}

case "$cmd" in
apply-schema) apply_schema ;;
verify) verify ;;
all)
  apply_schema
  verify
  ;;
esac
