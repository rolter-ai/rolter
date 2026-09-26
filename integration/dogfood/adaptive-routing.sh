#!/usr/bin/env bash
# Flip the deployment-wide adaptive-routing kill switch on the dogfood stack
# (#1817).
#
# `dogfood.toml` gives `deepseek-r1` `strategy = "adaptive"`, but that strategy
# only takes over once `adaptive_routing.enabled` is on, and the switch ships
# off. It lives in the `adaptive_routing_policy` row, which `rolter-seed
# --import` does not write (#1818), so declaring it in `dogfood.toml` would do
# nothing. Left off, the route quietly serves its fallback stack and the
# Adaptive Routing screens show `engaged: false` for the whole session.
#
#   ./integration/dogfood/adaptive-routing.sh on    # what `just dogfood` runs
#   ./integration/dogfood/adaptive-routing.sh off   # compare against the fallback
#
# `PUT /api/v1/adaptive-routing-policy` takes the whole policy, so this reads
# the current one and changes only `enabled`: blend weights, exploration and
# `min_samples` stay wherever the Settings screen left them.
#
# Reads ROLTER_CONTROL_URL when set (default http://127.0.0.1:4001) and
# ROLTER_ADMIN_TOKEN from the environment or `.tokens.env`. Waits up to
# CONTROL_WAIT_SECS (default 60) for the control plane to answer, since
# `just dogfood` calls this right after starting it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
control="${ROLTER_CONTROL_URL:-http://127.0.0.1:4001}"
control="${control%/}"
url="$control/api/v1/adaptive-routing-policy"

mode="${1:-on}"
case "$mode" in
  on) want=true ;;
  off) want=false ;;
  *)
    echo "usage: $0 [on|off]" >&2
    exit 2
    ;;
esac

if [ -z "${ROLTER_ADMIN_TOKEN:-}" ] && [ -f "$here/.tokens.env" ]; then
  set -a
  # shellcheck source=/dev/null
  . "$here/.tokens.env"
  set +a
fi
# the dogfood stack always has a token; a hand-run control plane without one
# serves the operator API open, and the header would only be ignored there
auth=()
if [ -n "${ROLTER_ADMIN_TOKEN:-}" ]; then
  auth=(-H "authorization: Bearer $ROLTER_ADMIN_TOKEN")
fi

# only "nothing is listening yet" is worth waiting out; any http answer other
# than 200 (a 401 from a stale token, say) is reported as it is
status=000
for _ in $(seq 1 "${CONTROL_WAIT_SECS:-60}"); do
  current="$(curl -sS --max-time 5 -w '\n%{http_code}' ${auth[@]+"${auth[@]}"} "$url" 2>/dev/null)" || current=$'\n000'
  status="${current##*$'\n'}"
  current="${current%$'\n'*}"
  [ "$status" = 000 ] || break
  sleep 1
done
if [ "$status" = 000 ]; then
  echo "[adaptive] no answer from $url — is the control plane up? rerun with: just dogfood-adaptive $mode" >&2
  exit 1
fi
if [ "$status" != 200 ]; then
  echo "[adaptive] GET $url answered $status: $current" >&2
  exit 1
fi

body="$(printf '%s' "$current" | WANT="$want" python3 -c '
import json, os, sys
policy = json.load(sys.stdin)
keys = ("latency_weight", "cost_weight", "load_weight", "exploration_ratio", "min_samples")
body = {key: policy[key] for key in keys}
body["enabled"] = os.environ["WANT"] == "true"
print(json.dumps(body))
')"

# -f would hide the body, and a 400 here names the field it refused
resp="$(curl -sS --max-time 10 -w '\n%{http_code}' ${auth[@]+"${auth[@]}"} -X PUT "$url" \
  -H 'content-type: application/json' --data-binary "$body")" || {
  echo "[adaptive] PUT $url failed" >&2
  exit 1
}
status="${resp##*$'\n'}"
resp="${resp%$'\n'*}"
if [ "$status" != 200 ]; then
  echo "[adaptive] PUT $url answered $status: $resp" >&2
  exit 1
fi

printf '%s' "$resp" | python3 -c '
import json, sys
policy = json.load(sys.stdin)
routes = ", ".join(policy.get("affected_routes") or [])
state = "on" if policy["enabled"] else "off"
samples = policy["min_samples"]
print(f"[adaptive] adaptive routing {state} (min_samples={samples})")
if policy["enabled"]:
    if routes:
        print(f"[adaptive] governs: {routes}; each engages after min_samples picks")
    else:
        print("[adaptive] no adaptive route yet: just dogfood-seed adds deepseek-r1")
'
