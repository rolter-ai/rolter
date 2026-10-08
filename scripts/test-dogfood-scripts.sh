#!/usr/bin/env bash
# runs the dogfood shell scripts in integration/dogfood/ against a stub http
# server and checks their exit codes and what they tell the operator (#1928).
#
# the scripts' exit codes are a contract, and nothing else exercised them:
# provision-signoz.sh's code decides whether the operator is pointed at
# `just signoz-reset` (exit 1, an account mismatch, which deletes SigNoz's
# users, dashboards and alerts) or told the SigNoz release moved its api
# (exit 2, nothing changes), and a misclassification there was only ever found
# by reading the code (#1792, #1892). adaptive-routing.sh must fail loudly on a
# refused call instead of printing a success line (#1817), and ux-capture.sh must
# take the operator's own account and reach a ClickHouse that is not published
# (#2794). no SigNoz, no docker and no control plane is needed: the stub plays
# the answers from a scenario file, one line per route, rewritten between
# cases. it runs as a step of quality.yml's `static checks` job and as a prek
# hook.
#
# adding a script: write a `cases_<name>` function below that sets up scenarios
# and calls `expect`, then add one `run_cases <name>` line at the bottom.
#
# needs python3 and curl. bash 3.2 compatible, so it runs on a stock mac too.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dogfood="$root/integration/dogfood"

work=$(mktemp -d)
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

scenario="$work/scenario"
requests="$work/requests"
: >"$scenario"
: >"$requests"

# ── the stub server ───────────────────────────────────────────────────────────
# the scenario file holds one route per line, `METHOD|PATH|STATUS|CONTENT-TYPE|BODY`,
# matched on method and path (the query string is ignored), first match wins and
# an unmatched route answers 200 text/html, which is what SigNoz does for any
# path that is not an api route (its single page app). the file is read on every
# request, so a case changes the answers by rewriting it. each request is
# appended to the request log as `METHOD PATH auth=<authorization header> body=<request body>`.
cat >"$work/stub.py" <<'PY'
import http.server
import sys

scenario_path, request_log, port_file = sys.argv[1:4]
SPA = "<!doctype html><html><body><div id=root></div></body></html>"


class Handler(http.server.BaseHTTPRequestHandler):
    def handle_any(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length).decode() if length else ""
        path = self.path.split("?", 1)[0]
        with open(request_log, "a") as log:
            auth = self.headers.get("authorization", "")
            log.write(f"{self.command} {path} auth={auth} body={body}\n")
        status, ctype, payload = 200, "text/html", SPA
        with open(scenario_path) as routes:
            for line in routes:
                line = line.rstrip("\n")
                if not line:
                    continue
                method, route, code, kind, text = line.split("|", 4)
                if method == self.command and route == path:
                    status, ctype, payload = int(code), kind, text
                    break
        data = payload.encode()
        self.send_response(status)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    do_GET = do_POST = do_PUT = do_DELETE = handle_any

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as out:
    out.write(str(server.server_address[1]))
server.serve_forever()
PY

python3 "$work/stub.py" "$scenario" "$requests" "$work/port" &
server_pid=$!
for _ in $(seq 1 100); do
  [ -s "$work/port" ] && break
  sleep 0.1
done
if [ ! -s "$work/port" ]; then
  echo "FAIL the stub server did not start" >&2
  exit 1
fi
stub="http://127.0.0.1:$(cat "$work/port")"

# ── harness ───────────────────────────────────────────────────────────────────
failures=0
checks=0

# route METHOD PATH STATUS CONTENT-TYPE BODY: add an answer to the scenario
route() {
  printf '%s|%s|%s|%s|%s\n' "$1" "$2" "$3" "$4" "$5" >>"$scenario"
}
json() { route "$1" "$2" "$3" application/json "$4"; }
reset() {
  : >"$scenario"
  : >"$requests"
}

# expect DESCRIPTION WANT-EXIT [grep -F needle | !needle ...] -- COMMAND...
# runs COMMAND, checks its exit code, and checks each needle against its stdout
# and stderr together: a plain needle must be there, one starting with `!` must
# not be. the command's output is kept in $out for the caller's own checks
out=""
expect() {
  local desc=$1 want=$2 needle got
  shift 2
  local needles=()
  while [ "$1" != "--" ]; do
    needles+=("$1")
    shift
  done
  shift
  checks=$((checks + 1))
  got=0
  out=$("$@" 2>&1) || got=$?
  if [ "$got" != "$want" ]; then
    echo "FAIL [$desc] exit $got, wanted $want" >&2
    printf '%s\n' "$out" | sed 's/^/    | /' >&2
    failures=$((failures + 1))
    return
  fi
  for needle in ${needles[@]+"${needles[@]}"}; do
    case "$needle" in
      '!'*)
        if printf '%s' "$out" | grep -qF -- "${needle#!}"; then
          echo "FAIL [$desc] output has \"${needle#!}\"" >&2
          printf '%s\n' "$out" | sed 's/^/    | /' >&2
          failures=$((failures + 1))
        fi
        ;;
      *)
        if ! printf '%s' "$out" | grep -qF -- "$needle"; then
          echo "FAIL [$desc] output lacks \"$needle\"" >&2
          printf '%s\n' "$out" | sed 's/^/    | /' >&2
          failures=$((failures + 1))
        fi
        ;;
    esac
  done
}

# requested NEEDLE: whether the request log has a line containing it
requested() { grep -qF -- "$1" "$requests"; }
not_requested() { ! requested "$1"; }
nothing_requested() { [ ! -s "$requests" ]; }

# check DESCRIPTION CONDITION-COMMAND...: a check on the request log
check() {
  local desc=$1
  shift
  checks=$((checks + 1))
  if ! "$@"; then
    echo "FAIL [$desc]" >&2
    cut -c1-160 "$requests" | sed 's/^/    | /' >&2
    failures=$((failures + 1))
  fi
}

run_cases() {
  echo "== $1"
  reset
  "cases_$1"
}

# ── provision-signoz.sh ───────────────────────────────────────────────────────
# the exit-code table of the script's header. SIGNOZ_VERSION is what the stub's
# /api/v1/version reports and what the exit 2 message must name.
SIGNOZ_VERSION=v0.136.0

# the titles the checked-in boards go by, one per line
board_titles() {
  python3 -c '
import glob, json, sys
for path in sorted(glob.glob(sys.argv[1] + "/signoz/dashboards/*.json")):
    print(json.load(open(path))["title"])' "$dogfood"
}

# signoz_base SETUP-COMPLETED: version and every route a signed-in run needs
signoz_base() {
  json GET /api/v1/version 200 "{\"version\":\"$SIGNOZ_VERSION\",\"setupCompleted\":$1}"
  json GET /api/v2/sessions/context 200 '{"status":"success","data":{"exists":true,"orgs":[{"id":"org-1"}]}}'
  json POST /api/v2/sessions/email_password 200 '{"status":"success","data":{"accessToken":"tok-1"}}'
  json GET /api/v2/dashboards 200 '{"status":"success","data":{"dashboards":[],"total":0}}'
  json POST /api/v2/dashboards 200 '{"status":"success","data":{}}'
}

provision() {
  SIGNOZ_URL="$stub" bash "$dogfood/provision-signoz.sh"
}

# a case overrides one answer by adding it before calling signoz_base: routes
# are first-match, so the override shadows signoz_base's own line
cases_provision_signoz() {
  local kept status

  reset
  json POST /api/v1/register 200 '{"status":"success","data":{}}'
  signoz_base false
  expect "fresh instance registers, signs in and imports the boards" 0 \
    "registered" "signed in via /api/v2/sessions/email_password" "3 imported, 0 already present, 0 failed" -- provision
  check "fresh instance registered once" requested "POST /api/v1/register"
  check "boards were created with the session token" requested "POST /api/v2/dashboards auth=Bearer tok-1"

  reset
  kept=$(board_titles | python3 -c '
import json, sys
titles = [t.strip() for t in sys.stdin if t.strip()]
print(json.dumps({"status": "success", "data": {"total": len(titles),
    "dashboards": [{"spec": {"display": {"name": t}}} for t in titles]}}))')
  json GET /api/v2/dashboards 200 "$kept"
  signoz_base true
  expect "re-run with every board present changes nothing" 0 \
    "0 imported, 3 already present, 0 failed" -- provision
  check "re-run did not create a board" not_requested "POST /api/v2/dashboards"

  reset
  json POST /api/v2/sessions/email_password 401 '{"status":"error","error":{"message":"invalid password"}}'
  signoz_base true
  expect "401 json on sign-in is an account mismatch" 1 \
    "could not sign in to SigNoz $SIGNOZ_VERSION" "invalid password" "just signoz-reset" -- provision

  reset
  json GET /api/v2/sessions/context 200 '{"status":"success","data":{"exists":false}}'
  signoz_base true
  expect "no account for the email is an account mismatch" 1 \
    "SigNoz has no account for" "just signoz-reset" -- provision
  check "no sign-in was attempted without an account" not_requested "email_password"

  for status in 500 502 503; do
    reset
    json POST /api/v2/sessions/email_password "$status" '{"status":"error","error":{"message":"boom"}}'
    signoz_base true
    expect "$status json on sign-in is SigNoz failing, not a mismatch" 1 \
      "sign-in failed" "answered $status" "!just signoz-reset" "!could not sign in to SigNoz" -- provision
  done

  reset
  route POST /api/v2/sessions/email_password 200 text/html '<!doctype html><div id=root></div>'
  signoz_base true
  expect "the SPA on sign-in is an unknown api" 2 \
    "SigNoz $SIGNOZ_VERSION is not a version this script knows" "with the SPA" "!just signoz-reset" -- provision

  reset
  json POST /api/v2/sessions/email_password 200 '{"status":"success","data":{"token":"moved"}}'
  signoz_base true
  expect "a 2xx without data.accessToken is an unknown api" 2 \
    "SigNoz $SIGNOZ_VERSION is not a version this script knows" "with no" "data.accessToken" "!just signoz-reset" -- provision

  for status in 404 405 501; do
    reset
    json POST /api/v2/sessions/email_password "$status" '{"status":"error","error":{"message":"route moved"}}'
    signoz_base true
    expect "$status json on sign-in is an unknown api" 2 \
      "SigNoz $SIGNOZ_VERSION is not a version this script knows" "answered $status" "!just signoz-reset" -- provision
  done

  reset
  json GET /api/v2/sessions/context 200 '{"status":"success","data":{"orgs":[{"id":"org-1"}]}}'
  signoz_base true
  expect "a session context without data.exists is an unknown api" 2 \
    "SigNoz $SIGNOZ_VERSION is not a version this script knows" "data.exists" "!just signoz-reset" -- provision

  reset
  json GET /api/v2/sessions/context 200 '{"status":"success","data":{"exists":true,"orgs":[]}}'
  signoz_base true
  expect "a session context without an organisation is an unknown api" 2 \
    "SigNoz $SIGNOZ_VERSION is not a version this script knows" "names no" "!just signoz-reset" -- provision

  reset
  json GET /api/v1/version 200 "{\"version\":\"$SIGNOZ_VERSION\",\"setupCompleted\":true}"
  expect "the SPA on every route is an unknown api" 2 \
    "SigNoz $SIGNOZ_VERSION is not a version this script knows" "No sign-in route answered" "!just signoz-reset" -- provision

  reset
  # first match wins, so the failing create goes in before signoz_base's own
  json POST /api/v2/dashboards 500 '{"status":"error","error":{"message":"clickhouse is down"}}'
  signoz_base true
  expect "a board that fails to import is exit 1" 1 \
    "could not import" "clickhouse is down" "0 imported, 0 already present, 3 failed" -- provision

  # the script waits up to 3 minutes for SigNoz to come up; a sleep that returns
  # at once keeps this case quick
  mkdir -p "$work/fast-sleep"
  printf '#!/bin/sh\nexit 0\n' >"$work/fast-sleep/sleep"
  chmod +x "$work/fast-sleep/sleep"
  reset
  expect "an unreachable SigNoz is exit 1" 1 "not reachable" -- \
    env SIGNOZ_URL=http://127.0.0.1:9 PATH="$work/fast-sleep:$PATH" bash "$dogfood/provision-signoz.sh"
}

# ── adaptive-routing.sh ───────────────────────────────────────────────────────
POLICY='{"enabled":false,"latency_weight":0.5,"cost_weight":0.3,"load_weight":0.2,"exploration_ratio":0.05,"min_samples":20,"affected_routes":["deepseek-r1"]}'
POLICY_ON='{"enabled":true,"latency_weight":0.5,"cost_weight":0.3,"load_weight":0.2,"exploration_ratio":0.05,"min_samples":20,"affected_routes":["deepseek-r1"]}'

adaptive() {
  ROLTER_CONTROL_URL="$stub" ROLTER_ADMIN_TOKEN=admin-tok CONTROL_WAIT_SECS=2 \
    bash "$dogfood/adaptive-routing.sh" "$@"
}

cases_adaptive_routing() {
  reset
  json GET /api/v1/adaptive-routing-policy 200 "$POLICY"
  json PUT /api/v1/adaptive-routing-policy 200 "$POLICY_ON"
  expect "on flips the switch" 0 "adaptive routing on" "governs: deepseek-r1" -- adaptive on
  check "the policy was read with the admin token" requested "GET /api/v1/adaptive-routing-policy auth=Bearer admin-tok"
  check "only enabled changed on the way back" requested '"min_samples": 20'
  check "on sends enabled true" requested '"enabled": true'

  reset
  json GET /api/v1/adaptive-routing-policy 200 "$POLICY_ON"
  json PUT /api/v1/adaptive-routing-policy 200 "$POLICY"
  expect "off flips the switch" 0 "adaptive routing off" -- adaptive off
  check "off sends enabled false" requested '"enabled": false'

  reset
  json GET /api/v1/adaptive-routing-policy 200 "$POLICY"
  json PUT /api/v1/adaptive-routing-policy 200 "$POLICY_ON"
  expect "the mode defaults to on" 0 "adaptive routing on" -- adaptive

  reset
  json GET /api/v1/adaptive-routing-policy 200 "$POLICY"
  expect "an unknown mode is a usage error" 2 "usage:" -- adaptive sideways
  check "an unknown mode makes no request" nothing_requested

  reset
  json GET /api/v1/adaptive-routing-policy 403 '{"error":{"message":"forbidden"}}'
  expect "403 reading the policy is exit 1" 1 "GET" "answered 403" "forbidden" -- adaptive on
  check "a refused read does not write" not_requested "PUT "

  reset
  json GET /api/v1/adaptive-routing-policy 401 '{"error":{"message":"bad token"}}'
  expect "401 reading the policy is exit 1" 1 "answered 401" -- adaptive on

  reset
  json GET /api/v1/adaptive-routing-policy 500 '{"error":{"message":"boom"}}'
  expect "500 reading the policy is exit 1" 1 "answered 500" -- adaptive on

  reset
  json GET /api/v1/adaptive-routing-policy 200 "$POLICY"
  json PUT /api/v1/adaptive-routing-policy 403 '{"error":{"message":"viewer cannot write"}}'
  expect "403 writing the policy is exit 1, not a success line" 1 \
    "PUT" "answered 403" "viewer cannot write" "!adaptive routing on" -- adaptive on

  reset
  json GET /api/v1/adaptive-routing-policy 200 "$POLICY"
  json PUT /api/v1/adaptive-routing-policy 500 '{"error":{"message":"boom"}}'
  expect "500 writing the policy is exit 1" 1 "answered 500" "!adaptive routing on" -- adaptive on

  reset
  expect "nothing listening is exit 1" 1 "no answer from" -- \
    env ROLTER_CONTROL_URL=http://127.0.0.1:9 ROLTER_ADMIN_TOKEN=admin-tok CONTROL_WAIT_SECS=1 \
      bash "$dogfood/adaptive-routing.sh" on
}

# ── ux-capture.sh ─────────────────────────────────────────────────────────────
# the stub plays the control plane and a ClickHouse at once: its POST / is
# ClickHouse's http interface, so every statement is answered with one scenario
# line, which is why the read-back below is "1" for the probe row and the total
# alike. a fake `docker` on PATH plays `docker compose exec clickhouse
# clickhouse-client` for the team shape, where ClickHouse is not published (#2794)
mkdir -p "$work/fake-docker"
cat >"$work/fake-docker/docker" <<'SH'
#!/bin/sh
# records each call, swallows the sql file a --multiquery call is fed, and
# answers a count query with $FAKE_DOCKER_ROWS (the probe row) or 42 (the total)
printf '%s\n' "$*" >>"$FAKE_DOCKER_LOG"
case "$*" in
*--multiquery*) cat >/dev/null ;;
esac
case "$*" in
*"where session_id"*) printf '%s\n' "${FAKE_DOCKER_ROWS:-1}" ;;
*"select count() from ui_events"*) printf '42\n' ;;
esac
SH
chmod +x "$work/fake-docker/docker"
docker_log="$work/docker.log"
: >"$docker_log"

# extra VAR=value pairs for the next run, on top of a hermetic environment: the
# account, the team-shape knobs and the urls are the caller's to set
ux_env=()
ux() {
  env -u DEV_EMAIL -u DEV_PASSWORD -u DEV_TOTP -u UX_COMPOSE -u ROLTER_ENV_FILE \
    CLICKHOUSE_URL="$stub" ROLTER_CONTROL_URL="$stub" FAKE_DOCKER_LOG="$docker_log" \
    ${ux_env[@]+"${ux_env[@]}"} bash "$dogfood/ux-capture.sh" "$@"
}

# every answer a signed-in, healthy run needs. routes are first-match, so a case
# that wants one of these to go wrong adds its own line before calling this
ux_base() {
  json POST /api/v1/auth/login 200 '{"token":"tok-1"}'
  json POST /api/v1/ui-events 202 '{}'
  json POST /api/v1/auth/logout 200 '{}'
  json POST / 200 1
}

# login_sent EMAIL PASSWORD: the sign-in body carries exactly this account. read
# back as json, so a password with a quote or a backslash in it is checked as the
# server would read it, not as a string a shell happened to interpolate
login_sent() {
  python3 - "$requests" "$1" "$2" <<'PY'
import json, sys

log, email, password = sys.argv[1:4]
for line in open(log):
    if line.startswith("POST /api/v1/auth/login "):
        body = json.loads(line.split(" body=", 1)[1])
        sys.exit(0 if body == {"email": email, "password": password} else 1)
sys.exit(1)
PY
}

# basic_for USER PASSWORD: the Authorization header those credentials make
basic_for() {
  python3 -c 'import base64, sys; print("Basic " + base64.b64encode(f"{sys.argv[1]}:{sys.argv[2]}".encode()).decode())' "$1" "$2"
}

# a throwaway password, made at run time: a literal one is what secret scanners
# are there to find
random_secret() { python3 -c 'import secrets; print(secrets.token_hex(12))'; }

cases_ux_capture() {
  local operator_pw ch_pw team_env
  operator_pw="Zq\"$(random_secret)\\x!"
  ch_pw=$(random_secret)
  team_env="$work/team.env"
  printf 'ROLTER_CONTROL_HOST=0.0.0.0\n' >"$team_env"

  reset
  ux_base
  ux_env=()
  expect "no account in the environment falls back to creds.env" 0 \
    "signed in as dev@rolter.local" "address from creds.env, password from creds.env" \
    "the probe row is in ClickHouse" -- ux verify
  check "the checked-in account signed in" login_sent dev@rolter.local Rolter-dev-2026

  reset
  ux_base
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw")
  expect "the environment's account wins over creds.env" 0 \
    "signed in as operator@example.test" "address from the environment, password from the environment" \
    "!dev@rolter.local" -- ux verify
  check "the operator's account signed in, password intact" login_sent operator@example.test "$operator_pw"
  check "the probe went out with the operator's session" requested "POST /api/v1/ui-events auth=Bearer tok-1"
  check "the session was signed out again" requested "POST /api/v1/auth/logout auth=Bearer tok-1"

  reset
  ux_base
  ux_env=("DEV_PASSWORD=$operator_pw")
  expect "an unset variable still falls back, one at a time" 0 \
    "signed in as dev@rolter.local" "address from creds.env, password from the environment" -- ux verify
  check "the stack's address pairs with the caller's password" login_sent dev@rolter.local "$operator_pw"

  reset
  ux_base
  ux_env=(DEV_EMAIL=operator@example.test)
  expect "a default clickhouse is read without credentials" 0 -- ux verify
  check "no credentials went to clickhouse" requested "POST / auth= body=select count() from ui_events where session_id"

  reset
  json POST /api/v1/auth/login 401 '{"error":{"message":"invalid credentials"}}'
  ux_env=()
  expect "a refused sign-in names where the account came from" 1 \
    "refused dev@rolter.local" "address from creds.env" "pass your own" -- ux verify
  check "a refused sign-in has no session to sign out" not_requested "/api/v1/auth/logout"

  reset
  json POST /api/v1/auth/login 401 '{"error":{"message":"invalid credentials"}}'
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw")
  expect "a refused account of the caller's own is not blamed on creds.env" 1 \
    "refused operator@example.test" "address from the environment, password from the environment" \
    "!creds.env is" "!pass your own" -- ux verify

  reset
  json POST /api/v1/auth/login 429 '{"error":{"message":"slow down"}}'
  expect "a throttled sign-in says to wait" 1 "throttling sign-ins" -- ux verify

  reset
  expect "no control plane is exit 1" 1 "could not reach http://127.0.0.1:9" -- \
    env ROLTER_CONTROL_URL=http://127.0.0.1:9 CLICKHOUSE_URL="$stub" bash "$dogfood/ux-capture.sh" verify

  # a second factor: the login answers a challenge, and DEV_TOTP redeems it
  reset
  json POST /api/v1/auth/login 200 '{"mfa_required":true,"mfa_token":"chal-1","expires_in":300}'
  ux_base
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw")
  expect "an account with a second factor and no code is told how to pass one" 1 \
    "has a second factor" "DEV_TOTP" -- ux verify
  check "no probe without a session" not_requested "/api/v1/ui-events"

  reset
  json POST /api/v1/auth/login 200 '{"mfa_required":true,"mfa_token":"chal-1","expires_in":300}'
  json POST /api/v1/auth/mfa/verify 200 '{"token":"tok-2"}'
  ux_base
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw" DEV_TOTP=123456)
  expect "DEV_TOTP redeems the challenge for a session" 0 "signed in as operator@example.test" -- ux verify
  check "the challenge and the code went to the step-up" requested '"mfa_token":"chal-1","code":"123456"'
  check "the probe used the step-up's session" requested "POST /api/v1/ui-events auth=Bearer tok-2"

  reset
  json POST /api/v1/auth/login 200 '{"mfa_required":true,"mfa_token":"chal-1","expires_in":300}'
  json POST /api/v1/auth/mfa/verify 401 '{"error":{"message":"invalid credentials"}}'
  expect "a refused code is exit 1" 1 "refused the DEV_TOTP code" -- ux verify

  reset
  json POST /api/v1/auth/login 200 '{"mfa_enrolment_required":true,"enrolment_token":"enrol-1"}'
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw" DEV_TOTP=123456)
  expect "an unenrolled account is told to enrol first" 1 "has to enrol a second factor" -- ux verify

  # what the probe says when a hop is broken
  reset
  json POST /api/v1/ui-events 500 '{"error":{"message":"event store did not accept the write"}}'
  ux_base
  ux_env=()
  expect "a 500 from the endpoint is exit 1 and consults query_log" 1 \
    "the endpoint answered 500" "insert errors" -- ux verify
  check "query_log was read" requested "system.query_log"

  reset
  json POST /api/v1/ui-events 401 '{"error":{"message":"missing or invalid session"}}'
  ux_base
  expect "a 401 from the endpoint is exit 1" 1 "disable its UX stream" -- ux verify

  reset
  json POST / 200 0
  ux_base
  json GET /api/v1/logging-settings 200 '{"ui_events":true}'
  expect "a 202 with no row is exit 1" 1 "accepted but 0 rows arrived" "!switched off" -- ux verify

  reset
  json POST / 200 0
  ux_base
  json GET /api/v1/logging-settings 200 '{"ui_events":false}'
  expect "a 202 with no row while UX events are off says so" 1 \
    "dashboard usage events are switched off" "Dashboard Usage Events" -- ux verify
  check "the switch was read with the session" requested "GET /api/v1/logging-settings auth=Bearer tok-1"

  # clickhouse over http with credentials in CLICKHOUSE_URL
  reset
  ux_base
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw" "CLICKHOUSE_URL=http://rolter:$ch_pw@${stub#http://}")
  expect "credentials in CLICKHOUSE_URL are sent and never printed" 0 \
    "the probe row is in ClickHouse" "!$ch_pw" -- ux verify
  check "the read-back carried the credentials as basic auth" \
    requested "POST / auth=$(basic_for rolter "$ch_pw") body=select count() from ui_events where session_id"

  reset
  ux_base
  expect "apply-schema sends credentials with every statement" 0 "schema applied" "!$ch_pw" -- ux apply-schema
  check "a statement carried the credentials" requested "POST / auth=$(basic_for rolter "$ch_pw") body=create table"

  # the team shape: --team goes through docker compose exec, not http
  reset
  ux_base
  : >"$docker_log"
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw" "ROLTER_ENV_FILE=$team_env" "PATH=$work/fake-docker:$PATH")
  expect "--team verifies through docker compose exec" 0 \
    "docker compose exec clickhouse" "the probe row is in ClickHouse" "!$operator_pw" -- ux --team verify
  check "clickhouse was never read over http" not_requested "POST / auth="
  check "the team compose files and env file were layered" \
    grep -qF -- "-f $root/docker/docker-compose.yml -f $root/docker/docker-compose.team.yml --env-file $team_env exec -T clickhouse clickhouse-client --query" "$docker_log"
  check "the probe row was counted by session" grep -qF "where session_id" "$docker_log"

  reset
  ux_base
  : >"$docker_log"
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw" "ROLTER_ENV_FILE=$team_env" "PATH=$work/fake-docker:$PATH" FAKE_DOCKER_ROWS=0)
  expect "--team with no row in clickhouse is exit 1" 1 "accepted but 0 rows arrived" -- ux --team verify

  reset
  : >"$docker_log"
  ux_env=("ROLTER_ENV_FILE=$team_env" "PATH=$work/fake-docker:$PATH")
  expect "--team apply-schema feeds every file to clickhouse-client" 0 "schema applied" "008_ui_events.sql" -- ux --team apply-schema
  check "each file went through --multiquery" grep -qF -- "exec -T clickhouse clickhouse-client --multiquery" "$docker_log"
  check "apply-schema made no http call" nothing_requested

  reset
  ux_base
  : >"$docker_log"
  ux_env=(DEV_EMAIL=operator@example.test "DEV_PASSWORD=$operator_pw" "UX_COMPOSE=docker compose -p mine -f custom.yml" "PATH=$work/fake-docker:$PATH")
  expect "UX_COMPOSE is the compose command, spelled out" 0 "the probe row is in ClickHouse" -- ux verify
  check "the command was used as given" grep -qF -- "compose -p mine -f custom.yml exec -T clickhouse clickhouse-client" "$docker_log"

  reset
  ux_env=("ROLTER_ENV_FILE=$work/no-such.env")
  expect "--team without its env file says where it looked" 1 "no env file at $work/no-such.env" -- ux --team verify
  check "nothing was requested without an env file" nothing_requested

  reset
  ux_env=()
  expect "an unknown argument is a usage error" 2 "usage:" -- ux sideways
}

# ── the scripts under test ────────────────────────────────────────────────────
# one line per script; a script's cases live in `cases_<name>`, with the dashes
# of its file name as underscores
run_cases provision_signoz
run_cases adaptive_routing
run_cases ux_capture

if [ "$failures" -gt 0 ]; then
  echo "$failures of $checks dogfood script checks failed" >&2
  exit 1
fi
echo "ok: $checks dogfood script checks passed"
