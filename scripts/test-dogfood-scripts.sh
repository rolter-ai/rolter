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
# refused call instead of printing a success line (#1817). no SigNoz, no
# docker and no control plane is needed: the stub plays the answers from a
# scenario file, one line per route, rewritten between cases. it runs as a
# step of quality.yml's `static checks` job and as a prek hook.
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

# ── the scripts under test ────────────────────────────────────────────────────
# one line per script; a script's cases live in `cases_<name>`, with the dashes
# of its file name as underscores
run_cases provision_signoz
run_cases adaptive_routing

if [ "$failures" -gt 0 ]; then
  echo "$failures of $checks dogfood script checks failed" >&2
  exit 1
fi
echo "ok: $checks dogfood script checks passed"
