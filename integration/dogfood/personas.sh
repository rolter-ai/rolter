#!/usr/bin/env bash
# Create the persona accounts the user-journey scripts sign in as.
#
# The scripts in docs/dev-docs/product/ walk one person at a time through the
# product — an org admin, a team lead, an engineer, a viewer, finance — and a
# script is only a test if each of those people exists with exactly the role
# the script assumes. This makes them, on the dogfood stack, with one command:
#
#   ./integration/dogfood/personas.sh          # create what is missing, print the roster
#
# It is idempotent: an account, team, project or membership that already exists
# is left as it is, so it is safe to run after every `just dogfood`. Every
# account shares the dogfood password from creds.env — local-only by
# construction, like every other credential the stack prints.
#
# Two persona accounts are project-only on purpose. The API grants an org role
# when it creates an account, so for those the org membership is removed again
# once the project one exists — otherwise "member of one project" would really
# be "viewer of the whole org", and the scoping the scripts test would be
# invisible.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TOKENS_FILE="${ROLTER_DOGFOOD_TOKENS_FILE:-$DIR/.tokens.env}"
CONTROL="${ROLTER_CONTROL_URL:-http://127.0.0.1:4001}"
CONTROL="${CONTROL%/}"

# shellcheck source=integration/dogfood/creds.env
set -a; . "$DIR/creds.env"; set +a
if [ -f "$TOKENS_FILE" ]; then
  set -a; . "$TOKENS_FILE"; set +a
fi

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
ok() { printf '\033[32m✓ %s\033[0m\n' "$1"; }
fail() {
  printf '\033[31m✗ %s\033[0m\n' "$1" >&2
  exit 1
}

[ -n "${ROLTER_ADMIN_TOKEN:-}" ] ||
  fail "no ROLTER_ADMIN_TOKEN — start the stack with \`just dogfood\` first (it writes $TOKENS_FILE)"
curl -fsS -o /dev/null "$CONTROL/healthz" 2>/dev/null ||
  fail "no control plane at $CONTROL — start it with \`just dogfood\`"

# one python process owns the whole reconciliation: the API is JSON in and out,
# and doing it in bash would be a page of jq for no gain
python3 - "$CONTROL" "$ROLTER_ADMIN_TOKEN" "$DEV_PASSWORD" <<'PYEOF'
import json
import sys
import urllib.error
import urllib.request

base, token, password = sys.argv[1], sys.argv[2], sys.argv[3]
api = f"{base}/api/v1"


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(
        f"{api}{path}",
        data=data,
        method=method,
        headers={"authorization": f"Bearer {token}", "content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as err:
        sys.exit(f"{method} {path} -> {err.code}: {err.read().decode(errors='replace')}")


def one(rows, what, **match):
    found = [r for r in rows if all(r.get(k) == v for k, v in match.items())]
    return found[0] if found else None


org = one(call("GET", "/orgs"), "org", slug="default")
if org is None:
    sys.exit("no org with slug 'default' — seed the stack first (just dogfood)")
org_id = org["id"]

# the fleet lives in default/default; a second team and project exist only so
# the scripts can show one project's people not seeing another's traffic
teams = call("GET", f"/orgs/{org_id}/teams")
team_default = one(teams, "team", name="default")
team_research = one(teams, "team", name="research") or call(
    "POST", f"/orgs/{org_id}/teams", {"name": "research"}
)
projects = call("GET", f"/orgs/{org_id}/projects")
project_default = one(projects, "project", team_id=team_default["id"], name="default")
project_sandbox = one(projects, "project", team_id=team_research["id"], name="sandbox") or call(
    "POST", f"/teams/{team_research['id']}/projects", {"name": "sandbox"}
)

# email, role, scope kind, scope row, what the scripts call this person
PERSONAS = [
    ("orgadmin@rolter.local", "admin", "org", org, "org admin (platform-admin.md)"),
    ("lead@rolter.local", "admin", "team", team_default, "team lead (team-lead.md)"),
    ("engineer@rolter.local", "member", "project", project_default, "engineer (engineer.md)"),
    ("engineer2@rolter.local", "member", "project", project_sandbox, "engineer, other project"),
    ("viewer@rolter.local", "viewer", "project", project_default, "viewer (viewer.md)"),
    ("finops@rolter.local", "viewer", "org", org, "finops (finops.md)"),
]

users = {u["email"]: u for u in call("GET", f"/orgs/{org_id}/users")}
for email, role, kind, scope, _ in PERSONAS:
    if email not in users:
        # the org role granted at creation: the persona's own role when the
        # persona is org-scoped, a placeholder viewer otherwise (removed below)
        org_role = role if kind == "org" else "viewer"
        created = call(
            "POST",
            f"/orgs/{org_id}/users",
            {"email": email, "password": password, "role": org_role},
        )
        users[email] = created["user"]

memberships = call("GET", f"/orgs/{org_id}/memberships")
for email, role, kind, scope, _ in PERSONAS:
    user_id = users[email]["id"]
    mine = [m for m in memberships if m["user_id"] == user_id]
    key = {"org": "org_id", "team": "team_id", "project": "project_id"}[kind]
    has_scope = any(
        m.get(key) == scope["id"]
        and (kind == "project" or not m.get("project_id"))
        and (kind != "org" or not m.get("team_id"))
        and m["role"] == role
        for m in mine
    )
    if not has_scope:
        call(
            "POST",
            f"/orgs/{org_id}/memberships",
            {"user_id": user_id, "scope_type": kind, "scope_id": scope["id"], "role": role},
        )
    if kind != "org":
        # a narrower persona holds nothing at org level, or the scripts would
        # be testing "viewer of everything" by accident
        for m in mine:
            if m.get("org_id") == org_id and not m.get("team_id") and not m.get("project_id"):
                call("DELETE", f"/memberships/{m['id']}")

print(f"\n  {'account':<26} {'role':<8} {'scope':<28} persona")
print(f"  {'-' * 26} {'-' * 8} {'-' * 28} {'-' * 30}")
print(f"  {'dev@rolter.local':<26} {'super':<8} {'deployment':<28} platform operator, devops, secops")
for email, role, kind, scope, persona in PERSONAS:
    label = f"{kind}:{scope['name']}"
    print(f"  {email:<26} {role:<8} {label:<28} {persona}")
print(f"\n  password for every account: {password}\n")
PYEOF
ok "personas ready — the scripts are in docs/dev-docs/product/"
