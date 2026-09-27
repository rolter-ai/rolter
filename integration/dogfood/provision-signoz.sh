#!/usr/bin/env bash
# Give the local SigNoz the shared dev credential and the checked-in dashboards
# (#956), so a fresh stack is immediately useful instead of asking every
# developer to sign up and then rebuild the same panels by hand.
#
# Targets SigNoz v0.136.0, the release docker/docker-compose.signoz.yml pins.
# That release moved both APIs this script needs (#1864):
#
#   - sign-in is POST /api/v2/sessions/email_password, which takes the org id
#     that GET /api/v2/sessions/context returns for the email
#   - dashboards are created through POST /api/v2/dashboards; every v1
#     dashboard route answers 501 `dashboard_deprecated`
#
# The older spellings (/api/v1/login, /api/v2/auth/login, /api/v1/auth/login,
# /api/v1/dashboards) stay as fallbacks for a SigNoz from before the move. A
# SigNoz that answers none of them fails with exit 2 and names its version.
#
# The boards in signoz/dashboards/ are kept in SigNoz's v1 import format
# (title/widgets/layout plus a `version`), the shape its "Import JSON" dialog
# still accepts: v0.136's create endpoint converts it to the v2 (Perses) schema
# server-side. A board already in the v2 schema (a top-level `spec`, the shape
# GET /api/v2/dashboards/{id} returns) is posted as it is.
#
# Safe to re-run. On a stack that is already provisioned this signs in, skips
# every board whose title is already there, and changes nothing else.
#
# It deliberately does **not** rewrite an existing SigNoz account's password.
# Editing the credential store of a running service behind its own back is the
# kind of thing that works until it silently doesn't; when the existing account
# does not match, this says so and points at `just signoz-reset`, which discards
# SigNoz's metadata database on purpose and starts clean. Traces are unaffected
# either way — they live in ClickHouse, not in SigNoz's sqlite.
#
# Exit codes: 0 provisioned (or already was), 1 SigNoz unreachable or failing,
# the account does not match or a board failed to import, 2 a SigNoz whose API
# this script does not know. Only SigNoz refusing the credential itself (a 4xx
# json error from the sign-in route) counts as a mismatch: a route that moved,
# or a token under a key this script does not read, is a 2 that names the
# version, never the mismatch message and its `just signoz-reset` (#1792).
set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE="${SIGNOZ_URL:-http://127.0.0.1:8080}"
# shellcheck source=/dev/null
set -a; . "$DIR/creds.env"; set +a

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

say() { printf '[signoz] %s\n' "$1"; }
die() { printf '[signoz] %s\n' "$1" >&2; exit "${2:-1}"; }

# call METHOD PATH [BODY]: the response body, then one last line holding the
# status code and the content type. BODY is json, or @file for a file of it.
#
# SigNoz serves the SPA (200 text/html) for any path that is not an API route,
# so "does this endpoint exist" cannot be answered by the status code alone —
# only a json content type means an API answered.
call() {
  local args=(-s --max-time 20 -X "$1" -w '\n%{http_code} %{content_type}')
  [ -n "${TOKEN:-}" ] && args+=(-H "Authorization: Bearer $TOKEN")
  [ -n "${3:-}" ] && args+=(-H 'Content-Type: application/json' --data-binary "$3")
  curl "${args[@]}" "$BASE$2" 2>/dev/null
}
body() { printf '%s' "$1" | sed '$d'; }
status_of() { printf '%s' "$1" | tail -n1 | cut -d' ' -f1; }
is_json() { printf '%s' "$1" | tail -n1 | grep -qi 'application/json'; }
answered_ok() { is_json "$1" && [ "$(status_of "$1")" -ge 200 ] && [ "$(status_of "$1")" -lt 300 ]; }

# jget PATH...: the first non-empty value found at any of the dotted paths in
# the json on stdin (`data.orgs.0.id`), or nothing. Responses nest the payload
# under `data` on v0.136 and did not on older releases, hence several paths.
jget() {
  python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for path in sys.argv[1:]:
    cur = doc
    for part in path.split("."):
        if isinstance(cur, dict):
            cur = cur.get(part)
        elif isinstance(cur, list) and part.isdigit() and int(part) < len(cur):
            cur = cur[int(part)]
        else:
            cur = None
    if cur is not None and cur != "":
        print(json.dumps(cur) if isinstance(cur, (dict, list)) else cur)
        break' "$@" 2>/dev/null
}
api_error() { body "$1" | jget error.message error; }
# why RESPONSE: the api's error message, or a stand-in when it gave none
why() { local e; e="$(api_error "$1")"; printf '%s' "${e:-no error message}"; }

say "waiting for $BASE"
for _ in $(seq 1 90); do
  curl -fsS --max-time 2 "$BASE/api/v1/version" >/dev/null 2>&1 && break
  sleep 2
done
version_json="$(curl -fsS --max-time 5 "$BASE/api/v1/version" 2>/dev/null)" \
  || die "not reachable at $BASE — is the stack up? (just dogfood)"
VERSION="$(printf '%s' "$version_json" | jget version)"
VERSION="${VERSION:-an unknown version}"
say "SigNoz $VERSION"

# a SigNoz whose API moved again. fail rather than warn: a warning here scrolls
# past in `just dogfood` and the stack comes up with no dashboards (#1792)
unsupported() {
  cat >&2 <<EOF
[signoz] SigNoz $VERSION is not a version this script knows how to provision

  $1

  It targets SigNoz v0.136.0, the release docker/docker-compose.signoz.yml
  pins, and keeps the spellings of the releases before it as fallbacks.
  Nothing was imported. Teach integration/dogfood/provision-signoz.sh the new
  route, or import the dashboards by hand: SigNoz → Dashboards → New
  dashboard → Import JSON, using the files in
  integration/dogfood/signoz/dashboards/.
EOF
  exit 2
}

setup="$(printf '%s' "$version_json" | jget setupCompleted)"
if [ "$setup" != "True" ] && [ "$setup" != "true" ]; then
  say "fresh instance — registering $DEV_EMAIL"
  out="$(call POST /api/v1/register \
    "$(python3 -c 'import json,os;print(json.dumps({
      "name": os.environ["DEV_NAME"],
      "email": os.environ["DEV_EMAIL"],
      "password": os.environ["DEV_PASSWORD"]}))')")"
  is_json "$out" || unsupported "POST /api/v1/register answered with the SPA rather than json."
  answered_ok "$out" || die "register refused: $(api_error "$out")"
  say "registered"
  # some older builds hand back a session on register, which saves the login
  REGISTER_TOKEN="$(body "$out" | jget data.accessJwt accessJwt data.accessToken accessToken)"
fi

# ── sign in ──────────────────────────────────────────────────────────────────
TOKEN="${REGISTER_TOKEN:-}"
ANSWERED=""
LOGIN_ERR=""
[ -n "$TOKEN" ] && say "using the session returned by register"

creds_json() {
  python3 -c 'import json,os,sys
creds = {"email": os.environ["DEV_EMAIL"], "password": os.environ["DEV_PASSWORD"]}
if len(sys.argv) > 1:
    creds["orgId"] = sys.argv[1]
print(json.dumps(creds))' "$@"
}

# v0.136: the org id comes from the session context for the email, and the
# token is `data.accessToken`.
#
# once the context route answers, only SigNoz saying no to the credential is an
# account mismatch. anything else (the SPA, a token under another key, a route
# answering 404/405/501) means this release moved the api, and blaming the
# account would send the operator to `just signoz-reset`, which deletes
# SigNoz's users and dashboards and then fails the same way (#1792)
if [ -z "$TOKEN" ]; then
  query="$(python3 -c 'import os,sys,urllib.parse as u
print(u.urlencode({"email": os.environ["DEV_EMAIL"], "ref": sys.argv[1]}))' "$BASE")"
  out="$(call GET "/api/v2/sessions/context?$query")"
  if answered_ok "$out"; then
    ANSWERED=/api/v2/sessions/email_password
    org="$(body "$out" | jget data.orgs.0.id)"
    case "$(body "$out" | jget data.exists)" in
      True) ;;
      False) LOGIN_ERR="SigNoz has no account for $DEV_EMAIL" ;;
      *) unsupported "GET /api/v2/sessions/context answered without data.exists, so it
  cannot say whether $DEV_EMAIL has an account. The credential was not changed." ;;
    esac
    if [ -z "$LOGIN_ERR" ]; then
      [ -n "$org" ] || unsupported "GET /api/v2/sessions/context says $DEV_EMAIL exists but names no
  organisation at data.orgs[0].id. The credential was not changed."
      out="$(call POST /api/v2/sessions/email_password "$(creds_json "$org")")"
      is_json "$out" || unsupported "POST /api/v2/sessions/email_password answered with the SPA rather
  than json. The credential was not changed."
      TOKEN="$(body "$out" | jget data.accessToken)"
      code="$(status_of "$out")"; code="${code:-0}"
      if [ -n "$TOKEN" ]; then
        say "signed in via $ANSWERED"
      elif [ "$code" -ge 200 ] && [ "$code" -lt 300 ]; then
        unsupported "POST /api/v2/sessions/email_password answered $code with no
  data.accessToken, so the session token moved. The credential was not changed."
      elif [ "$code" = 404 ] || [ "$code" = 405 ] || [ "$code" = 501 ]; then
        unsupported "POST /api/v2/sessions/email_password answered $code: $(why "$out").
  The credential was not changed."
      elif [ "$code" -ge 400 ] && [ "$code" -lt 500 ]; then
        LOGIN_ERR="$(why "$out")"
      else
        die "sign-in failed: POST /api/v2/sessions/email_password answered $code: $(why "$out").
  That is SigNoz $VERSION failing, not the account: check the signoz container's logs and rerun."
      fi
    fi
  fi
fi

# before v0.136 the route moved between releases, so try each old spelling and
# use whichever actually answers as an api rather than hardcoding a guess
if [ -z "$TOKEN" ] && [ -z "$ANSWERED" ]; then
  for path in /api/v1/login /api/v2/auth/login /api/v1/auth/login; do
    out="$(call POST "$path" "$(creds_json)")"
    is_json "$out" || continue
    ANSWERED="$path"
    TOKEN="$(body "$out" | jget data.accessJwt accessJwt data.accessToken accessToken)"
    if [ -n "$TOKEN" ]; then say "signed in via $path"; break; fi
    LOGIN_ERR="$(api_error "$out")"
  done
fi

if [ -z "$TOKEN" ] && [ -z "$ANSWERED" ]; then
  unsupported "No sign-in route answered: tried /api/v2/sessions/email_password, then
  /api/v1/login, /api/v2/auth/login and /api/v1/auth/login. Each returned the
  SPA rather than json. The credential was not changed."
fi

if [ -z "$TOKEN" ]; then
  cat >&2 <<EOF
[signoz] could not sign in to SigNoz $VERSION as $DEV_EMAIL${LOGIN_ERR:+ ($LOGIN_ERR)}

  This instance already has an account that is not the shared dev credential.
  Nothing was changed. Either sign in with the password you chose, or discard
  SigNoz's metadata and let this script set it up cleanly:

      just signoz-reset

  Your traces are in ClickHouse and survive that — only SigNoz's own users,
  dashboards and alerts live in the database it removes.

  The dashboards can also be imported by hand: SigNoz → Dashboards → New
  dashboard → Import JSON, using the files in integration/dogfood/signoz/dashboards/.
EOF
  exit 1
fi

# ── dashboards ───────────────────────────────────────────────────────────────
# the titles already present, one per line. v0.136 pages the list (200 at most
# per page) and names a board by its `spec.display.name`
list_v2() {
  local offset=0 total out
  while :; do
    out="$(call GET "/api/v2/dashboards?limit=200&offset=$offset")"
    answered_ok "$out" || return 1
    body "$out" | python3 -c 'import json,sys
for d in json.load(sys.stdin)["data"].get("dashboards") or []:
    name = ((d.get("spec") or {}).get("display") or {}).get("name")
    if name:
        print(name)' || return 1
    total="$(body "$out" | jget data.total)"
    offset=$((offset + 200))
    [ "${total:-0}" -gt "$offset" ] || return 0
  done
}

list_v1() {
  local out
  out="$(call GET /api/v1/dashboards)"
  answered_ok "$out" || return 1
  body "$out" | python3 -c 'import json,sys
d = json.load(sys.stdin)
rows = d.get("data", d) if isinstance(d, dict) else d
for r in rows or []:
    t = (r.get("data") or {}).get("title") or r.get("title") or ""
    if t:
        print(t)'
}

if existing="$(list_v2)"; then
  API=v2
elif existing="$(list_v1)"; then
  API=v1
else
  unsupported "Neither GET /api/v2/dashboards nor GET /api/v1/dashboards listed the
  dashboards."
fi

# prepare API FILE OUT: the create request for one board, or the reason it
# cannot be made. The first line of stdout is the board's title.
prepare() {
  python3 - "$@" <<'PY'
import json, sys

api, path, out = sys.argv[1:4]
try:
    board = json.load(open(path))
except Exception as err:
    sys.exit(f"is not valid json: {err}")

if "spec" in board:
    # already the v2 (Perses) schema, e.g. fetched from GET /api/v2/dashboards/{id}
    title = ((board.get("spec") or {}).get("display") or {}).get("name") or ""
    if api != "v2":
        sys.exit("is in the v2 dashboard schema, which this SigNoz cannot import")
    # a fetched board carries the server's id, timestamps and unique `name`;
    # the create endpoint rejects unknown fields, and a fresh name cannot collide
    payload = {
        "schemaVersion": board.get("schemaVersion") or "v6",
        "generateName": True,
        "tags": [{"key": t["key"], "value": t["value"]} for t in board.get("tags") or []],
        "spec": board["spec"],
    }
    if board.get("image"):
        payload["image"] = board["image"]
else:
    title = board.get("title") or ""
    payload = board
    if api == "v2" and not board.get("version"):
        # v0.136 only converts a v1 board that says which query-builder version
        # its panels are written in. that is moot for ClickHouse SQL and PromQL
        # panels, which the conversion copies untouched, so say v5 for them; a
        # builder panel has to carry its own, which a SigNoz export always does
        kinds = {((w.get("query") or {}).get("queryType") or "builder")
                 for w in board.get("widgets") or []}
        if kinds - {"clickhouse_sql", "promql"}:
            sys.exit("declares no `version` and has query-builder panels; "
                     "re-export it from SigNoz so it carries one")
        payload["version"] = "v5"

if not title:
    sys.exit("has no title")
json.dump(payload, open(out, "w"))
print(title)
PY
}

imported=0 skipped=0 failed=0
for f in "$DIR"/signoz/dashboards/*.json; do
  [ -e "$f" ] || continue
  name="${f#"$DIR"/}"
  if ! title="$(prepare "$API" "$f" "$TMP/board.json" 2>"$TMP/why")"; then
    failed=$((failed + 1))
    say "could not import $name: it $(tail -n1 "$TMP/why")"
    continue
  fi
  if printf '%s\n' "$existing" | grep -Fxq -- "$title"; then
    skipped=$((skipped + 1)); continue
  fi
  out="$(call POST "/api/$API/dashboards" "@$TMP/board.json")"
  if answered_ok "$out" && [ -z "$(body "$out" | jget error)" ]; then
    imported=$((imported + 1))
    say "imported '$title'"
  else
    failed=$((failed + 1))
    say "could not import '$title' from $name: $(api_error "$out")"
  fi
done

say "dashboards: $imported imported, $skipped already present, $failed failed"
[ "$failed" -gt 0 ] && exit 1
exit 0
